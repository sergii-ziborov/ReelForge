//! `reelforge project` — compile a `CaptureProject` JSON file.

use reelforge::{CaptureProject, compile_project, explain_render_graph};

/// Compile `path`.
///
/// The default printout is the render schedule. `graph_json` prints the
/// compiled `RenderGraph` instead, so it can be passed to `reelforge graph`.
/// Warnings go to stderr and leave stdout as the schedule or the graph.
///
/// # Errors
///
/// Unreadable file, invalid project JSON, or a compile refusal.
pub fn run(path: &str, graph_json: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("read {path}: {err}"))?;
    let project = CaptureProject::from_json(&text).map_err(|err| err.to_string())?;
    let compiled = compile_project(&project).map_err(|err| err.to_string())?;
    if graph_json {
        let json = compiled
            .graph
            .to_json_pretty()
            .map_err(|err| err.to_string())?;
        println!("{json}");
    } else {
        let explained = explain_render_graph(&compiled.graph).map_err(|err| err.to_string())?;
        println!("{explained}");
    }
    for warning in &compiled.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}
