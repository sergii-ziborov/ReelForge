//! Decode and encode media containers and streams for `ReelForge`.
//!
//! File I/O uses the host `ffmpeg` / `ffprobe` CLI — no link-time `libav`
//! dependency. Set `REELFORGE_FFMPEG` / `REELFORGE_FFPROBE` or put the tools on
//! `PATH`.

mod adapter;
mod audio_file;
mod control;
mod encode_native;
mod error;
mod exec;
mod ffmpeg;
mod filtergraph;
mod graph_run;
mod image_clip;
mod manifest_seal;
mod mask_bridge;
mod options;
mod pipeline;
mod pool;
mod preview;
mod realtime;
mod render_plan;
mod stage_cache;
mod tracks_json;
mod video_file;
mod waveform;
mod write;

pub use adapter::{AdapterHost, AdapterOutput, execute_adapter};
pub use encode_native::{encode_sampled_rawvideo, is_native_raw_format};
pub use audio_file::{AudioFileClip, layout_for_channels, open_audio};
pub use control::{CancelToken, ProgressCallback, WriteControl, WriteProgress, WriteStage};
pub use error::{IoError, Result};
pub use ffmpeg::{
    AudioProbe, FfmpegTools, FrameTimingIndex, SequentialMode, VideoProbe, encode_rawvideo_h264,
    ffmpeg_available, frame_to_rgb24, probe_audio, probe_frame_timing, probe_has_audio,
};
pub use filtergraph::{
    AudioCopyMode, FilterGraph, FilterOp, FiltergraphRunOptions, mux_copy_audio, run_filtergraph,
    run_filtergraph_with,
};
pub use graph_run::{
    GraphBundle, GraphEncodeHints, GraphRunOptions, explain_render_graph,
    explain_render_graph_with, is_executable_op, materialize_execution_plan, materialize_graph,
    materialize_graph_bundle, materialize_graph_with_seeds, node_backend, run_execution_plan,
    run_execution_plan_with, run_execution_plan_with_manifest, run_render_graph,
    run_render_graph_with, run_render_graph_with_manifest,
};
pub use image_clip::ImageClip;
pub use manifest_seal::{fingerprint_file, seal_manifest_on_disk};
pub use mask_bridge::{
    apply_region_redaction, mask_timeline_from_box, mask_timeline_from_box_subject,
    mask_timeline_to_track_set, privacy_style_from_redaction, region_redaction_from_value,
    track_timelines_to_track_set,
};
pub use options::{OpenAudioOptions, OpenVideoOptions, WriteGifOptions, WriteVideoOptions};
pub use pool::RgbFramePool;
pub use preview::{ProxyOptions, proxy_size, thumbnail_png_bytes, write_proxy, write_thumbnail};
pub use realtime::{
    HwEncoderSupport, RealtimeExport, detect_hw_encoders, nvenc_available, realtime_write_options,
    run_filtergraph_encode,
};
pub use render_plan::{
    ExtractedPlan, OptimizeStats, OptimizedPlan, PlanBackend, PlanOp, PlanOutput, PlanSource,
    RENDER_PLAN_VERSION, RenderPlan, apply_plan_ops, explain_plan, extract_ffmpeg,
    extract_from_optimized, is_known_custom, optimize, optimize_plan, require_full_ffmpeg,
    run_render_plan, run_render_plan_with, validate_remainder,
};
pub use stage_cache::StageCache;
pub use tracks_json::{
    SampleJson, TRACKS_JSON_VERSION, TrackJson, TracksDocument, load_track_set, parse_track_set,
    track_set_from_value,
};
pub use video_file::{VideoFileClip, open_video};
pub use waveform::{
    WaveformOptions, WaveformPeak, compute_waveform, compute_waveform_default, waveform_to_json,
};
pub use write::{
    write_av, write_av_with, write_duration, write_gif, write_gif_with, write_video,
    write_video_with,
};
