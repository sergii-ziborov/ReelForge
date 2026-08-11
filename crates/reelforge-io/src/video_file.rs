//! File-backed video clips via the `FFmpeg` CLI.

use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, decode_frame_rgb, probe_video};
use crate::options::OpenVideoOptions;
use reelforge_core::{CoreError, Duration, Frame, Size, Time, VideoClip};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Video clip backed by a media file on disk.
///
/// Frames are decoded on demand through `ffmpeg` (seek + single-frame extract).
/// A one-frame cache reduces duplicate work for sequential writers.
#[derive(Debug)]
pub struct VideoFileClip {
    path: PathBuf,
    size: Size,
    duration: Duration,
    fps: f64,
    tools: FfmpegTools,
    cache: Mutex<Option<(u64, Frame)>>,
}

impl VideoFileClip {
    /// Open a video file using default options.
    ///
    /// # Errors
    ///
    /// Returns tool, probe, or path errors.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let options = OpenVideoOptions::new(path.as_ref().to_string_lossy());
        Self::open_with(&options)
    }

    /// Open a video file with options.
    ///
    /// # Errors
    ///
    /// Returns tool, probe, or path errors.
    pub fn open_with(options: &OpenVideoOptions) -> Result<Self> {
        let path = PathBuf::from(&options.path);
        if !path.is_file() {
            return Err(IoError::message(format!(
                "video file not found: {}",
                path.display()
            )));
        }
        let tools = FfmpegTools::discover()?;
        let probe = probe_video(&tools, &path)?;
        if !probe.duration.is_positive() {
            return Err(IoError::probe("video duration is zero"));
        }
        let _ = options.with_audio; // reserved for multi-track open
        Ok(Self {
            path,
            size: probe.size,
            duration: probe.duration,
            fps: probe.fps,
            tools,
            cache: Mutex::new(None),
        })
    }

    /// Source path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Nominal frames per second from the container.
    #[must_use]
    pub fn source_fps(&self) -> f64 {
        self.fps
    }

    fn frame_index(&self, t: Time) -> u64 {
        if self.fps <= 0.0 {
            return 0;
        }
        let idx = (t.as_secs() * self.fps).floor();
        if idx < 0.0 {
            0
        } else {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            {
                idx as u64
            }
        }
    }
}

impl VideoClip for VideoFileClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.size
    }

    fn fps(&self) -> Option<f64> {
        Some(self.fps)
    }

    fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }

        let index = self.frame_index(t);
        if let Ok(guard) = self.cache.lock()
            && let Some((cached_idx, frame)) = guard.as_ref()
            && *cached_idx == index
        {
            return Ok(frame.clone());
        }

        let frame = decode_frame_rgb(&self.tools, &self.path, self.size, t)
            .map_err(|e| CoreError::invalid_frame(format!("decode failed at {t}: {e}")))?;

        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some((index, frame.clone()));
        }
        Ok(frame)
    }
}

// Manual Clone: fresh cache per clone.
impl Clone for VideoFileClip {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            size: self.size,
            duration: self.duration,
            fps: self.fps,
            tools: self.tools.clone(),
            cache: Mutex::new(None),
        }
    }
}

/// Open a video file (convenience wrapper).
///
/// # Errors
///
/// Propagates [`VideoFileClip::open_with`] errors.
pub fn open_video(options: &OpenVideoOptions) -> Result<VideoFileClip> {
    VideoFileClip::open_with(options)
}
