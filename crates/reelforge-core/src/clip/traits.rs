//! Core clip trait contracts.

use super::id::ClipId;
use crate::audio::{AudioBuffer, AudioFormat};
use crate::audio_time::AudioTimeline;
use crate::error::Result;
use crate::frame::{Frame, Mask};
use crate::layout::Size;
use crate::media_time::MediaTime;
use crate::surface::VideoSurface;
use crate::time::{Duration, Time};

/// Timed video source: maps media time to a raster (and optional mask).
///
/// Implementations are expected to be cheap to clone via [`std::sync::Arc`] and
/// free of interior timeline mutation after construction. Sampling outside
/// `[0, duration)` should return [`crate::CoreError::TimeOutOfRange`] unless the
/// concrete type documents otherwise.
pub trait VideoClip: Send + Sync {
    /// Active duration of this clip.
    fn duration(&self) -> Duration;

    /// Pixel size of frames produced by this clip.
    fn size(&self) -> Size;

    /// Nominal frames per second, when known (file sources). Synthetic clips
    /// may return `None`.
    fn fps(&self) -> Option<f64> {
        None
    }

    /// Optional stable id for graph tooling.
    fn id(&self) -> Option<&ClipId> {
        None
    }

    /// Sample a frame at local media time `t` where `0 <= t < duration`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::CoreError::TimeOutOfRange`] or format-specific failures.
    fn frame_at(&self, t: Time) -> Result<Frame>;

    /// Sample a timed [`VideoSurface`] at local time `t`.
    ///
    /// Default: wrap [`Self::frame_at`] with a microsecond PTS from `t` and a
    /// frame duration from `1/fps` when fps is known. File clips override this
    /// with stream PTS.
    ///
    /// # Errors
    ///
    /// Same as [`Self::frame_at`].
    fn surface_at(&self, t: Time) -> Result<VideoSurface> {
        let frame = self.frame_at(t)?;
        let ts = MediaTime::from_time(t, MediaTime::HZ_1K).unwrap_or_else(|_| MediaTime::zero(1));
        let duration = self.fps().and_then(|fps| {
            if fps.is_finite() && fps > 0.0 {
                MediaTime::from_secs(1.0 / fps, MediaTime::HZ_1K).ok()
            } else {
                None
            }
        });
        Ok(VideoSurface::from_frame(frame, ts, duration))
    }

    /// Optional alpha/coverage mask at time `t`. Default: fully opaque.
    ///
    /// # Errors
    ///
    /// Propagates sampling errors from the clip implementation.
    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        let _ = t;
        Ok(None)
    }

    /// Whether `t` is inside the active range.
    fn contains(&self, t: Time) -> bool {
        t.as_secs() >= 0.0 && t.as_secs() < self.duration().as_secs()
    }
}

/// Timed audio source: maps media time to PCM.
pub trait AudioClip: Send + Sync {
    /// Active duration of this clip.
    fn duration(&self) -> Duration;

    /// Sample format of buffers produced by this clip.
    fn format(&self) -> AudioFormat;

    /// Optional stable id for graph tooling.
    fn id(&self) -> Option<&ClipId> {
        None
    }

    /// Read `frame_count` sample frames starting at local time `t`.
    ///
    /// # Errors
    ///
    /// Returns [`crate::CoreError::TimeOutOfRange`] or format-specific failures.
    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer>;

    /// Sample PCM starting at exact [`MediaTime`].
    ///
    /// Default: convert `t` to floating [`Time`] and call [`Self::samples_at`].
    /// File clips override this with sample-accurate indexes.
    ///
    /// # Errors
    ///
    /// Same as [`Self::samples_at`].
    fn samples_at_media(&self, t: MediaTime, frame_count: usize) -> Result<AudioBuffer> {
        self.samples_at(t.to_time(), frame_count)
    }

    /// Timeline for this clip's sample clock, when the rate is known.
    #[must_use]
    fn audio_timeline(&self) -> Option<AudioTimeline> {
        AudioTimeline::from_format(self.format()).ok()
    }

    /// Whether `t` is inside the active range.
    fn contains(&self, t: Time) -> bool {
        t.as_secs() >= 0.0 && t.as_secs() < self.duration().as_secs()
    }
}
