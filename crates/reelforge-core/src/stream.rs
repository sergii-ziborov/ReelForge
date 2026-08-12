//! Sequential frame streams for realtime / encode-style consumption.

use crate::cache::CachedVideo;
use crate::clip::VideoClip;
use crate::error::{CoreError, Result};
use crate::frame::Frame;
use crate::time::{Duration, Time};
use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;
use std::time::{Duration as StdDuration, Instant};

/// Sequential frame stream over a clip at a fixed fps.
///
/// Advances monotonically (best for file sequential decode + cache hits on
/// re-read of the current/recent window). Optional ring prefetch fills a
/// bounded buffer on a background worker for realtime consumers.
pub struct FrameStream {
    clip: Arc<dyn VideoClip>,
    fps: f64,
    next_index: u64,
    end_index: u64,
    /// Optional lookahead buffer filled by [`FrameStream::prefetch`].
    prefetch: VecDeque<(u64, Result<Frame>)>,
    prefetch_capacity: usize,
}

impl FrameStream {
    /// Stream `[0, duration)` at `fps` (or clip fps / 24 fallback).
    ///
    /// # Errors
    ///
    /// Returns when fps/duration are invalid.
    pub fn new(clip: Arc<dyn VideoClip>, fps: Option<f64>) -> Result<Self> {
        let fps = fps
            .or_else(|| clip.fps())
            .filter(|f| f.is_finite() && *f > 0.0)
            .unwrap_or(24.0);
        let duration = clip.duration();
        if !duration.is_positive() {
            return Err(CoreError::invalid_timing("stream duration must be > 0"));
        }
        let end_index = frame_count(duration, fps);
        Ok(Self {
            clip,
            fps,
            next_index: 0,
            end_index,
            prefetch: VecDeque::new(),
            prefetch_capacity: 0,
        })
    }

    /// Stream with an LRU cache wrapped around the clip (recommended for effects).
    ///
    /// # Errors
    ///
    /// Invalid fps/duration.
    pub fn cached(clip: Arc<dyn VideoClip>, cache_seconds: f64, fps: Option<f64>) -> Result<Self> {
        let fps_hint = fps.or_else(|| clip.fps());
        let cached = Arc::new(CachedVideo::realtime(clip, cache_seconds)) as Arc<dyn VideoClip>;
        let mut s = Self::new(cached, fps_hint)?;
        // Keep a small sequential window warm for realtime (about 0.5s).
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let win = ((s.fps * 0.5).ceil() as usize).clamp(2, 48);
        s.prefetch_capacity = win;
        Ok(s)
    }

    /// Frames per second of this stream.
    #[must_use]
    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// Next absolute frame index that will be produced.
    #[must_use]
    pub fn next_index(&self) -> u64 {
        self.next_index
    }

    /// Total frames in the stream.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.end_index
    }

    /// Whether the stream has no frames.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.end_index == 0
    }

    /// Whether more frames remain.
    #[must_use]
    pub fn has_remaining(&self) -> bool {
        self.next_index < self.end_index
    }

    /// Time of the next frame.
    #[must_use]
    pub fn next_time(&self) -> Time {
        time_at(self.next_index, self.fps)
    }

    /// Pull the next frame (from prefetch queue or by sampling).
    ///
    /// # Errors
    ///
    /// Propagates clip sampling errors.
    pub fn next_frame(&mut self) -> Result<Option<(u64, Time, Frame)>> {
        if self.next_index >= self.end_index {
            return Ok(None);
        }
        let idx = self.next_index;
        let t = time_at(idx, self.fps);
        let frame = if let Some((i, res)) = self.prefetch.pop_front() {
            debug_assert_eq!(i, idx);
            res?
        } else {
            self.clip.frame_at(t)?
        };
        self.next_index += 1;
        self.fill_prefetch();
        Ok(Some((idx, t, frame)))
    }

    /// Fill the prefetch ring up to `prefetch_capacity` (sync, same thread).
    pub fn fill_prefetch(&mut self) {
        if self.prefetch_capacity == 0 {
            return;
        }
        while self.prefetch.len() < self.prefetch_capacity {
            let idx = self.next_index + self.prefetch.len() as u64;
            if idx >= self.end_index {
                break;
            }
            let t = time_at(idx, self.fps);
            let res = self.clip.frame_at(t);
            self.prefetch.push_back((idx, res));
        }
    }

    /// Enable / resize sequential prefetch window (0 disables).
    pub fn set_prefetch_capacity(&mut self, n: usize) {
        self.prefetch_capacity = n;
        while self.prefetch.len() > self.prefetch_capacity {
            self.prefetch.pop_back();
        }
        self.fill_prefetch();
    }

    /// Realtime pump: yield frames paced at stream fps until end or `max_seconds`.
    ///
    /// Returns number of frames delivered. The callback receives `(index, time, frame, late_ms)`.
    ///
    /// # Errors
    ///
    /// Propagates sampling / callback errors.
    pub fn pump_realtime<F>(&mut self, max_seconds: Option<f64>, mut on_frame: F) -> Result<u64>
    where
        F: FnMut(u64, Time, Frame, f64) -> Result<()>,
    {
        let start = Instant::now();
        let period = StdDuration::from_secs_f64(1.0 / self.fps.max(1e-6));
        let deadline = max_seconds.map(|s| start + StdDuration::from_secs_f64(s));
        let mut count = 0_u64;
        let mut next_deadline = Instant::now();

        while self.has_remaining() {
            if let Some(d) = deadline
                && Instant::now() >= d
            {
                break;
            }
            let now = Instant::now();
            if now < next_deadline {
                thread::sleep(next_deadline - now);
            }
            let late_ms = Instant::now()
                .saturating_duration_since(next_deadline)
                .as_secs_f64()
                * 1000.0;
            if let Some((idx, t, frame)) = self.next_frame()? {
                on_frame(idx, t, frame, late_ms)?;
                count += 1;
            }
            next_deadline += period;
            // If we fell behind more than 2 frames, resync clock to avoid spiral.
            let behind = Instant::now().saturating_duration_since(next_deadline);
            if behind > period * 2 {
                next_deadline = Instant::now();
            }
        }
        Ok(count)
    }
}

impl Iterator for FrameStream {
    type Item = Result<(u64, Time, Frame)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_frame() {
            Ok(None) => None,
            Ok(Some(v)) => Some(Ok(v)),
            Err(e) => Some(Err(e)),
        }
    }
}

/// Build a cached sequential stream for realtime-ish consumption.
///
/// # Errors
///
/// Invalid duration / fps.
pub fn stream_video(clip: Arc<dyn VideoClip>, cache_seconds: f64) -> Result<FrameStream> {
    FrameStream::cached(clip, cache_seconds, None)
}

/// Build an uncached sequential stream.
///
/// # Errors
///
/// Invalid duration / fps.
pub fn stream_video_raw(clip: Arc<dyn VideoClip>, fps: Option<f64>) -> Result<FrameStream> {
    FrameStream::new(clip, fps)
}

#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn frame_count(duration: Duration, fps: f64) -> u64 {
    let n = (duration.as_secs() * fps).round();
    if n < 1.0 { 1 } else { n as u64 }
}

#[must_use]
#[allow(clippy::cast_precision_loss)]
fn time_at(index: u64, fps: f64) -> Time {
    Time::from_secs(index as f64 / fps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb8;
    use crate::layout::Size;
    use crate::solid::ColorClip;

    #[test]
    fn streams_all_frames() {
        let clip: Arc<dyn VideoClip> = Arc::new(
            ColorClip::new(Size::new(4, 4), Rgb8::GREEN, Duration::from_secs(0.5)).with_fps(10.0),
        );
        let mut s = FrameStream::cached(clip, 1.0, Some(10.0)).unwrap();
        let mut n = 0;
        while let Some((_i, _t, f)) = s.next_frame().unwrap() {
            assert_eq!(f.size().width, 4);
            n += 1;
        }
        assert_eq!(n, 5); // 0.5s * 10fps rounded
    }

    #[test]
    fn iterator_works() {
        let clip: Arc<dyn VideoClip> = Arc::new(
            ColorClip::new(Size::new(2, 2), Rgb8::RED, Duration::from_secs(0.2)).with_fps(10.0),
        );
        let s = stream_video_raw(clip, Some(10.0)).unwrap();
        let mut count = 0;
        for r in s {
            let _ = r.unwrap();
            count += 1;
        }
        assert!(count >= 2);
    }
}
