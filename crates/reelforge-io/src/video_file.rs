//! File-backed video clips via the `FFmpeg` CLI.

use crate::audio_file::AudioFileClip;
use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, SequentialRgbDecoder, decode_frame_rgb, probe_video};
use crate::options::{OpenAudioOptions, OpenVideoOptions};
use reelforge_core::{CoreError, Duration, Frame, MediaTime, Size, Time, VideoClip};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Video clip backed by a media file on disk.
///
/// Random access uses single-frame seeks. Monotonic sequential access reuses a
/// long-lived rawvideo pipe (sequential RGB decoder) — the fast path for
/// ordered writers.
pub struct VideoFileClip {
    path: PathBuf,
    size: Size,
    duration: Duration,
    fps: f64,
    tools: FfmpegTools,
    /// Optional companion audio opened when `with_audio` is true.
    audio: Option<AudioFileClip>,
    /// Multi-frame LRU (index → frame) for random / repeated access.
    cache: Mutex<FrameLru>,
    seq: Mutex<Option<SequentialRgbDecoder>>,
}

/// Small LRU for decoded file frames (index-keyed).
struct FrameLru {
    map: std::collections::HashMap<u64, Frame>,
    order: std::collections::VecDeque<u64>,
    capacity: usize,
}

impl FrameLru {
    fn new(capacity: usize) -> Self {
        Self {
            map: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    fn get(&mut self, index: u64) -> Option<Frame> {
        if !self.map.contains_key(&index) {
            return None;
        }
        if let Some(pos) = self.order.iter().position(|&k| k == index) {
            self.order.remove(pos);
            self.order.push_back(index);
        }
        self.map.get(&index).cloned()
    }

    fn insert(&mut self, index: u64, frame: Frame) {
        if let std::collections::hash_map::Entry::Occupied(mut e) = self.map.entry(index) {
            e.insert(frame);
            if let Some(pos) = self.order.iter().position(|&k| k == index) {
                self.order.remove(pos);
            }
            self.order.push_back(index);
            return;
        }
        while self.map.len() >= self.capacity {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            } else {
                break;
            }
        }
        self.order.push_back(index);
        self.map.insert(index, frame);
    }
}

impl std::fmt::Debug for VideoFileClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoFileClip")
            .field("path", &self.path)
            .field("size", &self.size)
            .field("duration", &self.duration)
            .field("fps", &self.fps)
            .field("has_audio", &self.audio.is_some())
            .finish_non_exhaustive()
    }
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
        let audio = if options.with_audio {
            // Video-only containers are fine when audio open fails.
            AudioFileClip::open_with(&OpenAudioOptions::new(options.path.clone())).ok()
        } else {
            None
        };
        Ok(Self {
            path,
            size: probe.size,
            duration: probe.duration,
            fps: probe.fps,
            tools,
            audio,
            // ~2s of 30fps by default — warm seeks without huge RAM.
            cache: Mutex::new(FrameLru::new(64)),
            seq: Mutex::new(None),
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

    /// Attached audio track when open requested audio and the container has one.
    #[must_use]
    pub fn audio(&self) -> Option<&AudioFileClip> {
        self.audio.as_ref()
    }

    /// Drop the sequential decoder (forces fresh stream on next sequential run).
    pub fn reset_sequential(&self) {
        if let Ok(mut g) = self.seq.lock() {
            *g = None;
        }
    }

    /// Map presentation time → frame index via [`MediaTime`] (tick math).
    ///
    /// Uses a 90 kHz timescale when converting floating [`Time`], then
    /// `MediaTime::frame_index` so CFR indexing avoids `floor(secs × fps)` drift.
    fn frame_index(&self, t: Time) -> u64 {
        if self.fps <= 0.0 {
            return 0;
        }
        MediaTime::from_time(t, MediaTime::HZ_90K).map_or(0, |mt| mt.frame_index(self.fps))
    }

    /// Decode at exact media time (same cache key as [`VideoClip::frame_at`]).
    ///
    /// # Errors
    ///
    /// Out of range or decode failure.
    pub fn frame_at_media(&self, t: MediaTime) -> reelforge_core::Result<Frame> {
        self.frame_at(t.to_time())
    }

    fn decode_sequential(&self, index: u64) -> reelforge_core::Result<Frame> {
        let mut guard = self
            .seq
            .lock()
            .map_err(|_| CoreError::invalid_frame("sequential decoder lock poisoned"))?;
        if guard.is_none() {
            let dec = SequentialRgbDecoder::open(&self.tools, &self.path, self.size, self.fps)
                .map_err(|e| CoreError::invalid_frame(format!("seq open: {e}")))?;
            *guard = Some(dec);
        }
        let dec = guard
            .as_mut()
            .ok_or_else(|| CoreError::invalid_frame("seq decoder missing"))?;
        dec.frame_at_index(index)
            .map_err(|e| CoreError::invalid_frame(format!("seq decode: {e}")))
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
        if let Ok(mut guard) = self.cache.lock()
            && let Some(frame) = guard.get(index)
        {
            return Ok(frame);
        }

        // Prefer sequential pipe (handles restart on backward jumps); fall back to seek.
        let frame = if let Ok(f) = self.decode_sequential(index) {
            f
        } else {
            self.reset_sequential();
            decode_frame_rgb(&self.tools, &self.path, self.size, t)
                .map_err(|e| CoreError::invalid_frame(format!("decode failed at {t}: {e}")))?
        };

        if let Ok(mut guard) = self.cache.lock() {
            guard.insert(index, frame.clone());
        }
        Ok(frame)
    }
}

// Manual Clone: fresh cache / decoder per clone.
impl Clone for VideoFileClip {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            size: self.size,
            duration: self.duration,
            fps: self.fps,
            tools: self.tools.clone(),
            audio: self.audio.clone(),
            cache: Mutex::new(FrameLru::new(64)),
            seq: Mutex::new(None),
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
