//! Open / write options.

use reelforge_core::{Duration, Size};

/// Options for writing a video file.
#[derive(Debug, Clone)]
pub struct WriteVideoOptions {
    /// Output path (UTF-8).
    pub path: String,
    /// Target frames per second.
    pub fps: f64,
    /// Optional override of output frame size.
    pub size: Option<Size>,
    /// Video codec name (default `libx264`).
    pub video_codec: Option<String>,
    /// Audio codec name (default `aac` when audio is written).
    pub audio_codec: Option<String>,
    /// Optional maximum duration to write (defaults to clip duration).
    pub duration: Option<Duration>,
    /// CRF quality for libx264-style encoders (`18`–`28` typical). `None` skips `-crf`.
    pub crf: Option<u8>,
    /// Pixel format for the encoder (default `yuv420p`).
    pub pixel_format: Option<String>,
    /// Extra `ffmpeg` arguments after `-c:v` / `-pix_fmt` (hardware encode, presets, bitrates).
    ///
    /// Example: `["-preset", "p4", "-cq", "23", "-b:v", "0"]` for NVIDIA encode.
    pub extra_ffmpeg_args: Vec<String>,
    /// Prefer native YUV/NV12 stdin when the clip can emit those surfaces.
    ///
    /// Default `true`. Set `false` to force the packed-RGB encode path.
    pub prefer_native_encode: bool,
}

impl WriteVideoOptions {
    /// Write to `path` at `fps`.
    #[must_use]
    pub fn new(path: impl Into<String>, fps: f64) -> Self {
        Self {
            path: path.into(),
            fps,
            size: None,
            video_codec: None,
            audio_codec: None,
            duration: None,
            crf: Some(23),
            pixel_format: None,
            extra_ffmpeg_args: Vec::new(),
            prefer_native_encode: true,
        }
    }

    /// Force packed-RGB stdin (skip native YUV/NV12 encode).
    #[must_use]
    pub fn with_rgb_encode(mut self) -> Self {
        self.prefer_native_encode = false;
        self
    }

    /// Override video codec (e.g. `libx264`, `h264_nvenc`, `h264_qsv`, `hevc_amf`).
    #[must_use]
    pub fn with_video_codec(mut self, codec: impl Into<String>) -> Self {
        self.video_codec = Some(codec.into());
        self
    }

    /// Override CRF (software x264/x265). Cleared by [`Self::with_nvenc`] helpers.
    #[must_use]
    pub fn with_crf(mut self, crf: u8) -> Self {
        self.crf = Some(crf);
        self
    }

    /// Disable `-crf` (useful for hardware encoders that use `-cq` / `-qp` instead).
    #[must_use]
    pub fn without_crf(mut self) -> Self {
        self.crf = None;
        self
    }

    /// Override audio codec (used by [`crate::write_av`]).
    #[must_use]
    pub fn with_audio_codec(mut self, codec: impl Into<String>) -> Self {
        self.audio_codec = Some(codec.into());
        self
    }

    /// Limit written duration.
    #[must_use]
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }

    /// Append raw ffmpeg CLI args (after `-c:v` / `-pix_fmt` / optional `-crf`).
    #[must_use]
    pub fn with_extra_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.extra_ffmpeg_args
            .extend(args.into_iter().map(Into::into));
        self
    }

    /// x264/x265 encode preset (`ultrafast` … `veryslow`). Large impact on wall time.
    ///
    /// Prefer `veryfast` / `superfast` for preview/export throughput; keep `medium`
    /// (ffmpeg default) for archival quality.
    #[must_use]
    pub fn with_x264_preset(self, preset: impl Into<String>) -> Self {
        self.with_extra_args(["-preset".into(), preset.into()])
    }

    /// Throughput-oriented defaults: `libx264` + `veryfast` (keeps CRF if set).
    #[must_use]
    pub fn with_fast_encode(self) -> Self {
        self.with_video_codec("libx264")
            .with_x264_preset("veryfast")
    }

    /// Maximum throughput: `libx264` + `ultrafast` (lower quality, much faster).
    #[must_use]
    pub fn with_ultrafast_encode(self) -> Self {
        self.with_video_codec("libx264")
            .with_x264_preset("ultrafast")
    }

    /// NVIDIA NVENC H.264 (`h264_nvenc`) with constant quality `cq` (typical 19–28).
    ///
    /// Requires an ffmpeg build with NVENC and a supported GPU.
    #[must_use]
    pub fn with_nvenc(self, cq: u8) -> Self {
        self.with_video_codec("h264_nvenc")
            .without_crf()
            .with_extra_args([
                "-preset".into(),
                "p4".into(),
                "-tune".into(),
                "hq".into(),
                "-rc".into(),
                "vbr".into(),
                "-cq".into(),
                cq.to_string(),
                "-b:v".into(),
                "0".into(),
            ])
    }

    /// NVIDIA NVENC H.264 **low-latency** realtime preset (`p1` + `ll`).
    ///
    /// Prefer this for live preview / interactive export. Quality is lower than
    /// [`Self::with_nvenc`] but latency and wall-clock are much better.
    #[must_use]
    pub fn with_nvenc_realtime(self, cq: u8) -> Self {
        self.with_video_codec("h264_nvenc")
            .without_crf()
            .with_extra_args([
                "-preset".into(),
                "p1".into(),
                "-tune".into(),
                "ll".into(),
                "-rc".into(),
                "vbr".into(),
                "-cq".into(),
                cq.to_string(),
                "-b:v".into(),
                "0".into(),
                "-rc-lookahead".into(),
                "0".into(),
            ])
    }

    /// NVIDIA NVENC HEVC (`hevc_nvenc`).
    #[must_use]
    pub fn with_nvenc_hevc(self, cq: u8) -> Self {
        self.with_video_codec("hevc_nvenc")
            .without_crf()
            .with_extra_args([
                "-preset".into(),
                "p4".into(),
                "-rc".into(),
                "vbr".into(),
                "-cq".into(),
                cq.to_string(),
                "-b:v".into(),
                "0".into(),
            ])
    }

    /// Intel Quick Sync H.264 (`h264_qsv`) with global quality.
    #[must_use]
    pub fn with_qsv(self, global_quality: u8) -> Self {
        self.with_video_codec("h264_qsv")
            .without_crf()
            .with_extra_args([
                "-global_quality".into(),
                global_quality.to_string(),
                "-look_ahead".into(),
                "1".into(),
            ])
    }

    /// AMD AMF H.264 (`h264_amf`) with quality mode.
    #[must_use]
    pub fn with_amf(self, quality: u8) -> Self {
        self.with_video_codec("h264_amf")
            .without_crf()
            .with_extra_args([
                "-quality".into(),
                "quality".into(),
                "-rc".into(),
                "cqp".into(),
                "-qp_i".into(),
                quality.to_string(),
                "-qp_p".into(),
                quality.to_string(),
            ])
    }
}

/// Options for opening a video file.
#[derive(Debug, Clone)]
pub struct OpenVideoOptions {
    /// Input path (UTF-8).
    pub path: String,
    /// Reserved: attach audio track when multi-track open is implemented.
    pub with_audio: bool,
}

impl OpenVideoOptions {
    /// Open media at `path`.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            with_audio: true,
        }
    }

    /// Disable audio association (video-only open).
    #[must_use]
    pub fn video_only(mut self) -> Self {
        self.with_audio = false;
        self
    }
}

/// Options for writing an animated GIF.
#[derive(Debug, Clone)]
pub struct WriteGifOptions {
    /// Output path (UTF-8), typically ending in `.gif`.
    pub path: String,
    /// Frames per second.
    pub fps: f64,
    /// Optional size override (must fit source if smaller).
    pub size: Option<Size>,
    /// Optional max duration.
    pub duration: Option<Duration>,
}

impl WriteGifOptions {
    /// Write GIF to `path` at `fps`.
    #[must_use]
    pub fn new(path: impl Into<String>, fps: f64) -> Self {
        Self {
            path: path.into(),
            fps,
            size: None,
            duration: None,
        }
    }

    /// Limit duration.
    #[must_use]
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = Some(duration);
        self
    }
}

/// Options for opening an audio file.
#[derive(Debug, Clone)]
pub struct OpenAudioOptions {
    /// Input path (UTF-8).
    pub path: String,
    /// Target sample rate for decoded PCM (default `48_000`).
    pub sample_rate: u32,
    /// Decode as stereo when true (default), else mono.
    ///
    /// Ignored when [`Self::layout`] is set or [`Self::native_layout`] is true.
    pub stereo: bool,
    /// Explicit decode layout (overrides [`Self::stereo`]).
    pub layout: Option<reelforge_core::SampleLayout>,
    /// Keep the file's channel count (`ffprobe`) instead of forcing stereo/mono.
    pub native_layout: bool,
}

impl OpenAudioOptions {
    /// Open audio at `path`.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            sample_rate: 48_000,
            stereo: true,
            layout: None,
            native_layout: false,
        }
    }

    /// Force a decode layout (5.1, 7.1, discrete, …).
    #[must_use]
    pub fn with_layout(mut self, layout: reelforge_core::SampleLayout) -> Self {
        self.layout = Some(layout);
        self.native_layout = false;
        self
    }

    /// Decode with the source channel count (no stereo downmix).
    #[must_use]
    pub fn with_native_layout(mut self) -> Self {
        self.native_layout = true;
        self.layout = None;
        self
    }
}
