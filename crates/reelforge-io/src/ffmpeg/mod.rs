//! `FFmpeg` / `ffprobe` CLI backend (no link-time `libav` dependency).

mod helpers;
mod path;
mod probe;
mod process;

pub use helpers::{frame_count_for, frame_to_rgb24};
pub use path::{FfmpegTools, ffmpeg_available};
pub use probe::{AudioProbe, VideoProbe, probe_audio, probe_video};
pub use process::{decode_frame_rgb, decode_pcm_f32le, default_pcm_format, encode_rawvideo_h264};
