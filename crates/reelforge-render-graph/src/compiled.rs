//! Canonical compiled form of a [`RenderGraph`].
//!
//! Authoring graphs use string [`NodeId`] / [`MediaAssetId`]. Execution identity
//! is a dense [`NodeIndex`] assigned in **canonical topological order**, so a
//! permutation of the authoring `nodes` / `assets` arrays compiles to the same
//! program. String ids remain debug aliases for [`crate::ExecutionPlan`] adapters.

use crate::compile::{CompiledOp, compile_op, ensure_executable};
use crate::contract::infer_node_contract;
use crate::error::{GraphError, Result};
use crate::graph::{MediaAsset, MediaAssetId, NodeId, RenderGraph, RenderNodeKind};
use crate::op::{MediaContract, OperationId, OperationRegistry};
use crate::redaction::RegionRedaction;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Dense index into [`CompiledGraph::nodes`] (canonical topo order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeIndex(pub u32);

impl NodeIndex {
    /// Wrap a raw index.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Slot into [`CompiledGraph::nodes`].
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl core::fmt::Display for NodeIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// Dense index into [`CompiledGraph::assets`] (sorted by [`MediaAssetId`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetIndex(pub u32);

impl AssetIndex {
    /// Wrap a raw index.
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    /// Raw value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Slot into [`CompiledGraph::assets`].
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl core::fmt::Display for AssetIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}

/// Body of a compiled node (no free-form JSON left on the op path).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompiledNodeKind {
    /// File / seed source.
    Source {
        /// Canonical asset slot.
        asset: AssetIndex,
    },
    /// Typed, validated operation.
    Op(CompiledOp),
    /// Fused privacy redaction (masks stay on the node).
    Redaction {
        /// Redaction payload from the authoring graph.
        redaction: RegionRedaction,
    },
    /// Output sink marker.
    Output {
        /// Logical output name.
        name: String,
    },
}

/// One node in a [`CompiledGraph`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledNode {
    /// Equals this node's position in [`CompiledGraph::nodes`].
    pub index: NodeIndex,
    /// Authoring id (debug / plan adapter only).
    pub id: NodeId,
    /// Compiled body.
    pub kind: CompiledNodeKind,
    /// Upstream nodes as indexes (always `< index` after a successful compile).
    pub inputs: Vec<NodeIndex>,
    /// Inferred output streams after contract check.
    #[serde(default)]
    pub output: crate::op::MediaContract,
}

/// Graph output bound to a compiled node index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledOutput {
    /// Output name.
    pub name: String,
    /// Producing node.
    pub node: NodeIndex,
    /// Destination path or URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// Validated, typed, index-addressed render program.
///
/// `nodes` are stored in canonical topological order. `assets` are stored
/// sorted by [`MediaAssetId`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledGraph {
    /// Schema version copied from the authoring graph.
    pub version: u32,
    /// Assets in stable id order.
    #[serde(default)]
    pub assets: Vec<MediaAsset>,
    /// Nodes in canonical topo order.
    #[serde(default)]
    pub nodes: Vec<CompiledNode>,
    /// Outputs (authoring order; names unique).
    #[serde(default)]
    pub outputs: Vec<CompiledOutput>,
}

impl CompiledGraph {
    /// Node at `index`.
    ///
    /// # Errors
    ///
    /// Out of range.
    pub fn get(&self, index: NodeIndex) -> Result<&CompiledNode> {
        self.nodes
            .get(index.as_usize())
            .ok_or_else(|| GraphError::UnknownId(format!("node index {index}")))
    }

    /// Asset at `index`.
    ///
    /// # Errors
    ///
    /// Out of range.
    pub fn get_asset(&self, index: AssetIndex) -> Result<&MediaAsset> {
        self.assets
            .get(index.as_usize())
            .ok_or_else(|| GraphError::UnknownId(format!("asset index {index}")))
    }

    /// Resolve an authoring [`NodeId`] to its compiled index.
    ///
    /// # Errors
    ///
    /// Unknown id.
    pub fn lookup(&self, id: &NodeId) -> Result<NodeIndex> {
        self.nodes
            .iter()
            .find(|n| n.id == *id)
            .map(|n| n.index)
            .ok_or_else(|| GraphError::UnknownId(id.0.clone()))
    }

    /// Resolve an authoring [`MediaAssetId`] to its compiled index.
    ///
    /// # Errors
    ///
    /// Unknown id.
    pub fn lookup_asset(&self, id: &MediaAssetId) -> Result<AssetIndex> {
        self.assets
            .iter()
            .position(|a| a.id == *id)
            .map(index_u32_asset)
            .transpose()?
            .ok_or_else(|| GraphError::UnknownId(id.0.clone()))
    }

    /// Canonical JSON (stable field order from struct definition).
    ///
    /// # Errors
    ///
    /// Serde failure.
    pub fn to_json_canonical(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| GraphError::message(e.to_string()))
    }
}

fn index_u32(i: usize, what: &str) -> Result<u32> {
    u32::try_from(i).map_err(|_| GraphError::message(format!("{what} count exceeds u32")))
}

fn index_u32_asset(i: usize) -> Result<AssetIndex> {
    Ok(AssetIndex(index_u32(i, "asset")?))
}

/// Validate, type-check ops, and assign dense indexes.
///
/// # Errors
///
/// Structural graph errors, unknown / non-executable ops, or invalid params.
pub fn compile_graph(graph: &RenderGraph, registry: &OperationRegistry) -> Result<CompiledGraph> {
    graph.validate()?;

    let mut assets = graph.assets.clone();
    assets.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    let mut asset_ix: BTreeMap<&str, AssetIndex> = BTreeMap::new();
    for (i, a) in assets.iter().enumerate() {
        asset_ix.insert(a.id.0.as_str(), AssetIndex(index_u32(i, "asset")?));
    }

    let order = graph.topo_order()?;
    let mut id_ix: BTreeMap<&str, NodeIndex> = BTreeMap::new();
    for (i, id) in order.iter().enumerate() {
        id_ix.insert(id.0.as_str(), NodeIndex(index_u32(i, "node")?));
    }

    let node_by_id: BTreeMap<&str, &crate::graph::RenderNode> =
        graph.nodes.iter().map(|n| (n.id.0.as_str(), n)).collect();

    let mut nodes: Vec<CompiledNode> = Vec::with_capacity(order.len());
    for (i, id) in order.iter().enumerate() {
        let src = node_by_id
            .get(id.0.as_str())
            .ok_or_else(|| GraphError::UnknownId(id.0.clone()))?;
        let index = NodeIndex(index_u32(i, "node")?);
        let inputs = src
            .inputs
            .iter()
            .map(|inp| {
                id_ix
                    .get(inp.0.as_str())
                    .copied()
                    .ok_or_else(|| GraphError::UnknownId(inp.0.clone()))
            })
            .collect::<Result<Vec<_>>>()?;
        for inp in &inputs {
            if inp.get() >= index.get() {
                return Err(GraphError::message(format!(
                    "node {} input {} is not strictly upstream",
                    id.0,
                    inp.get()
                )));
            }
        }
        let kind = match &src.body {
            RenderNodeKind::Source { asset } => {
                let slot = asset_ix
                    .get(asset.0.as_str())
                    .copied()
                    .ok_or_else(|| GraphError::UnknownId(asset.0.clone()))?;
                CompiledNodeKind::Source { asset: slot }
            }
            RenderNodeKind::Op { operation, params } => {
                CompiledNodeKind::Op(compile_op(registry, operation, params)?)
            }
            RenderNodeKind::Redaction { redaction } => {
                ensure_executable(registry, &OperationId::new("rf.redaction.region"))?;
                CompiledNodeKind::Redaction {
                    redaction: redaction.clone(),
                }
            }
            RenderNodeKind::Output { name } => CompiledNodeKind::Output { name: name.clone() },
        };
        let draft = CompiledNode {
            index,
            id: id.clone(),
            kind,
            inputs,
            output: MediaContract::default(),
        };
        let upstreams: Vec<&MediaContract> = draft
            .inputs
            .iter()
            .map(|inp| &nodes[inp.as_usize()].output)
            .collect();
        let output = infer_node_contract(&draft, &assets, &upstreams, registry)?;
        nodes.push(CompiledNode { output, ..draft });
    }

    let outputs = graph
        .outputs
        .iter()
        .map(|o| {
            let node = id_ix
                .get(o.node.0.as_str())
                .copied()
                .ok_or_else(|| GraphError::UnknownId(o.node.0.clone()))?;
            Ok(CompiledOutput {
                name: o.name.clone(),
                node,
                uri: o.uri.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(CompiledGraph {
        version: graph.version,
        assets,
        nodes,
        outputs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RENDER_GRAPH_VERSION;
    use crate::graph::{GraphOutput, RenderNode};
    use crate::mask::{MaskSample, MaskTimeline};
    use crate::op::OperationRegistry;
    use crate::redaction::RegionRedaction;
    use reelforge_core::MediaTime;

    fn linear_redaction() -> RenderGraph {
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
                id: MediaAssetId("cam_b".into()),
                uri: "b.mp4".into(),
                duration: None,
                role: None,
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("cam_b".into()),
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
    fn assigns_dense_topo_indexes() {
        let g = linear_redaction();
        let compiled = compile_graph(&g, &OperationRegistry::with_builtins()).unwrap();
        assert_eq!(compiled.nodes.len(), 4);
        let ids: Vec<_> = compiled.nodes.iter().map(|n| n.id.0.as_str()).collect();
        // Kahn + min-id ready set: src → trim → blur → out.
        assert_eq!(ids, ["src", "trim", "blur", "out"]);
        for (i, n) in compiled.nodes.iter().enumerate() {
            assert_eq!(n.index.as_usize(), i);
            assert!(n.inputs.iter().all(|inp| inp.as_usize() < i));
        }
        assert_eq!(
            compiled.lookup(&NodeId("trim".into())).unwrap().as_usize(),
            1
        );
        let src = compiled.get(NodeIndex(0)).unwrap();
        match src.kind {
            CompiledNodeKind::Source { asset } => {
                assert_eq!(compiled.get_asset(asset).unwrap().id.0, "cam_b");
            }
            _ => panic!("src"),
        }
        match &compiled.nodes[2].kind {
            CompiledNodeKind::Redaction { redaction } => {
                assert_eq!(redaction.masks.samples.len(), 1);
            }
            other => panic!("expected redaction, got {other:?}"),
        }
        assert_eq!(compiled.outputs[0].node.as_usize(), 3);
        assert!(compiled.nodes[0].output.video && compiled.nodes[0].output.audio);
        assert!(compiled.nodes[2].output.video && compiled.nodes[2].output.audio);
        assert!(!compiled.nodes[2].output.masks);
    }

    #[test]
    fn compile_rejects_gain_after_audio_drop() {
        let g = RenderGraph {
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
                    id: NodeId("drop".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.audio.drop"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("gain".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.audio.gain"),
                        params: serde_json::json!({ "factor": 0.5 }),
                    },
                    inputs: vec![NodeId("drop".into())],
                },
            ],
            outputs: vec![],
        };
        let err = compile_graph(&g, &OperationRegistry::with_builtins()).unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_MEDIA_CONTRACT");
    }

    #[test]
    fn authoring_permutation_is_byte_identical() {
        let registry = OperationRegistry::with_builtins();
        let mut g = linear_redaction();
        g.assets.push(MediaAsset {
            id: MediaAssetId("cam_a".into()),
            uri: "a.mp4".into(),
            duration: None,
            role: None,
        });
        // Extra unused asset must still canonicalize by id (cam_a before cam_b).
        let golden = compile_graph(&g, &registry)
            .unwrap()
            .to_json_canonical()
            .unwrap();
        assert!(golden.contains("cam_a"));
        assert!(
            golden.find("cam_a").unwrap() < golden.find("cam_b").unwrap(),
            "assets must sort by id"
        );

        for seed in 1_u64..=64 {
            let mut shuffled = g.clone();
            shuffle(&mut shuffled.nodes, seed);
            shuffle(&mut shuffled.assets, seed.wrapping_mul(17));
            let json = compile_graph(&shuffled, &registry)
                .unwrap()
                .to_json_canonical()
                .unwrap();
            assert_eq!(json, golden, "mismatch at seed {seed}");
        }
    }

    #[test]
    fn rejects_invalid_op_params() {
        let mut g = linear_redaction();
        g.nodes[1].body = RenderNodeKind::Op {
            operation: OperationId::new("rf.transform.trim"),
            params: serde_json::json!({}),
        };
        let err = compile_graph(&g, &OperationRegistry::with_builtins()).unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_INVALID_PARAMS");
    }

    #[test]
    fn json_roundtrip() {
        let compiled =
            compile_graph(&linear_redaction(), &OperationRegistry::with_builtins()).unwrap();
        let text = compiled.to_json_canonical().unwrap();
        let back: CompiledGraph = serde_json::from_str(&text).unwrap();
        assert_eq!(back, compiled);
    }
}
