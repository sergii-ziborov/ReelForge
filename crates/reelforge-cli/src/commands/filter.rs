//! `reelforge filter` — simple vf chain.

use reelforge::{FilterGraph, FilterOp, run_filtergraph};

/// Apply hflip/vflip/scale via `FFmpeg` filtergraph.
///
/// # Errors
///
/// Returns a string error on invalid args or I/O failure.
pub fn run(
    input: &str,
    output: &str,
    hflip: bool,
    vflip: bool,
    width: Option<u32>,
    height: Option<u32>,
) -> Result<(), String> {
    let mut graph = FilterGraph::new();
    if hflip {
        graph = graph.then(FilterOp::HFlip);
    }
    if vflip {
        graph = graph.then(FilterOp::VFlip);
    }
    match (width, height) {
        (Some(w), Some(h)) => {
            graph = graph.then(FilterOp::Scale { w, h });
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err("both --width and --height are required for scale".into());
        }
        (None, None) => {}
    }
    if graph.is_empty() {
        return Err("specify at least one of --hflip, --vflip, or scale size".into());
    }
    graph = graph.then(FilterOp::EvenDims);
    run_filtergraph(input, output, &graph).map_err(|e| e.to_string())?;
    println!("wrote {output}");
    Ok(())
}
