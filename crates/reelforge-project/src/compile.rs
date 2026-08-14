//! Compile [`CaptureProject`] → [`RenderGraph`] (executable DAG).

use crate::emit::CompileCtx;
use crate::error::{ProjectError, Result};
use crate::project::CaptureProject;
use reelforge_render_graph::{
    GraphOutput, NodeId, RENDER_GRAPH_VERSION, RenderGraph, RenderNode, RenderNodeKind,
};

/// Compile result: graph + editorial warnings (markers / skipped wipes).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectCompile {
    /// Executable graph.
    pub graph: RenderGraph,
    /// Non-fatal notes.
    pub warnings: Vec<String>,
}

/// Compile the active sequence.
///
/// Video clips: `Source` → trim (ticks) → optional speed / fade → compose.
/// Audio tracks: same chain, then `rf.audio.mix` onto the picture.
/// Project `semantic` refs compile to `rf.adapter.sightloom` + empty redaction
/// (subject / event / query / policy handles — not bboxes).
/// Markers stay editorial. Wipe is stored, not compiled. Muted tracks skip.
///
/// # Errors
///
/// Missing media, empty picture, nested cycles, bad speed, or graph validate.
pub fn compile_project(project: &CaptureProject) -> Result<ProjectCompile> {
    let seq = project.active()?;
    let mut warnings = Vec::new();
    if !project.markers.is_empty() || !seq.markers.is_empty() {
        warnings.push("markers are editorial and are not compiled into the graph".into());
    }
    let mut ctx = CompileCtx::new(project, warnings);
    ctx.emit_sequence(seq)?;
    if ctx.layers.is_empty() {
        return Err(ProjectError::message(
            "active sequence has no video clips to compile",
        ));
    }

    let mut picture = if ctx.layers.len() == 1 && ctx.layers[0].start < 1e-9 && seq.canvas.is_none()
    {
        ctx.layers[0].node.clone()
    } else {
        ctx.emit_compose(seq.canvas)
    };
    if !ctx.audio.is_empty() {
        picture = ctx.emit_audio_mix(picture);
    }
    if !project.semantic.is_empty() {
        picture = ctx.emit_semantic_privacy(&project.semantic, picture);
    }

    let out_id = NodeId("n_out".into());
    ctx.nodes.push(RenderNode {
        id: out_id.clone(),
        body: RenderNodeKind::Output {
            name: "main".into(),
        },
        inputs: vec![picture],
    });

    let graph = RenderGraph {
        version: RENDER_GRAPH_VERSION,
        assets: ctx.assets.into_values().collect(),
        nodes: ctx.nodes,
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: out_id,
            uri: None,
        }],
    };
    graph
        .validate()
        .map_err(|e| ProjectError::Graph(e.to_string()))?;
    Ok(ProjectCompile {
        graph,
        warnings: ctx.warnings,
    })
}
