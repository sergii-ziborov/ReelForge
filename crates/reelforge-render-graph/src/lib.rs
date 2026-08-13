//! Deterministic render contracts for `ReelForge` (Milestone 0+).
//!
//! This crate owns **executable** graph types — not Capture projects and not
//! `SightLoom` `VisionIndex`. Layering:
//!
//! ```text
//! SemanticEditPlan / CaptureProject
//!         → RenderGraph.validate
//!         → compile_graph → CompiledGraph   (indexes, typed ops, contracts)
//!         → schedule_compiled → ExecutionPlan
//!         → artifact_manifest → ArtifactManifest
//! ```
//!
//! Typed compile (`CompiledOp` / `TypedParams`) is the source of truth for
//! registry↔executor parity. Media execution lives in `reelforge-io`.
//!
//! `RenderPlan` v1 (linear one-shot) remains in `reelforge-io` for simple jobs.

mod animated;
mod artifact;
mod compile;
mod compiled;
mod contract;
mod error;
mod fingerprint;
mod geometry;
mod graph;
mod ids;
mod mask;
mod op;
mod redaction;
mod schedule;
mod stage;
mod track;

pub use animated::{Animated, Easing, Keyframe};
pub use artifact::{
    ARTIFACT_MANIFEST_VERSION, ArtifactKind, ArtifactManifest, ArtifactRef, StageArtifacts,
    artifact_manifest,
};
pub use compile::{
    CompileDiagnostics, CompiledOp, CostEstimate, TypedParams, check_registry_executor_parity,
    compile_graph_ops, compile_op, ensure_executable, is_executable_op_id,
};
pub use compiled::{
    AssetIndex, CompiledGraph, CompiledNode, CompiledNodeKind, CompiledOutput, NodeIndex,
    compile_graph,
};
pub use contract::infer_node_contract;
pub use error::{GraphError, GraphErrorCode, Result};
pub use fingerprint::{
    StageCacheKey, fingerprint_compiled_graph, fingerprint_execution_plan, fingerprint_graph_run,
    fingerprint_render_graph, fingerprint_stage, fingerprint_stage_key,
};
pub use geometry::{Geometry, MaskRef};
pub use graph::{
    GraphOutput, MediaAsset, MediaAssetId, NodeId, RenderGraph, RenderNode, RenderNodeKind,
};
pub use ids::{AppearanceId, ObservationId, SubjectId, TrackId};
pub use mask::{
    MaskInterpolation, MaskLifecycle, MaskProvenance, MaskRegionAt, MaskSample, MaskTimeline,
    MissingMaskPolicy,
};
pub use op::{
    BackendClass, CapabilitySet, ExecutorKind, MediaContract, OperationDescriptor, OperationId,
    OperationLimits, OperationRegistry, SemVer,
};
pub use redaction::{RedactionStyle, RegionRedaction};
pub use schedule::{schedule_compiled, schedule_graph};
pub use stage::{
    AdapterStage, ExecutionPlan, ExecutionStage, FfmpegStage, GpuStage, RustStage, StageIo,
    StagePort,
};
pub use track::{OcclusionState, TrackSample, TrackTimeline, mask_timeline_from_tracks};

/// Schema version for serialized `RenderGraph` documents.
pub const RENDER_GRAPH_VERSION: u32 = 1;
