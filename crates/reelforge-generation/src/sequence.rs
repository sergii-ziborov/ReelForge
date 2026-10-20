use crate::{Clock, Error, FrameReceipt, FrameStore, GenerationReceipt, Result};
use reelforge_core::{
    CoreError, Duration, Frame, FrameFormat, MediaTime, Size, Time, VideoClip, VideoSurface,
};
use std::collections::BTreeMap;

/// Verified generated sequence usable by native `ReelForge` composition/effects/writers.
/// Frames are read on demand; no sprite atlas, blending or duplicated-FPS expansion.
pub struct GeneratedClip {
    store: FrameStore,
    clock: Clock,
    frames: BTreeMap<u32, FrameReceipt>,
}
impl GeneratedClip {
    /// Verify owned coverage and canvas before admitting a receipt as a native clip.
    pub fn open(store: FrameStore, receipt: &GenerationReceipt) -> Result<Self> {
        let c = &receipt.clock;
        if c.frames == 0
            || c.fps == 0
            || c.fps > 120
            || c.frames > 3600
            || !receipt.frame_clock_verified
            || receipt.frames.keys().copied().ne(0..c.frames)
        {
            return Err(Error::Invalid("generated clip clock/coverage".into()));
        }
        for f in receipt.frames.values() {
            store.verify(f)?;
            if (f.width, f.height) != (c.width, c.height) {
                return Err(Error::Invalid("generated clip canvas".into()));
            }
        }
        Ok(Self {
            store,
            clock: c.clone(),
            frames: receipt.frames.clone(),
        })
    }
    /// Sample by exact integer frame index, avoiding floating timeline quantization.
    pub fn frame_at_index(&self, index: u32) -> reelforge_core::Result<Frame> {
        let receipt = self
            .frames
            .get(&index)
            .ok_or_else(|| CoreError::invalid_frame("generated frame index outside clip"))?;
        let image = self
            .store
            .load(receipt)
            .map_err(|e| CoreError::invalid_frame(e.to_string()))?;
        Frame::from_raw(self.size(), FrameFormat::Rgba8, image.into_raw())
    }
    fn index(&self, t: Time) -> reelforge_core::Result<u32> {
        let s = t.as_secs();
        if !s.is_finite() || s < 0.0 || s >= self.duration().as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration().as_secs())),
            });
        }
        let scaled = s * f64::from(self.clock.fps);
        let rounded = scaled.round();
        // i/fps can multiply back to i-one-ULP. Snap only floating arithmetic error,
        // not real sub-frame times; otherwise 24fps sources silently repeat a frame.
        let scaled = if (scaled - rounded).abs() <= 8.0 * f64::EPSILON * scaled.max(1.0) {
            rounded
        } else {
            scaled.floor()
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        // Bounded finite, non-negative timeline with at most 3600 integer frames.
        let index = scaled as u32;
        Ok(index.min(self.clock.frames - 1))
    }
}
impl VideoClip for GeneratedClip {
    fn duration(&self) -> Duration {
        Duration::from_secs(f64::from(self.clock.frames) / f64::from(self.clock.fps))
    }
    fn size(&self) -> Size {
        Size::new(self.clock.width, self.clock.height)
    }
    fn fps(&self) -> Option<f64> {
        Some(f64::from(self.clock.fps))
    }
    fn sample_times(&self) -> Option<Vec<Time>> {
        Some(
            (0..self.clock.frames)
                .map(|i| Time::from_secs(f64::from(i) / f64::from(self.clock.fps)))
                .collect(),
        )
    }
    fn requires_frame_grid(&self) -> bool {
        false
    }
    fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
        self.frame_at_index(self.index(t)?)
    }
    fn surface_at(&self, t: Time) -> reelforge_core::Result<VideoSurface> {
        let index = self.index(t)?;
        Ok(VideoSurface::from_frame(
            self.frame_at_index(index)?,
            MediaTime::new(i64::from(index), self.clock.fps)?,
            Some(MediaTime::new(1, self.clock.fps)?),
        ))
    }
}
