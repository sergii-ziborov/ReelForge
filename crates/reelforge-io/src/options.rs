//! Open / write options.

use reelforge_core::{Duration, Size};

/// Options for writing a video file.
#[derive(Debug, Clone)]
pub struct WriteVideoOptions {
    /// Output path (UTF-8).
    pub path: String,
    /// Target frames per second.
    pub fps: f64,
    /// Optional override of output frame size (must match clip unless set for future scale).
    pub size: Option<Size>,
    /// Video codec name (default `libx264`).
    pub video_codec: Option<String>,
    /// Audio codec name (default `aac` when audio is written).
    pub audio_codec: Option<String>,
    /// Optional maximum duration to write (defaults to clip duration).
    pub duration: Option<Duration>,
    /// CRF quality for libx264-style encoders (`18`–`28` typical). `None` uses encoder default.
    pub crf: Option<u8>,
    /// Pixel format for the encoder input path after conversion (default `yuv420p`).
    pub pixel_format: Option<String>,
}

impl WriteVideoOptions {
    /// Write to `path` at `fps`.
    #[must_use]
    pub fn new(path: impl Into<String>, fps: f64) -> Self {
        Self {
            path: path.into(),
            fps,
            size: None,
            video_codec: None,
            audio_codec: None,
            duration: None,
            crf: Some(23),
            pixel_format: None,
        }
    }

    /// Override video codec.
    #[must_use]
    pub fn with_video_codec(mut self, codec: impl Into<String>) -> Self {
        self.video_codec = Some(codec.into());
        self
    }

    /// Override CRF.
    #[must_use]
    pub fn with_crf(mut self, crf: u8) -> Self {
        self.crf = Some(crf);
        self
    }

    /// Override audio codec (used by [`crate::write_av`]).
    #[must_use]
    pub fn with_audio_codec(mut self, codec: impl Into<String>) -> Self {
        self.audio_codec = Some(codec.into());
        self
    }

    /// Limit written duration.
    #[must_use]
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }
}

/// Options for opening a video file.
#[derive(Debug, Clone)]
pub struct OpenVideoOptions {
    /// Input path (UTF-8).
    pub path: String,
    /// Reserved: attach audio track when multi-track open is implemented.
    pub with_audio: bool,
}

impl OpenVideoOptions {
    /// Open media at `path`.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            with_audio: true,
        }
    }

    /// Disable audio association (video-only open).
    #[must_use]
    pub fn video_only(mut self) -> Self {
        self.with_audio = false;
        self
    }
}

/// Options for opening an audio file.
#[derive(Debug, Clone)]
pub struct OpenAudioOptions {
    /// Input path (UTF-8).
    pub path: String,
    /// Target sample rate for decoded PCM (default `48_000`).
    pub sample_rate: u32,
    /// Decode as stereo when true (default), else mono.
    pub stereo: bool,
}

impl OpenAudioOptions {
    /// Open audio at `path`.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            sample_rate: 48_000,
            stereo: true,
        }
    }
}
