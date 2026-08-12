//! Realtime / GPU-oriented export (file → filtergraph → HW encode).
//!
//! Unlike the Rust pixel path (`write_video`), this keeps frames in the host
//! `ffmpeg` process: decode + filters + NVENC/QSV/AMF (or fast libx264) without
//! RGB round-trips. `MoviePy` cannot do this — it always materializes frames in
//! Python/numpy.

use crate::error::{IoError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::filtergraph::FilterGraph;
use crate::options::WriteVideoOptions;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Hardware / accelerated video encoders detected in the host `ffmpeg`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct HwEncoderSupport {
    /// NVIDIA NVENC H.264 (`h264_nvenc`).
    pub nvenc_h264: bool,
    /// NVIDIA NVENC HEVC (`hevc_nvenc`).
    pub nvenc_hevc: bool,
    /// Intel Quick Sync H.264 (`h264_qsv`).
    pub qsv_h264: bool,
    /// AMD AMF H.264 (`h264_amf`).
    pub amf_h264: bool,
}

impl HwEncoderSupport {
    /// Whether any GPU H.264 path is present.
    #[must_use]
    pub fn any_h264(&self) -> bool {
        self.nvenc_h264 || self.qsv_h264 || self.amf_h264
    }

    /// Preferred codec name for realtime H.264, if any.
    #[must_use]
    pub fn preferred_h264(&self) -> Option<&'static str> {
        if self.nvenc_h264 {
            Some("h264_nvenc")
        } else if self.qsv_h264 {
            Some("h264_qsv")
        } else if self.amf_h264 {
            Some("h264_amf")
        } else {
            None
        }
    }
}

static HW_CACHE: OnceLock<HwEncoderSupport> = OnceLock::new();

/// Probe host `ffmpeg -encoders` once (cached).
///
/// # Errors
///
/// Tools missing or probe spawn failure.
pub fn detect_hw_encoders() -> Result<HwEncoderSupport> {
    if let Some(s) = HW_CACHE.get() {
        return Ok(s.clone());
    }
    let tools = FfmpegTools::discover()?;
    let output = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .output()
        .map_err(|e| IoError::process(format!("ffmpeg -encoders failed: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let support = HwEncoderSupport {
        nvenc_h264: text.contains("h264_nvenc"),
        nvenc_hevc: text.contains("hevc_nvenc"),
        qsv_h264: text.contains("h264_qsv"),
        amf_h264: text.contains("h264_amf"),
    };
    let _ = HW_CACHE.set(support.clone());
    Ok(support)
}

/// Whether NVIDIA NVENC H.264 is available.
#[must_use]
pub fn nvenc_available() -> bool {
    detect_hw_encoders().is_ok_and(|s| s.nvenc_h264)
}

/// Build encode options for realtime: GPU if present, else `libx264` ultrafast.
///
/// # Errors
///
/// Propagates tool discovery failures from [`detect_hw_encoders`].
pub fn realtime_write_options(path: impl Into<String>, fps: f64) -> Result<WriteVideoOptions> {
    let path = path.into();
    let hw = detect_hw_encoders()?;
    let base = WriteVideoOptions::new(path, fps);
    if let Some(codec) = hw.preferred_h264() {
        let opts = match codec {
            "h264_nvenc" => base.with_nvenc_realtime(23),
            "h264_qsv" => base.with_qsv(23),
            "h264_amf" => base.with_amf(23),
            _ => base.with_ultrafast_encode(),
        };
        Ok(opts)
    } else {
        Ok(base.with_ultrafast_encode())
    }
}

/// Run input through an `FFmpeg` filtergraph and encode with full write options.
///
/// This is the **zero-copy-to-Rust** path: no RGB frames enter the process.
/// Ideal for preview/realtime export when the edit is filter-expressible.
///
/// # Errors
///
/// Missing files, empty graph, or process failure.
pub fn run_filtergraph_encode(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    graph: &FilterGraph,
    options: &WriteVideoOptions,
) -> Result<()> {
    let tools = FfmpegTools::discover()?;
    let vf = graph.to_vf().map_err(IoError::message)?;
    let input = input.as_ref();
    let output_path = output.as_ref();
    if !input.is_file() {
        return Err(IoError::message(format!(
            "input not found: {}",
            input.display()
        )));
    }
    if let Some(parent) = output_path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| IoError::message(format!("create output dir: {e}")))?;
    }

    let codec = options.video_codec.as_deref().unwrap_or("libx264");
    let pix = options.pixel_format.as_deref().unwrap_or("yuv420p");

    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vf", &vf, "-an", "-c:v", codec, "-pix_fmt", pix]);
    if let Some(crf) = options.crf {
        cmd.args(["-crf", &crf.to_string()]);
    }
    if !options.extra_ffmpeg_args.is_empty() {
        cmd.args(&options.extra_ffmpeg_args);
    }
    // Cap duration if requested (seconds).
    if let Some(dur) = options.duration {
        cmd.args(["-t", &format!("{:.6}", dur.as_secs())]);
    }
    cmd.arg(output_path);

    let status = cmd
        .status()
        .map_err(|e| IoError::process(format!("ffmpeg realtime encode spawn failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(IoError::process(format!(
            "ffmpeg realtime encode failed with {status}"
        )))
    }
}

/// Fluent builder for file-based realtime exports.
#[derive(Debug, Clone)]
pub struct RealtimeExport {
    input: PathBuf,
    graph: FilterGraph,
    options: WriteVideoOptions,
}

impl RealtimeExport {
    /// Start from an input media path; encode options default to realtime GPU/CPU.
    ///
    /// # Errors
    ///
    /// HW probe / tools discovery.
    pub fn new(input: impl Into<PathBuf>, output: impl Into<String>, fps: f64) -> Result<Self> {
        let options = realtime_write_options(output, fps)?;
        Ok(Self {
            input: input.into(),
            graph: FilterGraph::new(),
            options,
        })
    }

    /// Replace encode options (e.g. force NVENC or CRF).
    #[must_use]
    pub fn with_options(mut self, options: WriteVideoOptions) -> Self {
        self.options = options;
        self
    }

    /// Force NVIDIA NVENC realtime preset (replaces prior encode args).
    #[must_use]
    pub fn with_nvenc(mut self, cq: u8) -> Self {
        let path = self.options.path.clone();
        let fps = self.options.fps;
        let dur = self.options.duration;
        self.options = WriteVideoOptions::new(path, fps).with_nvenc_realtime(cq);
        if let Some(d) = dur {
            self.options = self.options.with_duration(d);
        }
        self
    }

    /// Force software ultrafast (replaces prior encode args).
    #[must_use]
    pub fn with_cpu_ultrafast(mut self) -> Self {
        let path = self.options.path.clone();
        let fps = self.options.fps;
        let dur = self.options.duration;
        self.options = WriteVideoOptions::new(path, fps).with_ultrafast_encode();
        if let Some(d) = dur {
            self.options = self.options.with_duration(d);
        }
        self
    }

    /// Append filter ops.
    #[must_use]
    pub fn then(mut self, op: crate::FilterOp) -> Self {
        self.graph = self.graph.then(op);
        self
    }

    /// Replace the whole filter graph.
    #[must_use]
    pub fn with_graph(mut self, graph: FilterGraph) -> Self {
        self.graph = graph;
        self
    }

    /// Limit output duration.
    #[must_use]
    pub fn with_duration(mut self, duration: reelforge_core::Duration) -> Self {
        self.options = self.options.with_duration(duration);
        self
    }

    /// Run export.
    ///
    /// # Errors
    ///
    /// See [`run_filtergraph_encode`].
    pub fn run(&self) -> Result<()> {
        if self.graph.is_empty() {
            return Err(IoError::message(
                "RealtimeExport has empty filter graph; add ops with then()",
            ));
        }
        run_filtergraph_encode(&self.input, &self.options.path, &self.graph, &self.options)
    }

    /// Compiled `-vf` for debugging.
    ///
    /// # Errors
    ///
    /// Empty graph.
    pub fn vf(&self) -> Result<String> {
        self.graph.to_vf().map_err(IoError::message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FilterOp;

    #[test]
    fn hw_probe_does_not_panic() {
        // May fail if ffmpeg missing in unit env; just ensure no panic.
        let _ = detect_hw_encoders();
        let _ = nvenc_available();
    }

    #[test]
    fn builder_builds_vf() {
        let exp = RealtimeExport {
            input: PathBuf::from("in.mp4"),
            graph: FilterGraph::new()
                .then(FilterOp::Crop {
                    w: 100,
                    h: 100,
                    x: 0,
                    y: 0,
                })
                .then(FilterOp::Scale { w: 50, h: 50 })
                .then(FilterOp::BlackAndWhite)
                .then(FilterOp::FadeIn { duration: 0.25 }),
            options: WriteVideoOptions::new("out.mp4", 30.0).with_ultrafast_encode(),
        };
        let vf = exp.vf().unwrap();
        assert!(vf.contains("crop="));
        assert!(vf.contains("scale=50:50"));
        assert!(vf.contains("hue=s=0") || vf.contains("format=gray"));
        assert!(vf.contains("fade="));
    }
}
