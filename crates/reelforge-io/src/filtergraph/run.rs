//! Execute a [`FilterGraph`] via the host `ffmpeg` binary.

use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, probe_has_audio};
use crate::filtergraph::plan::FilterGraph;
use std::path::Path;
use std::process::Command;

/// How a filtergraph run treats the source audio track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioCopyMode {
    /// Keep source audio (`-c:a copy`, or `aac` when `atrim` is required).
    #[default]
    Preserve,
    /// Drop audio (`-an`).
    Drop,
}

/// Encode / audio options for [`run_filtergraph_with`].
#[derive(Debug, Clone, Default)]
pub struct FiltergraphRunOptions {
    /// Video codec (default `libx264`).
    pub video_codec: Option<String>,
    /// Optional CRF.
    pub crf: Option<u8>,
    /// Pixel format (default `yuv420p`).
    pub pixel_format: Option<String>,
    /// Extra args after codec / `pix_fmt` / audio.
    pub extra_args: Vec<String>,
    /// Output `-t` seconds (caps both video and audio).
    pub duration_secs: Option<f64>,
    /// Audio policy (default [`AudioCopyMode::Preserve`]).
    pub audio: AudioCopyMode,
    /// Audio codec when re-encoding (`atrim`, or copy fallback). Default `aac`.
    pub audio_codec: Option<String>,
}

impl FiltergraphRunOptions {
    /// Defaults: libx264 + yuv420p + preserve audio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop source audio.
    #[must_use]
    pub fn drop_audio(mut self) -> Self {
        self.audio = AudioCopyMode::Drop;
        self
    }

    /// Override video codec.
    #[must_use]
    pub fn with_video_codec(mut self, codec: impl Into<String>) -> Self {
        self.video_codec = Some(codec.into());
        self
    }

    /// Set CRF.
    #[must_use]
    pub fn with_crf(mut self, crf: u8) -> Self {
        self.crf = Some(crf);
        self
    }

    /// Override pixel format.
    #[must_use]
    pub fn with_pixel_format(mut self, pix: impl Into<String>) -> Self {
        self.pixel_format = Some(pix.into());
        self
    }

    /// Extra `ffmpeg` arguments.
    #[must_use]
    pub fn with_extra_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.extra_args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Cap output duration (`-t`).
    #[must_use]
    pub fn with_duration_secs(mut self, secs: f64) -> Self {
        self.duration_secs = Some(secs);
        self
    }

    /// Audio codec used when the bitstream cannot be copied.
    #[must_use]
    pub fn with_audio_codec(mut self, codec: impl Into<String>) -> Self {
        self.audio_codec = Some(codec.into());
        self
    }
}

/// Run `input` through `graph` and write `output` (re-encode H.264, keep audio).
///
/// # Errors
///
/// Returns tool or process errors.
pub fn run_filtergraph(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    graph: &FilterGraph,
) -> Result<()> {
    run_filtergraph_with(input, output, graph, &FiltergraphRunOptions::default())
}

/// Like [`run_filtergraph`] with codec / audio options.
///
/// # Errors
///
/// Returns tool or process errors.
pub fn run_filtergraph_with(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    graph: &FilterGraph,
    options: &FiltergraphRunOptions,
) -> Result<()> {
    let tools = FfmpegTools::discover()?;
    let vf = graph.to_vf().map_err(IoError::message)?;
    let input = input.as_ref();
    let output = output.as_ref();
    if !input.is_file() {
        return Err(IoError::message(format!(
            "input not found: {}",
            input.display()
        )));
    }
    ensure_parent(output)?;

    let has_audio = probe_has_audio(&tools, input).unwrap_or(false);
    spawn_filtergraph(&tools, input, output, &vf, graph, options, has_audio)
}

/// Remux `audio_src`'s first audio stream onto `video` (`-c:v copy`, audio copy).
///
/// # Errors
///
/// Process failures. No-op (Ok) when `audio_src` has no audio.
pub fn mux_copy_audio(
    video: impl AsRef<Path>,
    audio_src: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<()> {
    let tools = FfmpegTools::discover()?;
    let video = video.as_ref();
    let audio_src = audio_src.as_ref();
    let output = output.as_ref();
    if !probe_has_audio(&tools, audio_src).unwrap_or(false) {
        return Ok(());
    }
    ensure_parent(output)?;
    if spawn_mux(&tools, video, audio_src, output, true).is_ok() {
        return Ok(());
    }
    spawn_mux(&tools, video, audio_src, output, false)
}

fn spawn_filtergraph(
    tools: &FfmpegTools,
    input: &Path,
    output: &Path,
    vf: &str,
    graph: &FilterGraph,
    options: &FiltergraphRunOptions,
    has_audio: bool,
) -> Result<()> {
    let codec = options.video_codec.as_deref().unwrap_or("libx264");
    let pix = options.pixel_format.as_deref().unwrap_or("yuv420p");
    let audio_codec = options.audio_codec.as_deref().unwrap_or("aac");
    let preserve = options.audio == AudioCopyMode::Preserve && has_audio;
    let af = if preserve { graph.to_af() } else { None };
    let copy_audio = preserve && af.is_none();

    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vf", vf, "-c:v", codec, "-pix_fmt", pix]);
    if let Some(crf) = options.crf {
        cmd.args(["-crf", &crf.to_string()]);
    }
    if let Some(ref af) = af {
        cmd.args(["-af", af, "-c:a", audio_codec]);
    } else if copy_audio {
        cmd.args(["-c:a", "copy"]);
    } else {
        cmd.args(["-an"]);
    }
    if !options.extra_args.is_empty() {
        cmd.args(&options.extra_args);
    }
    if let Some(secs) = options.duration_secs {
        cmd.args(["-t", &format!("{secs:.6}")]);
    }
    cmd.arg(output);

    let status = cmd
        .status()
        .map_err(|e| IoError::process(format!("ffmpeg filtergraph spawn failed: {e}")))?;
    if status.success() {
        return Ok(());
    }
    if copy_audio {
        let mut recode = options.clone();
        recode.audio_codec = Some(audio_codec.to_string());
        recode.audio = AudioCopyMode::Preserve;
        return spawn_filtergraph_recode(tools, input, output, vf, &recode);
    }
    Err(IoError::process(format!(
        "ffmpeg filtergraph failed with {status}"
    )))
}

fn spawn_filtergraph_recode(
    tools: &FfmpegTools,
    input: &Path,
    output: &Path,
    vf: &str,
    options: &FiltergraphRunOptions,
) -> Result<()> {
    let codec = options.video_codec.as_deref().unwrap_or("libx264");
    let pix = options.pixel_format.as_deref().unwrap_or("yuv420p");
    let audio_codec = options.audio_codec.as_deref().unwrap_or("aac");
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args([
            "-vf",
            vf,
            "-c:v",
            codec,
            "-pix_fmt",
            pix,
            "-c:a",
            audio_codec,
        ]);
    if let Some(crf) = options.crf {
        cmd.args(["-crf", &crf.to_string()]);
    }
    if !options.extra_args.is_empty() {
        cmd.args(&options.extra_args);
    }
    if let Some(secs) = options.duration_secs {
        cmd.args(["-t", &format!("{secs:.6}")]);
    }
    cmd.arg(output);
    let status = cmd
        .status()
        .map_err(|e| IoError::process(format!("ffmpeg filtergraph recode spawn failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(IoError::process(format!(
            "ffmpeg filtergraph recode failed with {status}"
        )))
    }
}

fn spawn_mux(
    tools: &FfmpegTools,
    video: &Path,
    audio_src: &Path,
    output: &Path,
    copy_audio: bool,
) -> Result<()> {
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(video)
        .args(["-i"])
        .arg(audio_src)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:v",
            "copy",
            "-shortest",
        ]);
    if copy_audio {
        cmd.args(["-c:a", "copy"]);
    } else {
        cmd.args(["-c:a", "aac"]);
    }
    cmd.arg(output);
    let status = cmd
        .status()
        .map_err(|e| IoError::process(format!("ffmpeg audio mux spawn failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(IoError::process(format!(
            "ffmpeg audio mux failed with {status}"
        )))
    }
}

fn ensure_parent(output: &Path) -> Result<()> {
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| IoError::message(format!("create output dir: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preserves_audio() {
        assert_eq!(
            FiltergraphRunOptions::default().audio,
            AudioCopyMode::Preserve
        );
        assert_eq!(
            FiltergraphRunOptions::new().drop_audio().audio,
            AudioCopyMode::Drop
        );
    }
}
