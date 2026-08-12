//! Execute [`RenderPlan`]s: pure `FFmpeg` or hybrid (prefix + Rust remainder).

use super::extract::{extract_ffmpeg, require_full_ffmpeg};
use super::hybrid::run_hybrid_plan;
use super::ops::PlanSource;
use super::plan::RenderPlan;
use crate::control::WriteControl;
use crate::error::{IoError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::{FilterGraph, run_filtergraph};
use std::path::Path;
use std::process::Command;

/// Run a plan end-to-end.
///
/// * Fully `FFmpeg`-extractable → single filtergraph encode (no Rust pixels).
/// * Hybrid → `FFmpeg` prefix to a temp file, then Rust remainder (`reelforge-fx`
///   + known `custom` names), then [`crate::write_video`].
///
/// # Errors
///
/// Missing output, non-file source, unknown custom ops, or process failure.
pub fn run_render_plan(plan: &RenderPlan) -> Result<()> {
    run_render_plan_with(plan, &WriteControl::default())
}

/// Like [`run_render_plan`] with progress / cancel for the Rust encode stage.
///
/// # Errors
///
/// Same as [`run_render_plan`], plus [`IoError::Cancelled`].
pub fn run_render_plan_with(plan: &RenderPlan, control: &WriteControl) -> Result<()> {
    let _output = plan
        .output
        .as_ref()
        .ok_or_else(|| IoError::message("render plan has no output.path"))?;
    let _input = match &plan.source {
        PlanSource::File { path } => path.as_str(),
    };
    if plan.ops.is_empty() {
        return Err(IoError::message(
            "render plan has no ops; use cut/copy tooling for passthrough",
        ));
    }

    control.check_cancel()?;
    let extracted = extract_ffmpeg(plan);

    if extracted.fully_ffmpeg {
        return run_pure_ffmpeg(plan, control);
    }

    run_hybrid_plan(plan, &extracted, control)
}

fn run_pure_ffmpeg(plan: &RenderPlan, control: &WriteControl) -> Result<()> {
    let output = plan.output.as_ref().expect("checked");
    let input = match &plan.source {
        PlanSource::File { path } => path.as_str(),
    };
    control.check_cancel()?;
    let graph = require_full_ffmpeg(plan)?;
    if output.video_codec.is_none() && output.crf.is_none() {
        run_filtergraph(input, &output.path, &graph)?;
    } else {
        run_filtergraph_with_encode(
            input,
            &output.path,
            &graph,
            output.video_codec.as_deref(),
            output.crf,
        )?;
    }
    control.report(crate::control::WriteProgress::new(
        crate::control::WriteStage::Done,
        1,
        1,
    ));
    Ok(())
}

/// Explain extraction / hybrid routing without running ffmpeg.
#[must_use]
pub fn explain_plan(plan: &RenderPlan) -> String {
    let extracted = extract_ffmpeg(plan);
    let mode = if extracted.fully_ffmpeg {
        "ffmpeg"
    } else if extracted.has_ffmpeg_segment() {
        "hybrid"
    } else {
        "rust"
    };
    let mut lines = Vec::new();
    lines.push(format!(
        "source: {}",
        plan.source.path().unwrap_or("<unknown>")
    ));
    lines.push(format!("mode: {mode}"));
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
    fn explain_hybrid_mode() {
        let plan = RenderPlan::from_file("in.mp4")
            .then(PlanOp::HFlip)
            .then(PlanOp::Custom {
                name: "black_and_white".into(),
                params: None,
            })
            .with_output(PlanOutput::new("out.mp4"));
        let text = explain_plan(&plan);
        assert!(text.contains("mode: hybrid"));
        assert!(text.contains("fully_ffmpeg: false"));
    }

    #[test]
    fn explain_rust_only_mode() {
        let plan = RenderPlan::from_file("in.mp4")
            .then(PlanOp::Custom {
                name: "invert".into(),
                params: None,
            })
            .with_output(PlanOutput::new("out.mp4"));
        let text = explain_plan(&plan);
        assert!(text.contains("mode: rust"));
    }

    #[test]
    fn explain_ffmpeg_mode() {
        let plan = RenderPlan::from_file("in.mp4")
            .then(PlanOp::HFlip)
            .then(PlanOp::Scale { w: 320, h: 180 })
            .with_output(PlanOutput::new("out.mp4"));
        let text = explain_plan(&plan);
        assert!(text.contains("mode: ffmpeg"));
        assert!(text.contains("fully_ffmpeg: true"));
    }
}
