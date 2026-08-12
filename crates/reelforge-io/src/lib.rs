//! Decode and encode media containers and streams for `ReelForge`.
//!
//! File I/O uses the host `ffmpeg` / `ffprobe` CLI — no link-time `libav`
//! dependency. Set `REELFORGE_FFMPEG` / `REELFORGE_FFPROBE` or put the tools on
//! `PATH`.

mod audio_file;
mod control;
mod error;
mod ffmpeg;
mod filtergraph;
mod graph_run;
mod image_clip;
mod mask_bridge;
mod options;
mod pipeline;
mod pool;
mod render_plan;
mod tracks_json;
mod video_file;
mod write;

pub use audio_file::{AudioFileClip, layout_for_channels, open_audio};
pub use control::{CancelToken, ProgressCallback, WriteControl, WriteProgress, WriteStage};
pub use error::{IoError, Result};
pub use ffmpeg::{
    AudioProbe, FfmpegTools, VideoProbe, encode_rawvideo_h264, ffmpeg_available, frame_to_rgb24,
};
pub use filtergraph::{FilterGraph, FilterOp, run_filtergraph};
pub use graph_run::{
    GraphEncodeHints, GraphRunOptions, explain_render_graph, explain_render_graph_with,
    is_executable_op, materialize_graph, materialize_graph_with_seeds, node_backend,
    run_execution_plan, run_execution_plan_with, run_render_graph, run_render_graph_with,
};
pub use image_clip::ImageClip;
pub use mask_bridge::{
    apply_region_redaction, mask_timeline_from_box, mask_timeline_to_track_set,
    region_redaction_from_value,
};
pub use options::{OpenAudioOptions, OpenVideoOptions, WriteGifOptions, WriteVideoOptions};
pub use pool::RgbFramePool;
pub use render_plan::{
    ExtractedPlan, OptimizeStats, OptimizedPlan, PlanBackend, PlanOp, PlanOutput, PlanSource,
    RENDER_PLAN_VERSION, RenderPlan, apply_plan_ops, explain_plan, extract_ffmpeg,
    extract_from_optimized, is_known_custom, optimize, optimize_plan, require_full_ffmpeg,
    run_render_plan, run_render_plan_with, validate_remainder,
};
pub use tracks_json::{
    SampleJson, TRACKS_JSON_VERSION, TrackJson, TracksDocument, load_track_set, parse_track_set,
    track_set_from_value,
};
pub use video_file::{VideoFileClip, open_video};
pub use write::{
    write_av, write_av_with, write_duration, write_gif, write_gif_with, write_video,
    write_video_with,
};
