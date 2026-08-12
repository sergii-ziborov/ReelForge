//! Core media model for `ReelForge`: time, frames, audio, and clip traits.

mod audio;
mod cache;
mod clip;
mod color;
mod effect;
mod error;
mod frame;
mod layout;
mod media_time;
mod quality;
mod solid;
mod stream;
mod time;

pub use audio::{AudioBuffer, AudioFormat, SampleLayout};
pub use media_time::MediaTime;
pub use cache::{
    CacheConfig, CacheStats, CachedVideo, cache_video, cache_video_realtime, cache_video_with,
};
pub use clip::{
    AudioClip, ClipId, TimedAudio, TimedVideo, VideoClip, subclip_audio, subclip_video,
};
pub use color::{Rgb8, Rgba8};
pub use effect::{AudioEffect, VideoEffect, apply_audio_effects, apply_video_effects};
pub use error::{CoreError, Result};
pub use frame::{Frame, FrameFormat, Mask};
pub use layout::{Anchor, Position, Size};
pub use quality::{psnr_rgb, ssim_rgb};
pub use solid::{ColorClip, SilenceClip};
pub use stream::{FrameStream, stream_video, stream_video_raw};
pub use time::{Duration, Time, TimeRange};

/// Shared prelude for application code built on the core model.
pub mod prelude {
    pub use crate::{
        Anchor, AudioBuffer, AudioClip, AudioEffect, AudioFormat, CacheConfig, CacheStats,
        CachedVideo, ClipId, ColorClip, CoreError, Duration, Frame, FrameFormat, FrameStream, Mask,
        Position, Result, Rgb8, Rgba8, SilenceClip, Size, Time, TimeRange, TimedAudio, TimedVideo,
        VideoClip, VideoEffect, cache_video, cache_video_realtime, stream_video, stream_video_raw,
        subclip_audio, subclip_video,
    };
}
