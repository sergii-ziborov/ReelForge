//! Schedule a [`crate::RenderGraph`] / [`crate::CompiledGraph`] into an [`crate::ExecutionPlan`].

use crate::compiled::{CompiledGraph, CompiledNodeKind, NodeIndex, compile_graph};
use crate::error::Result;
use crate::graph::RenderGraph;
use crate::op::{BackendClass, OperationId, OperationRegistry};
use crate::stage::{
    AdapterStage, ExecutionPlan, ExecutionStage, FfmpegStage, GpuStage, RustStage, StageIo,
    StagePort,
};
use std::collections::{BTreeMap, BTreeSet};

/// Schedule hybrid stages from a validated graph + operation registry.
///
/// Pipeline:
/// 1. [`compile_graph`] (validate, typed ops, numeric indexes)
/// 2. [`schedule_compiled`] (fuse consecutive same-backend nodes)
///
/// Sources and outputs without an explicit op use [`BackendClass::Ffmpeg`].
/// Redaction nodes always schedule as Rust.
///
/// # Errors
///
/// Graph structure errors, unknown operations, invalid params, or
/// registered-but-not-executable ops.
pub fn schedule_graph(graph: &RenderGraph, registry: &OperationRegistry) -> Result<ExecutionPlan> {
    let compiled = compile_graph(graph, registry)?;
    schedule_compiled(&compiled)
}

/// Fuse a compiled program into backend stages.
///
/// Walks [`CompiledGraph::nodes`] (already canonical topo). Stage lists still
/// carry authoring [`crate::NodeId`] so the existing I/O runner can consume the
/// plan without a second compile.
///
/// # Errors
///
/// None today (graph already compiled); reserved for planner constraints.
pub fn schedule_compiled(compiled: &CompiledGraph) -> Result<ExecutionPlan> {
    let (compiled_ops, total_cpu, total_mem, total_io) = compiled_cost(compiled);
    let mut plan = ExecutionPlan::new();
    plan.notes = Some(format!(
        "scheduled {} nodes, {compiled_ops} compiled ops; cost cpu={total_cpu:.1} mem={total_mem:.1} io={total_io:.1}",
        compiled.nodes.len(),
    ));

    let mut current_backend: Option<BackendClass> = None;
    let mut current_indexes: Vec<NodeIndex> = Vec::new();
    let mut current_ops = Vec::new();
    let mut current_adapter: Option<String> = None;

    for node in &compiled.nodes {
        let (backend, op_id, adapter_name) = node_schedule(node);
        let needs_flush = match current_backend {
            Some(b) if b != backend => true,
            Some(BackendClass::Adapter) if current_adapter != adapter_name => true,
            Some(_) | None => false,
        };
        if needs_flush {
            flush_stage(
                &mut plan,
                compiled,
                current_backend.unwrap(),
                &std::mem::take(&mut current_indexes),
                std::mem::take(&mut current_ops),
                current_adapter.take(),
            );
        }

        current_backend = Some(backend);
        current_adapter = adapter_name;
        current_indexes.push(node.index);
        if let Some(op) = op_id {
            current_ops.push(op);
        }
    }

    if let Some(b) = current_backend {
        flush_stage(
            &mut plan,
            compiled,
            b,
            &current_indexes,
            current_ops,
            current_adapter,
        );
    }

    Ok(plan)
}

fn compiled_cost(compiled: &CompiledGraph) -> (usize, f64, f64, f64) {
    let mut ops = 0_usize;
    let mut cpu = 0.0;
    let mut mem = 0.0;
    let mut io = 0.0;
    for n in &compiled.nodes {
        match &n.kind {
            CompiledNodeKind::Op(op) => {
                ops += 1;
                cpu += op.cost.cpu;
                mem += op.cost.memory;
                io += op.cost.io;
            }
            CompiledNodeKind::Redaction { .. } => {
                ops += 1;
                cpu += 5.0;
                mem += 3.0;
                io += 0.5;
            }
            CompiledNodeKind::Source { .. } | CompiledNodeKind::Output { .. } => {}
        }
    }
    (ops, cpu, mem, io)
}

fn node_schedule(
    node: &crate::compiled::CompiledNode,
) -> (BackendClass, Option<OperationId>, Option<String>) {
    match &node.kind {
        CompiledNodeKind::Source { .. } | CompiledNodeKind::Output { .. } => {
            (BackendClass::Ffmpeg, None, None)
        }
        CompiledNodeKind::Redaction { .. } => (
            BackendClass::Rust,
            Some(OperationId::new("rf.redaction.region")),
            None,
        ),
        CompiledNodeKind::Op(op) => {
            let adapter = if op.backend == BackendClass::Adapter {
                Some(op.id.as_str().to_string())
            } else {
                None
            };
            (op.backend, Some(op.id.clone()), adapter)
        }
    }
}

fn flush_stage(
    plan: &mut ExecutionPlan,
    compiled: &CompiledGraph,
    backend: BackendClass,
    indexes: &[NodeIndex],
    ops: Vec<OperationId>,
    adapter: Option<String>,
) {
    if indexes.is_empty() {
        return;
    }
    let nodes: Vec<crate::graph::NodeId> = indexes
        .iter()
        .filter_map(|i| compiled.get(*i).ok().map(|n| n.id.clone()))
        .collect();
    let io = compute_stage_io(compiled, plan.stages.len(), indexes);
    let stage = match backend {
        BackendClass::Ffmpeg => ExecutionStage::Ffmpeg(FfmpegStage {
            nodes,
            vf: None,
            video_codec: None,
        }),
        BackendClass::Rust => ExecutionStage::Rust(RustStage {
            nodes,
            operations: ops,
        }),
        BackendClass::Adapter => ExecutionStage::Adapter(AdapterStage {
            adapter: adapter.unwrap_or_else(|| "external".into()),
            nodes,
        }),
        BackendClass::Gpu => ExecutionStage::Gpu(GpuStage {
            nodes,
            backend: None,
        }),
    };
    plan.push_stage(stage, io);
}

fn compute_stage_io(compiled: &CompiledGraph, index: usize, nodes: &[NodeIndex]) -> StageIo {
    let in_stage: BTreeSet<NodeIndex> = nodes.iter().copied().collect();
    let mut inputs: BTreeMap<NodeIndex, StagePort> = BTreeMap::new();
    let mut outputs: BTreeMap<NodeIndex, StagePort> = BTreeMap::new();

    for &nid in nodes {
        let Ok(node) = compiled.get(nid) else {
            continue;
        };
        for &up in &node.inputs {
            if in_stage.contains(&up) {
                continue;
            }
            if let Ok(src) = compiled.get(up) {
                inputs.entry(up).or_insert_with(|| StagePort {
                    node: up,
                    contract: src.output.clone(),
                });
            }
        }
    }

    for &nid in nodes {
        let Ok(node) = compiled.get(nid) else {
            continue;
        };
        let consumed_outside = compiled
            .nodes
            .iter()
            .any(|other| !in_stage.contains(&other.index) && other.inputs.contains(&nid));
        let is_graph_out = compiled.outputs.iter().any(|o| o.node == nid);
        if consumed_outside || is_graph_out {
            outputs.entry(nid).or_insert_with(|| StagePort {
                node: nid,
                contract: node.output.clone(),
            });
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    StageIo {
        index: u32::try_from(index).unwrap_or(u32::MAX),
        nodes: nodes.to_vec(),
        inputs: inputs.into_values().collect(),
        outputs: outputs.into_values().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        GraphOutput, MediaAsset, MediaAssetId, NodeId, RenderGraph, RenderNode, RenderNodeKind,
    };
    use crate::mask::MaskTimeline;
    use crate::redaction::RegionRedaction;

    #[test]
    fn schedules_ffmpeg_then_rust_then_ffmpeg() {
        let registry = OperationRegistry::with_builtins();
        let g = RenderGraph {
            version: 1,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "in.mp4".into(),
                duration: None,
                role: None,
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("a".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("trim".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.trim"),
                        params: serde_json::json!({ "start": 0.0, "duration": 1.0 }),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("blur".into()),
                    body: RenderNodeKind::Redaction {
                        redaction: RegionRedaction::gaussian(MaskTimeline::new(), 10.0),
                    },
                    inputs: vec![NodeId("trim".into())],
                },
                RenderNode {
                    id: NodeId("enc".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.encode.h264"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("blur".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("enc".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        };

        let plan = schedule_graph(&g, &registry).unwrap();
        assert!(plan.stages.len() >= 3);
        // First stage FFmpeg (src+trim), then Rust redaction, then FFmpeg encode+out.
        assert!(matches!(plan.stages[0], ExecutionStage::Ffmpeg(_)));
        assert!(
            plan.stages
                .iter()
                .any(|s| matches!(s, ExecutionStage::Rust(_)))
        );
        assert!(matches!(
            plan.stages.last().unwrap(),
            ExecutionStage::Ffmpeg(_)
        ));
        let notes = plan.notes.as_deref().unwrap_or("");
        assert!(
            notes.contains("cost cpu="),
            "notes should include cost: {notes}"
        );
    }

    #[test]
    fn schedule_rejects_invalid_trim_params() {
        let registry = OperationRegistry::with_builtins();
        let g = RenderGraph {
            version: 1,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "in.mp4".into(),
                duration: None,
                role: None,
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("a".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("trim".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.trim"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
            ],
            outputs: vec![],
        };
        let err = schedule_graph(&g, &registry).unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_INVALID_PARAMS");
    }

    #[test]
    fn compiled_then_schedule_matches_schedule_graph() {
        let registry = OperationRegistry::with_builtins();
        let g = RenderGraph {
            version: 1,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "in.mp4".into(),
                duration: None,
                role: None,
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("a".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("src".into())],
                },
            ],
            outputs: vec![],
        };
        let via_graph = schedule_graph(&g, &registry).unwrap();
        let compiled = compile_graph(&g, &registry).unwrap();
        let via_compiled = schedule_compiled(&compiled).unwrap();
        assert_eq!(
            serde_json::to_string(&via_graph).unwrap(),
            serde_json::to_string(&via_compiled).unwrap()
        );
    }

    #[test]
    fn stages_expose_numeric_ports_and_contracts() {
        let registry = OperationRegistry::with_builtins();
        let g = RenderGraph {
            version: 1,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "in.mp4".into(),
                duration: None,
                role: None,
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("a".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("trim".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.trim"),
                        params: serde_json::json!({ "start": 0.0, "duration": 1.0 }),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("blur".into()),
                    body: RenderNodeKind::Redaction {
                        redaction: RegionRedaction::gaussian(MaskTimeline::new(), 10.0),
                    },
                    inputs: vec![NodeId("trim".into())],
                },
                RenderNode {
                    id: NodeId("enc".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.encode.h264"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("blur".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("enc".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        };
        let compiled = compile_graph(&g, &registry).unwrap();
        let plan = schedule_compiled(&compiled).unwrap();
        assert_eq!(plan.io.len(), plan.stages.len());
        assert!(plan.stages.len() >= 3);

        let first = plan.stage_io(0).unwrap();
        assert!(first.inputs.is_empty(), "first stage is a source");
        let trim = compiled.lookup(&NodeId("trim".into())).unwrap();
        assert!(
            first
                .outputs
                .iter()
                .any(|p| p.node == trim && p.contract.video),
            "trim must leave the ffmpeg prefix: {first:?}"
        );

        let rust = plan
            .io
            .iter()
            .find(|s| {
                plan.stages
                    .get(s.index as usize)
                    .is_some_and(|st| matches!(st, ExecutionStage::Rust(_)))
            })
            .expect("rust stage");
        assert!(rust.inputs.iter().any(|p| p.node == trim));
        let blur = compiled.lookup(&NodeId("blur".into())).unwrap();
        assert!(
            rust.outputs
                .iter()
                .any(|p| p.node == blur && p.contract.video)
        );

        let last = plan.io.last().unwrap();
        let out = compiled.lookup(&NodeId("out".into())).unwrap();
        assert!(last.inputs.iter().any(|p| p.node == blur));
        assert!(last.outputs.iter().any(|p| p.node == out));
    }
}
