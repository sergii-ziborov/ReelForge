//! Synthetic solid-color video and silent audio clips (no file I/O).

use crate::audio::{AudioBuffer, AudioFormat};
use crate::clip::{AudioClip, VideoClip};
use crate::color::Rgb8;
use crate::error::{CoreError, Result};
use crate::frame::Frame;
use crate::layout::Size;
use crate::time::{Duration, Time};
use std::sync::Arc;

/// Video clip that yields a constant color for its entire duration.
#[derive(Debug, Clone)]
pub struct ColorClip {
    size: Size,
    color: Rgb8,
    duration: Duration,
    fps: Option<f64>,
    /// Cached solid frame (built once on construction when size is valid).
    frame: Option<Arc<Frame>>,
}

impl ColorClip {
    /// Create a solid color clip.
    ///
    /// Invalid sizes surface on first sample via [`VideoClip::frame_at`].
    #[must_use]
    pub fn new(size: Size, color: Rgb8, duration: Duration) -> Self {
        let frame = Frame::solid_rgb(size, color).ok().map(Arc::new);
        Self {
            size,
            color,
            duration,
            fps: None,
            frame,
        }
    }

    /// Attach a nominal FPS (used by writers / previews).
    #[must_use]
    pub fn with_fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Fill color.
    #[must_use]
    pub const fn color(&self) -> Rgb8 {
        self.color
    }
}

impl VideoClip for ColorClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.size
    }

    fn fps(&self) -> Option<f64> {
        self.fps
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        if let Some(frame) = &self.frame {
            return Ok(Frame::clone(frame));
        }
        Frame::solid_rgb(self.size, self.color)
    }
}

/// Audio clip that yields digital silence.
#[derive(Debug, Clone)]
pub struct SilenceClip {
    format: AudioFormat,
    duration: Duration,
}

impl SilenceClip {
    /// Create a silent clip of the given duration and format.
    #[must_use]
    pub fn new(format: AudioFormat, duration: Duration) -> Self {
        Self { format, duration }
    }
}

impl AudioClip for SilenceClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        if !self.contains(t) && frame_count > 0 {
            if t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs() {
                return Err(CoreError::TimeOutOfRange {
                    time: t,
                    range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
                });
            }
        }
        AudioBuffer::silence(self.format, frame_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_clip_samples() {
        let clip = ColorClip::new(Size::new(8, 4), Rgb8::GREEN, Duration::from_secs(1.0));
        let frame = clip.frame_at(Time::from_secs(0.25)).unwrap();
        assert_eq!(frame.size(), Size::new(8, 4));
        assert_eq!(&frame.data()[0..3], &[0, 255, 0]);
        assert!(clip.frame_at(Time::from_secs(1.0)).is_err());
    }

    #[test]
    fn silence_clip() {
        let clip = SilenceClip::new(AudioFormat::STEREO_48K, Duration::from_secs(1.0));
        let buf = clip.samples_at(Time::ZERO, 100).unwrap();
        assert!(buf.samples().iter().all(|&s| s == 0.0));
    }
}
