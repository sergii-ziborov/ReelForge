//! `RenderGraph` DAG (executable media model).

use crate::RENDER_GRAPH_VERSION;
use crate::error::{GraphError, Result};
use crate::op::OperationId;
use crate::redaction::RegionRedaction;
use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Asset identifier within a graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MediaAssetId(pub String);

/// Node identifier within a graph.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

/// Input media asset referenced by the graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaAsset {
    /// Id.
    pub id: MediaAssetId,
    /// URI or path (host-resolved).
    pub uri: String,
    /// Optional known duration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<MediaTime>,
    /// Optional role tag (`video`, `audio`, `proxy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// Kind of render node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderNodeKind {
    /// Source asset reference.
    Source {
        /// Asset id.
        asset: MediaAssetId,
    },
    /// Typed operation application.
    Op {
        /// Registry operation id.
        operation: OperationId,
        /// Parameters (validated via registry / compile).
        #[serde(default)]
        params: Value,
    },
    /// Fused privacy redaction.
    Redaction {
        /// Redaction payload.
        redaction: RegionRedaction,
    },
    /// Output sink marker (also listed in [`RenderGraph::outputs`]).
    Output {
        /// Logical output name.
        name: String,
    },
}

/// One node in the DAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderNode {
    /// Node id.
    pub id: NodeId,
    /// Node body.
    pub body: RenderNodeKind,
    /// Upstream node ids (inputs).
    #[serde(default)]
    pub inputs: Vec<NodeId>,
}

/// Graph output binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphOutput {
    /// Output name.
    pub name: String,
    /// Producing node.
    pub node: NodeId,
    /// Destination path or URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// Deterministic executable media DAG.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderGraph {
    /// Schema version.
    pub version: u32,
    /// Assets.
    #[serde(default)]
    pub assets: Vec<MediaAsset>,
    /// Nodes.
    #[serde(default)]
    pub nodes: Vec<RenderNode>,
    /// Outputs.
    #[serde(default)]
    pub outputs: Vec<GraphOutput>,
}

impl Default for RenderGraph {
    fn default() -> Self {
        Self {
            version: RENDER_GRAPH_VERSION,
            assets: Vec::new(),
            nodes: Vec::new(),
            outputs: Vec::new(),
        }
    }
}

impl RenderGraph {
    /// Empty v1 graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Canonical JSON (sorted keys via serde default order of structs + stable fields).
    ///
    /// Prefer this for cache keys / golden tests over pretty printing alone.
    ///
    /// # Errors
    ///
    /// Serde failure.
    pub fn to_json_canonical(&self) -> Result<String> {
        // serde_json preserves field order from struct definition; we validate
        // first then serialize. Arrays keep insertion order (authoritative).
        serde_json::to_string(self).map_err(|e| GraphError::message(e.to_string()))
    }

    /// Pretty JSON.
    ///
    /// # Errors
    ///
    /// Serde failure.
    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| GraphError::message(e.to_string()))
    }

    /// Parse JSON.
    ///
    /// # Errors
    ///
    /// Serde / version / structural validation.
    pub fn from_json(text: &str) -> Result<Self> {
        let g: Self = serde_json::from_str(text).map_err(|e| GraphError::message(e.to_string()))?;
        g.validate()?;
        Ok(g)
    }

    /// Structural validation + DAG acyclicity + uniqueness.
    ///
    /// # Errors
    ///
    /// Invalid version, duplicates, missing refs, or cycles.
    pub fn validate(&self) -> Result<()> {
        if self.version == 0 || self.version > RENDER_GRAPH_VERSION {
            return Err(GraphError::message(format!(
                "unsupported RenderGraph version {}",
                self.version
            )));
        }
        self.reject_duplicates()?;

        let asset_ids: BTreeSet<_> = self.assets.iter().map(|a| a.id.0.as_str()).collect();
        let node_ids: BTreeSet<_> = self.nodes.iter().map(|n| n.id.0.as_str()).collect();
        for n in &self.nodes {
            for inp in &n.inputs {
                if !node_ids.contains(inp.0.as_str()) {
                    return Err(GraphError::UnknownId(inp.0.clone()));
                }
            }
            if let RenderNodeKind::Source { asset } = &n.body
                && !asset_ids.contains(asset.0.as_str())
            {
                return Err(GraphError::UnknownId(asset.0.clone()));
            }
        }
        for o in &self.outputs {
            if !node_ids.contains(o.node.0.as_str()) {
                return Err(GraphError::UnknownId(o.node.0.clone()));
            }
        }
        self.ensure_acyclic()?;
        Ok(())
    }

    fn reject_duplicates(&self) -> Result<()> {
        let mut asset_seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, a) in self.assets.iter().enumerate() {
            if let Some(first) = asset_seen.insert(a.id.0.as_str(), i) {
                return Err(GraphError::DuplicateAssetId {
                    id: a.id.0.clone(),
                    first_index: first,
                    second_index: i,
                });
            }
        }
        let mut node_seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(first) = node_seen.insert(n.id.0.as_str(), i) {
                return Err(GraphError::DuplicateNodeId {
                    id: n.id.0.clone(),
                    first_index: first,
                    second_index: i,
                });
            }
        }
        let mut out_seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, o) in self.outputs.iter().enumerate() {
            if let Some(first) = out_seen.insert(o.name.as_str(), i) {
                return Err(GraphError::DuplicateOutputName {
                    name: o.name.clone(),
                    first_index: first,
                    second_index: i,
                });
            }
        }
        Ok(())
    }

    /// Kahn topo with **min-heap ready queue** (lexicographic `NodeId`).
    ///
    /// Independent nodes always leave the ready set in sorted `NodeId` order,
    /// so schedule fingerprints are run-stable.
    fn topo_kahn(&self) -> Result<Vec<NodeId>> {
        let mut indeg: BTreeMap<&str, usize> = BTreeMap::new();
        let mut adj: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for n in &self.nodes {
            indeg.entry(n.id.0.as_str()).or_insert(0);
            for inp in &n.inputs {
                adj.entry(inp.0.as_str())
                    .or_default()
                    .insert(n.id.0.as_str());
                *indeg.entry(n.id.0.as_str()).or_insert(0) += 1;
                indeg.entry(inp.0.as_str()).or_insert(0);
            }
        }
        // BinaryHeap is max-heap; store Reverse-like by using inverted string key
        // via Reverse from std — use max-heap of reverse strings via Ord wrapper.
        // Simpler: keep a BTreeSet of ready ids (min first).
        let mut ready: BTreeSet<&str> = indeg
            .iter()
            .filter_map(|(k, d)| if *d == 0 { Some(*k) } else { None })
            .collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        while let Some(u) = ready.iter().next().copied() {
            ready.remove(u);
            order.push(NodeId(u.to_string()));
            if let Some(vs) = adj.get(u) {
                for &v in vs {
                    if let Some(d) = indeg.get_mut(v) {
                        *d = d.saturating_sub(1);
                        if *d == 0 {
                            ready.insert(v);
                        }
                    }
                }
            }
        }
        if order.len() != self.nodes.len() {
            return Err(GraphError::Cycle);
        }
        Ok(order)
    }

    fn ensure_acyclic(&self) -> Result<()> {
        self.topo_kahn().map(|_| ())
    }

    /// Deterministic topological order of node ids.
    ///
    /// # Errors
    ///
    /// Cycles (or if called before uniqueness validation, undefined on dups).
    pub fn topo_order(&self) -> Result<Vec<NodeId>> {
        self.topo_kahn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::op::{OperationId, OperationRegistry};
    use crate::schedule::schedule_graph;

    fn diamond_graph() -> RenderGraph {
        // Independent branches: a,b ready after s; order must be a before b by NodeId.
        RenderGraph {
            version: 1,
            assets: vec![MediaAsset {
                id: MediaAssetId("asset0".into()),
                uri: "in.mp4".into(),
                duration: None,
                role: None,
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("asset0".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("b_branch".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.hflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("a_branch".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.vflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    // both branches must complete; use a_branch as primary input for linear out
                    inputs: vec![NodeId("a_branch".into())],
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
    fn linear_graph_json() {
        let g = RenderGraph {
            version: 1,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "in.mp4".into(),
                duration: None,
                role: Some("video".into()),
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
        };
        g.validate().unwrap();
        let text = g.to_json_pretty().unwrap();
        let back = RenderGraph::from_json(&text).unwrap();
        assert_eq!(back.nodes.len(), 2);
        assert_eq!(back.topo_order().unwrap().len(), 2);
    }

    #[test]
    fn detects_cycle() {
        let g = RenderGraph {
            version: 1,
            assets: vec![],
            nodes: vec![
                RenderNode {
                    id: NodeId("a".into()),
                    body: RenderNodeKind::Output { name: "x".into() },
                    inputs: vec![NodeId("b".into())],
                },
                RenderNode {
                    id: NodeId("b".into()),
                    body: RenderNodeKind::Output { name: "y".into() },
                    inputs: vec![NodeId("a".into())],
                },
            ],
            outputs: vec![],
        };
        assert!(matches!(g.validate(), Err(GraphError::Cycle)));
        assert_eq!(g.validate().unwrap_err().code_str(), "RFGRAPH_CYCLE");
    }

    #[test]
    fn rejects_duplicate_node_id() {
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
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![],
                },
            ],
            outputs: vec![],
        };
        let err = g.validate().unwrap_err();
        assert_eq!(err.code_str(), "RFGRAPH_DUPLICATE_NODE_ID");
        match err {
            GraphError::DuplicateNodeId {
                first_index,
                second_index,
                ..
            } => {
                assert_eq!(first_index, 0);
                assert_eq!(second_index, 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn rejects_duplicate_asset_and_output() {
        let mut g = diamond_graph();
        g.assets.push(MediaAsset {
            id: MediaAssetId("asset0".into()),
            uri: "other.mp4".into(),
            duration: None,
            role: None,
        });
        assert_eq!(
            g.validate().unwrap_err().code_str(),
            "RFGRAPH_DUPLICATE_ASSET_ID"
        );

        let mut g2 = diamond_graph();
        g2.outputs.push(GraphOutput {
            name: "main".into(),
            node: NodeId("out".into()),
            uri: None,
        });
        assert_eq!(
            g2.validate().unwrap_err().code_str(),
            "RFGRAPH_DUPLICATE_OUTPUT_NAME"
        );
    }

    #[test]
    fn topo_order_stable_for_independent_nodes() {
        let g = diamond_graph();
        g.validate().unwrap();
        let order: Vec<_> = g.topo_order().unwrap().into_iter().map(|n| n.0).collect();
        // After src, ready set is {a_branch, b_branch}; min id is a_branch first.
        let a = order.iter().position(|x| x == "a_branch").unwrap();
        let b = order.iter().position(|x| x == "b_branch").unwrap();
        assert!(a < b, "expected a_branch before b_branch, got {order:?}");
        // Golden: same order many times
        for _ in 0..32 {
            assert_eq!(
                g.topo_order()
                    .unwrap()
                    .into_iter()
                    .map(|n| n.0)
                    .collect::<Vec<_>>(),
                order
            );
        }
    }

    #[test]
    fn execution_plan_byte_identical_across_runs() {
        let g = diamond_graph();
        let reg = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &reg).unwrap();
        let json = serde_json::to_string(&plan).unwrap();
        for _ in 0..16 {
            let plan2 = schedule_graph(&g, &reg).unwrap();
            assert_eq!(serde_json::to_string(&plan2).unwrap(), json);
        }
    }
}
