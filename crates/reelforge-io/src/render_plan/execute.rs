//! Execute [`RenderPlan`]s: pure `FFmpeg` or hybrid (prefix + Rust remainder).

use super::extract::{extract_ffmpeg, require_full_ffmpeg};
use super::hybrid::run_hybrid_plan;
use super::ops::PlanSource;
use super::plan::RenderPlan;
use crate::control::WriteControl;
use crate::error::{IoError, Result};
use crate::{FiltergraphRunOptions, run_filtergraph_with};

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
    let mut opts = FiltergraphRunOptions::new();
    if let Some(codec) = &output.video_codec {
        opts = opts.with_video_codec(codec.clone());
    }
    if let Some(crf) = output.crf {
        opts = opts.with_crf(crf);
    }
    run_filtergraph_with(input, &output.path, &graph, &opts)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_plan::ops::{PlanOp, PlanOutput};

    #[test]
    fn explain_hybrid_mode() {
        let plan = RenderPlan::from_file("in.mp4")
            .then(PlanOp::HFlip)
            .then(PlanOp::Custom {
                name: "head_blur".into(),
                params: Some(serde_json::json!({"radius": 12})),
            })
            .with_output(PlanOutput::new("out.mp4"));
        let text = explain_plan(&plan);
        assert!(text.contains("mode: hybrid"));
        assert!(text.contains("fully_ffmpeg: false"));
    }

    #[test]
    fn explain_bw_is_pure_ffmpeg() {
        let plan = RenderPlan::from_file("in.mp4")
            .then(PlanOp::HFlip)
            .then(PlanOp::Custom {
                name: "black_and_white".into(),
                params: None,
            })
            .with_output(PlanOutput::new("out.mp4"));
        let text = explain_plan(&plan);
        assert!(text.contains("mode: ffmpeg"));
        assert!(text.contains("fully_ffmpeg: true"));
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
