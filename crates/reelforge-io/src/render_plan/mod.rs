//! Typed deterministic [`RenderPlan`] with JSON I/O and `FFmpeg` extraction.
//!
//! # Flow
//!
//! ```text
//! JSON / builder  →  optimize (DCE, fuse)  →  extract FFmpeg prefix
//!                                          ↘ remainder (Rust / custom)
//! ```
//!
//! Fully extractable plans can be executed via host `ffmpeg` without importing
//! frames into Rust.

mod execute;
mod extract;
mod ops;
mod optimize;
mod plan;

pub use execute::{explain_plan, run_render_plan};
pub use extract::{ExtractedPlan, extract_ffmpeg, extract_from_optimized, require_full_ffmpeg};
pub use ops::{
    PlanBackend, PlanOp, PlanOutput, PlanSource, RENDER_PLAN_VERSION,
};
pub use optimize::{OptimizeStats, OptimizedPlan, optimize, optimize_plan};
pub use plan::RenderPlan;
