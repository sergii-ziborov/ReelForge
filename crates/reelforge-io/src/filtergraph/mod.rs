//! `FFmpeg` filtergraph fast path (no per-frame pixel import).

mod plan;
mod run;

pub use plan::{FilterGraph, FilterOp};
pub use run::{
    AudioCopyMode, FiltergraphRunOptions, mux_copy_audio, run_filtergraph, run_filtergraph_with,
};
