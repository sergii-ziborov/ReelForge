//! Bounded sample → order → encode pipeline for rawvideo `ffmpeg` stdin.

use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, frame_to_rgb24, frame_to_rgb24_into};
use crate::pool::RgbFramePool;
use reelforge_core::{Frame, Size};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

/// Encode RGB24 frames produced by `sample(index)` into an H.264 (or other) file.
///
/// When `control.max_in_flight > 1`, samples frames on a bounded worker pool and
/// writes them in index order (backpressure via a sync channel).
///
/// # Errors
///
/// Returns tool, cancel, sample, or process errors.
#[allow(clippy::too_many_arguments)]
pub fn encode_sampled_h264(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    fps: f64,
    video_codec: &str,
    crf: Option<u8>,
    pixel_format: &str,
    extra_args: &[String],
    frame_count: u64,
    sample: &(dyn Fn(u64) -> Result<Frame> + Sync),
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

    let write_result = pump_frames(&mut stdin, size, expected, frame_count, sample, control);

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

fn pump_frames(
    stdin: &mut impl Write,
    size: Size,
    expected: usize,
    frame_count: u64,
    sample: &(dyn Fn(u64) -> Result<Frame> + Sync),
    control: &WriteControl,
) -> Result<()> {
    let depth = control.in_flight();
    if depth == 1 {
        pump_sequential(stdin, size, expected, frame_count, sample, control)
    } else {
        pump_parallel(stdin, size, expected, frame_count, sample, control, depth)
    }
}

fn pump_sequential(
    stdin: &mut impl Write,
    size: Size,
    expected: usize,
    frame_count: u64,
    sample: &(dyn Fn(u64) -> Result<Frame> + Sync),
    control: &WriteControl,
) -> Result<()> {
    let pool = RgbFramePool::new(expected, 2);
    for i in 0..frame_count {
        control.check_cancel()?;
        let frame = sample(i)?;
        if frame.size() != size {
            return Err(IoError::message(format!(
                "frame size {:?} does not match output {size:?}",
                frame.size()
            )));
        }
        let mut rgb = pool.take();
        frame_to_rgb24_into(&frame, &mut rgb).map_err(IoError::from)?;
        if rgb.len() != expected {
            return Err(IoError::message(format!(
                "unexpected rgb length {}, expected {expected}",
                rgb.len()
            )));
        }
        stdin
            .write_all(&rgb)
            .map_err(|e| IoError::process(format!("write to ffmpeg stdin failed: {e}")))?;
        pool.give(rgb);
        control.report(WriteProgress::new(WriteStage::Video, i + 1, frame_count));
    }
    Ok(())
}

fn pump_parallel(
    stdin: &mut impl Write,
    size: Size,
    expected: usize,
    frame_count: u64,
    sample: &(dyn Fn(u64) -> Result<Frame> + Sync),
    control: &WriteControl,
    depth: usize,
) -> Result<()> {
    // Sync channel provides backpressure: at most `depth` completed RGB frames
    // wait for ordered join / encode.
    let (tx, rx) = mpsc::sync_channel::<(u64, Result<Vec<u8>>)>(depth);
    let next = AtomicU64::new(0);
    let workers = depth;

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || {
                loop {
                    if control.check_cancel().is_err() {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i < frame_count {
                            let _ = tx.send((i, Err(IoError::Cancelled)));
                        }
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= frame_count {
                        break;
                    }
                    let result = (|| {
                        let frame = sample(i)?;
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
                        Ok(rgb)
                    })();
                    if tx.send((i, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut pending: BTreeMap<u64, Result<Vec<u8>>> = BTreeMap::new();
        let mut expect = 0_u64;
        while expect < frame_count {
            control.check_cancel()?;
            let (i, res) = rx
                .recv()
                .map_err(|_| IoError::process("encode pipeline worker channel closed early"))?;
            pending.insert(i, res);
            while let Some(res) = pending.remove(&expect) {
                let rgb = res?;
                stdin
                    .write_all(&rgb)
                    .map_err(|e| IoError::process(format!("write to ffmpeg stdin failed: {e}")))?;
                control.report(WriteProgress::new(
                    WriteStage::Video,
                    expect + 1,
                    frame_count,
                ));
                expect += 1;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::CancelToken;
    use reelforge_core::{ColorClip, Duration, Rgb8, Time, VideoClip};

    #[test]
    fn sequential_cancel_before_work() {
        let token = CancelToken::new();
        token.cancel();
        let control = WriteControl::new().with_cancel(token);
        let clip = ColorClip::new(Size::new(4, 4), Rgb8::RED, Duration::from_secs(1.0));
        let mut sink = Vec::new();
        let err = pump_sequential(
            &mut sink,
            Size::new(4, 4),
            48,
            5,
            &|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = Time::from_secs(i as f64 / 24.0);
                clip.frame_at(t).map_err(IoError::from)
            },
            &control,
        );
        assert!(matches!(err, Err(IoError::Cancelled)));
    }
}
