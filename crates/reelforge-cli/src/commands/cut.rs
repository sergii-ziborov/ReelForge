//! `reelforge cut` — filtergraph trim.

use reelforge::{FilterGraph, FilterOp, run_filtergraph};

/// Trim `[start, start+duration)` via `FFmpeg` filtergraph.
///
/// # Errors
///
/// Returns a string error on I/O failure.
pub fn run(input: &str, output: &str, start: f64, duration: f64) -> Result<(), String> {
    if !(start.is_finite() && start >= 0.0) {
        return Err("start must be finite and >= 0".into());
    }
    if !(duration.is_finite() && duration > 0.0) {
        return Err("duration must be finite and > 0".into());
    }
    let graph = FilterGraph::new()
        .then(FilterOp::Trim { start, duration })
        .then(FilterOp::EvenDims);
    run_filtergraph(input, output, &graph).map_err(|e| e.to_string())?;
    println!("wrote {output}");
    Ok(())
}
