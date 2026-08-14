//! Execution plan stages (scheduled hybrid backends).

use crate::compiled::NodeIndex;
use crate::graph::NodeId;
use crate::op::{MediaContract, OperationId};
use serde::{Deserialize, Serialize};

/// `FFmpeg` filter / encode stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FfmpegStage {
    /// Nodes covered.
    pub nodes: Vec<NodeId>,
    /// Optional compiled `-vf` fragment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vf: Option<String>,
    /// Optional encode codec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
}

/// In-process Rust stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RustStage {
    /// Nodes covered.
    pub nodes: Vec<NodeId>,
    /// Operations applied in order.
    #[serde(default)]
    pub operations: Vec<OperationId>,
}

/// Adapter stage (e.g. `SightLoom` mask materialization).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterStage {
    /// Adapter name (`sightloom`, …).
    pub adapter: String,
    /// Nodes covered.
    pub nodes: Vec<NodeId>,
}

/// GPU stage (`rf.gpu.passthrough`, `rf.encode.hw`, host compute).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuStage {
    /// Nodes covered.
    pub nodes: Vec<NodeId>,
    /// Backend hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
}

/// One scheduled execution stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ExecutionStage {
    /// `FFmpeg`.
    Ffmpeg(FfmpegStage),
    /// Rust.
    Rust(RustStage),
    /// External adapter.
    Adapter(AdapterStage),
    /// GPU.
    Gpu(GpuStage),
}

impl ExecutionStage {
    /// Node ids covered by this stage (schedule order within the stage).
    #[must_use]
    pub fn node_ids(&self) -> &[NodeId] {
        match self {
            Self::Ffmpeg(s) => &s.nodes,
            Self::Rust(s) => &s.nodes,
            Self::Adapter(s) => &s.nodes,
            Self::Gpu(s) => &s.nodes,
        }
    }

    /// Stable backend tag for cache keys / logs.
    #[must_use]
    pub fn backend_tag(&self) -> &'static str {
        match self {
            Self::Ffmpeg(_) => "ffmpeg",
            Self::Rust(_) => "rust",
            Self::Adapter(_) => "adapter",
            Self::Gpu(_) => "gpu",
        }
    }
}

/// One media value crossing a stage boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagePort {
    /// Compiled node that produces this value.
    pub node: NodeIndex,
    /// Inferred streams on that node.
    pub contract: MediaContract,
}

/// Explicit inputs/outputs of one [`ExecutionStage`].
///
/// `nodes` is the fused set in schedule order. `inputs` are edges from
/// **outside** the stage; `outputs` are nodes this stage produces that a later
/// stage or a graph output consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StageIo {
    /// Index in [`ExecutionPlan::stages`] / [`ExecutionPlan::io`].
    pub index: u32,
    /// Compiled node indexes in this stage (same order as authoring `NodeId`s).
    #[serde(default)]
    pub nodes: Vec<NodeIndex>,
    /// External inputs (sorted by [`NodeIndex`]).
    #[serde(default)]
    pub inputs: Vec<StagePort>,
    /// Live outputs (sorted by [`NodeIndex`]).
    #[serde(default)]
    pub outputs: Vec<StagePort>,
}

/// Ordered hybrid execution plan derived from a [`crate::RenderGraph`].
///
/// Stages are **runtime boundaries**: executors must walk them in order and
/// only evaluate each stage's [`ExecutionStage::node_ids`], carrying media
/// products forward. Full-DAG re-materialize ignoring stages is a legacy path.
///
/// [`Self::io`] is the typed program view (numeric ports + contracts). The
/// existing I/O runner still uses [`ExecutionStage::node_ids`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ExecutionPlan {
    /// Stages in order.
    #[serde(default)]
    pub stages: Vec<ExecutionStage>,
    /// Per-stage ports; same length as [`Self::stages`] after [`crate::schedule_compiled`].
    #[serde(default)]
    pub io: Vec<StageIo>,
    /// Fingerprint / notes for cache keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl ExecutionPlan {
    /// Empty plan.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a stage without ports (legacy / tests).
    pub fn push(&mut self, stage: ExecutionStage) {
        self.stages.push(stage);
    }

    /// Append a stage together with its explicit I/O.
    pub fn push_stage(&mut self, stage: ExecutionStage, io: StageIo) {
        self.stages.push(stage);
        self.io.push(io);
    }

    /// Total nodes across all stages (may count a node once if schedule is correct).
    #[must_use]
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    /// Flattened node ids in stage order (for diagnostics).
    #[must_use]
    pub fn all_node_ids(&self) -> Vec<&NodeId> {
        self.stages
            .iter()
            .flat_map(ExecutionStage::node_ids)
            .collect()
    }

    /// Ports for stage `i`.
    #[must_use]
    pub fn stage_io(&self, i: usize) -> Option<&StageIo> {
        self.io.get(i)
    }
}
