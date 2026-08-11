//! Play a clip backwards.

use reelforge_core::{
    AudioBuffer, AudioClip, AudioEffect, AudioFormat, CoreError, Duration, Frame, Result, Size,
    Time, VideoClip, VideoEffect,
};
use std::sync::Arc;

/// Reverse timeline: `t` samples source at `duration - t`.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeMirror;

impl VideoEffect for TimeMirror {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(TimeMirroredVideo { inner: clip }))
    }
}

impl AudioEffect for TimeMirror {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        Ok(Arc::new(TimeMirroredAudio { inner: clip }))
    }
}

fn map_time(duration: Duration, t: Time) -> Result<Time> {
    let d = duration.as_secs();
    if t.as_secs() < 0.0 || t.as_secs() >= d {
        return Err(CoreError::TimeOutOfRange {
            time: t,
            range: (Time::ZERO, Time::from_secs(d)),
        });
    }
    // Reverse: t=0 → near end, t→duration → near start. Keep result in [0, d).
    let mut src = d - t.as_secs();
    if src >= d {
        src = (d - 1e-9).max(0.0);
    }
    if src < 0.0 {
        src = 0.0;
    }
    Ok(Time::from_secs(src))
}

struct TimeMirroredVideo {
    inner: Arc<dyn VideoClip>,
}

impl VideoClip for TimeMirroredVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let src = map_time(self.inner.duration(), t)?;
        self.inner.frame_at(src)
    }
}

struct TimeMirroredAudio {
    inner: Arc<dyn AudioClip>,
}

impl AudioClip for TimeMirroredAudio {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format(), 0);
        }
        let src = map_time(self.inner.duration(), t)?;
        // Phase 2: reverse window start only (true sample-reverse of the window is later).
        self.inner.samples_at(src, frame_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn time_mirror_keeps_duration() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::BLUE,
            Duration::from_secs(3.0),
        ));
        let out = VideoEffect::apply(&TimeMirror, clip).unwrap();
        assert!((out.duration().as_secs() - 3.0).abs() < 1e-9);
        let _ = out.frame_at(Time::from_secs(0.0)).unwrap();
        let _ = out.frame_at(Time::from_secs(2.9)).unwrap();
    }
}
