//! Media metadata via `ffprobe`.

use crate::error::{IoError, Result};
use crate::ffmpeg::path::FfmpegTools;
use reelforge_core::{Duration, Size};
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
    /// Nominal frames per second.
    pub fps: f64,
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
    sample_rate: Option<String>,
    channels: Option<u16>,
}

/// Probe the primary video stream of `path`.
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

    let fps = parse_frame_rate(
        video
            .avg_frame_rate
            .as_deref()
            .or(video.r_frame_rate.as_deref()),
    )
    .unwrap_or(24.0);
    if !(fps.is_finite() && fps > 0.0) {
        return Err(IoError::probe(format!("invalid fps {fps}")));
    }

    let duration_secs = first_duration(&[
        video.duration.as_deref(),
        doc.format.as_ref().and_then(|f| f.duration.as_deref()),
    ])
    .ok_or_else(|| IoError::probe("duration missing"))?;

    Ok(VideoProbe {
        size,
        duration: Duration::from_secs(duration_secs),
        fps,
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
        return Some(n / d);
    }
    raw.parse().ok()
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
