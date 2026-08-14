//! Frame cache for hot `frame_at` paths (preview, realtime, repeated samples).

use crate::clip::{ClipId, VideoClip};
use crate::error::Result;
use crate::frame::{Frame, Mask};
use crate::layout::Size;
use crate::surface::VideoSurface;
use crate::time::{Duration, Time};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// LRU frame store keyed by quantized frame index.
#[derive(Debug)]
struct LruFrames {
    map: HashMap<i64, Frame>,
    order: VecDeque<i64>,
    capacity: usize,
}

impl LruFrames {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            map: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn get(&mut self, key: i64) -> Option<Frame> {
        if !self.map.contains_key(&key) {
            return None;
        }
        // Move to most-recently used end.
        if let Some(pos) = self.order.iter().position(|&k| k == key) {
            self.order.remove(pos);
            self.order.push_back(key);
        }
        self.map.get(&key).cloned()
    }

    fn insert(&mut self, key: i64, frame: Frame) {
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.map.entry(key) {
            e.insert(frame);
            if let Some(pos) = self.order.iter().position(|&k| k == key) {
                self.order.remove(pos);
            }
            self.order.push_back(key);
            return;
        }
        while self.map.len() >= self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            } else {
                break;
            }
        }
        self.order.push_back(key);
        self.map.insert(key, frame);
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

/// Hit / miss counters for tuning realtime capacity.
#[derive(Debug, Default)]
pub struct CacheStats {
    hits: AtomicU64,
    misses: AtomicU64,
}

impl CacheStats {
    /// Cache hits.
    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Cache misses.
    #[must_use]
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Hit rate in `0.0..=1.0` (0 when empty).
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        let h = self.hits();
        let m = self.misses();
        let t = h + m;
        if t == 0 {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                h as f64 / t as f64
            }
        }
    }
}

/// Configuration for [`CachedVideo`].
#[derive(Debug, Clone, Copy)]
pub struct CacheConfig {
    /// Max frames retained (LRU).
    pub capacity: usize,
    /// Quantization rate: time → key `round(t * quantum_hz)`.
    ///
    /// Prefer source / output fps so re-sampling the same frame index hits.
    pub quantum_hz: f64,
}

impl CacheConfig {
    /// Default: 64 frames at 24 Hz quantization.
    #[must_use]
    pub const fn new(capacity: usize, quantum_hz: f64) -> Self {
        Self {
            capacity,
            quantum_hz,
        }
    }

    /// Sensible realtime defaults from clip fps (fallback 24).
    #[must_use]
    pub fn realtime(capacity: usize, fps: Option<f64>) -> Self {
        let hz = fps.filter(|f| f.is_finite() && *f > 0.0).unwrap_or(24.0);
        Self {
            capacity: capacity.max(1),
            quantum_hz: hz,
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self::new(64, 24.0)
    }
}

/// Video clip wrapper that caches `frame_at` / `mask_at` results (LRU).
///
/// Cheap `Frame` clones (`Arc` pixels) make multi-hit preview / realtime paths
/// much faster than recomputing effect graphs.
pub struct CachedVideo {
    inner: Arc<dyn VideoClip>,
    frames: Mutex<LruFrames>,
    config: CacheConfig,
    stats: Arc<CacheStats>,
}

impl std::fmt::Debug for CachedVideo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedVideo")
            .field("config", &self.config)
            .field("hits", &self.stats.hits())
            .field("misses", &self.stats.misses())
            .finish_non_exhaustive()
    }
}

impl CachedVideo {
    /// Wrap `inner` with an LRU of `config.capacity` frames.
    #[must_use]
    pub fn new(inner: Arc<dyn VideoClip>, config: CacheConfig) -> Self {
        Self {
            inner,
            frames: Mutex::new(LruFrames::new(config.capacity)),
            config,
            stats: Arc::new(CacheStats::default()),
        }
    }

    /// Default capacity (64) quantized at clip fps when known.
    #[must_use]
    pub fn wrap(inner: Arc<dyn VideoClip>) -> Self {
        let cfg = CacheConfig::realtime(64, inner.fps());
        Self::new(inner, cfg)
    }

    /// Realtime-oriented capacity (seconds of footage at clip fps).
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn realtime(inner: Arc<dyn VideoClip>, seconds: f64) -> Self {
        let fps = inner
            .fps()
            .filter(|f| f.is_finite() && *f > 0.0)
            .unwrap_or(24.0);
        let cap = ((seconds.max(0.25) * fps).ceil() as usize).clamp(8, 256);
        Self::new(inner, CacheConfig::realtime(cap, Some(fps)))
    }

    /// Shared stats handle.
    #[must_use]
    pub fn stats(&self) -> Arc<CacheStats> {
        Arc::clone(&self.stats)
    }

    /// Drop all cached frames.
    pub fn clear(&self) {
        if let Ok(mut g) = self.frames.lock() {
            g.clear();
        }
    }

    /// Current number of cached frames.
    #[must_use]
    pub fn cached_len(&self) -> usize {
        self.frames.lock().map_or(0, |g| g.len())
    }

    /// Quantized key for media time `t`.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn key_for(&self, t: Time) -> i64 {
        let hz = if self.config.quantum_hz.is_finite() && self.config.quantum_hz > 0.0 {
            self.config.quantum_hz
        } else {
            24.0
        };
        (t.as_secs() * hz).round() as i64
    }

    fn lookup_or_compute(&self, t: Time) -> Result<Frame> {
        let key = self.key_for(t);
        if let Ok(mut g) = self.frames.lock()
            && let Some(frame) = g.get(key)
        {
            self.stats.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(frame);
        }
        self.stats.misses.fetch_add(1, Ordering::Relaxed);
        let frame = self.inner.frame_at(t)?;
        if let Ok(mut g) = self.frames.lock() {
            g.insert(key, frame.clone());
        }
        Ok(frame)
    }
}

impl VideoClip for CachedVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn id(&self) -> Option<&ClipId> {
        self.inner.id()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        self.lookup_or_compute(t)
    }

    fn surface_at(&self, t: Time) -> Result<VideoSurface> {
        // Do not collapse native YUV through the RGB frame cache.
        self.inner.surface_at(t)
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        // Masks are usually cheap / rare; pass-through without separate cache.
        self.inner.mask_at(t)
    }
}

/// Wrap a clip in a default [`CachedVideo`] as `Arc<dyn VideoClip>`.
#[must_use]
pub fn cache_video(clip: Arc<dyn VideoClip>) -> Arc<dyn VideoClip> {
    Arc::new(CachedVideo::wrap(clip))
}

/// Wrap with realtime capacity (~`seconds` of frames at clip fps).
#[must_use]
pub fn cache_video_realtime(clip: Arc<dyn VideoClip>, seconds: f64) -> Arc<dyn VideoClip> {
    Arc::new(CachedVideo::realtime(clip, seconds))
}

/// Wrap with explicit config.
#[must_use]
pub fn cache_video_with(clip: Arc<dyn VideoClip>, config: CacheConfig) -> Arc<dyn VideoClip> {
    Arc::new(CachedVideo::new(clip, config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb8;
    use crate::solid::ColorClip;
    use std::sync::atomic::AtomicUsize;

    struct CountingClip {
        inner: ColorClip,
        hits: Arc<AtomicUsize>,
    }

    impl VideoClip for CountingClip {
        fn duration(&self) -> Duration {
            self.inner.duration()
        }

        fn size(&self) -> Size {
            self.inner.size()
        }

        fn frame_at(&self, t: Time) -> Result<Frame> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            self.inner.frame_at(t)
        }
    }

    #[test]
    fn second_sample_is_cache_hit() {
        let hits = Arc::new(AtomicUsize::new(0));
        let base: Arc<dyn VideoClip> = Arc::new(CountingClip {
            inner: ColorClip::new(Size::new(4, 4), Rgb8::RED, Duration::from_secs(1.0))
                .with_fps(10.0),
            hits: Arc::clone(&hits),
        });
        let cached = CachedVideo::new(base, CacheConfig::realtime(8, Some(10.0)));
        let t = Time::from_secs(0.3);
        let _ = cached.frame_at(t).unwrap();
        let _ = cached.frame_at(t).unwrap();
        let _ = cached.frame_at(t).unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(cached.stats().hits(), 2);
        assert_eq!(cached.stats().misses(), 1);
        assert!((cached.stats().hit_rate() - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn lru_evicts_oldest() {
        let hits = Arc::new(AtomicUsize::new(0));
        let base: Arc<dyn VideoClip> = Arc::new(CountingClip {
            inner: ColorClip::new(Size::new(2, 2), Rgb8::BLUE, Duration::from_secs(2.0))
                .with_fps(10.0),
            hits: Arc::clone(&hits),
        });
        let cached = CachedVideo::new(base, CacheConfig::new(2, 10.0));
        let _ = cached.frame_at(Time::from_secs(0.0)).unwrap();
        let _ = cached.frame_at(Time::from_secs(0.1)).unwrap();
        let _ = cached.frame_at(Time::from_secs(0.2)).unwrap(); // evicts 0.0
        hits.store(0, Ordering::SeqCst);
        let _ = cached.frame_at(Time::from_secs(0.0)).unwrap(); // miss again
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
