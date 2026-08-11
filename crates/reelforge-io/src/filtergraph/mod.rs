//! `FFmpeg` filtergraph fast path (no per-frame pixel import).

mod plan;
mod run;

pub use plan::{FilterGraph, FilterOp};
pub use run::run_filtergraph;
