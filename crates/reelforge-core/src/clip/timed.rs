//! Subclip adapters for video and audio.

use super::traits::{AudioClip, VideoClip};
use crate::audio::{AudioBuffer, AudioFormat};
use crate::audio_time::AudioTimeline;
use crate::error::{CoreError, Result};
use crate::frame::{Frame, Mask};
use crate::layout::Size;
use crate::media_time::MediaTime;
use crate::surface::VideoSurface;
use crate::time::{Duration, Time};
use std::sync::Arc;

/// Validate subclip bounds against a parent duration.
fn validate_subclip(parent: Duration, start: Time, duration: Duration) -> Result<()> {
    if start.as_secs() < 0.0 || !start.as_secs().is_finite() {
        return Err(CoreError::invalid_timing(
            "subclip start must be finite >= 0",
        ));
    }
    if !duration.is_positive() {
        return Err(CoreError::invalid_timing("subclip duration must be > 0"));
    }
    let end = start.as_secs() + duration.as_secs();
    if end > parent.as_secs() + f64::EPSILON {
        return Err(CoreError::SubclipOutOfBounds {
            requested: (start, duration),
            parent,
        });
    }
    Ok(())
}

fn map_local(start: Time, duration: Duration, t: Time) -> Result<Time> {
    if t.as_secs() < 0.0 || t.as_secs() >= duration.as_secs() {
        return Err(CoreError::TimeOutOfRange {
            time: t,
            range: (Time::ZERO, Time::from_secs(duration.as_secs())),
        });
    }
    Ok(start + t.as_duration())
}

/// Video clip that presents a contiguous sub-range of another clip.
#[derive(Clone)]
pub struct TimedVideo {
    inner: Arc<dyn VideoClip>,
    start: Time,
    duration: Duration,
}

impl TimedVideo {
    /// Wrap `inner` as the subclip `[start, start + duration)`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::SubclipOutOfBounds`] or timing errors.
    pub fn new(inner: Arc<dyn VideoClip>, start: Time, duration: Duration) -> Result<Self> {
        validate_subclip(inner.duration(), start, duration)?;
        Ok(Self {
            inner,
            start,
            duration,
        })
    }
}

impl VideoClip for TimedVideo {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        self.inner
            .frame_at(map_local(self.start, self.duration, t)?)
    }

    fn surface_at(&self, t: Time) -> Result<VideoSurface> {
        self.inner
            .surface_at(map_local(self.start, self.duration, t)?)
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        self.inner.mask_at(map_local(self.start, self.duration, t)?)
    }
}

/// Audio clip that presents a contiguous sub-range of another clip.
#[derive(Clone)]
pub struct TimedAudio {
    inner: Arc<dyn AudioClip>,
    start: Time,
    duration: Duration,
}

impl TimedAudio {
    /// Wrap `inner` as the subclip `[start, start + duration)`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::SubclipOutOfBounds`] or timing errors.
    pub fn new(inner: Arc<dyn AudioClip>, start: Time, duration: Duration) -> Result<Self> {
        validate_subclip(inner.duration(), start, duration)?;
        Ok(Self {
            inner,
            start,
            duration,
        })
    }
}

impl AudioClip for TimedAudio {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        self.inner
            .samples_at(map_local(self.start, self.duration, t)?, frame_count)
    }

    fn samples_at_media(&self, t: MediaTime, frame_count: usize) -> Result<AudioBuffer> {
        let mapped = map_local(self.start, self.duration, t.to_time())?;
        let rate = self.inner.format().sample_rate.max(1);
        let mt = MediaTime::from_time(mapped, rate).unwrap_or_else(|_| MediaTime::zero(rate));
        self.inner.samples_at_media(mt, frame_count)
    }

    fn audio_timeline(&self) -> Option<AudioTimeline> {
        self.inner.audio_timeline()
    }
}

/// Subclip a video source into an [`Arc<dyn VideoClip>`].
///
/// # Errors
///
/// Propagates [`TimedVideo::new`] errors.
pub fn subclip_video(
    clip: Arc<dyn VideoClip>,
    start: Time,
    duration: Duration,
) -> Result<Arc<dyn VideoClip>> {
    Ok(Arc::new(TimedVideo::new(clip, start, duration)?))
}

/// Subclip an audio source into an [`Arc<dyn AudioClip>`].
///
/// # Errors
///
/// Propagates [`TimedAudio::new`] errors.
pub fn subclip_audio(
    clip: Arc<dyn AudioClip>,
    start: Time,
    duration: Duration,
) -> Result<Arc<dyn AudioClip>> {
    Ok(Arc::new(TimedAudio::new(clip, start, duration)?))
}
