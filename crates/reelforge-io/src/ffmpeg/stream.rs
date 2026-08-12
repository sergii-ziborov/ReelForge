//! Sequential raw RGB decode stream (one ffmpeg process, ordered frames).

use crate::error::{IoError, Result};
use crate::ffmpeg::path::FfmpegTools;
use reelforge_core::{Frame, FrameFormat, Size};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

/// Ordered RGB24 decoder: forward reads reuse one `ffmpeg` process.
///
/// Seeking backward restarts the process. Ideal for sequential `write_*` paths.
pub struct SequentialRgbDecoder {
    tools: FfmpegTools,
    path: PathBuf,
    size: Size,
    fps: f64,
    frame_len: usize,
    child: Child,
    stdout: ChildStdout,
    /// Index of the next frame that will be read from the pipe.
    next_index: u64,
}

impl SequentialRgbDecoder {
    /// Start decoding `path` as RGB24 at `size` / `fps`.
    ///
    /// # Errors
    ///
    /// Returns spawn or size errors.
    pub fn open(tools: &FfmpegTools, path: &Path, size: Size, fps: f64) -> Result<Self> {
        size.require_positive().map_err(IoError::from)?;
        if !(fps.is_finite() && fps > 0.0) {
            return Err(IoError::message(format!("invalid stream fps {fps}")));
        }
        let frame_len = usize::try_from(size.pixel_count().saturating_mul(3))
            .map_err(|_| IoError::message("frame size exceeds usize"))?;

        let (child, stdout) = spawn_stream(tools, path, size, fps)?;
        Ok(Self {
            tools: tools.clone(),
            path: path.to_path_buf(),
            size,
            fps,
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
    /// Returns I/O or process errors when the pipe fails or ends early.
    pub fn frame_at_index(&mut self, index: u64) -> Result<Frame> {
        if index < self.next_index {
            self.restart()?;
        }
        while self.next_index < index {
            self.read_exact_frame_bytes()?;
            self.next_index += 1;
        }
        let data = self.read_exact_frame_bytes()?;
        self.next_index += 1;
        Frame::from_raw(self.size, FrameFormat::Rgb8, data).map_err(IoError::from)
    }

    fn restart(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let (child, stdout) = spawn_stream(&self.tools, &self.path, self.size, self.fps)?;
        self.child = child;
        self.stdout = stdout;
        self.next_index = 0;
        Ok(())
    }

    fn read_exact_frame_bytes(&mut self) -> Result<Vec<u8>> {
        let mut buf = vec![0_u8; self.frame_len];
        self.stdout
            .read_exact(&mut buf)
            .map_err(|e| IoError::process(format!("sequential decode read failed: {e}")))?;
        Ok(buf)
    }
}

impl Drop for SequentialRgbDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_stream(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    fps: f64,
) -> Result<(Child, ChildStdout)> {
    let size_arg = format!("{}x{}", size.width, size.height);
    let fps_arg = format!("{fps}");
    let mut child = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-an", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &size_arg, "-r", &fps_arg, "pipe:1",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| IoError::process(format!("ffmpeg sequential spawn failed: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| IoError::process("ffmpeg sequential stdout missing"))?;
    Ok((child, stdout))
}
