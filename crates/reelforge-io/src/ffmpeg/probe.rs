//! Media metadata via `ffprobe`.

use crate::error::{IoError, Result};
use crate::ffmpeg::path::FfmpegTools;
use reelforge_core::{Duration, MediaTime, Size};
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
    })
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
}
