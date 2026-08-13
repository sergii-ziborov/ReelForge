//! `FFmpeg` / `ffprobe` CLI backend (no link-time `libav` dependency).

mod helpers;
mod path;
mod planar;
mod probe;
mod process;
mod stream;
mod timing;

pub use helpers::{frame_count_for, frame_to_rgb24, frame_to_rgb24_into};
pub use path::{FfmpegTools, ffmpeg_available};
pub use planar::{SequentialPlanarDecoder, decode_frame_planes};
pub use probe::{AudioProbe, VideoProbe, probe_audio, probe_has_audio, probe_video};
pub use process::{
    decode_frame_rgb, decode_pcm_f32le, default_pcm_format, encode_rawvideo_gif,
    encode_rawvideo_h264, mux_video_audio,
};
pub use stream::{SequentialMode, SequentialRgbDecoder};
pub use timing::{FrameTimingIndex, probe_frame_timing};
