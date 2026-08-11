//! Execute a [`FilterGraph`] via the host `ffmpeg` binary.

use crate::error::{IoError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::filtergraph::plan::FilterGraph;
use std::path::Path;
use std::process::Command;

/// Run `input` through `graph` and write `output` (re-encode H.264 by default).
///
/// # Errors
///
/// Returns tool or process errors.
pub fn run_filtergraph(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    graph: &FilterGraph,
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
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| IoError::message(format!("create output dir: {e}")))?;
    }

    let status = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vf", &vf, "-an", "-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(output)
        .status()
        .map_err(|e| IoError::process(format!("ffmpeg filtergraph spawn failed: {e}")))?;

    if status.success() {
        Ok(())
    } else {
        Err(IoError::process(format!(
            "ffmpeg filtergraph failed with {status}"
        )))
    }
}
