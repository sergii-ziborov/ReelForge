//! Boolean combine of clip masks (and / or).

use reelforge_core::{Duration, Frame, Mask, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Per-pixel minimum of this clip's mask and `other`'s mask.
#[derive(Clone)]
pub struct MasksAnd {
    /// Other mask source (same size expected).
    pub other: Arc<dyn VideoClip>,
}

impl MasksAnd {
    /// AND with `other`.
    #[must_use]
    pub fn new(other: Arc<dyn VideoClip>) -> Self {
        Self { other }
    }
}

impl VideoEffect for MasksAnd {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MaskBoolVideo {
            a: clip,
            b: Arc::clone(&self.other),
            mode: BoolMode::And,
        }))
    }
}

/// Per-pixel maximum of this clip's mask and `other`'s mask.
#[derive(Clone)]
pub struct MasksOr {
    /// Other mask source.
    pub other: Arc<dyn VideoClip>,
}

impl MasksOr {
    /// OR with `other`.
    #[must_use]
    pub fn new(other: Arc<dyn VideoClip>) -> Self {
        Self { other }
    }
}

impl VideoEffect for MasksOr {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MaskBoolVideo {
            a: clip,
            b: Arc::clone(&self.other),
            mode: BoolMode::Or,
        }))
    }
}

#[derive(Clone, Copy)]
enum BoolMode {
    And,
    Or,
}

struct MaskBoolVideo {
    a: Arc<dyn VideoClip>,
    b: Arc<dyn VideoClip>,
    mode: BoolMode,
}

impl VideoClip for MaskBoolVideo {
    fn duration(&self) -> Duration {
        // Intersection of timelines (shorter wins).
        let da = self.a.duration().as_secs();
        let db = self.b.duration().as_secs();
        Duration::from_secs(da.min(db))
    }

    fn size(&self) -> Size {
        self.a.size()
    }

    fn fps(&self) -> Option<f64> {
        self.a.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        self.a.frame_at(t)
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        let ma = resolve_mask(&*self.a, t)?;
        let mb = resolve_mask(&*self.b, t)?;
        if ma.size() != mb.size() {
            return Ok(Some(ma));
        }
        let mut out = ma.data().to_vec();
        for (o, &b) in out.iter_mut().zip(mb.data().iter()) {
            *o = match self.mode {
                BoolMode::And => o.min(b),
                BoolMode::Or => o.max(b),
            };
        }
        Ok(Some(Mask::from_raw(ma.size(), out)?))
    }
}

fn resolve_mask(clip: &dyn VideoClip, t: Time) -> Result<Mask> {
    if let Some(m) = clip.mask_at(t)? {
        return Ok(m);
    }
    Mask::opaque(clip.size())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask_color::MaskColor;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn and_keys() {
        let a: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::GREEN,
            Duration::from_secs(0.5),
        ));
        let b = MaskColor::new(Rgb8::GREEN, 5).apply(a.clone()).unwrap();
        let out = MasksAnd::new(b).apply(a).unwrap();
        let m = out.mask_at(Time::ZERO).unwrap().unwrap();
        // a is opaque, b is zero → and is zero
        assert!(m.data().iter().all(|&v| v == 0.0));
    }
}
