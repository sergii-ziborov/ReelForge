//! Spawn `ffmpeg` for frame decode, audio decode, and rawvideo encode.

use crate::error::{IoError, Result};
use crate::ffmpeg::helpers::frame_to_rgb24;
use crate::ffmpeg::path::FfmpegTools;
use reelforge_core::{AudioBuffer, AudioFormat, Frame, FrameFormat, SampleLayout, Size, Time};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Decode a single RGB24 frame at media time `t` from `path`.
///
/// # Errors
///
/// Returns process or frame errors when decode fails.
pub fn decode_frame_rgb(tools: &FfmpegTools, path: &Path, size: Size, t: Time) -> Result<Frame> {
    let expected = size
        .pixel_count()
        .checked_mul(3)
        .ok_or_else(|| IoError::message("frame size overflow"))?;
    let expected =
        usize::try_from(expected).map_err(|_| IoError::message("frame size exceeds usize"))?;

    let t_arg = format!("{:.6}", t.as_secs());
    let size_arg = format!("{}x{}", size.width, size.height);

    let output = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss", &t_arg, "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            &size_arg,
            "-an",
            "pipe:1",
        ])
        .output()
        .map_err(|e| IoError::process(format!("ffmpeg decode spawn failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!(
            "ffmpeg decode failed at t={t}: {stderr}"
        )));
    }

    if output.stdout.len() != expected {
        return Err(IoError::process(format!(
            "ffmpeg returned {} bytes, expected {expected} for {size:?}",
            output.stdout.len()
        )));
    }

    Frame::from_raw(size, FrameFormat::Rgb8, output.stdout).map_err(IoError::from)
}

/// Decode an audio file to interleaved `f32` PCM at `format`.
///
/// # Errors
///
/// Returns process errors when decode fails.
pub fn decode_pcm_f32le(
    tools: &FfmpegTools,
    path: &Path,
    format: AudioFormat,
) -> Result<AudioBuffer> {
    let channels = format.channels().to_string();
    let rate = format.sample_rate.to_string();

    let output = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-vn", "-ac", &channels, "-ar", &rate, "-f", "f32le", "pipe:1",
        ])
        .output()
        .map_err(|e| IoError::process(format!("ffmpeg audio decode spawn failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!(
            "ffmpeg audio decode failed: {stderr}"
        )));
    }

    if !output.stdout.len().is_multiple_of(4) {
        return Err(IoError::process(
            "ffmpeg audio output length is not a multiple of 4",
        ));
    }

    let samples: Vec<f32> = output
        .stdout
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    AudioBuffer::from_interleaved(format, samples).map_err(IoError::from)
}

/// Encode RGB24 frames from `frames` into an H.264 (or other) file via `ffmpeg`.
///
/// # Errors
///
/// Returns process errors when encode fails.
#[allow(clippy::too_many_arguments)]
pub fn encode_rawvideo_h264(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    fps: f64,
    video_codec: &str,
    crf: Option<u8>,
    pixel_format: &str,
    extra_args: &[String],
    mut frames: impl Iterator<Item = Result<Frame>>,
) -> Result<()> {
    if !(fps.is_finite() && fps > 0.0) {
        return Err(IoError::message(format!("invalid fps {fps}")));
    }
    size.require_positive().map_err(IoError::from)?;

    // yuv420p requires even dimensions for most encoders.
    if !size.is_even() {
        return Err(IoError::message(format!(
            "output size {size:?} must be even for {pixel_format}"
        )));
    }

    let size_arg = format!("{}x{}", size.width, size.height);
    let fps_arg = format!("{fps}");
    let expected = usize::try_from(size.pixel_count().saturating_mul(3))
        .map_err(|_| IoError::message("frame size exceeds usize"))?;

    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-s",
        &size_arg,
        "-r",
        &fps_arg,
        "-i",
        "pipe:0",
        "-an",
        "-c:v",
        video_codec,
        "-pix_fmt",
        pixel_format,
    ]);
    if let Some(crf) = crf {
        cmd.args(["-crf", &crf.to_string()]);
    }
    if !extra_args.is_empty() {
        cmd.args(extra_args);
    }
    cmd.arg(path);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| IoError::process(format!("ffmpeg encode spawn failed: {e}")))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| IoError::process("ffmpeg stdin missing"))?;

    let write_result = (|| -> Result<()> {
        for frame in frames.by_ref() {
            let frame = frame?;
            if frame.size() != size {
                return Err(IoError::message(format!(
                    "frame size {:?} does not match output {size:?}",
                    frame.size()
                )));
            }
            let rgb = frame_to_rgb24(&frame).map_err(IoError::from)?;
            if rgb.len() != expected {
                return Err(IoError::message(format!(
                    "unexpected rgb length {}, expected {expected}",
                    rgb.len()
                )));
            }
            stdin
                .write_all(&rgb)
                .map_err(|e| IoError::process(format!("write to ffmpeg stdin failed: {e}")))?;
        }
        Ok(())
    })();

    // Drop stdin to signal EOF even on write errors.
    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|e| IoError::process(format!("ffmpeg encode wait failed: {e}")))?;

    write_result?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!("ffmpeg encode failed: {stderr}")));
    }

    Ok(())
}

/// Encode RGB24 frames to an animated GIF via `ffmpeg` (`gif` codec + palette).
///
/// # Errors
///
/// Returns process errors when encode fails.
pub fn encode_rawvideo_gif(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    fps: f64,
    mut frames: impl Iterator<Item = Result<Frame>>,
) -> Result<()> {
    if !(fps.is_finite() && fps > 0.0) {
        return Err(IoError::message(format!("invalid fps {fps}")));
    }
    size.require_positive().map_err(IoError::from)?;
    let size_arg = format!("{}x{}", size.width, size.height);
    let fps_arg = format!("{fps}");
    let expected = usize::try_from(size.pixel_count().saturating_mul(3))
        .map_err(|_| IoError::message("frame size exceeds usize"))?;

    // Two-pass palette in one filtergraph: palettegen + paletteuse.
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-s",
        &size_arg,
        "-r",
        &fps_arg,
        "-i",
        "pipe:0",
        "-an",
        "-filter_complex",
        "[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer",
        "-loop",
        "0",
    ]);
    cmd.arg(path);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| IoError::process(format!("ffmpeg gif spawn failed: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| IoError::process("ffmpeg gif stdin missing"))?;

    let write_result = (|| -> Result<()> {
        for frame in frames.by_ref() {
            let frame = frame?;
            if frame.size() != size {
                return Err(IoError::message(format!(
                    "frame size {:?} does not match output {size:?}",
                    frame.size()
                )));
            }
            let rgb = frame_to_rgb24(&frame).map_err(IoError::from)?;
            if rgb.len() != expected {
                return Err(IoError::message(format!(
                    "unexpected rgb length {}, expected {expected}",
                    rgb.len()
                )));
            }
            stdin
                .write_all(&rgb)
                .map_err(|e| IoError::process(format!("write to ffmpeg gif stdin failed: {e}")))?;
        }
        Ok(())
    })();
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|e| IoError::process(format!("ffmpeg gif wait failed: {e}")))?;
    write_result?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!("ffmpeg gif failed: {stderr}")));
    }
    Ok(())
}

/// Mux a video file with a raw `f32le` PCM audio file into `out_path`.
///
/// # Errors
///
/// Returns process errors when `ffmpeg` fails.
pub fn mux_video_audio(
    tools: &FfmpegTools,
    video_path: &Path,
    pcm_path: &Path,
    out_path: &Path,
    audio_codec: &str,
    sample_rate: u32,
    channels: u16,
) -> Result<()> {
    if sample_rate == 0 || channels == 0 {
        return Err(IoError::message("invalid audio format for mux"));
    }
    let rate = sample_rate.to_string();
    let ch = channels.to_string();

    let output = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video_path)
        .args(["-f", "f32le", "-ar", &rate, "-ac", &ch, "-i"])
        .arg(pcm_path)
        .args([
            "-c:v",
            "copy",
            "-c:a",
            audio_codec,
            "-shortest",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
        ])
        .arg(out_path)
        .output()
        .map_err(|e| IoError::process(format!("ffmpeg mux spawn failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!("ffmpeg mux failed: {stderr}")));
    }
    Ok(())
}

/// Build the default PCM format for audio decode.
#[must_use]
pub fn default_pcm_format(sample_rate: u32, stereo: bool) -> AudioFormat {
    AudioFormat {
        sample_rate,
        layout: if stereo {
            SampleLayout::Stereo
        } else {
            SampleLayout::Mono
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_format_stereo() {
        let f = default_pcm_format(48_000, true);
        assert_eq!(f.channels(), 2);
        let m = default_pcm_format(44_100, false);
        assert_eq!(m.channels(), 1);
    }
}
