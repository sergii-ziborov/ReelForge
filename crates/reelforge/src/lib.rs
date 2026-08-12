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
    apply_video_effects, psnr_rgb, ssim_rgb, subclip_audio, subclip_video,
};
pub use reelforge_fx::{
    AccelDecel, AudioDelay, AudioFadeIn, AudioFadeOut, AudioNormalize, BlackAndWhite, Blink, Crop,
    CrossFadeIn, CrossFadeOut, EvenSize, FadeIn, FadeOut, Freeze, FreezeRegion, GammaCorrection,
    HeadBlur, Identity, InvertColors, Loop, LumContrast, Margin, MaskColor, MasksAnd, MasksOr,
    MirrorX, MirrorY, MultiplyColor, MultiplyStereoVolume, Painting, Resize, ResizeFilter, Rotate,
    Scroll, SlideIn, SlideOut, SlideSide, Speed, SuperSample, TimeMirror, TimeSymmetrize,
    VolumeGain, resize_bicubic, resize_bilinear, validate_gain,
};
pub use reelforge_io::{
    AudioFileClip, CancelToken, ExtractedPlan, FilterGraph, FilterOp, ImageClip, IoError,
    OpenAudioOptions, OpenVideoOptions, OptimizeStats, OptimizedPlan, PlanBackend, PlanOp,
    PlanOutput, PlanSource, ProgressCallback, RENDER_PLAN_VERSION, RenderPlan, RgbFramePool,
    VideoFileClip, WriteControl, WriteGifOptions, WriteProgress, WriteStage, WriteVideoOptions,
    explain_plan, extract_ffmpeg, extract_from_optimized, ffmpeg_available, open_audio, open_video,
    optimize, optimize_plan, require_full_ffmpeg, run_filtergraph, run_render_plan, write_av,
    write_av_with, write_gif, write_gif_with, write_video, write_video_with,
};
pub use reelforge_text::{
    BITMAP_FONT, BurnInOptions, SubtitleCue, TextClip, TextClipOptions, TextError, burn_in_layers,
    parse_ass, parse_srt, parse_subtitles, parse_subtitles_path, parse_vtt, text_clip,
};

/// Convenient imports for application code.
pub mod prelude {
    pub use crate::{
        AccelDecel, Anchor, AudioBuffer, AudioClip, AudioDelay, AudioEffect, AudioFadeIn,
        AudioFadeOut, AudioFileClip, AudioFormat, AudioNormalize, BITMAP_FONT, BlackAndWhite,
        Blink, BurnInOptions, CancelToken, ClipId, ColorClip, ComposeError, CompositeLayer,
        CompositeVideo, ConcatAudio, ConcatVideo, CoreError, Crop, CrossFadeIn, CrossFadeOut,
        Duration, EvenSize, ExtractedPlan, FadeIn, FadeOut, FilterGraph, FilterOp, Frame,
        FrameFormat, Freeze, FreezeRegion, GammaCorrection, HeadBlur, Identity, ImageClip,
        InvertColors, IoError, Loop, LumContrast, Margin, Mask, MaskColor, MasksAnd, MasksOr,
        MirrorX, MirrorY, MultiplyColor, MultiplyStereoVolume, OpenAudioOptions, OpenVideoOptions,
        OptimizeStats, OptimizedPlan, Painting, PlanBackend, PlanOp, PlanOutput, PlanSource,
        Position, ProgressCallback, RENDER_PLAN_VERSION, RenderPlan, Resize, ResizeFilter, Rgb8,
        RgbFramePool, Rgba8, Rotate, SampleLayout, Scroll, SilenceClip, Size, SlideIn, SlideOut,
        SlideSide, Speed, SubtitleCue, SuperSample, TextClip, TextClipOptions, TextError, Time,
        TimeMirror, TimeRange, TimeSymmetrize, TimedAudio, TimedVideo, VideoClip, VideoEffect,
        VideoFileClip, VolumeGain, WriteControl, WriteGifOptions, WriteProgress, WriteStage,
        WriteVideoOptions, apply_audio_effects, apply_video_effects, burn_in_layers,
        composite_video, composite_video_with_background, concatenate_audio, concatenate_video,
        explain_plan, extract_ffmpeg, extract_from_optimized, ffmpeg_available, open_audio,
        open_video, optimize, optimize_plan, parse_ass, parse_srt, parse_subtitles,
        parse_subtitles_path, parse_vtt, psnr_rgb, require_full_ffmpeg, resize_bicubic,
        resize_bilinear, run_filtergraph, run_render_plan, ssim_rgb, subclip_audio, subclip_video,
        text_clip, validate_gain, write_av, write_av_with, write_gif, write_gif_with, write_video,
        write_video_with,
    };
}

/// Library version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
