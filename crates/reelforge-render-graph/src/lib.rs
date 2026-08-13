//! Deterministic render contracts for `ReelForge` (Milestone 0+).
//!
//! This crate owns **executable** graph types — not Capture projects and not
//! `SightLoom` `VisionIndex`. Layering:
//!
//! ```text
//! SemanticEditPlan / CaptureProject  → RenderGraph  → compile_op → schedule → ExecutionPlan
//! ```
//!
//! Typed compile (`CompiledOp` / `TypedParams`) is the source of truth for
//! registry↔executor parity. Media execution lives in `reelforge-io`.
//!
//! `RenderPlan` v1 (linear one-shot) remains in `reelforge-io` for simple jobs.

mod animated;
mod compile;
mod error;
mod fingerprint;
mod graph;
mod mask;
mod op;
mod redaction;
mod schedule;
mod stage;

pub use animated::{Animated, Easing, Keyframe};
pub use compile::{
    CompileDiagnostics, CompiledOp, CostEstimate, TypedParams, check_registry_executor_parity,
    compile_graph_ops, compile_op, ensure_executable, is_executable_op_id,
};
pub use error::{GraphError, GraphErrorCode, Result};
pub use fingerprint::{
    StageCacheKey, fingerprint_execution_plan, fingerprint_graph_run, fingerprint_render_graph,
    fingerprint_stage, fingerprint_stage_key,
};
pub use graph::{
    GraphOutput, MediaAsset, MediaAssetId, NodeId, RenderGraph, RenderNode, RenderNodeKind,
};
pub use mask::{MaskInterpolation, MaskSample, MaskTimeline, MissingMaskPolicy};
pub use op::{
    BackendClass, CapabilitySet, MediaContract, OperationDescriptor, OperationId, OperationLimits,
    OperationRegistry, SemVer,
};
pub use redaction::{RedactionStyle, RegionRedaction};
pub use schedule::schedule_graph;
pub use stage::{AdapterStage, ExecutionPlan, ExecutionStage, FfmpegStage, GpuStage, RustStage};

/// Schema version for serialized `RenderGraph` documents.
pub const RENDER_GRAPH_VERSION: u32 = 1;
