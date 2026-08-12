//! Execute fully `FFmpeg`-extractable [`RenderPlan`]s.

use super::extract::{extract_ffmpeg, require_full_ffmpeg};
use super::ops::PlanSource;
use super::plan::RenderPlan;
use crate::error::{IoError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::{FilterGraph, run_filtergraph};
use std::path::Path;
use std::process::Command;

/// Run a plan end-to-end when it is fully `FFmpeg`-extractable.
///
/// Optimizes, extracts the filtergraph, and encodes with host `ffmpeg`.
/// Hybrid plans (Rust remainder) return an error in this MVP — execute the
/// `FFmpeg` prefix separately or expand the runner later.
///
/// # Errors
///
/// Missing output, non-file source, non-extractable ops, or process failure.
pub fn run_render_plan(plan: &RenderPlan) -> Result<()> {
    let output = plan
        .output
        .as_ref()
        .ok_or_else(|| IoError::message("render plan has no output.path"))?;
    let input = match &plan.source {
        PlanSource::File { path } => path.as_str(),
    };
    if plan.ops.is_empty() {
        return Err(IoError::message(
            "render plan has no ops; use cut/copy tooling for passthrough",
        ));
    }

    let graph = require_full_ffmpeg(plan)?;
    // Prefer codec/crf from plan when set.
    if output.video_codec.is_none() && output.crf.is_none() {
        return run_filtergraph(input, &output.path, &graph);
    }
    run_filtergraph_with_encode(input, &output.path, &graph, output.video_codec.as_deref(), output.crf)
}

/// Explain extraction without running ffmpeg (CLI / agents).
#[must_use]
pub fn explain_plan(plan: &RenderPlan) -> String {
    let extracted = extract_ffmpeg(plan);
    let mut lines = Vec::new();
    lines.push(format!(
        "source: {}",
        plan.source.path().unwrap_or("<unknown>")
    ));
    lines.push(format!(
        "ops: {} → optimized {}, ffmpeg_prefix {}, remainder {}",
        plan.ops.len(),
        extracted.optimized.ops.len(),
        extracted.ffmpeg_op_count,
        extracted.remainder_op_count
    ));
    lines.push(format!("fully_ffmpeg: {}", extracted.fully_ffmpeg));
    if let Some(vf) = &extracted.ffmpeg_vf {
        lines.push(format!("-vf: {vf}"));
    }
    if !extracted.remainder.is_empty() {
        lines.push(format!("remainder: {:?}", extracted.remainder));
    }
    if let Some(out) = &plan.output {
        lines.push(format!("output: {}", out.path));
    }
    lines.join("\n")
}

fn run_filtergraph_with_encode(
    input: impl AsRef<Path>,
    output: impl AsRef<Path>,
    graph: &FilterGraph,
    video_codec: Option<&str>,
    crf: Option<u8>,
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

    let codec = video_codec.unwrap_or("libx264");
    let mut cmd = Command::new(&tools.ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(input)
        .args(["-vf", &vf, "-an", "-c:v", codec, "-pix_fmt", "yuv420p"]);
    if let Some(crf) = crf {
        cmd.args(["-crf", &crf.to_string()]);
    }
    cmd.arg(output);

    let status = cmd
        .status()
        .map_err(|e| IoError::process(format!("ffmpeg render plan spawn failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(IoError::process(format!(
            "ffmpeg render plan failed with {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_plan::ops::{PlanOp, PlanOutput};

    #[test]
    fn explain_mentions_split() {
        let plan = RenderPlan::from_file("in.mp4")
            .then(PlanOp::HFlip)
            .then(PlanOp::Custom {
                name: "paint".into(),
                params: None,
            })
            .with_output(PlanOutput::new("out.mp4"));
        let text = explain_plan(&plan);
        assert!(text.contains("fully_ffmpeg: false"));
        assert!(text.contains("remainder"));
    }
}
