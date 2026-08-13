//! Media metadata via `ffprobe`.

use crate::error::{IoError, Result};
use crate::ffmpeg::path::FfmpegTools;
use reelforge_core::{
    ColorInfo, ColorPrimaries, ColorRange, ColorSpace, ColorTransfer, Duration, MediaTime,
    PixelFormat, Size,
};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// Video stream metadata used to build [`crate::VideoFileClip`].
#[derive(Debug, Clone)]
pub struct VideoProbe {
    /// Frame size.
    pub size: Size,
    /// Duration of the media.
    pub duration: Duration,
    /// Nominal frames per second (`avg_frame_rate`, else `r_frame_rate`).
    pub fps: f64,
    /// Average frame rate when reported.
    pub avg_fps: Option<f64>,
    /// Base / codec frame rate when reported (`r_frame_rate`).
    pub r_fps: Option<f64>,
    /// Stream `time_base` numerator (e.g. `1` in `1/90000`).
    pub time_base_num: u32,
    /// Stream `time_base` denominator (e.g. `90000`).
    pub time_base_den: u32,
    /// Duration in stream time-base ticks when `duration_ts` is present.
    pub duration_ts: Option<i64>,
    /// Container/stream frame count when `nb_frames` is present.
    pub nb_frames: Option<u64>,
    /// True when avg/r rates diverge or rates are missing in a VFR-like way.
    pub is_vfr: bool,
    /// Color tags from the stream (`color_range` / `space` / primaries / transfer).
    pub color: ColorInfo,
    /// Raw `ffprobe` `pix_fmt` when present.
    pub pix_fmt: Option<String>,
    /// Decode target for [`crate::VideoFileClip::surface_at`] (YUV stays YUV).
    pub pixel_format: PixelFormat,
}

impl VideoProbe {
    /// Stream timescale for [`MediaTime`] (`time_base` denominator).
    #[must_use]
    pub fn timescale(&self) -> u32 {
        self.time_base_den.max(1)
    }

    /// Duration as [`MediaTime`] when `duration_ts` is known; else from seconds.
    #[must_use]
    pub fn duration_media(&self) -> MediaTime {
        if let Some(ts) = self.duration_ts {
            return MediaTime::from_pts(ts, self.time_base_num.max(1), self.time_base_den.max(1))
                .unwrap_or_else(|_| {
                    MediaTime::from_secs(self.duration.as_secs(), self.timescale())
                        .unwrap_or_else(|_| MediaTime::zero(self.timescale()))
                });
        }
        MediaTime::from_secs(self.duration.as_secs(), self.timescale())
            .unwrap_or_else(|_| MediaTime::zero(self.timescale()))
    }

    /// CFR half-open frame range for `[start, end)` using nominal fps.
    #[must_use]
    pub fn frame_range_cfr(&self, start: MediaTime, end: MediaTime) -> (u64, u64) {
        MediaTime::frame_range_cfr(start, end, self.fps)
    }
}

/// Audio stream metadata used to build [`crate::AudioFileClip`].
#[derive(Debug, Clone)]
pub struct AudioProbe {
    /// Duration of the media.
    pub duration: Duration,
    /// Sample rate in Hz when reported.
    pub sample_rate: Option<u32>,
    /// Channel count when reported.
    pub channels: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct ProbeDoc {
    format: Option<FormatSection>,
    streams: Option<Vec<StreamSection>>,
}

#[derive(Debug, Deserialize)]
struct FormatSection {
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamSection {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    duration_ts: Option<i64>,
    time_base: Option<String>,
    nb_frames: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u16>,
    color_range: Option<String>,
    color_space: Option<String>,
    color_primaries: Option<String>,
    color_transfer: Option<String>,
    pix_fmt: Option<String>,
}

/// Probe the primary video stream of `path`.
///
/// Populates VFR-related fields (`time_base`, `avg`/`r` rates, `duration_ts`,
/// `is_vfr`). Does **not** load a full PTS index (see
/// [`crate::ffmpeg::timing::probe_frame_timing`]).
///
/// # Errors
///
/// Returns I/O or probe errors when `ffprobe` fails or metadata is missing.
pub fn probe_video(tools: &FfmpegTools, path: &Path) -> Result<VideoProbe> {
    let doc = run_probe(tools, path)?;
    let streams = doc.streams.unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| IoError::probe("no video stream"))?;

    let width = video
        .width
        .ok_or_else(|| IoError::probe("video width missing"))?;
    let height = video
        .height
        .ok_or_else(|| IoError::probe("video height missing"))?;
    let size = Size::new(width, height)
        .require_positive()
        .map_err(|e| IoError::probe(e.to_string()))?;

    let avg_fps = parse_frame_rate(video.avg_frame_rate.as_deref());
    let r_fps = parse_frame_rate(video.r_frame_rate.as_deref());
    let fps = avg_fps.or(r_fps).unwrap_or(24.0);
    if !(fps.is_finite() && fps > 0.0) {
        return Err(IoError::probe(format!("invalid fps {fps}")));
    }

    let (tb_num, tb_den) = parse_time_base(video.time_base.as_deref()).unwrap_or((1, 90_000));

    let duration_ts = video.duration_ts.filter(|&t| t > 0);
    let duration_secs = duration_from_ts(duration_ts, tb_num, tb_den)
        .or_else(|| {
            first_duration(&[
                video.duration.as_deref(),
                doc.format.as_ref().and_then(|f| f.duration.as_deref()),
            ])
        })
        .ok_or_else(|| IoError::probe("duration missing"))?;

    let nb_frames = video
        .nb_frames
        .as_deref()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&n| n > 0);

    let is_vfr = detect_vfr(avg_fps, r_fps, nb_frames, duration_secs, fps);
    let pix_fmt = video.pix_fmt.clone();
    let pixel_format = pix_fmt
        .as_deref()
        .map_or(PixelFormat::Yuv420p, PixelFormat::from_ffmpeg_pix_fmt);

    Ok(VideoProbe {
        size,
        duration: Duration::from_secs(duration_secs),
        fps,
        avg_fps,
        r_fps,
        time_base_num: tb_num,
        time_base_den: tb_den,
        duration_ts,
        nb_frames,
        is_vfr,
        color: parse_color_info(
            video.color_range.as_deref(),
            video.color_space.as_deref(),
            video.color_primaries.as_deref(),
            video.color_transfer.as_deref(),
            video.pix_fmt.as_deref(),
        ),
        pix_fmt,
        pixel_format,
    })
}

pub(crate) fn parse_color_info(
    range: Option<&str>,
    space: Option<&str>,
    primaries: Option<&str>,
    transfer: Option<&str>,
    pix_fmt: Option<&str>,
) -> ColorInfo {
    let mut info = ColorInfo {
        range: parse_color_range(range),
        space: parse_color_space(space),
        primaries: parse_color_primaries(primaries),
        transfer: parse_color_transfer(transfer),
    };
    if info.range == ColorRange::Unspecified
        && pix_fmt.is_some_and(|p| p.starts_with("yuvj") || p.contains("rgb") || p.contains("gbr"))
    {
        info.range = ColorRange::Full;
    }
    info
}

fn parse_color_range(raw: Option<&str>) -> ColorRange {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("tv" | "mpeg" | "limited") => ColorRange::Limited,
        Some("pc" | "jpeg" | "full") => ColorRange::Full,
        _ => ColorRange::Unspecified,
    }
}

fn parse_color_space(raw: Option<&str>) -> ColorSpace {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("rgb" | "gbr") => ColorSpace::Rgb,
        Some("bt709") => ColorSpace::Bt709,
        Some("bt2020nc" | "bt2020_ncl" | "bt2020") => ColorSpace::Bt2020,
        Some("bt470bg" | "smpte170m" | "bt601") => ColorSpace::Bt601,
        _ => ColorSpace::Unspecified,
    }
}

fn parse_color_primaries(raw: Option<&str>) -> ColorPrimaries {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("bt709") => ColorPrimaries::Bt709,
        Some("bt2020") => ColorPrimaries::Bt2020,
        Some("bt470bg" | "smpte170m" | "bt601") => ColorPrimaries::Bt601,
        _ => ColorPrimaries::Unspecified,
    }
}

fn parse_color_transfer(raw: Option<&str>) -> ColorTransfer {
    match raw.map(str::to_ascii_lowercase).as_deref() {
        Some("bt709" | "iec61966-2-1" | "bt470bg") => ColorTransfer::Bt709,
        Some("linear") => ColorTransfer::Linear,
        Some("smpte2084") => ColorTransfer::Smpte2084,
        Some("arib-std-b67") => ColorTransfer::Hlg,
        _ => ColorTransfer::Unspecified,
    }
}

/// Whether `path` has at least one audio stream.
///
/// # Errors
///
/// `ffprobe` spawn / parse failures.
pub fn probe_has_audio(tools: &FfmpegTools, path: &Path) -> Result<bool> {
    let doc = run_probe(tools, path)?;
    Ok(doc
        .streams
        .unwrap_or_default()
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio")))
}

/// Probe the primary audio stream of `path`.
///
/// # Errors
///
/// Returns I/O or probe errors when `ffprobe` fails or metadata is missing.
pub fn probe_audio(tools: &FfmpegTools, path: &Path) -> Result<AudioProbe> {
    let doc = run_probe(tools, path)?;
    let streams = doc.streams.unwrap_or_default();
    let audio = streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"))
        .ok_or_else(|| IoError::probe("no audio stream"))?;

    let duration_secs = first_duration(&[
        audio.duration.as_deref(),
        doc.format.as_ref().and_then(|f| f.duration.as_deref()),
    ])
    .ok_or_else(|| IoError::probe("audio duration missing"))?;

    let sample_rate = audio
        .sample_rate
        .as_deref()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&r| r > 0);

    Ok(AudioProbe {
        duration: Duration::from_secs(duration_secs),
        sample_rate,
        channels: audio.channels,
    })
}

fn run_probe(tools: &FfmpegTools, path: &Path) -> Result<ProbeDoc> {
    let output = Command::new(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .map_err(|e| IoError::process(format!("ffprobe spawn failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!(
            "ffprobe exited {}: {stderr}",
            output.status
        )));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| IoError::probe(format!("invalid ffprobe json: {e}")))
}

fn parse_frame_rate(raw: Option<&str>) -> Option<f64> {
    let raw = raw?;
    if raw == "0/0" || raw.is_empty() {
        return None;
    }
    if let Some((num, den)) = raw.split_once('/') {
        let n: f64 = num.parse().ok()?;
        let d: f64 = den.parse().ok()?;
        if d == 0.0 {
            return None;
        }
        let v = n / d;
        if v.is_finite() && v > 0.0 {
            return Some(v);
        }
        return None;
    }
    raw.parse().ok().filter(|v: &f64| v.is_finite() && *v > 0.0)
}

fn parse_time_base(raw: Option<&str>) -> Option<(u32, u32)> {
    let raw = raw?;
    let (num, den) = raw.split_once('/')?;
    let n: u32 = num.parse().ok()?;
    let d: u32 = den.parse().ok()?;
    if n == 0 || d == 0 {
        return None;
    }
    Some((n, d))
}

fn duration_from_ts(duration_ts: Option<i64>, num: u32, den: u32) -> Option<f64> {
    let ts = duration_ts?;
    if ts <= 0 || den == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let secs = (ts as f64) * f64::from(num) / f64::from(den);
    if secs.is_finite() && secs > 0.0 {
        Some(secs)
    } else {
        None
    }
}

fn first_duration(candidates: &[Option<&str>]) -> Option<f64> {
    for s in candidates.iter().flatten() {
        if let Ok(v) = s.parse::<f64>()
            && v.is_finite()
            && v > 0.0
        {
            return Some(v);
        }
    }
    None
}

/// Heuristic: rates disagree, or frame count disagrees with duration×fps.
fn detect_vfr(
    avg: Option<f64>,
    r: Option<f64>,
    nb_frames: Option<u64>,
    duration_secs: f64,
    fps: f64,
) -> bool {
    if let (Some(a), Some(rr)) = (avg, r)
        && a > 0.0
        && rr > 0.0
    {
        let rel = (a - rr).abs() / a.max(rr);
        if rel > 0.02 {
            return true;
        }
    }
    // Only one rate present as 0/0-style missing other → mild signal only if nb_frames mismatches.
    if let Some(n) = nb_frames
        && duration_secs > 0.0
        && fps > 0.0
    {
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let expected = (duration_secs * fps).round();
        if expected > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            let err = (expected - n as f64).abs() / expected;
            if err > 0.05 {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_time_base_ok() {
        assert_eq!(parse_time_base(Some("1/90000")), Some((1, 90_000)));
        assert_eq!(parse_time_base(Some("0/1")), None);
        assert_eq!(parse_time_base(Some("bad")), None);
    }

    #[test]
    fn vfr_when_rates_diverge() {
        assert!(detect_vfr(Some(24.0), Some(30.0), None, 10.0, 24.0));
        assert!(!detect_vfr(Some(30.0), Some(30.0), Some(300), 10.0, 30.0));
        assert!(detect_vfr(Some(30.0), Some(30.0), Some(200), 10.0, 30.0));
    }

    #[test]
    fn duration_from_ts_90k() {
        let d = duration_from_ts(Some(180_000), 1, 90_000).unwrap();
        assert!((d - 2.0).abs() < 1e-9);
    }

    #[test]
    fn parses_color_tags_and_yuvj_implies_full() {
        let c = parse_color_info(
            Some("tv"),
            Some("bt709"),
            Some("bt709"),
            Some("smpte2084"),
            Some("yuv420p10le"),
        );
        assert_eq!(c.range, ColorRange::Limited);
        assert_eq!(c.space, ColorSpace::Bt709);
        assert_eq!(c.primaries, ColorPrimaries::Bt709);
        assert_eq!(c.transfer, ColorTransfer::Smpte2084);

        let jpeg = parse_color_info(None, None, None, None, Some("yuvj420p"));
        assert_eq!(jpeg.range, ColorRange::Full);
    }
}
