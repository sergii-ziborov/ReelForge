//! Deterministic render contracts for `ReelForge` (Milestone 0+).
//!
//! This crate owns **executable** graph types — not Capture projects and not
//! `SightLoom` `VisionIndex`. Layering:
//!
//! ```text
//! SemanticEditPlan / CaptureProject  → compile →  RenderGraph  → schedule → ExecutionPlan
//! ```
//!
//! `RenderPlan` v1 (linear one-shot) remains in `reelforge-io` for simple jobs.

mod animated;
mod error;
mod fingerprint;
mod graph;
mod mask;
mod op;
mod redaction;
mod schedule;
mod stage;

pub use animated::{Animated, Easing, Keyframe};
pub use error::{GraphError, Result};
pub use fingerprint::{
    fingerprint_execution_plan, fingerprint_graph_run, fingerprint_render_graph, fingerprint_stage,
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
