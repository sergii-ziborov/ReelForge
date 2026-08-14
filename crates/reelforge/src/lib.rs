//! `ReelForge` — programmatic video composition and editing for Rust.
//!
//! Re-exports the workspace crates behind a single dependency.

pub use reelforge_compose as compose;
pub use reelforge_core as core;
pub use reelforge_fx as fx;
pub use reelforge_io as io;
pub use reelforge_project as project;
pub use reelforge_render_graph as render_graph;
pub use reelforge_sightloom_adapter as sightloom_adapter;
pub use reelforge_text as text;

pub use reelforge_compose::{
    ComposeError, CompositeLayer, CompositeVideo, ConcatAudio, ConcatVideo, MixAudio, MixTrack,
    composite_video, composite_video_with_background, concatenate_audio, concatenate_video,
    mix_audio, mix_audio_clips,
};
pub use reelforge_core::{
    AlphaMode, Anchor, AudioBuffer, AudioClip, AudioEffect, AudioFormat, AudioTimeline,
    CacheConfig, CacheStats, CachedVideo, ClipId, ColorClip, ColorInfo, ColorPrimaries, ColorRange,
    ColorSpace, ColorTransfer, CoreError, Duration, Frame, FrameFormat, FrameStream, Mask,
    MediaRange, MediaTime, MemoryLocation, PixelFormat, Position, Rgb8, Rgba8, SampleLayout,
    SilenceClip, Size,
    StreamTimeBase, SurfacePlane, Time, TimeRange, TimedAudio, TimedVideo, VideoClip, VideoEffect,
    VideoSurface, apply_audio_effects, apply_video_effects, cache_video, cache_video_realtime,
    cache_video_with, psnr_rgb, resample_linear, ssim_rgb, stream_video, stream_video_raw,
    subclip_audio, subclip_video,
};
pub use reelforge_fx::{
    AccelDecel, AudioDelay, AudioFadeIn, AudioFadeOut, AudioNormalize, BlackAndWhite, Blink, Crop,
    CrossFadeIn, CrossFadeOut, EvenSize, FadeIn, FadeOut, Freeze, FreezeRegion, GammaCorrection,
    HeadBlur, Identity, InvertColors, Loop, LumContrast, Margin, MaskColor, MasksAnd, MasksOr,
    CoverageMask, MirrorX, MirrorY, MultiplyColor, MultiplyStereoVolume, Painting, PrivacyStyle,
    RegionAt, RegionSample, RegionTrack, Resize, ResizeFilter, Rotate, Scroll, SlideIn, SlideOut,
    SlideSide, Speed,
    SuperSample, TimeMirror, TimeSymmetrize, TrackSet, TrackedBlur, TrackedPrivacy, VolumeGain,
    resize_bicubic, resize_bilinear, validate_gain,
};
pub use reelforge_io::{
    AudioCopyMode, AudioFileClip, CancelToken, ExtractedPlan, FilterGraph, FilterOp,
    FiltergraphRunOptions, FrameTimingIndex, GraphBundle, GraphEncodeHints, GraphRunOptions,
    HwEncoderSupport, ImageClip, IoError, OpenAudioOptions, OpenVideoOptions, OptimizeStats,
    OptimizedPlan, PlanBackend, PlanOp, PlanOutput, PlanSource, ProgressCallback, ProxyOptions,
    RENDER_PLAN_VERSION, RealtimeExport, RenderPlan, RgbFramePool, SampleJson, SequentialMode,
    StageCache, TRACKS_JSON_VERSION, TrackJson, TracksDocument, VideoFileClip, WaveformOptions,
    WaveformPeak, WriteControl, WriteGifOptions, WriteProgress, WriteStage, WriteVideoOptions,
    AdapterHost, AdapterOutput, apply_plan_ops, apply_region_redaction, compute_waveform,
    compute_waveform_default, execute_adapter,
    detect_hw_encoders, explain_plan, explain_render_graph, explain_render_graph_with,
    extract_ffmpeg, extract_from_optimized, ffmpeg_available, fingerprint_file, is_executable_op,
    is_known_custom, load_track_set, mask_timeline_from_box, mask_timeline_from_box_subject,
    mask_timeline_to_track_set, materialize_execution_plan, materialize_graph,
    materialize_graph_bundle, materialize_graph_with_seeds, mux_copy_audio, node_backend,
    nvenc_available, open_audio, open_video, optimize, optimize_plan, parse_track_set,
    privacy_style_from_redaction, probe_audio, probe_frame_timing, probe_has_audio, proxy_size,
    realtime_write_options, region_redaction_from_value, require_full_ffmpeg, run_execution_plan,
    run_execution_plan_with, run_execution_plan_with_manifest, run_filtergraph,
    run_filtergraph_encode, run_filtergraph_with, run_render_graph, run_render_graph_with,
    run_render_graph_with_manifest, run_render_plan, run_render_plan_with, seal_manifest_on_disk,
    thumbnail_png_bytes, track_set_from_value, track_timelines_to_track_set, validate_remainder,
    waveform_to_json, write_av, write_av_with, write_gif, write_gif_with, write_proxy,
    write_thumbnail, write_video, write_video_with,
};
pub use reelforge_project::{
    CAPTURE_PROJECT_VERSION, CaptureProject, Gap, Marker, MediaRef, MediaRefId, Metadata,
    NestedSequence, ProjectCompile, ProjectError, ProjectId, Retiming, SemanticRef, Sequence,
    SequenceId, SourceRange, TimelineClip, TimelineClipId, TimelineItem, TimelineTrack,
    TimelineTrackId, TrackKind, Transition, TransitionKind, compile_project,
};
pub use reelforge_render_graph::{
    ARTIFACT_MANIFEST_VERSION, AdapterStage, Animated, AppearanceId, ArtifactKind,
    ArtifactManifest, ArtifactRef, AssetIndex, BackendClass, CapabilitySet, CompileDiagnostics,
    CompiledGraph, CompiledNode, CompiledNodeKind, CompiledOp, CompiledOutput, CostEstimate,
    Easing, ExecutionPlan, ExecutionStage, ExecutorKind, FfmpegStage, Geometry, GpuStage,
    GraphErrorCode, GraphOutput, Keyframe, MaskInterpolation, MaskLifecycle, MaskProvenance,
    MaskAsset, MaskAssetRef, MaskFrame, MaskRef, MaskRegionAt, MaskSample, MaskTimeline, MediaAsset,
    MediaAssetId, MediaContract,
    MissingMaskPolicy, NodeId, NodeIndex, ObservationId, OcclusionState, OperationDescriptor,
    OperationId, OperationLimits, OperationRegistry, RENDER_GRAPH_VERSION, RedactionStyle,
    RegionRedaction, RenderGraph, RenderNode, RenderNodeKind, RustStage, SemVer, StageArtifacts,
    StageCacheKey, StageIo, StagePort, SubjectId, TrackId, TrackSample, TrackTimeline, TypedParams,
    artifact_manifest, check_registry_executor_parity, compile_graph, compile_graph_ops,
    compile_op, ensure_executable, fingerprint_compiled_graph, fingerprint_execution_plan,
    fingerprint_graph_run, fingerprint_render_graph, fingerprint_stage, fingerprint_stage_key,
    infer_node_contract, is_executable_op_id, mask_timeline_from_tracks, schedule_compiled,
    schedule_graph,
};
pub use reelforge_sightloom_adapter::{
    AdapterError, TrackDocument, load_track_timelines, parse_track_timelines,
    track_timelines_from_value,
};
pub use reelforge_text::{
    BITMAP_FONT, BurnInOptions, SubtitleCue, TextClip, TextClipOptions, TextError, burn_in_layers,
    parse_ass, parse_srt, parse_subtitles, parse_subtitles_path, parse_vtt, text_clip,
};

/// Convenient imports for application code.
pub mod prelude {
    pub use crate::{
        AccelDecel, AlphaMode, Anchor, AudioBuffer, AudioClip, AudioDelay, AudioEffect,
        AudioFadeIn, AudioFadeOut, AudioFileClip, AudioFormat, AudioNormalize, BITMAP_FONT,
        BlackAndWhite, Blink, BurnInOptions, CacheConfig, CacheStats, CachedVideo, CancelToken,
        ClipId, ColorClip, ComposeError, CompositeLayer, CompositeVideo, ConcatAudio, ConcatVideo,
        CoreError, Crop, CrossFadeIn, CrossFadeOut, Duration, EvenSize, ExtractedPlan, FadeIn,
        FadeOut, FilterGraph, FilterOp, Frame, FrameFormat, FrameStream, Freeze, FreezeRegion,
        GammaCorrection, HeadBlur, Identity, ImageClip, InvertColors, IoError, Loop, LumContrast,
        Margin, Mask, MaskColor, MasksAnd, MasksOr, MirrorX, MirrorY, MultiplyColor,
        MultiplyStereoVolume, OpenAudioOptions, OpenVideoOptions, OptimizeStats, OptimizedPlan,
        Painting, PlanBackend, PlanOp, PlanOutput, PlanSource, Position, ProgressCallback,
        RENDER_PLAN_VERSION, RegionSample, RegionTrack, RenderPlan, Resize, ResizeFilter, Rgb8,
        RgbFramePool, Rgba8, Rotate, SampleJson, SampleLayout, Scroll, SilenceClip, Size, SlideIn,
        SlideOut, SlideSide, Speed, SubtitleCue, SuperSample, TRACKS_JSON_VERSION, TextClip,
        TextClipOptions, TextError, Time, TimeMirror, TimeRange, TimeSymmetrize, TimedAudio,
        TimedVideo, TrackJson, TrackSet, TrackedBlur, TracksDocument, VideoClip, VideoEffect,
        VideoFileClip, VolumeGain, WriteControl, WriteGifOptions, WriteProgress, WriteStage,
        WriteVideoOptions, apply_audio_effects, apply_plan_ops, apply_video_effects,
        burn_in_layers, cache_video, cache_video_realtime, cache_video_with, composite_video,
        composite_video_with_background, concatenate_audio, concatenate_video, explain_plan,
        extract_ffmpeg, extract_from_optimized, ffmpeg_available, is_known_custom, load_track_set,
        open_audio, open_video, optimize, optimize_plan, parse_ass, parse_srt, parse_subtitles,
        parse_subtitles_path, parse_track_set, parse_vtt, psnr_rgb, require_full_ffmpeg,
        resize_bicubic, resize_bilinear, run_filtergraph, run_render_plan, run_render_plan_with,
        ssim_rgb, stream_video, stream_video_raw, subclip_audio, subclip_video, text_clip,
        track_set_from_value, validate_gain, validate_remainder, write_av, write_av_with,
        write_gif, write_gif_with, write_video, write_video_with,
    };
}

/// Library version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
