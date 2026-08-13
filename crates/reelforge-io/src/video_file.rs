//! File-backed video clips via the `FFmpeg` CLI.

use crate::audio_file::AudioFileClip;
use crate::error::{IoError, Result};
use crate::ffmpeg::{
    FfmpegTools, FrameTimingIndex, SequentialMode, SequentialPlanarDecoder, SequentialRgbDecoder,
    decode_frame_planes, decode_frame_rgb, probe_frame_timing, probe_video,
};
use crate::options::{OpenAudioOptions, OpenVideoOptions};
use reelforge_core::{
    ColorInfo, CoreError, Duration, Frame, MediaTime, PixelFormat, Size, StreamTimeBase,
    SurfacePlane, Time, VideoClip, VideoSurface,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Video clip backed by a media file on disk.
///
/// Random access uses single-frame seeks. Monotonic sequential access reuses a
/// long-lived rawvideo pipe (sequential RGB decoder) — the fast path for
/// ordered writers. [`VideoClip::surface_at`] decodes native YUV/NV12 planes
/// (or packed RGB) without an RGB round-trip.
///
/// # VFR
///
/// When probe detects variable frame rate (`is_vfr`), frame indexing prefers a
/// lazy PTS table ([`FrameTimingIndex`]) and sequential decode uses native
/// passthrough (no `-r` CFR resample). Random access still seeks by timestamp.
pub struct VideoFileClip {
    path: PathBuf,
    size: Size,
    duration: Duration,
    fps: f64,
    time_base_num: u32,
    time_base_den: u32,
    is_vfr: bool,
    nb_frames: Option<u64>,
    color: ColorInfo,
    pixel_format: PixelFormat,
    tools: FfmpegTools,
    /// Optional companion audio opened when `with_audio` is true.
    audio: Option<AudioFileClip>,
    /// Multi-frame LRU (index → frame) for random / repeated access.
    cache: Mutex<FrameLru>,
    seq: Mutex<Option<SequentialRgbDecoder>>,
    seq_planar: Mutex<Option<SequentialPlanarDecoder>>,
    /// Lazy PTS index for VFR mapping (None until first need).
    timing: Mutex<Option<FrameTimingIndex>>,
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
            .field("is_vfr", &self.is_vfr)
            .field(
                "time_base",
                &format!("{}/{}", self.time_base_num, self.time_base_den),
            )
            .field("pixel_format", &self.pixel_format)
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
            time_base_num: probe.time_base_num,
            time_base_den: probe.time_base_den,
            is_vfr: probe.is_vfr,
            nb_frames: probe.nb_frames,
            color: probe.color,
            pixel_format: probe.pixel_format,
            tools,
            audio,
            // ~2s of 30fps by default — warm seeks without huge RAM.
            cache: Mutex::new(FrameLru::new(64)),
            seq: Mutex::new(None),
            seq_planar: Mutex::new(None),
            timing: Mutex::new(None),
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

    /// Whether probe classified the stream as variable frame rate.
    #[must_use]
    pub fn is_vfr(&self) -> bool {
        self.is_vfr
    }

    /// Stream time base as `(num, den)`.
    #[must_use]
    pub fn time_base(&self) -> (u32, u32) {
        (self.time_base_num, self.time_base_den)
    }

    /// Stream time base as [`StreamTimeBase`].
    #[must_use]
    pub fn stream_time_base(&self) -> StreamTimeBase {
        StreamTimeBase::new(self.time_base_num.max(1), self.time_base_den.max(1))
            .unwrap_or(StreamTimeBase::HZ_90K)
    }

    /// Color tags from `ffprobe`.
    #[must_use]
    pub const fn color(&self) -> ColorInfo {
        self.color
    }

    /// Native decode format for [`VideoClip::surface_at`] (typically `Yuv420p`).
    #[must_use]
    pub const fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
    }

    /// Stream timescale for [`MediaTime`] (`time_base` denominator).
    #[must_use]
    pub fn timescale(&self) -> u32 {
        self.time_base_den.max(1)
    }

    /// Optional container frame count.
    #[must_use]
    pub fn nb_frames(&self) -> Option<u64> {
        self.nb_frames
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
        if let Ok(mut g) = self.seq_planar.lock() {
            *g = None;
        }
    }

    /// Ensure a PTS timing index is loaded (no-op when already present).
    ///
    /// For VFR clips this is called automatically on first timed sample.
    ///
    /// # Errors
    ///
    /// `ffprobe` packet listing failures.
    pub fn ensure_timing_index(&self) -> Result<()> {
        let mut guard = self
            .timing
            .lock()
            .map_err(|_| IoError::message("timing lock poisoned"))?;
        if guard.is_some() {
            return Ok(());
        }
        // Cap very long files: 0 = unlimited for short clips; soft cap 500k packets.
        let max = if self.duration.as_secs() > 600.0 {
            250_000
        } else {
            0
        };
        let idx = probe_frame_timing(&self.tools, &self.path, max)?;
        *guard = Some(idx);
        Ok(())
    }

    /// Inject a prebuilt timing index (tests / hosts with external PTS tables).
    pub fn set_timing_index(&self, index: FrameTimingIndex) {
        if let Ok(mut g) = self.timing.lock() {
            *g = Some(index);
        }
    }

    /// Map media time → frame ordinal (PTS table when VFR / available, else CFR).
    ///
    /// # Errors
    ///
    /// Timing probe failures when VFR index must be built.
    pub fn frame_index_for_media(&self, t: MediaTime) -> Result<u64> {
        if self.is_vfr {
            self.ensure_timing_index()?;
            if let Ok(guard) = self.timing.lock()
                && let Some(idx) = guard.as_ref()
                && !idx.is_empty()
            {
                return Ok(idx.frame_index_at(t));
            }
        }
        Ok(t.frame_index(self.fps))
    }

    /// Half-open frame range `[start, end)` for a media interval.
    ///
    /// VFR uses the PTS index; CFR uses exact tick math at nominal fps.
    ///
    /// # Errors
    ///
    /// Timing probe failures for VFR.
    pub fn frame_range_media(&self, start: MediaTime, end: MediaTime) -> Result<(u64, u64)> {
        if self.is_vfr {
            self.ensure_timing_index()?;
            if let Ok(guard) = self.timing.lock()
                && let Some(idx) = guard.as_ref()
                && !idx.is_empty()
            {
                return Ok(idx.frame_range(start, end));
            }
        }
        Ok(MediaTime::frame_range_cfr(start, end, self.fps))
    }

    /// Map presentation time → frame index via [`MediaTime`] (tick math / PTS).
    fn frame_index(&self, t: Time) -> u64 {
        let mt = MediaTime::from_time(t, self.timescale()).unwrap_or_else(|_| {
            MediaTime::from_secs(t.as_secs(), MediaTime::HZ_90K)
                .unwrap_or_else(|_| MediaTime::zero(MediaTime::HZ_90K))
        });
        self.frame_index_for_media(mt)
            .unwrap_or_else(|_| mt.frame_index(self.fps))
    }

    /// Decode at exact media time (same cache key as [`VideoClip::frame_at`]).
    ///
    /// # Errors
    ///
    /// Out of range or decode failure.
    pub fn frame_at_media(&self, t: MediaTime) -> reelforge_core::Result<Frame> {
        self.frame_at(t.to_time())
    }

    /// Timed surface at media time (PTS from the index when available).
    ///
    /// # Errors
    ///
    /// Same as [`VideoClip::frame_at`].
    pub fn surface_at_media(&self, t: MediaTime) -> reelforge_core::Result<VideoSurface> {
        self.surface_at(t.to_time())
    }

    fn pts_for_index(&self, index: u64) -> MediaTime {
        if let Ok(guard) = self.timing.lock()
            && let Some(idx) = guard.as_ref()
            && let Some(pts) = idx.pts_at(index)
        {
            return pts;
        }
        if self.fps.is_finite() && self.fps > 0.0 {
            #[allow(clippy::cast_precision_loss)]
            let secs = index as f64 / self.fps;
            return MediaTime::from_secs(secs, self.timescale())
                .unwrap_or_else(|_| MediaTime::zero(self.timescale()));
        }
        MediaTime::zero(self.timescale())
    }

    fn duration_for_index(&self, index: u64) -> Option<MediaTime> {
        if let Ok(guard) = self.timing.lock()
            && let Some(idx) = guard.as_ref()
            && let Some(d) = idx.duration_at(index)
        {
            return Some(d);
        }
        if self.fps.is_finite() && self.fps > 0.0 {
            return MediaTime::from_secs(1.0 / self.fps, self.timescale()).ok();
        }
        None
    }

    fn decode_sequential(&self, index: u64) -> reelforge_core::Result<Frame> {
        let mut guard = self
            .seq
            .lock()
            .map_err(|_| CoreError::invalid_frame("sequential decoder lock poisoned"))?;
        if guard.is_none() {
            let dec = if self.is_vfr {
                SequentialRgbDecoder::open_native(&self.tools, &self.path, self.size)
            } else {
                SequentialRgbDecoder::open(&self.tools, &self.path, self.size, self.fps)
            }
            .map_err(|e| CoreError::invalid_frame(format!("seq open: {e}")))?;
            *guard = Some(dec);
        }
        let dec = guard
            .as_mut()
            .ok_or_else(|| CoreError::invalid_frame("seq decoder missing"))?;
        dec.frame_at_index(index)
            .map_err(|e| CoreError::invalid_frame(format!("seq decode: {e}")))
    }

    fn decode_sequential_planes(&self, index: u64) -> reelforge_core::Result<Vec<SurfacePlane>> {
        let mut guard = self
            .seq_planar
            .lock()
            .map_err(|_| CoreError::invalid_frame("sequential planar lock poisoned"))?;
        if guard.is_none() {
            let mode = if self.is_vfr {
                SequentialMode::Native
            } else {
                SequentialMode::Cfr { fps: self.fps }
            };
            let dec = SequentialPlanarDecoder::open(
                &self.tools,
                &self.path,
                self.size,
                self.pixel_format,
                mode,
            )
            .map_err(|e| CoreError::invalid_frame(format!("seq planar open: {e}")))?;
            *guard = Some(dec);
        }
        let dec = guard
            .as_mut()
            .ok_or_else(|| CoreError::invalid_frame("seq planar decoder missing"))?;
        dec.planes_at_index(index)
            .map_err(|e| CoreError::invalid_frame(format!("seq planar decode: {e}")))
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

    fn surface_at(&self, t: Time) -> reelforge_core::Result<VideoSurface> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let index = self.frame_index(t);
        if self.is_vfr {
            let _ = self.ensure_timing_index();
        }
        let ts = self.pts_for_index(index);
        let dur = self.duration_for_index(index);
        let planes = if let Ok(p) = self.decode_sequential_planes(index) {
            p
        } else {
            if let Ok(mut g) = self.seq_planar.lock() {
                *g = None;
            }
            decode_frame_planes(&self.tools, &self.path, self.size, t, self.pixel_format).map_err(
                |e| CoreError::invalid_frame(format!("planar decode failed at {t}: {e}")),
            )?
        };
        VideoSurface::from_planes(
            self.pixel_format,
            self.size,
            planes,
            ts,
            dur,
            self.color,
            self.stream_time_base(),
        )
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
        // Timestamp seek (`-ss`) is PTS-correct for both CFR and VFR random access.
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

// Manual Clone: fresh cache / decoder / timing share for PTS table.
impl Clone for VideoFileClip {
    fn clone(&self) -> Self {
        let timing = self.timing.lock().ok().and_then(|g| g.clone());
        Self {
            path: self.path.clone(),
            size: self.size,
            duration: self.duration,
            fps: self.fps,
            time_base_num: self.time_base_num,
            time_base_den: self.time_base_den,
            is_vfr: self.is_vfr,
            nb_frames: self.nb_frames,
            color: self.color,
            pixel_format: self.pixel_format,
            tools: self.tools.clone(),
            audio: self.audio.clone(),
            cache: Mutex::new(FrameLru::new(64)),
            seq: Mutex::new(None),
            seq_planar: Mutex::new(None),
            timing: Mutex::new(timing),
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

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::MediaTime;

    #[test]
    fn vfr_index_maps_without_file() {
        // Build a fake clip-like mapping via FrameTimingIndex alone.
        let idx = FrameTimingIndex::from_pts_secs([0.0, 0.05, 0.12, 0.13, 0.25], 1_000).unwrap();
        let t = |s: f64| MediaTime::from_secs(s, 1_000).unwrap();
        assert_eq!(idx.frame_index_at(t(0.12)), 2);
        assert_eq!(idx.frame_range(t(0.05), t(0.13)), (1, 3));
    }
}
