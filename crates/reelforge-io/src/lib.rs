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
mod image_clip;
mod options;
mod pipeline;
mod pool;
mod render_plan;
mod video_file;
mod write;

pub use audio_file::{AudioFileClip, layout_for_channels, open_audio};
pub use control::{CancelToken, ProgressCallback, WriteControl, WriteProgress, WriteStage};
pub use error::{IoError, Result};
pub use ffmpeg::{
    AudioProbe, FfmpegTools, VideoProbe, encode_rawvideo_h264, ffmpeg_available, frame_to_rgb24,
};
pub use filtergraph::{FilterGraph, FilterOp, run_filtergraph};
pub use image_clip::ImageClip;
pub use options::{OpenAudioOptions, OpenVideoOptions, WriteGifOptions, WriteVideoOptions};
pub use pool::RgbFramePool;
pub use render_plan::{
    ExtractedPlan, OptimizeStats, OptimizedPlan, PlanBackend, PlanOp, PlanOutput, PlanSource,
    RENDER_PLAN_VERSION, RenderPlan, explain_plan, extract_ffmpeg, extract_from_optimized,
    optimize, optimize_plan, require_full_ffmpeg, run_render_plan,
};
pub use video_file::{VideoFileClip, open_video};
pub use write::{
    write_av, write_av_with, write_duration, write_gif, write_gif_with, write_video,
    write_video_with,
};
