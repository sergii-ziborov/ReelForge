//! Encode native CPU surfaces (`yuv420p` / `nv12`) to `ffmpeg` without RGB.

use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::error::{IoError, Result};
use crate::ffmpeg::FfmpegTools;
use reelforge_core::{PixelFormat, Size};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Whether `format` can be piped as `ffmpeg` rawvideo without RGB conversion.
#[must_use]
pub fn is_native_raw_format(format: PixelFormat) -> bool {
    format.is_yuv()
}

/// Encode tight rawvideo samples (`sample(index)` → packed bytes).
///
/// `in_format` is the stdin `-pix_fmt`. `out_pixel_format` is the encoder
/// output (usually `yuv420p`).
///
/// # Errors
///
/// Tool, cancel, size, sample, or process failures.
#[allow(clippy::too_many_arguments)]
pub fn encode_sampled_rawvideo(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    fps: f64,
    video_codec: &str,
    crf: Option<u8>,
    in_format: PixelFormat,
    out_pixel_format: &str,
    extra_args: &[String],
    frame_count: u64,
    sample: &(dyn Fn(u64) -> Result<Vec<u8>> + Sync),
    control: &WriteControl,
) -> Result<()> {
    if frame_count == 0 {
        return Err(IoError::message("no frames to write"));
    }
    if !(fps.is_finite() && fps > 0.0) {
        return Err(IoError::message(format!("invalid fps {fps}")));
    }
    size.require_positive().map_err(IoError::from)?;
    if !size.is_even() {
        return Err(IoError::message(format!(
            "output size {size:?} must be even for {out_pixel_format}"
        )));
    }
    if !is_native_raw_format(in_format) {
        return Err(IoError::message(format!(
            "native encode expects YUV/NV12, got {in_format:?}"
        )));
    }
    let expected = in_format
        .packed_frame_bytes(size)
        .ok_or_else(|| IoError::message("native frame size overflow"))?;

    let size_arg = format!("{}x{}", size.width, size.height);
    let fps_arg = format!("{fps}");
    let in_pix = in_format.ffmpeg_raw_name();

    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        in_pix,
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
        out_pixel_format,
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
        .map_err(|e| IoError::process(format!("ffmpeg native encode spawn failed: {e}")))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| IoError::process("ffmpeg stdin missing"))?;

    let write_result = pump_raw(&mut stdin, expected, frame_count, sample, control);
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|e| IoError::process(format!("ffmpeg native encode wait failed: {e}")))?;
    write_result?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!(
            "ffmpeg native encode failed: {stderr}"
        )));
    }
    Ok(())
}

fn pump_raw(
    stdin: &mut impl Write,
    expected: usize,
    frame_count: u64,
    sample: &(dyn Fn(u64) -> Result<Vec<u8>> + Sync),
    control: &WriteControl,
) -> Result<()> {
    for i in 0..frame_count {
        control.check_cancel()?;
        let buf = sample(i)?;
        if buf.len() != expected {
            return Err(IoError::message(format!(
                "native frame {} bytes, expected {expected}",
                buf.len()
            )));
        }
        stdin
            .write_all(&buf)
            .map_err(|e| IoError::process(format!("write to ffmpeg stdin failed: {e}")))?;
        control.report(WriteProgress::new(WriteStage::Video, i + 1, frame_count));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yuv_and_nv12_are_native() {
        assert!(is_native_raw_format(PixelFormat::Yuv420p));
        assert!(is_native_raw_format(PixelFormat::Nv12));
        assert!(!is_native_raw_format(PixelFormat::Rgb8));
        assert!(!is_native_raw_format(PixelFormat::Bgra8));
    }
}
