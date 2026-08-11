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
    AccelDecel, AudioDelay, AudioFadeIn, AudioFadeOut, AudioNormalize, BlackAndWhite, Blink, Crop,
    CrossFadeIn, CrossFadeOut, EvenSize, FadeIn, FadeOut, Freeze, FreezeRegion, GammaCorrection,
    HeadBlur, Identity, InvertColors, Loop, LumContrast, Margin, MaskColor, MasksAnd, MasksOr,
    MirrorX, MirrorY, MultiplyColor, MultiplyStereoVolume, Painting, Resize, ResizeFilter, Rotate,
    Scroll, SlideIn, SlideOut, SlideSide, Speed, SuperSample, TimeMirror, TimeSymmetrize,
    VolumeGain, resize_bilinear, validate_gain,
};
pub use reelforge_io::{
    AudioFileClip, FilterGraph, FilterOp, ImageClip, IoError, OpenAudioOptions, OpenVideoOptions,
    VideoFileClip, WriteGifOptions, WriteVideoOptions, ffmpeg_available, open_audio, open_video,
    run_filtergraph, write_av, write_gif, write_video,
};
pub use reelforge_text::{
    BITMAP_FONT, BurnInOptions, SubtitleCue, TextClip, TextClipOptions, TextError, burn_in_layers,
    parse_srt, text_clip,
};

/// Convenient imports for application code.
pub mod prelude {
    pub use crate::{
        AccelDecel, Anchor, AudioBuffer, AudioClip, AudioDelay, AudioEffect, AudioFadeIn,
        AudioFadeOut, AudioFileClip, AudioFormat, AudioNormalize, BITMAP_FONT, BlackAndWhite,
        Blink, BurnInOptions, ClipId, ColorClip, ComposeError, CompositeLayer, CompositeVideo,
        ConcatAudio, ConcatVideo, CoreError, Crop, CrossFadeIn, CrossFadeOut, Duration, EvenSize,
        FadeIn, FadeOut, FilterGraph, FilterOp, Frame, FrameFormat, Freeze, FreezeRegion,
        GammaCorrection, HeadBlur, Identity, ImageClip, InvertColors, IoError, Loop, LumContrast,
        Margin, Mask, MaskColor, MasksAnd, MasksOr, MirrorX, MirrorY, MultiplyColor,
        MultiplyStereoVolume, OpenAudioOptions, OpenVideoOptions, Painting, Position, Resize,
        ResizeFilter, Rgb8, Rgba8, Rotate, SampleLayout, Scroll, SilenceClip, Size, SlideIn,
        SlideOut, SlideSide, Speed, SubtitleCue, SuperSample, TextClip, TextClipOptions, TextError,
        Time, TimeMirror, TimeRange, TimeSymmetrize, TimedAudio, TimedVideo, VideoClip,
        VideoEffect, VideoFileClip, VolumeGain, WriteGifOptions, WriteVideoOptions,
        apply_audio_effects, apply_video_effects, burn_in_layers, composite_video,
        composite_video_with_background, concatenate_audio, concatenate_video, ffmpeg_available,
        open_audio, open_video, parse_srt, resize_bilinear, run_filtergraph, subclip_audio,
        subclip_video, text_clip, validate_gain, write_av, write_gif, write_video,
    };
}

/// Library version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
