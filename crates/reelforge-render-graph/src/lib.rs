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
mod graph;
mod mask;
mod op;
mod redaction;
mod stage;

pub use animated::{Animated, Easing, Keyframe};
pub use error::{GraphError, Result};
pub use graph::{
    GraphOutput, MediaAsset, MediaAssetId, NodeId, RenderGraph, RenderNode, RenderNodeKind,
};
pub use mask::{MaskInterpolation, MaskSample, MaskTimeline, MissingMaskPolicy};
pub use op::{
    BackendClass, CapabilitySet, MediaContract, OperationDescriptor, OperationId, OperationLimits,
    OperationRegistry, SemVer,
};
pub use redaction::{RedactionStyle, RegionRedaction};
pub use stage::{ExecutionPlan, ExecutionStage, FfmpegStage, GpuStage, RustStage};

/// Schema version for serialized `RenderGraph` documents.
pub const RENDER_GRAPH_VERSION: u32 = 1;
