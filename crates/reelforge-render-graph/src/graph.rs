//! `RenderGraph` DAG (executable media model).

use crate::error::{GraphError, Result};
use crate::op::OperationId;
use crate::redaction::RegionRedaction;
use crate::RENDER_GRAPH_VERSION;
use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

/// Asset identifier within a graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MediaAssetId(pub String);

/// Node identifier within a graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
        /// Parameters (schema-validated later).
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
    /// Serde / version.
    pub fn from_json(text: &str) -> Result<Self> {
        let g: Self =
            serde_json::from_str(text).map_err(|e| GraphError::message(e.to_string()))?;
        g.validate()?;
        Ok(g)
    }

    /// Structural validation + DAG acyclicity.
    ///
    /// # Errors
    ///
    /// Invalid version, missing refs, or cycles.
    pub fn validate(&self) -> Result<()> {
        if self.version == 0 || self.version > RENDER_GRAPH_VERSION {
            return Err(GraphError::message(format!(
                "unsupported RenderGraph version {}",
                self.version
            )));
        }
        let asset_ids: HashSet<_> = self.assets.iter().map(|a| a.id.0.as_str()).collect();
        let node_ids: HashSet<_> = self.nodes.iter().map(|n| n.id.0.as_str()).collect();
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

    fn ensure_acyclic(&self) -> Result<()> {
        let mut indeg: HashMap<&str, usize> = HashMap::new();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for n in &self.nodes {
            indeg.entry(n.id.0.as_str()).or_insert(0);
            for inp in &n.inputs {
                adj.entry(inp.0.as_str()).or_default().push(n.id.0.as_str());
                *indeg.entry(n.id.0.as_str()).or_insert(0) += 1;
                indeg.entry(inp.0.as_str()).or_insert(0);
            }
        }
        let mut q: VecDeque<&str> = indeg
            .iter()
            .filter_map(|(k, d)| if *d == 0 { Some(*k) } else { None })
            .collect();
        let mut seen = 0_usize;
        while let Some(u) = q.pop_front() {
            seen += 1;
            if let Some(vs) = adj.get(u) {
                for v in vs {
                    if let Some(d) = indeg.get_mut(v) {
                        *d = d.saturating_sub(1);
                        if *d == 0 {
                            q.push_back(v);
                        }
                    }
                }
            }
        }
        if seen < self.nodes.len() {
            return Err(GraphError::Cycle);
        }
        Ok(())
    }

    /// Topological order of node ids.
    ///
    /// # Errors
    ///
    /// Cycles.
    pub fn topo_order(&self) -> Result<Vec<NodeId>> {
        self.ensure_acyclic()?;
        let mut indeg: HashMap<String, usize> = HashMap::new();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for n in &self.nodes {
            indeg.entry(n.id.0.clone()).or_insert(0);
            for inp in &n.inputs {
                adj.entry(inp.0.clone())
                    .or_default()
                    .push(n.id.0.clone());
                *indeg.entry(n.id.0.clone()).or_insert(0) += 1;
                indeg.entry(inp.0.clone()).or_insert(0);
            }
        }
        let mut q: VecDeque<String> = indeg
            .iter()
            .filter_map(|(k, d)| if *d == 0 { Some(k.clone()) } else { None })
            .collect();
        let mut order = Vec::new();
        while let Some(u) = q.pop_front() {
            order.push(NodeId(u.clone()));
            if let Some(vs) = adj.get(&u) {
                for v in vs {
                    if let Some(d) = indeg.get_mut(v) {
                        *d = d.saturating_sub(1);
                        if *d == 0 {
                            q.push_back(v.clone());
                        }
                    }
                }
            }
        }
        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    body: RenderNodeKind::Output {
                        name: "x".into(),
                    },
                    inputs: vec![NodeId("b".into())],
                },
                RenderNode {
                    id: NodeId("b".into()),
                    body: RenderNodeKind::Output {
                        name: "y".into(),
                    },
                    inputs: vec![NodeId("a".into())],
                },
            ],
            outputs: vec![],
        };
        assert!(matches!(g.validate(), Err(GraphError::Cycle)));
    }
}
