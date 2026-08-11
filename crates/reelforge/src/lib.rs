//! `ReelForge` — programmatic video composition and editing for Rust.
//!
//! Re-exports the workspace crates behind a single dependency.

pub use reelforge_compose as compose;
pub use reelforge_core as core;
pub use reelforge_fx as fx;
pub use reelforge_io as io;
pub use reelforge_text as text;

pub use reelforge_compose::{
    ComposeError, CompositeLayer, CompositeVideo, ConcatAudio, ConcatVideo, composite_video,
    composite_video_with_background, concatenate_audio, concatenate_video,
};
pub use reelforge_core::{
    Anchor, AudioBuffer, AudioClip, AudioEffect, AudioFormat, ClipId, ColorClip, CoreError,
    Duration, Frame, FrameFormat, Mask, Position, Rgb8, Rgba8, SampleLayout, SilenceClip, Size,
    Time, TimeRange, TimedAudio, TimedVideo, VideoClip, VideoEffect, apply_audio_effects,
    apply_video_effects, subclip_audio, subclip_video,
};
pub use reelforge_fx::{
    BlackAndWhite, Crop, CrossFadeIn, CrossFadeOut, EvenSize, FadeIn, FadeOut, Freeze, Identity,
    InvertColors, Loop, Margin, MirrorX, MirrorY, MultiplyColor, Resize, Rotate, Speed, TimeMirror,
    VolumeGain, validate_gain,
};
pub use reelforge_io::{
    AudioFileClip, FilterGraph, FilterOp, ImageClip, IoError, OpenAudioOptions, OpenVideoOptions,
    VideoFileClip, WriteVideoOptions, ffmpeg_available, open_audio, open_video, run_filtergraph,
    write_video,
};
pub use reelforge_text::{
    BITMAP_FONT, BurnInOptions, SubtitleCue, TextClip, TextClipOptions, TextError, burn_in_layers,
    parse_srt, text_clip,
};

/// Convenient imports for application code.
pub mod prelude {
    pub use crate::{
        Anchor, AudioBuffer, AudioClip, AudioEffect, AudioFileClip, AudioFormat, BITMAP_FONT,
        BlackAndWhite, BurnInOptions, ClipId, ColorClip, ComposeError, CompositeLayer,
        CompositeVideo, ConcatAudio, ConcatVideo, CoreError, Crop, CrossFadeIn, CrossFadeOut,
        Duration, EvenSize, FadeIn, FadeOut, FilterGraph, FilterOp, Frame, FrameFormat, Freeze,
        Identity, ImageClip, InvertColors, IoError, Loop, Margin, Mask, MirrorX, MirrorY,
        MultiplyColor, OpenAudioOptions, OpenVideoOptions, Position, Resize, Rgb8, Rgba8, Rotate,
        SampleLayout, SilenceClip, Size, Speed, SubtitleCue, TextClip, TextClipOptions, TextError,
        Time, TimeMirror, TimeRange, TimedAudio, TimedVideo, VideoClip, VideoEffect, VideoFileClip,
        VolumeGain, WriteVideoOptions, apply_audio_effects, apply_video_effects, burn_in_layers,
        composite_video, composite_video_with_background, concatenate_audio, concatenate_video,
        ffmpeg_available, open_audio, open_video, parse_srt, run_filtergraph, subclip_audio,
        subclip_video, text_clip, validate_gain, write_video,
    };
}

/// Library version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
