//! Core media model for `ReelForge`: time, frames, audio, and clip traits.

mod alpha;
mod audio;
mod audio_time;
mod cache;
mod clip;
mod color;
mod convert;
mod effect;
mod error;
mod external;
mod frame;
mod layout;
mod media_range;
mod media_time;
mod plane;
mod quality;
mod resample;
mod solid;
mod stream;
mod surface;
mod time;
mod yuv;

pub use alpha::AlphaMode;
pub use audio::{AudioBuffer, AudioFormat, SampleLayout};
pub use audio_time::AudioTimeline;
pub use cache::{
    CacheConfig, CacheStats, CachedVideo, cache_video, cache_video_realtime, cache_video_with,
};
pub use clip::{
    AudioClip, ClipId, TimedAudio, TimedVideo, VideoClip, subclip_audio, subclip_video,
};
pub use color::{Rgb8, Rgba8};
pub use convert::surface_to_rgb_frame;
pub use external::{ExternalBackend, ExternalSurface};
pub use effect::{AudioEffect, VideoEffect, apply_audio_effects, apply_video_effects};
pub use error::{CoreError, Result};
pub use frame::{Frame, FrameFormat, Mask};
pub use layout::{Anchor, Position, Size};
pub use media_range::MediaRange;
pub use media_time::MediaTime;
pub use plane::{SurfacePlane, validate_planes};
pub use quality::{psnr_rgb, ssim_rgb};
pub use resample::resample_linear;
pub use solid::{ColorClip, SilenceClip};
pub use stream::{FrameStream, stream_video, stream_video_raw};
pub use surface::{
    ColorInfo, ColorPrimaries, ColorRange, ColorSpace, ColorTransfer, MemoryLocation, PixelFormat,
    StreamTimeBase, VideoSurface,
};
pub use time::{Duration, Time, TimeRange};
pub use yuv::{join_packed_planes, split_packed_planes, surface_to_rawvideo};

/// Shared prelude for application code built on the core model.
pub mod prelude {
    pub use crate::{
        AlphaMode, Anchor, AudioBuffer, AudioClip, AudioEffect, AudioFormat, AudioTimeline,
        CacheConfig, CacheStats, CachedVideo, ClipId, ColorClip, ColorInfo, ColorPrimaries,
        ColorRange, ColorSpace, ColorTransfer, CoreError, Duration, ExternalBackend,
        ExternalSurface, Frame, FrameFormat, FrameStream, Mask, MemoryLocation, PixelFormat,
        Position, Result, Rgb8, Rgba8, SilenceClip,
        Size, StreamTimeBase, SurfacePlane, Time, TimeRange, TimedAudio, TimedVideo, VideoClip,
        VideoEffect, VideoSurface, cache_video, cache_video_realtime, resample_linear,
        stream_video, stream_video_raw, subclip_audio, subclip_video,
    };
}
