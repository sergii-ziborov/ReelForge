//! Planned artifacts of an [`ExecutionPlan`] (stable program output).
//!
//! Runtime may later fill file URIs; the *shape* (stage ports, contracts,
//! fingerprints) is determined by compile + schedule and must be deterministic.

use crate::compiled::{CompiledGraph, NodeIndex};
use crate::error::{GraphError, Result};
use crate::fingerprint::{fingerprint_compiled_graph, fingerprint_execution_plan};
use crate::op::MediaContract;
use crate::stage::ExecutionPlan;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Schema version for [`ArtifactManifest`].
pub const ARTIFACT_MANIFEST_VERSION: u32 = 1;

/// Role of an artifact in the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Produced by a stage and consumed by a later stage.
    Intermediate,
    /// Bound to a graph output (final file / sink).
    Output,
}

/// One named media product.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Stable id (`s{stage}.n{node}` or `out.{name}`).
    pub id: String,
    /// Compiled node that produced this value.
    pub node: NodeIndex,
    /// Inferred streams.
    pub contract: MediaContract,
    /// Destination URI when known (graph output or cache file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    /// Content / identity fingerprint (planned; not a file hash).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Hash of the file on disk after a successful run (`None` until sealed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_fingerprint: Option<String>,
    /// Intermediate vs final.
    pub kind: ArtifactKind,
}

/// Artifacts emitted by one execution stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageArtifacts {
    /// Index in [`ExecutionPlan::stages`].
    pub stage_index: u32,
    /// Backend tag (`ffmpeg`, `rust`, …).
    pub backend: String,
    /// Stage outputs (same order as [`crate::StageIo::outputs`]).
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
}

/// Deterministic inventory of what a compiled plan will produce.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactManifest {
    /// Schema version.
    pub version: u32,
    /// Combined compile + schedule fingerprint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_fingerprint: Option<String>,
    /// Per-stage products.
    #[serde(default)]
    pub stages: Vec<StageArtifacts>,
    /// Graph outputs (authoring names + URIs).
    #[serde(default)]
    pub outputs: Vec<ArtifactRef>,
}

impl ArtifactManifest {
    /// Canonical JSON for golden / cache keys.
    ///
    /// # Errors
    ///
    /// Serde failure.
    pub fn to_json_canonical(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| GraphError::message(e.to_string()))
    }

    /// Number of scheduled stages recorded.
    #[must_use]
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }
}

/// Build a planned manifest from a compiled program + its execution plan.
#[must_use]
pub fn artifact_manifest(compiled: &CompiledGraph, plan: &ExecutionPlan) -> ArtifactManifest {
    let run_fingerprint = match (
        fingerprint_compiled_graph(compiled),
        fingerprint_execution_plan(plan),
    ) {
        (Ok(c), Ok(p)) => Some(hash_hex(format!("{c}:{p}").as_bytes())),
        _ => None,
    };

    let stages = plan
        .io
        .iter()
        .enumerate()
        .map(|(i, io)| {
            let backend = plan
                .stages
                .get(i)
                .map_or("unknown", |s| s.backend_tag())
                .to_string();
            let artifacts = io
                .outputs
                .iter()
                .map(|port| {
                    let is_out = compiled.outputs.iter().any(|o| o.node == port.node);
                    let id = format!("s{}.n{}", io.index, port.node.get());
                    ArtifactRef {
                        fingerprint: Some(hash_hex(
                            format!(
                                "{}:{id}:{}:{}",
                                run_fingerprint.as_deref().unwrap_or("-"),
                                port.contract.video,
                                port.contract.audio
                            )
                            .as_bytes(),
                        )),
                        id,
                        node: port.node,
                        contract: port.contract.clone(),
                        uri: compiled
                            .outputs
                            .iter()
                            .find(|o| o.node == port.node)
                            .and_then(|o| o.uri.clone()),
                        file_fingerprint: None,
                        kind: if is_out {
                            ArtifactKind::Output
                        } else {
                            ArtifactKind::Intermediate
                        },
                    }
                })
                .collect();
            StageArtifacts {
                stage_index: io.index,
                backend,
                artifacts,
            }
        })
        .collect();

    let outputs = compiled
        .outputs
        .iter()
        .map(|o| {
            let contract = compiled
                .get(o.node)
                .map(|n| n.output.clone())
                .unwrap_or_default();
            let id = format!("out.{}", o.name);
            ArtifactRef {
                fingerprint: Some(hash_hex(
                    format!(
                        "{}:{id}:{}",
                        run_fingerprint.as_deref().unwrap_or("-"),
                        o.uri.as_deref().unwrap_or("")
                    )
                    .as_bytes(),
                )),
                id,
                node: o.node,
                contract,
                uri: o.uri.clone(),
                file_fingerprint: None,
                kind: ArtifactKind::Output,
            }
        })
        .collect();

    ArtifactManifest {
        version: ARTIFACT_MANIFEST_VERSION,
        run_fingerprint,
        stages,
        outputs,
    }
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
    use crate::compiled::compile_graph;
    use crate::graph::{
        GraphOutput, MediaAsset, MediaAssetId, NodeId, RenderGraph, RenderNode, RenderNodeKind,
    };
    use crate::mask::{MaskSample, MaskTimeline};
    use crate::op::{OperationId, OperationRegistry};
    use crate::redaction::RegionRedaction;
    use crate::schedule::schedule_compiled;
    use reelforge_core::MediaTime;

    fn linear() -> RenderGraph {
        let mut masks = MaskTimeline::new();
        masks.push(MaskSample::ellipse(
            MediaTime::new(0, 30).unwrap(),
            16.0,
            16.0,
            8.0,
        ));
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
                        redaction: RegionRedaction::gaussian(masks, 10.0),
                    },
                    inputs: vec![NodeId("trim".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("blur".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        }
    }

    fn shuffle<T>(items: &mut [T], seed: u64) {
        let mut s = seed;
        for i in (1..items.len()).rev() {
            s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let span = u64::try_from(i + 1).unwrap_or(1);
            let j = usize::try_from(s % span).unwrap_or(0);
            items.swap(i, j);
        }
    }

    #[test]
    fn manifest_mirrors_stage_ports_and_graph_outputs() {
        let reg = OperationRegistry::with_builtins();
        let compiled = compile_graph(&linear(), &reg).unwrap();
        let plan = schedule_compiled(&compiled).unwrap();
        let man = artifact_manifest(&compiled, &plan);
        assert_eq!(man.stage_count(), plan.io.len());
        assert_eq!(man.version, ARTIFACT_MANIFEST_VERSION);
        assert!(man.run_fingerprint.is_some());
        for (io, st) in plan.io.iter().zip(&man.stages) {
            assert_eq!(st.artifacts.len(), io.outputs.len());
            for (port, art) in io.outputs.iter().zip(&st.artifacts) {
                assert_eq!(art.node, port.node);
                assert_eq!(art.contract, port.contract);
            }
        }
        assert_eq!(man.outputs.len(), 1);
        assert_eq!(man.outputs[0].uri.as_deref(), Some("out.mp4"));
        assert_eq!(man.outputs[0].kind, ArtifactKind::Output);
        assert!(man.outputs[0].contract.video);
    }

    #[test]
    fn graph_to_manifest_is_byte_identical_under_authoring_shuffle() {
        let reg = OperationRegistry::with_builtins();
        let g = linear();
        let compiled = compile_graph(&g, &reg).unwrap();
        let plan = schedule_compiled(&compiled).unwrap();
        let golden = artifact_manifest(&compiled, &plan)
            .to_json_canonical()
            .unwrap();

        for seed in 1_u64..=64 {
            let mut shuffled = g.clone();
            shuffle(&mut shuffled.nodes, seed);
            shuffle(&mut shuffled.assets, seed.wrapping_mul(9));
            let c = compile_graph(&shuffled, &reg).unwrap();
            let p = schedule_compiled(&c).unwrap();
            let json = artifact_manifest(&c, &p).to_json_canonical().unwrap();
            assert_eq!(json, golden, "mismatch at seed {seed}");
        }
    }
}
