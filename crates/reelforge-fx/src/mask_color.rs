//! Build an alpha mask from a chroma color key.

use reelforge_core::{
    Duration, Frame, Mask, Result, Rgb8, Size, Time, VideoClip, VideoEffect,
};
use std::sync::Arc;

/// Marks pixels near `color` as transparent in the clip mask.
#[derive(Debug, Clone, Copy)]
pub struct MaskColor {
    /// Key color.
    pub color: Rgb8,
    /// Max per-channel distance to treat as key (0–255).
    pub threshold: u8,
}

impl MaskColor {
    /// Key `color` with the given threshold.
    #[must_use]
    pub const fn new(color: Rgb8, threshold: u8) -> Self {
        Self { color, threshold }
    }
}

impl VideoEffect for MaskColor {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MaskColorVideo {
            inner: clip,
            color: self.color,
            threshold: self.threshold,
        }))
    }
}

struct MaskColorVideo {
    inner: Arc<dyn VideoClip>,
    color: Rgb8,
    threshold: u8,
}

impl VideoClip for MaskColorVideo {
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
        self.inner.frame_at(t)
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        let frame = self.inner.frame_at(t)?;
        let bpp = frame.format().bytes_per_pixel();
        let thr = u16::from(self.threshold);
        let cr = self.color.r;
        let cg = self.color.g;
        let cb = self.color.b;
        let mut m = Vec::with_capacity(frame.data().len() / bpp);
        for px in frame.data().chunks_exact(bpp) {
            let dr = u16::from(px[0].abs_diff(cr));
            let dg = u16::from(px[1].abs_diff(cg));
            let db = u16::from(px[2].abs_diff(cb));
            let hit = dr <= thr && dg <= thr && db <= thr;
            m.push(if hit { 0.0 } else { 1.0 });
        }
        Ok(Some(Mask::from_raw(frame.size(), m)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::ColorClip;

    #[test]
    fn keys_solid() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::GREEN,
            Duration::from_secs(0.5),
        ));
        let out = MaskColor::new(Rgb8::GREEN, 10).apply(clip).unwrap();
        let mask = out.mask_at(Time::ZERO).unwrap().unwrap();
        assert!(mask.data().iter().all(|&v| v == 0.0));
    }
}
