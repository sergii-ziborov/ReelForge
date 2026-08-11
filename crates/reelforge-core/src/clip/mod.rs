//! Video and audio clip traits and timing adapters.

mod arc_impl;
mod id;
mod timed;
mod traits;

pub use id::ClipId;
pub use timed::{TimedAudio, TimedVideo, subclip_audio, subclip_video};
pub use traits::{AudioClip, VideoClip};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb8;
    use crate::layout::Size;
    use crate::solid::ColorClip;
    use crate::time::{Duration, Time};
    use std::sync::Arc;

    #[test]
    fn subclip_maps_time() {
        let base = Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::BLUE,
            Duration::from_secs(10.0),
        ));
        let cut = TimedVideo::new(base, Time::from_secs(2.0), Duration::from_secs(3.0)).unwrap();
        assert!((cut.duration().as_secs() - 3.0).abs() < f64::EPSILON);
        let frame = cut.frame_at(Time::from_secs(0.0)).unwrap();
        assert_eq!(frame.size(), Size::new(4, 4));
        assert!(cut.frame_at(Time::from_secs(3.0)).is_err());
    }

    #[test]
    fn subclip_rejects_overflow() {
        let base = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::BLACK,
            Duration::from_secs(1.0),
        ));
        let err = TimedVideo::new(base, Time::from_secs(0.5), Duration::from_secs(1.0));
        assert!(matches!(
            err,
            Err(crate::CoreError::SubclipOutOfBounds { .. })
        ));
    }

    #[test]
    fn clip_id_display() {
        let id = ClipId::new("scene-1");
        assert_eq!(id.as_str(), "scene-1");
        assert_eq!(id.to_string(), "scene-1");
    }
}
