//! Stable fingerprints for stage cache keys (engine hooks for Capture policy).

use crate::compile::CompiledOp;
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

/// Stage-local key from ordered node ids + backend tag (legacy, weak).
///
/// Prefer [`fingerprint_stage_key`] with full [`StageCacheKey`] parts.
#[must_use]
pub fn fingerprint_stage(backend: &str, node_ids: &[impl AsRef<str>]) -> String {
    let mut h = DefaultHasher::new();
    backend.hash(&mut h);
    for id in node_ids {
        id.as_ref().hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

/// Strong stage-cache identity: inputs, compiled ops (id+version+params),
/// backend class, and optional host `FFmpeg` version.
///
/// Cache invalidates when any of these change — not only node id order.
#[derive(Debug, Clone)]
pub struct StageCacheKey<'a> {
    /// Backend tag (`ffmpeg`, `rust`, `adapter`, `gpu`).
    pub backend: &'a str,
    /// Ordered node ids in the stage.
    pub node_ids: &'a [String],
    /// Hash of upstream inputs (URI list, prior stage artifact, seed content).
    pub input_fingerprint: &'a str,
    /// Compiled ops for this stage (version + typed params).
    pub compiled: &'a [CompiledOp],
    /// Host `FFmpeg` version string when the stage uses `FFmpeg` (else empty).
    pub ffmpeg_version: &'a str,
    /// Optional host/engine tag (OS, GPU encoder id, …).
    pub host_tag: &'a str,
}

/// Fingerprint a full stage key.
#[must_use]
pub fn fingerprint_stage_key(key: &StageCacheKey<'_>) -> String {
    let mut h = DefaultHasher::new();
    "stage_v2".hash(&mut h);
    key.backend.hash(&mut h);
    key.input_fingerprint.hash(&mut h);
    key.ffmpeg_version.hash(&mut h);
    key.host_tag.hash(&mut h);
    for id in key.node_ids {
        id.hash(&mut h);
    }
    for op in key.compiled {
        op.id.as_str().hash(&mut h);
        op.version.major.hash(&mut h);
        op.version.minor.hash(&mut h);
        op.version.patch.hash(&mut h);
        format!("{:?}", op.backend).hash(&mut h);
        // Typed params as canonical JSON.
        if let Ok(js) = serde_json::to_string(&op.params) {
            js.hash(&mut h);
        }
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
    use crate::compile::{TypedParams, compile_op};
    use crate::graph::{GraphOutput, MediaAsset, MediaAssetId, NodeId, RenderNode, RenderNodeKind};
    use crate::op::{BackendClass, OperationId, OperationRegistry, SemVer};

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

    #[test]
    fn strong_key_includes_params_version_ffmpeg() {
        let r = OperationRegistry::with_builtins();
        let scale = compile_op(
            &r,
            &OperationId::new("rf.transform.scale"),
            &serde_json::json!({ "w": 64, "h": 32 }),
        )
        .unwrap();
        let scale2 = compile_op(
            &r,
            &OperationId::new("rf.transform.scale"),
            &serde_json::json!({ "w": 128, "h": 32 }),
        )
        .unwrap();
        let nodes = vec!["n1".into()];
        let k1 = StageCacheKey {
            backend: "ffmpeg",
            node_ids: &nodes,
            input_fingerprint: "inA",
            compiled: std::slice::from_ref(&scale),
            ffmpeg_version: "6.1",
            host_tag: "win",
        };
        let k2 = StageCacheKey {
            compiled: std::slice::from_ref(&scale2),
            ..k1
        };
        let k3 = StageCacheKey {
            ffmpeg_version: "7.0",
            ..k1
        };
        let k4 = StageCacheKey {
            input_fingerprint: "inB",
            ..k1
        };
        assert_ne!(fingerprint_stage_key(&k1), fingerprint_stage_key(&k2));
        assert_ne!(fingerprint_stage_key(&k1), fingerprint_stage_key(&k3));
        assert_ne!(fingerprint_stage_key(&k1), fingerprint_stage_key(&k4));
        assert_eq!(fingerprint_stage_key(&k1), fingerprint_stage_key(&k1));

        // Op version change also invalidates.
        let mut scale_v2 = scale.clone();
        scale_v2.version = SemVer::new(2, 0, 0);
        let k5 = StageCacheKey {
            compiled: std::slice::from_ref(&scale_v2),
            ..k1
        };
        assert_ne!(fingerprint_stage_key(&k1), fingerprint_stage_key(&k5));
        // Sanity: typed params round-trip for scale.
        assert!(matches!(scale.params, TypedParams::Scale { w: 64, h: 32 }));
        assert_eq!(scale.backend, BackendClass::Ffmpeg);
    }
}
