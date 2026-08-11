//! Effect contracts applied to video and audio clips.

use crate::clip::{AudioClip, VideoClip};
use crate::error::Result;
use std::sync::Arc;

/// Transform that produces a new video clip from an existing one.
///
/// Effects are pure graph nodes: they capture parameters and wrap the source
/// clip without rendering until frames are sampled.
pub trait VideoEffect: Send + Sync {
    /// Apply this effect to `clip`, returning a new clip graph node.
    ///
    /// # Errors
    ///
    /// Returns a core or effect-specific error when parameters are invalid for
    /// the source.
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>>;
}

/// Transform that produces a new audio clip from an existing one.
pub trait AudioEffect: Send + Sync {
    /// Apply this effect to `clip`, returning a new clip graph node.
    ///
    /// # Errors
    ///
    /// Returns a core or effect-specific error when parameters are invalid for
    /// the source.
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>>;
}

/// Apply a sequence of video effects in order.
///
/// # Errors
///
/// Propagates the first effect error.
pub fn apply_video_effects(
    clip: Arc<dyn VideoClip>,
    effects: &[Arc<dyn VideoEffect>],
) -> Result<Arc<dyn VideoClip>> {
    let mut current = clip;
    for effect in effects {
        current = effect.apply(current)?;
    }
    Ok(current)
}

/// Apply a sequence of audio effects in order.
///
/// # Errors
///
/// Propagates the first effect error.
pub fn apply_audio_effects(
    clip: Arc<dyn AudioClip>,
    effects: &[Arc<dyn AudioEffect>],
) -> Result<Arc<dyn AudioClip>> {
    let mut current = clip;
    for effect in effects {
        current = effect.apply(current)?;
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb8;
    use crate::layout::Size;
    use crate::solid::ColorClip;
    use crate::time::Duration;

    struct Noop;

    impl VideoEffect for Noop {
        fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
            Ok(clip)
        }
    }

    #[test]
    fn apply_video_chain() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let effects: Vec<Arc<dyn VideoEffect>> = vec![Arc::new(Noop), Arc::new(Noop)];
        let out = apply_video_effects(clip, &effects).unwrap();
        assert!((out.duration().as_secs() - 1.0).abs() < f64::EPSILON);
    }
}
