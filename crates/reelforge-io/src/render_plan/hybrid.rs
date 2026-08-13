//! Hybrid execution: `FFmpeg` prefix → temp file → Rust remainder → encode.

use super::apply::{apply_plan_ops, validate_remainder};
use super::extract::ExtractedPlan;
use super::ops::{PlanOutput, PlanSource};
use super::plan::RenderPlan;
use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, probe_has_audio};
use crate::options::{OpenVideoOptions, WriteVideoOptions};
use crate::video_file::open_video;
use crate::{
    FilterGraph, FiltergraphRunOptions, mux_copy_audio, run_filtergraph_with, write_video_with,
};
use reelforge_core::VideoClip;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Run a hybrid plan: optional pure-`FFmpeg` prefix, then Rust remainder, then encode.
///
/// # Errors
///
/// Missing paths, unknown custom ops, decode/encode, or process failures.
pub fn run_hybrid_plan(
    plan: &RenderPlan,
    extracted: &ExtractedPlan,
    control: &WriteControl,
) -> Result<()> {
    let output = plan
        .output
        .as_ref()
        .ok_or_else(|| IoError::message("render plan has no output.path"))?;
    let input = match &plan.source {
        PlanSource::File { path } => path.as_str(),
    };
    if !Path::new(input).is_file() {
        return Err(IoError::message(format!("input not found: {input}")));
    }
    validate_remainder(&extracted.remainder)?;

    let mut temps: Vec<PathBuf> = Vec::new();
    let result = (|| {
        let mut source = PathBuf::from(input);

        if extracted.has_ffmpeg_segment() {
            control.check_cancel()?;
            let mid = temp_plan_path(Path::new(&output.path), "rf-hyb-pfx");
            run_prefix_to_file(input, &mid, &extracted.filter_graph(), output)?;
            temps.push(mid.clone());
            source = mid;
            control.report(WriteProgress::new(WriteStage::Video, 0, 1));
        }

        control.check_cancel()?;
        let opened = open_video(&OpenVideoOptions::new(source.to_string_lossy()).video_only())?;
        let base: Arc<dyn VideoClip> = Arc::new(opened);
        let clip = apply_plan_ops(base, &extracted.remainder)?;

        let fps = resolve_fps(output, clip.as_ref())?;
        let mut opts = WriteVideoOptions::new(&output.path, fps);
        if let Some(codec) = &output.video_codec {
            opts = opts.with_video_codec(codec.clone());
        }
        if let Some(crf) = output.crf {
            opts = opts.with_crf(crf);
        } else if output.video_codec.is_none() {
            opts = opts.with_crf(23);
        }

        write_video_with(clip.as_ref(), &opts, control)?;
        remux_source_audio(&source, Path::new(&output.path))?;
        control.report(WriteProgress::new(WriteStage::Done, 1, 1));
        Ok(())
    })();

    for t in temps {
        let _ = std::fs::remove_file(t);
    }
    result
}

fn resolve_fps(output: &PlanOutput, clip: &dyn VideoClip) -> Result<f64> {
    if let Some(fps) = output.fps {
        if fps.is_finite() && fps > 0.0 {
            return Ok(fps);
        }
        return Err(IoError::message(format!("invalid plan output.fps {fps}")));
    }
    if let Some(fps) = clip.fps()
        && fps.is_finite()
        && fps > 0.0
    {
        return Ok(fps);
    }
    Ok(24.0)
}

fn run_prefix_to_file(
    input: &str,
    mid: &Path,
    graph: &FilterGraph,
    output: &PlanOutput,
) -> Result<()> {
    if let Some(parent) = mid.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| IoError::message(format!("create hybrid temp dir: {e}")))?;
    }

    let mut opts = FiltergraphRunOptions::new();
    if let Some(codec) = &output.video_codec {
        opts = opts.with_video_codec(codec.clone());
    }
    if let Some(crf) = output.crf.or(Some(23)) {
        opts = opts.with_crf(crf);
    }
    run_filtergraph_with(input, mid, graph, &opts)
}

fn remux_source_audio(audio_src: &Path, output: &Path) -> Result<()> {
    let tools = FfmpegTools::discover()?;
    if !probe_has_audio(&tools, audio_src).unwrap_or(false) {
        return Ok(());
    }
    let tagged = temp_plan_path(output, "rf-hyb-vid");
    std::fs::rename(output, &tagged)
        .map_err(|e| IoError::message(format!("hybrid rename for audio mux failed: {e}")))?;
    let muxed = mux_copy_audio(&tagged, audio_src, output);
    if muxed.is_err() {
        let _ = std::fs::rename(&tagged, output);
        return muxed;
    }
    let _ = std::fs::remove_file(&tagged);
    Ok(())
}

fn temp_plan_path(output: &Path, tag: &str) -> PathBuf {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("reelforge");
    parent.join(format!(".{stem}.{tag}.{}.mp4", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_plan::ops::PlanOp;
    use crate::render_plan::{RenderPlan, extract_ffmpeg};

    #[test]
    fn validate_blocks_unknown_before_run() {
        let plan = RenderPlan::from_file("missing.mp4")
            .then(PlanOp::HFlip)
            .then(PlanOp::Custom {
                name: "not_a_real_fx".into(),
                params: None,
            })
            .with_output(PlanOutput::new("out.mp4"));
        let extracted = extract_ffmpeg(&plan);
        let err = run_hybrid_plan(&plan, &extracted, &WriteControl::default());
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("unknown custom") || msg.contains("not found"));
    }
}
