//! Sequential raw RGB decode stream (one ffmpeg process, ordered frames).

use crate::error::{IoError, Result};
use crate::ffmpeg::path::FfmpegTools;
use reelforge_core::{Frame, FrameFormat, Size};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

/// How the sequential decoder times / counts output frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SequentialMode {
    /// Force constant output rate with `-r` (CFR resampling).
    Cfr {
        /// Output frames per second.
        fps: f64,
    },
    /// Pass through source frames without rate conversion (VFR-safe).
    ///
    /// Uses `-fps_mode passthrough` so packet count matches source frames.
    Native,
}

/// Ordered RGB24 decoder: forward reads reuse one `ffmpeg` process.
///
/// Seeking backward restarts the process. Ideal for sequential `write_*` paths.
pub struct SequentialRgbDecoder {
    tools: FfmpegTools,
    path: PathBuf,
    size: Size,
    mode: SequentialMode,
    frame_len: usize,
    child: Child,
    stdout: ChildStdout,
    /// Index of the next frame that will be read from the pipe.
    next_index: u64,
}

impl SequentialRgbDecoder {
    /// Start decoding `path` as RGB24 at `size` with CFR rate `fps`.
    ///
    /// # Errors
    ///
    /// Returns spawn or size errors.
    pub fn open(tools: &FfmpegTools, path: &Path, size: Size, fps: f64) -> Result<Self> {
        Self::open_with(tools, path, size, SequentialMode::Cfr { fps })
    }

    /// Native frame delivery (no `-r` resampling) for VFR sources.
    ///
    /// # Errors
    ///
    /// Returns spawn or size errors.
    pub fn open_native(tools: &FfmpegTools, path: &Path, size: Size) -> Result<Self> {
        Self::open_with(tools, path, size, SequentialMode::Native)
    }

    /// Start decoding with an explicit timing mode.
    ///
    /// # Errors
    ///
    /// Returns spawn or size errors.
    pub fn open_with(
        tools: &FfmpegTools,
        path: &Path,
        size: Size,
        mode: SequentialMode,
    ) -> Result<Self> {
        size.require_positive().map_err(IoError::from)?;
        if let SequentialMode::Cfr { fps } = mode
            && !(fps.is_finite() && fps > 0.0)
        {
            return Err(IoError::message(format!("invalid stream fps {fps}")));
        }
        let frame_len = usize::try_from(size.pixel_count().saturating_mul(3))
            .map_err(|_| IoError::message("frame size exceeds usize"))?;

        let (child, stdout) = spawn_stream(tools, path, size, mode)?;
        Ok(Self {
            tools: tools.clone(),
            path: path.to_path_buf(),
            size,
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
        let (child, stdout) = spawn_stream(&self.tools, &self.path, self.size, self.mode)?;
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
    mode: SequentialMode,
) -> Result<(Child, ChildStdout)> {
    let size_arg = format!("{}x{}", size.width, size.height);
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path)
        .args([
            "-an", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &size_arg,
        ]);
    match mode {
        SequentialMode::Cfr { fps } => {
            let fps_arg = format!("{fps}");
            cmd.args(["-r", &fps_arg]);
        }
        SequentialMode::Native => {
            // Preserve source frames (no CFR resample). Fallback for older ffmpeg:
            // vsync 0 is accepted by many builds if fps_mode is unknown.
            cmd.args(["-fps_mode", "passthrough"]);
        }
    }
    cmd.arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| IoError::process(format!("ffmpeg sequential spawn failed: {e}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| IoError::process("ffmpeg sequential stdout missing"))?;
    Ok((child, stdout))
}
