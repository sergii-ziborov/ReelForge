//! Cross-fade opacity effects (mask-based for compositing).

use reelforge_core::{
    CoreError, Duration, Frame, Mask, Result, Size, Time, VideoClip, VideoEffect,
};
use std::sync::Arc;

/// Fade opacity from 0 → 1 over `duration` at the start of the clip.
///
/// When composed, this drives transparency via [`VideoClip::mask_at`].
#[derive(Debug, Clone, Copy)]
pub struct CrossFadeIn {
    /// Fade length.
    pub duration: Duration,
}

impl CrossFadeIn {
    /// Construct a cross-fade in.
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl VideoEffect for CrossFadeIn {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing(
                "cross-fade-in duration must be > 0",
            ));
        }
        Ok(Arc::new(CrossFadedVideo {
            inner: clip,
            kind: CrossFadeKind::In {
                duration: self.duration,
            },
        }))
    }
}

/// Fade opacity from 1 → 0 over `duration` at the end of the clip.
#[derive(Debug, Clone, Copy)]
pub struct CrossFadeOut {
    /// Fade length.
    pub duration: Duration,
}

impl CrossFadeOut {
    /// Construct a cross-fade out.
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl VideoEffect for CrossFadeOut {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing(
                "cross-fade-out duration must be > 0",
            ));
        }
        Ok(Arc::new(CrossFadedVideo {
            inner: clip,
            kind: CrossFadeKind::Out {
                duration: self.duration,
            },
        }))
    }
}

#[derive(Clone, Copy)]
enum CrossFadeKind {
    In { duration: Duration },
    Out { duration: Duration },
}

struct CrossFadedVideo {
    inner: Arc<dyn VideoClip>,
    kind: CrossFadeKind,
}

impl CrossFadedVideo {
    fn opacity_at(&self, t: Time) -> f32 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        match self.kind {
            CrossFadeKind::In { duration } => {
                let d = duration.as_secs();
                if d <= 0.0 || t.as_secs() >= d {
                    1.0
                } else {
                    (t.as_secs() / d) as f32
                }
            }
            CrossFadeKind::Out { duration } => {
                let total = self.inner.duration().as_secs();
                let d = duration.as_secs();
                let start = (total - d).max(0.0);
                if d <= 0.0 || t.as_secs() <= start {
                    1.0
                } else {
                    (1.0 - (t.as_secs() - start) / d) as f32
                }
            }
        }
        .clamp(0.0, 1.0)
    }
}

impl VideoClip for CrossFadedVideo {
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
        let factor = self.opacity_at(t);
        let size = self.inner.size();
        match self.inner.mask_at(t)? {
            None => {
                if (factor - 1.0).abs() < f32::EPSILON {
                    Ok(None)
                } else {
                    let pixels = usize::try_from(size.pixel_count())
                        .map_err(|_| CoreError::invalid_frame("mask size exceeds usize"))?;
                    Ok(Some(Mask::from_raw(size, vec![factor; pixels])?))
                }
            }
            Some(mut mask) => {
                for s in mask.data_mut() {
                    *s = (*s * factor).clamp(0.0, 1.0);
                }
                Ok(Some(mask))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn cross_fade_in_mask() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::WHITE,
            Duration::from_secs(2.0),
        ));
        let out = CrossFadeIn::new(Duration::from_secs(1.0))
            .apply(clip)
            .unwrap();
        let m0 = out.mask_at(Time::ZERO).unwrap().unwrap();
        assert!(m0.data()[0] < 0.01);
        let m1 = out.mask_at(Time::from_secs(1.0)).unwrap();
        assert!(m1.is_none() || m1.unwrap().data()[0] > 0.99);
    }
}
