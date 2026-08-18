//! End-to-end render bench (wall time, peak RSS, codecs) — not Criterion micros.
//!
//! Drive it from the `e2e_bench` example:
//! ```bash
//! cargo run -p reelforge-io --example e2e_bench --release -- --quick
//! cargo run -p reelforge-io --example e2e_bench --release -- --input clip.mp4 --full
//! ```

use crate::av_sync::av_duration_drift;
use crate::error::{IoError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::options::{OpenVideoOptions, WriteVideoOptions};
use crate::realtime::detect_hw_encoders;
use crate::video_file::open_video;
use crate::write::{write_av, write_video};
use reelforge_core::{Duration, SilenceClip, Size, VideoClip, VideoEffect};
use reelforge_fx::{
    BlackAndWhite, CoverageMask, Crop, FadeIn, RegionSample, RegionTrack, Resize, ResizeFilter,
    TrackSet, TrackedPrivacy,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

/// Workload family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum E2eKind {
    /// Decode (or solid) → fused privacy → encode.
    Privacy,
    /// Crop → scale → fade → B&W (MoviePy-like edit).
    Edit,
}

/// Encoder the case asks for (skipped when the host `ffmpeg` lacks it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum E2eCodec {
    /// `libx264`.
    H264Sw,
    /// `libx265`.
    H265Sw,
    /// `libaom-av1`.
    Av1Sw,
    /// `h264_nvenc`.
    Nvenc,
    /// `h264_qsv`.
    Qsv,
    /// `h264_amf`.
    Amf,
}

impl E2eCodec {
    /// `ffmpeg` encoder name.
    #[must_use]
    pub const fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::H264Sw => "libx264",
            Self::H265Sw => "libx265",
            Self::Av1Sw => "libaom-av1",
            Self::Nvenc => "h264_nvenc",
            Self::Qsv => "h264_qsv",
            Self::Amf => "h264_amf",
        }
    }

    /// Software x264-style CRF path.
    #[must_use]
    pub const fn uses_crf(self) -> bool {
        matches!(self, Self::H264Sw | Self::H265Sw | Self::Av1Sw)
    }
}

/// One e2e case.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct E2eCase {
    /// Stable id (`privacy_1080p_50_h264`).
    pub name: String,
    /// Workload.
    pub kind: E2eKind,
    /// Canvas width.
    pub width: u32,
    /// Canvas height.
    pub height: u32,
    /// Timeline length.
    pub secs: f64,
    /// Nominal FPS.
    pub fps: f64,
    /// Privacy subject count (`0` for edit).
    pub subjects: usize,
    /// Attach small dense silhouettes instead of ellipses.
    pub dense_masks: bool,
    /// Target encoder.
    pub codec: E2eCodec,
    /// Mux silence and measure A/V drift.
    pub with_audio: bool,
    /// Time a host-`ffmpeg` `-vf` equivalent (edit only).
    pub compare_ffmpeg: bool,
}

impl E2eCase {
    /// Tiny CI / `--quick` privacy case.
    #[must_use]
    pub fn smoke() -> Self {
        Self {
            name: "privacy_smoke_h264".into(),
            kind: E2eKind::Privacy,
            width: 160,
            height: 90,
            secs: 0.3,
            fps: 10.0,
            subjects: 5,
            dense_masks: false,
            codec: E2eCodec::H264Sw,
            with_audio: false,
            compare_ffmpeg: false,
        }
    }
}

/// Timed result for one case.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct E2eReport {
    /// Case name.
    pub name: String,
    /// Encoder actually used.
    pub codec: String,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Subject count.
    pub subjects: usize,
    /// Repeat wall times (milliseconds).
    pub samples_ms: Vec<f64>,
    /// Median encode time.
    pub p50_ms: f64,
    /// 95th percentile encode time.
    pub p95_ms: f64,
    /// Process peak RSS when available (Linux `VmHWM`).
    pub peak_rss_bytes: Option<u64>,
    /// Last output size.
    pub output_bytes: u64,
    /// `|video − audio|` seconds when muxed.
    pub av_drift_secs: Option<f64>,
    /// Host `ffmpeg` `-vf` wall time (edit compare).
    pub ffmpeg_ms: Option<f64>,
    /// Why the case was skipped.
    pub skipped: Option<String>,
}

/// Process peak RSS (`VmHWM` on Linux).
#[must_use]
pub fn peak_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmHWM:") {
                let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kb.saturating_mul(1024));
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Linear interpolation percentile. `samples` must be sorted ascending.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn percentile(samples: &[f64], pct: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    if samples.len() == 1 {
        return samples[0];
    }
    let pct = pct.clamp(0.0, 1.0);
    let last = samples.len() - 1;
    let idx = pct * last as f64;
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    if lo == hi {
        samples[lo]
    } else {
        let t = idx - lo as f64;
        samples[lo] * (1.0 - t) + samples[hi] * t
    }
}

/// `--quick` matrix (CI / smoke).
#[must_use]
pub fn smoke_cases() -> Vec<E2eCase> {
    vec![
        E2eCase::smoke(),
        E2eCase {
            name: "edit_smoke_h264".into(),
            kind: E2eKind::Edit,
            width: 160,
            height: 90,
            secs: 0.3,
            fps: 10.0,
            subjects: 0,
            dense_masks: false,
            codec: E2eCodec::H264Sw,
            with_audio: false,
            compare_ffmpeg: true,
        },
    ]
}

/// Default matrix: 720p/1080p privacy + edit + A/V, optional HW/H.265/AV1.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn standard_cases() -> Vec<E2eCase> {
    let mut out = smoke_cases();
    out.extend([
        case_privacy(
            "privacy_720p_10_h264",
            1280,
            720,
            1.0,
            24.0,
            10,
            false,
            E2eCodec::H264Sw,
        ),
        case_privacy(
            "privacy_1080p_10_h264",
            1920,
            1080,
            1.0,
            24.0,
            10,
            false,
            E2eCodec::H264Sw,
        ),
        case_privacy(
            "privacy_1080p_50_h264",
            1920,
            1080,
            1.0,
            24.0,
            50,
            false,
            E2eCodec::H264Sw,
        ),
        case_privacy(
            "privacy_1080p_50_dense_h264",
            1920,
            1080,
            1.0,
            24.0,
            50,
            true,
            E2eCodec::H264Sw,
        ),
        case_privacy(
            "privacy_1080p_50_h265",
            1920,
            1080,
            1.0,
            24.0,
            50,
            false,
            E2eCodec::H265Sw,
        ),
        case_privacy(
            "privacy_1080p_50_av1",
            1920,
            1080,
            0.5,
            24.0,
            50,
            false,
            E2eCodec::Av1Sw,
        ),
        case_privacy(
            "privacy_1080p_50_nvenc",
            1920,
            1080,
            1.0,
            24.0,
            50,
            false,
            E2eCodec::Nvenc,
        ),
        case_privacy(
            "privacy_1080p_50_qsv",
            1920,
            1080,
            1.0,
            24.0,
            50,
            false,
            E2eCodec::Qsv,
        ),
        case_privacy(
            "privacy_1080p_50_amf",
            1920,
            1080,
            1.0,
            24.0,
            50,
            false,
            E2eCodec::Amf,
        ),
        E2eCase {
            name: "edit_1080p_h264".into(),
            kind: E2eKind::Edit,
            width: 1920,
            height: 1080,
            secs: 1.0,
            fps: 24.0,
            subjects: 0,
            dense_masks: false,
            codec: E2eCodec::H264Sw,
            with_audio: false,
            compare_ffmpeg: true,
        },
        E2eCase {
            name: "av_1080p_h264".into(),
            kind: E2eKind::Privacy,
            width: 1920,
            height: 1080,
            secs: 2.0,
            fps: 24.0,
            subjects: 10,
            dense_masks: false,
            codec: E2eCodec::H264Sw,
            with_audio: true,
            compare_ffmpeg: false,
        },
    ]);
    out
}

/// Adds 4K and 100-subject rows.
#[must_use]
pub fn full_cases() -> Vec<E2eCase> {
    let mut out = standard_cases();
    out.extend([
        case_privacy(
            "privacy_1080p_100_h264",
            1920,
            1080,
            1.0,
            24.0,
            100,
            false,
            E2eCodec::H264Sw,
        ),
        case_privacy(
            "privacy_4k_10_h264",
            3840,
            2160,
            0.5,
            24.0,
            10,
            false,
            E2eCodec::H264Sw,
        ),
        case_privacy(
            "privacy_4k_50_h264",
            3840,
            2160,
            0.5,
            24.0,
            50,
            true,
            E2eCodec::H264Sw,
        ),
    ]);
    out
}

/// Run one case `repeats` times. Missing encoder / ffmpeg → `skipped`.
///
/// `input` overrides generated lavfi when the file exists.
///
/// # Errors
///
/// Source generate, decode, effect, or encode failures (not skip).
pub fn run_e2e_case(
    case: &E2eCase,
    repeats: u32,
    work_dir: &Path,
    input: Option<&Path>,
) -> Result<E2eReport> {
    if !crate::ffmpeg_available() {
        return Ok(skipped(case, "ffmpeg not available"));
    }
    if !encoder_available(case.codec) {
        return Ok(skipped(
            case,
            &format!("encoder {} not in host ffmpeg", case.codec.ffmpeg_name()),
        ));
    }
    std::fs::create_dir_all(work_dir).map_err(|e| IoError::message(format!("e2e mkdir: {e}")))?;
    let source = resolve_source(case, work_dir, input)?;
    let mut samples_ms = Vec::new();
    let mut output_bytes = 0_u64;
    let mut av_drift = None;
    let repeats = repeats.max(1);
    for i in 0..repeats {
        let out = work_dir.join(format!("{}-{i}.mp4", case.name));
        let t0 = Instant::now();
        run_once(case, &source, &out)?;
        samples_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        output_bytes = std::fs::metadata(&out).map_or(0, |m| m.len());
        if case.with_audio {
            av_drift = probe_av_drift(&out, case.fps);
        }
    }
    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let ffmpeg_ms = if case.compare_ffmpeg && case.kind == E2eKind::Edit {
        time_ffmpeg_edit(case, &source, work_dir).ok()
    } else {
        None
    };
    Ok(E2eReport {
        name: case.name.clone(),
        codec: case.codec.ffmpeg_name().into(),
        width: case.width,
        height: case.height,
        subjects: case.subjects,
        p50_ms: percentile(&samples_ms, 0.50),
        p95_ms: percentile(&samples_ms, 0.95),
        samples_ms,
        peak_rss_bytes: peak_rss_bytes(),
        output_bytes,
        av_drift_secs: av_drift,
        ffmpeg_ms,
        skipped: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn case_privacy(
    name: &str,
    width: u32,
    height: u32,
    secs: f64,
    fps: f64,
    subjects: usize,
    dense: bool,
    codec: E2eCodec,
) -> E2eCase {
    E2eCase {
        name: name.into(),
        kind: E2eKind::Privacy,
        width,
        height,
        secs,
        fps,
        subjects,
        dense_masks: dense,
        codec,
        with_audio: false,
        compare_ffmpeg: false,
    }
}

fn skipped(case: &E2eCase, why: &str) -> E2eReport {
    E2eReport {
        name: case.name.clone(),
        codec: case.codec.ffmpeg_name().into(),
        width: case.width,
        height: case.height,
        subjects: case.subjects,
        samples_ms: Vec::new(),
        p50_ms: 0.0,
        p95_ms: 0.0,
        peak_rss_bytes: peak_rss_bytes(),
        output_bytes: 0,
        av_drift_secs: None,
        ffmpeg_ms: None,
        skipped: Some(why.into()),
    }
}

fn encoder_available(codec: E2eCodec) -> bool {
    match codec {
        E2eCodec::H264Sw => true,
        E2eCodec::Nvenc => detect_hw_encoders().is_ok_and(|s| s.nvenc_h264),
        E2eCodec::Qsv => detect_hw_encoders().is_ok_and(|s| s.qsv_h264),
        E2eCodec::Amf => detect_hw_encoders().is_ok_and(|s| s.amf_h264),
        E2eCodec::H265Sw | E2eCodec::Av1Sw => ffmpeg_lists_encoder(codec.ffmpeg_name()),
    }
}

fn ffmpeg_lists_encoder(name: &str) -> bool {
    let Ok(tools) = FfmpegTools::discover() else {
        return false;
    };
    let Ok(out) = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout).contains(name)
}

fn resolve_source(case: &E2eCase, work_dir: &Path, input: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = input
        && p.is_file()
    {
        return Ok(p.to_path_buf());
    }
    let path = work_dir.join(format!("src_{}x{}.mp4", case.width, case.height));
    if path.is_file() {
        return Ok(path);
    }
    let spec = format!(
        "color=c=red:s={}x{}:d={}:r={}",
        case.width, case.height, case.secs, case.fps
    );
    let tools = FfmpegTools::discover()?;
    let status = Command::new(&tools.ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &spec,
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "30",
        ])
        .arg(&path)
        .status()
        .map_err(|e| IoError::process(format!("lavfi source: {e}")))?;
    if !status.success() {
        return Err(IoError::process(format!("lavfi source failed: {status}")));
    }
    Ok(path)
}

fn run_once(case: &E2eCase, source: &Path, out: &Path) -> Result<()> {
    let clip = build_clip(case, source)?;
    let opts = write_opts(case, out);
    if case.with_audio {
        let audio = SilenceClip::new(reelforge_core::AudioFormat::STEREO_48K, clip.duration());
        write_av(clip.as_ref(), &audio, &opts)?;
    } else {
        write_video(clip.as_ref(), &opts)?;
    }
    Ok(())
}

fn build_clip(case: &E2eCase, source: &Path) -> Result<Arc<dyn VideoClip>> {
    let opened = open_video(&OpenVideoOptions::new(source.to_string_lossy()).video_only())?;
    let src: Arc<dyn VideoClip> =
        if opened.size().width == case.width && opened.size().height == case.height {
            Arc::new(opened)
        } else {
            Resize::to(Size::new(case.width, case.height))
                .with_filter(ResizeFilter::Bilinear)
                .apply(Arc::new(opened))
                .map_err(IoError::from)?
        };
    match case.kind {
        E2eKind::Privacy => {
            let tracks = crowd_tracks(
                case.subjects.max(1),
                case.width,
                case.height,
                case.dense_masks,
            );
            TrackedPrivacy::gaussian(tracks, 8.0)
                .apply(src)
                .map_err(IoError::from)
        }
        E2eKind::Edit => {
            let crop_w = (case.width * 9 / 10).max(2) & !1;
            let crop_h = (case.height * 9 / 10).max(2) & !1;
            let x = (case.width - crop_w) / 2;
            let y = (case.height - crop_h) / 2;
            let target = Size::new(((crop_w / 2).max(2)) & !1, ((crop_h / 2).max(2)) & !1);
            let cropped = Crop::new(x, y, crop_w, crop_h)
                .apply(src)
                .map_err(IoError::from)?;
            let resized = Resize::to(target)
                .with_filter(ResizeFilter::Bilinear)
                .apply(cropped)
                .map_err(IoError::from)?;
            let faded = FadeIn::new(Duration::from_secs((case.secs * 0.1).clamp(0.05, 0.4)))
                .apply(resized)
                .map_err(IoError::from)?;
            BlackAndWhite.apply(faded).map_err(IoError::from)
        }
    }
}

fn write_opts(case: &E2eCase, out: &Path) -> WriteVideoOptions {
    let mut opts = WriteVideoOptions::new(out.to_string_lossy(), case.fps)
        .with_video_codec(case.codec.ffmpeg_name());
    if case.codec.uses_crf() {
        opts = opts.with_crf(28);
    } else {
        opts = opts
            .without_crf()
            .with_extra_args(["-preset", "p4", "-b:v", "0"]);
    }
    opts
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn crowd_tracks(n: usize, width: u32, height: u32, dense: bool) -> TrackSet {
    let mut set = TrackSet::new();
    let cols = ((n as f32).sqrt().ceil() as usize).max(1);
    let rows = n.div_ceil(cols);
    let cell_w = (width as f32) / cols as f32;
    let cell_h = (height as f32) / rows as f32;
    let radius = (cell_w.min(cell_h) * 0.22).max(4.0);
    for i in 0..n {
        let col = i % cols;
        let row = i / cols;
        let cx = (col as f32 + 0.5) * cell_w;
        let cy = (row as f32 + 0.5) * cell_h;
        let mut sample = RegionSample::new(0.0, cx, cy, radius);
        if dense {
            sample = sample.with_coverage(dot_mask(cx, cy, radius));
        }
        let mut tr = RegionTrack::new(format!("s{i}"));
        tr.push(sample);
        set.push(tr);
    }
    set
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn dot_mask(cx: f32, cy: f32, radius: f32) -> CoverageMask {
    let r = radius.ceil().max(2.0) as u32;
    let left = (cx - radius).floor().max(0.0) as u32;
    let top = (cy - radius).floor().max(0.0) as u32;
    let w = (r * 2).max(2);
    let h = w;
    let mut data = vec![0_u8; (w * h) as usize];
    let cr = r as f32;
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cr;
            let dy = y as f32 - cr;
            if dx * dx + dy * dy <= cr * cr {
                data[(y * w + x) as usize] = 255;
            }
        }
    }
    CoverageMask {
        left,
        top,
        width: w,
        height: h,
        data: Arc::new(data),
    }
}

fn probe_av_drift(path: &Path, _fps: f64) -> Option<f64> {
    let tools = FfmpegTools::discover().ok()?;
    let v = crate::ffmpeg::probe_video(&tools, path).ok()?;
    let a = crate::ffmpeg::probe_audio(&tools, path).ok()?;
    Some(av_duration_drift(
        v.duration.as_secs(),
        a.duration.as_secs(),
    ))
}

fn time_ffmpeg_edit(case: &E2eCase, source: &Path, work_dir: &Path) -> Result<f64> {
    let tools = FfmpegTools::discover()?;
    let crop_w = (case.width * 9 / 10).max(2) & !1;
    let crop_h = (case.height * 9 / 10).max(2) & !1;
    let x = (case.width - crop_w) / 2;
    let y = (case.height - crop_h) / 2;
    let tw = ((crop_w / 2).max(2)) & !1;
    let th = ((crop_h / 2).max(2)) & !1;
    let vf = format!("crop={crop_w}:{crop_h}:{x}:{y},scale={tw}:{th},hue=s=0");
    let out = work_dir.join(format!("{}_ffmpeg.mp4", case.name));
    let t0 = Instant::now();
    let status = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(source)
        .args([
            "-vf",
            &vf,
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-crf",
            "28",
        ])
        .arg(&out)
        .status()
        .map_err(|e| IoError::process(format!("ffmpeg baseline: {e}")))?;
    if !status.success() {
        return Err(IoError::process(format!(
            "ffmpeg baseline failed: {status}"
        )));
    }
    Ok(t0.elapsed().as_secs_f64() * 1000.0)
}

/// Pretty one-line summary.
#[must_use]
pub fn format_report(r: &E2eReport) -> String {
    if let Some(why) = &r.skipped {
        return format!("{:<32} SKIP {why}", r.name);
    }
    let rss = r.peak_rss_bytes.map_or_else(
        || "-".into(),
        |b| {
            #[allow(clippy::cast_precision_loss)]
            let mb = b as f64 / 1_048_576.0;
            format!("{mb:.1}MB")
        },
    );
    let ff = r
        .ffmpeg_ms
        .map_or_else(|| "-".into(), |m| format!("{m:.0}"));
    let drift = r
        .av_drift_secs
        .map_or_else(|| "-".into(), |d| format!("{d:.3}s"));
    format!(
        "{:<32} {:>5}x{:<4} n={:<3} {:>8.0} / {:>8.0} ms  rss={rss}  ff={ff}  drift={drift}  out={}",
        r.name, r.width, r.height, r.subjects, r.p50_ms, r.p95_ms, r.output_bytes
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_known_set() {
        let s = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((percentile(&s, 0.50) - 3.0).abs() < 1e-9);
        assert!((percentile(&s, 0.0) - 1.0).abs() < 1e-9);
        assert!((percentile(&s, 1.0) - 5.0).abs() < 1e-9);
        assert!(percentile(&[], 0.5).abs() < 1e-12);
    }

    #[test]
    fn smoke_case_is_tiny() {
        let c = E2eCase::smoke();
        assert!(c.width <= 320 && c.subjects <= 10);
    }

    #[test]
    fn crowd_dense_has_coverage() {
        let set = crowd_tracks(4, 64, 64, true);
        assert_eq!(set.len(), 4);
        assert!(set.tracks[0].samples[0].coverage.is_some());
    }

    #[test]
    fn matrices_cover_the_agreed_axes() {
        let names: Vec<String> = standard_cases().into_iter().map(|c| c.name).collect();
        assert!(names.iter().any(|n| n == "privacy_1080p_50_h264"));
        assert!(names.iter().any(|n| n == "privacy_1080p_50_dense_h264"));
        assert!(names.iter().any(|n| n == "privacy_1080p_50_nvenc"));
        assert!(names.iter().any(|n| n == "edit_1080p_h264"));
        assert!(names.iter().any(|n| n == "av_1080p_h264"));
        let full: Vec<String> = full_cases().into_iter().map(|c| c.name).collect();
        assert!(full.iter().any(|n| n == "privacy_4k_50_h264"));
        assert!(full.iter().any(|n| n == "privacy_1080p_100_h264"));
    }
}
