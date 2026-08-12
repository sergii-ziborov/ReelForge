//! Stable fingerprints for stage cache keys (engine hooks for Capture policy).

use crate::error::{GraphError, Result};
use crate::graph::RenderGraph;
use crate::stage::ExecutionPlan;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Hex fingerprint of a full graph document (assets + nodes + outputs).
///
/// # Errors
///
/// Serde failure.
pub fn fingerprint_render_graph(graph: &RenderGraph) -> Result<String> {
    let json = serde_json::to_string(graph).map_err(|e| GraphError::message(e.to_string()))?;
    Ok(hash_hex(json.as_bytes()))
}

/// Hex fingerprint of a scheduled execution plan.
///
/// # Errors
///
/// Serde failure.
pub fn fingerprint_execution_plan(plan: &ExecutionPlan) -> Result<String> {
    let json = serde_json::to_string(plan).map_err(|e| GraphError::message(e.to_string()))?;
    Ok(hash_hex(json.as_bytes()))
}

/// Combined run key: graph body + schedule (cache key for a full hybrid render).
///
/// # Errors
///
/// Serde failure.
pub fn fingerprint_graph_run(graph: &RenderGraph, plan: &ExecutionPlan) -> Result<String> {
    let g = fingerprint_render_graph(graph)?;
    let p = fingerprint_execution_plan(plan)?;
    Ok(hash_hex(format!("{g}:{p}").as_bytes()))
}

/// Stage-local key from ordered node ids + backend tag.
#[must_use]
pub fn fingerprint_stage(backend: &str, node_ids: &[impl AsRef<str>]) -> String {
    let mut h = DefaultHasher::new();
    backend.hash(&mut h);
    for id in node_ids {
        id.as_ref().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn hash_hex(bytes: &[u8]) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RENDER_GRAPH_VERSION;
    use crate::graph::{GraphOutput, MediaAsset, MediaAssetId, NodeId, RenderNode, RenderNodeKind};

    fn tiny_graph() -> RenderGraph {
        RenderGraph {
            version: RENDER_GRAPH_VERSION,
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
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        }
    }

    #[test]
    fn stable_and_sensitive() {
        let g = tiny_graph();
        let a = fingerprint_render_graph(&g).unwrap();
        let b = fingerprint_render_graph(&g).unwrap();
        assert_eq!(a, b);
        let mut g2 = g.clone();
        g2.assets[0].uri = "other.mp4".into();
        let c = fingerprint_render_graph(&g2).unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn stage_ids_matter() {
        let x = fingerprint_stage("rust", &["a", "b"]);
        let y = fingerprint_stage("rust", &["b", "a"]);
        assert_ne!(x, y);
    }
}
