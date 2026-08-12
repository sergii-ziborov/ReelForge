//! Typed deterministic [`RenderPlan`] with JSON I/O and `FFmpeg` extraction.
//!
//! # Flow
//!
//! ```text
//! JSON / builder  →  optimize (DCE, fuse)  →  extract FFmpeg prefix
//!                         │
//!            fully FFmpeg ─┼─► single -vf encode
//!                         │
//!              hybrid/rust ─┴─► FFmpeg prefix (optional) → Rust remainder → write
//! ```

mod apply;
mod execute;
mod extract;
mod hybrid;
mod ops;
mod optimize;
mod plan;

pub use apply::{apply_plan_ops, is_known_custom, validate_remainder};
pub use execute::{explain_plan, run_render_plan, run_render_plan_with};
pub use extract::{ExtractedPlan, extract_ffmpeg, extract_from_optimized, require_full_ffmpeg};
pub use ops::{PlanBackend, PlanOp, PlanOutput, PlanSource, RENDER_PLAN_VERSION};
pub use optimize::{OptimizeStats, OptimizedPlan, optimize, optimize_plan};
pub use plan::RenderPlan;
