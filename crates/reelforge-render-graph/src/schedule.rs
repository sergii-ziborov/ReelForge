//! Schedule a [`crate::RenderGraph`] into an [`crate::ExecutionPlan`].

use crate::compile::{compile_graph_ops, ensure_executable};
use crate::error::{GraphError, Result};
use crate::graph::{RenderGraph, RenderNodeKind};
use crate::op::{BackendClass, OperationId, OperationRegistry};
use crate::stage::{AdapterStage, ExecutionPlan, ExecutionStage, FfmpegStage, GpuStage, RustStage};

/// Schedule hybrid stages from a validated graph + operation registry.
///
/// Pipeline:
/// 1. Structural validate (duplicates, cycles, refs)
/// 2. Compile all op params → typed + cost (registry↔executor parity)
/// 3. Walk stable topo order; fuse consecutive same-backend nodes
///
/// Sources and outputs without an explicit op use [`BackendClass::Ffmpeg`].
/// Redaction nodes always schedule as Rust.
///
/// # Errors
///
/// Graph structure errors, unknown operations, invalid params, or
/// registered-but-not-executable ops.
#[allow(clippy::too_many_lines)]
pub fn schedule_graph(graph: &RenderGraph, registry: &OperationRegistry) -> Result<ExecutionPlan> {
    graph.validate()?;

    let (compiled, diag) = compile_graph_ops(graph, registry);
    if diag.has_errors() {
        return Err(diag.errors.into_iter().next().unwrap_or_else(|| {
            GraphError::message("compile_graph_ops reported errors without details")
        }));
    }

    let mut total_cpu = 0.0;
    let mut total_io = 0.0;
    let mut total_mem = 0.0;
    for (_, c) in &compiled {
        total_cpu += c.cost.cpu;
        total_io += c.cost.io;
        total_mem += c.cost.memory;
    }

    let order = graph.topo_order()?;
    let mut plan = ExecutionPlan::new();
    plan.notes = Some(format!(
        "scheduled {} nodes, {} compiled ops, registry {} ops; cost cpu={total_cpu:.1} mem={total_mem:.1} io={total_io:.1}",
        order.len(),
        compiled.len(),
        registry.len()
    ));

    let mut current_backend: Option<BackendClass> = None;
    let mut current_nodes = Vec::new();
    let mut current_ops = Vec::new();
    let mut current_adapter: Option<String> = None;

    let flush = |plan: &mut ExecutionPlan,
                 backend: BackendClass,
                 nodes: Vec<_>,
                 ops: Vec<OperationId>,
                 adapter: Option<String>| {
        if nodes.is_empty() {
            return;
        }
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
        plan.push(stage);
    };

    for id in order {
        let node = graph
            .nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or_else(|| GraphError::UnknownId(id.0.clone()))?;
        let (backend, op_id, adapter_name) = match &node.body {
            RenderNodeKind::Source { .. } | RenderNodeKind::Output { .. } => {
                (BackendClass::Ffmpeg, None, None)
            }
            RenderNodeKind::Redaction { .. } => {
                let rid = OperationId::new("rf.redaction.region");
                ensure_executable(registry, &rid)?;
                (BackendClass::Rust, Some(rid), None)
            }
            RenderNodeKind::Op { operation, .. } => {
                // Params already validated in compile_graph_ops; re-check registry.
                ensure_executable(registry, operation)?;
                let desc = registry.get(operation)?;
                let adapter = if desc.backend == BackendClass::Adapter {
                    Some(operation.as_str().to_string())
                } else {
                    None
                };
                (desc.backend, Some(operation.clone()), adapter)
            }
        };

        let needs_flush = match current_backend {
            Some(b) if b != backend => true,
            Some(BackendClass::Adapter) if current_adapter != adapter_name => true,
            Some(_) | None => false,
        };
        if needs_flush {
            flush(
                &mut plan,
                current_backend.unwrap(),
                std::mem::take(&mut current_nodes),
                std::mem::take(&mut current_ops),
                current_adapter.take(),
            );
        }

        current_backend = Some(backend);
        current_adapter = adapter_name;
        current_nodes.push(id);
        if let Some(op) = op_id {
            current_ops.push(op);
        }
    }

    if let Some(b) = current_backend {
        flush(&mut plan, b, current_nodes, current_ops, current_adapter);
    }

    Ok(plan)
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
}
