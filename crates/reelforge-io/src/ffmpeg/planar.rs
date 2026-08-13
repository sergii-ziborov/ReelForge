//! Planar / packed `rawvideo` decode (`yuv420p`, `nv12`, RGB).

use crate::error::{IoError, Result};
use crate::ffmpeg::path::FfmpegTools;
use crate::ffmpeg::stream::SequentialMode;
use reelforge_core::{PixelFormat, Size, SurfacePlane, split_packed_planes};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

/// Decode one tightly packed frame at media time `t` into image planes.
///
/// # Errors
///
/// Process, size, or plane-split failures.
pub fn decode_frame_planes(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    t: reelforge_core::Time,
    format: PixelFormat,
) -> Result<Vec<SurfacePlane>> {
    let expected = packed_len(format, size)?;
    let t_arg = format!("{:.6}", t.as_secs());
    let size_arg = format!("{}x{}", size.width, size.height);
    let pix = format.ffmpeg_raw_name();

    let output = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss", &t_arg, "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            pix,
            "-s",
            &size_arg,
            "-an",
            "pipe:1",
        ])
        .output()
        .map_err(|e| IoError::process(format!("ffmpeg planar decode spawn failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(IoError::process(format!(
            "ffmpeg planar decode failed at t={t} ({pix}): {stderr}"
        )));
    }
    if output.stdout.len() != expected {
        return Err(IoError::process(format!(
            "ffmpeg returned {} bytes, expected {expected} for {size:?} {pix}",
            output.stdout.len()
        )));
    }
    split_packed_planes(format, size, &output.stdout).map_err(IoError::from)
}

/// Ordered rawvideo decoder that emits native planes (YUV / NV12 / packed RGB).
pub struct SequentialPlanarDecoder {
    tools: FfmpegTools,
    path: PathBuf,
    size: Size,
    format: PixelFormat,
    mode: SequentialMode,
    frame_len: usize,
    child: Child,
    stdout: ChildStdout,
    next_index: u64,
}

impl SequentialPlanarDecoder {
    /// Start decoding `path` as `format` with `mode`.
    ///
    /// # Errors
    ///
    /// Spawn or size errors.
    pub fn open(
        tools: &FfmpegTools,
        path: &Path,
        size: Size,
        format: PixelFormat,
        mode: SequentialMode,
    ) -> Result<Self> {
        size.require_positive().map_err(IoError::from)?;
        if let SequentialMode::Cfr { fps } = mode
            && !(fps.is_finite() && fps > 0.0)
        {
            return Err(IoError::message(format!("invalid stream fps {fps}")));
        }
        let frame_len = packed_len(format, size)?;
        let (child, stdout) = spawn_planar(tools, path, size, format, mode)?;
        Ok(Self {
            tools: tools.clone(),
            path: path.to_path_buf(),
            size,
            format,
            mode,
            frame_len,
            child,
            stdout,
            next_index: 0,
        })
    }

    /// Read (or skip forward to) absolute frame `index`.
    ///
    /// # Errors
    ///
    /// Pipe / process failures.
    pub fn planes_at_index(&mut self, index: u64) -> Result<Vec<SurfacePlane>> {
        if index < self.next_index {
            self.restart()?;
        }
        while self.next_index < index {
            self.read_exact_frame_bytes()?;
            self.next_index += 1;
        }
        let data = self.read_exact_frame_bytes()?;
        self.next_index += 1;
        split_packed_planes(self.format, self.size, &data).map_err(IoError::from)
    }

    fn restart(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let (child, stdout) =
            spawn_planar(&self.tools, &self.path, self.size, self.format, self.mode)?;
        self.child = child;
        self.stdout = stdout;
        self.next_index = 0;
        Ok(())
    }

    fn read_exact_frame_bytes(&mut self) -> Result<Vec<u8>> {
        let mut buf = vec![0_u8; self.frame_len];
        self.stdout
            .read_exact(&mut buf)
            .map_err(|e| IoError::process(format!("sequential planar decode read failed: {e}")))?;
        Ok(buf)
    }
}

impl Drop for SequentialPlanarDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn packed_len(format: PixelFormat, size: Size) -> Result<usize> {
    format
        .packed_frame_bytes(size)
        .ok_or_else(|| IoError::message("planar frame size overflow"))
}

fn spawn_planar(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    format: PixelFormat,
    mode: SequentialMode,
) -> Result<(Child, ChildStdout)> {
    let size_arg = format!("{}x{}", size.width, size.height);
    let pix = format.ffmpeg_raw_name();
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-an", "-f", "rawvideo", "-pix_fmt", pix, "-s", &size_arg]);
    match mode {
        SequentialMode::Cfr { fps } => {
            cmd.args(["-r", &format!("{fps}")]);
        }
        SequentialMode::Native => {
            cmd.args(["-fps_mode", "passthrough"]);
        }
    }
    cmd.arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| IoError::process(format!("ffmpeg sequential planar spawn failed: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| IoError::process("ffmpeg sequential planar stdout missing"))?;
    Ok((child, stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_len_yuv420p() {
        assert_eq!(
            packed_len(PixelFormat::Yuv420p, Size::new(8, 4)).unwrap(),
            48
        );
    }
}
