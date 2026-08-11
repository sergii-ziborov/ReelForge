//! Core media model for `ReelForge`: time, frames, audio, and clip traits.

mod audio;
mod clip;
mod color;
mod effect;
mod error;
mod frame;
mod layout;
mod solid;
mod time;

pub use audio::{AudioBuffer, AudioFormat, SampleLayout};
pub use clip::{
    AudioClip, ClipId, TimedAudio, TimedVideo, VideoClip, subclip_audio, subclip_video,
};
pub use color::{Rgb8, Rgba8};
pub use effect::{AudioEffect, VideoEffect, apply_audio_effects, apply_video_effects};
pub use error::{CoreError, Result};
pub use frame::{Frame, FrameFormat, Mask};
pub use layout::{Anchor, Position, Size};
pub use solid::{ColorClip, SilenceClip};
pub use time::{Duration, Time, TimeRange};

/// Shared prelude for application code built on the core model.
pub mod prelude {
    pub use crate::{
        Anchor, AudioBuffer, AudioClip, AudioEffect, AudioFormat, ClipId, ColorClip, CoreError,
        Duration, Frame, FrameFormat, Mask, Position, Result, Rgb8, Rgba8, SilenceClip, Size, Time,
        TimeRange, TimedAudio, TimedVideo, VideoClip, VideoEffect, subclip_audio, subclip_video,
    };
}
