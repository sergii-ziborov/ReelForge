//! `reelforge graph` — explain / run a JSON `RenderGraph`.

use reelforge::{
    GraphRunOptions, RenderGraph, SightloomPackageHost, WriteControl, explain_render_graph,
    run_render_graph_with,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Graph subcommand mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMode {
    /// Print nodes / schedule, no encode.
    Explain,
    /// Schedule + execute + write outputs.
    Run,
}

/// Load `path`, then explain or run.
///
/// `--output` overrides the first `GraphOutput.uri`.
/// `--mask-package` opens a `SightloomPackageHost` as the adapter host.
///
/// # Errors
///
/// I/O, JSON, package, execute, or missing output file after `--run`.
pub fn run(
    path: &str,
    mode: GraphMode,
    output: Option<&str>,
    mask_package: Option<&str>,
) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let mut graph = RenderGraph::from_json(&text).map_err(|e| e.to_string())?;
    if let Some(out) = output {
        apply_output_override(&mut graph, out)?;
    }
    match mode {
        GraphMode::Explain => {
            let text = explain_render_graph(&graph).map_err(|e| e.to_string())?;
            println!("{text}");
            Ok(())
        }
        GraphMode::Run => run_graph(&graph, mask_package),
    }
}

fn run_graph(graph: &RenderGraph, mask_package: Option<&str>) -> Result<(), String> {
    let mut options = GraphRunOptions::new();
    if let Some(dir) = mask_package {
        let host = SightloomPackageHost::open(dir).map_err(|e| e.to_string())?;
        options = options.with_adapter_host(Arc::new(host));
    }
    run_render_graph_with(graph, &WriteControl::default(), &options).map_err(|e| e.to_string())?;
    let written = expected_outputs(graph);
    if written.is_empty() {
        return Err("no output path: set GraphOutput.uri or --output".into());
    }
    for path in &written {
        if !path.is_file() {
            return Err(format!("output missing: {}", path.display()));
        }
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn apply_output_override(graph: &mut RenderGraph, output: &str) -> Result<(), String> {
    let slot = graph
        .outputs
        .first_mut()
        .ok_or_else(|| "RenderGraph has no outputs".to_string())?;
    slot.uri = Some(output.to_string());
    if let Some(parent) = Path::new(output).parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    Ok(())
}

fn expected_outputs(graph: &RenderGraph) -> Vec<PathBuf> {
    graph
        .outputs
        .iter()
        .filter_map(|o| o.uri.as_deref())
        .map(PathBuf::from)
        .collect()
}
