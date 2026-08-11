//! Core clip trait contracts.

use super::id::ClipId;
use crate::audio::{AudioBuffer, AudioFormat};
use crate::error::Result;
use crate::frame::{Frame, Mask};
use crate::layout::Size;
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

    /// Whether `t` is inside the active range.
    fn contains(&self, t: Time) -> bool {
        t.as_secs() >= 0.0 && t.as_secs() < self.duration().as_secs()
    }
}
