//! Preview planner: slice the DAG for one sample instead of full materialize.
//!
//! ```text
//! requested time + quality
//!   → keep output cone
//!   → drop encode / audio-only / inactive compose layers
//!   → proxy seeds
//!   → draft masks
//!   → materialize + sample
//! ```

use crate::error::{IoError, Result};
use crate::preview_contract::{PreviewQuality, PreviewRequest};
use reelforge_core::{MediaTime, VideoClip, VideoEffect};
use reelforge_fx::Resize;
use reelforge_render_graph::{
    MediaAssetId, NodeId, RENDER_GRAPH_VERSION, RenderGraph, RenderNode, RenderNodeKind,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::BuildHasher;
use std::sync::Arc;

/// Planned slice of a graph for one preview sample.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewPlan {
    /// Output node used as the root.
    pub output: NodeId,
    /// Nodes that will be materialized.
    pub keep_nodes: Vec<NodeId>,
    /// Nodes dropped (inactive layers, unused branches).
    pub skip_nodes: Vec<NodeId>,
    /// Identity-for-preview nodes (encode / audio) rewired around.
    pub bypass_nodes: Vec<NodeId>,
    /// Request that produced this plan.
    pub request: PreviewRequest,
}

impl PreviewPlan {
    /// Whether `id` is materialized.
    #[must_use]
    pub fn keeps(&self, id: &NodeId) -> bool {
        self.keep_nodes.iter().any(|k| k == id)
    }
}

/// Build a preview plan for `graph` at `request.time`.
///
/// # Errors
///
/// Empty graph or unknown output.
pub fn plan_preview(graph: &RenderGraph, request: PreviewRequest) -> Result<PreviewPlan> {
    let output = graph
        .outputs
        .first()
        .map(|o| o.node.clone())
        .or_else(|| {
            graph.nodes.iter().rev().find_map(|n| match &n.body {
                RenderNodeKind::Output { .. } => Some(n.id.clone()),
                _ => None,
            })
        })
        .ok_or_else(|| IoError::message("preview plan: graph has no output"))?;

    let node_map: BTreeMap<&str, &RenderNode> =
        graph.nodes.iter().map(|n| (n.id.0.as_str(), n)).collect();
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let mut bypass: BTreeMap<String, String> = BTreeMap::new();
    let mut keep_inputs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    visit(
        output.0.as_str(),
        request.time,
        &node_map,
        &mut keep,
        &mut bypass,
        &mut keep_inputs,
    )?;

    let mut skip_nodes = Vec::new();
    let mut bypass_nodes = Vec::new();
    for n in &graph.nodes {
        if keep.contains(n.id.0.as_str()) {
            continue;
        }
        if bypass.contains_key(n.id.0.as_str()) {
            bypass_nodes.push(n.id.clone());
        } else {
            skip_nodes.push(n.id.clone());
        }
    }
    let keep_nodes = graph
        .nodes
        .iter()
        .filter(|n| keep.contains(n.id.0.as_str()))
        .map(|n| n.id.clone())
        .collect();
    Ok(PreviewPlan {
        output,
        keep_nodes,
        skip_nodes,
        bypass_nodes,
        request,
    })
}

/// Apply [`PreviewPlan`] to produce a smaller graph (rewired, draft masks).
#[must_use]
pub fn slice_preview_graph(graph: &RenderGraph, plan: &PreviewPlan) -> RenderGraph {
    let node_map: BTreeMap<&str, &RenderNode> =
        graph.nodes.iter().map(|n| (n.id.0.as_str(), n)).collect();
    let mut bypass: BTreeMap<String, String> = BTreeMap::new();
    let mut keep_inputs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut keep: BTreeSet<String> = BTreeSet::new();
    let _ = visit(
        plan.output.0.as_str(),
        plan.request.time,
        &node_map,
        &mut keep,
        &mut bypass,
        &mut keep_inputs,
    );

    let resolve = |id: &str| -> String {
        let mut cur = id.to_string();
        let mut guard = 0_u32;
        while let Some(next) = bypass.get(&cur) {
            guard += 1;
            if guard > 64 || next == &cur {
                break;
            }
            cur = next.clone();
        }
        cur
    };

    let mut nodes = Vec::new();
    let mut used_assets: BTreeSet<String> = BTreeSet::new();
    let draft = matches!(plan.request.spec.quality, PreviewQuality::Draft);
    for n in &graph.nodes {
        if !keep.contains(n.id.0.as_str()) {
            continue;
        }
        let idxs = keep_inputs
            .get(n.id.0.as_str())
            .cloned()
            .unwrap_or_else(|| (0..n.inputs.len()).collect());
        let inputs: Vec<NodeId> = idxs
            .iter()
            .filter_map(|&i| n.inputs.get(i))
            .map(|id| NodeId(resolve(&id.0)))
            .collect();
        let mut body = n.body.clone();
        rewrite_body(&mut body, &idxs, draft);
        if let RenderNodeKind::Source { asset } = &body {
            used_assets.insert(asset.0.clone());
        }
        nodes.push(RenderNode {
            id: n.id.clone(),
            body,
            inputs,
        });
    }

    let assets = graph
        .assets
        .iter()
        .filter(|a| used_assets.contains(&a.id.0))
        .cloned()
        .collect();
    let outputs = graph
        .outputs
        .iter()
        .filter(|o| keep.contains(o.node.0.as_str()) || bypass.contains_key(o.node.0.as_str()))
        .map(|o| {
            let mut out = o.clone();
            out.node = NodeId(resolve(&o.node.0));
            out
        })
        .collect();

    RenderGraph {
        version: RENDER_GRAPH_VERSION,
        assets,
        nodes,
        outputs,
    }
}

pub(crate) fn proxy_preview_seeds<S: BuildHasher>(
    seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    request: PreviewRequest,
) -> HashMap<MediaAssetId, Arc<dyn VideoClip>> {
    let mut out = HashMap::new();
    for (id, clip) in seeds {
        let target = request.spec.output_size(clip.size());
        let wrapped = if request.spec.quality == PreviewQuality::Full || target == clip.size() {
            Arc::clone(clip)
        } else {
            Resize::to(target)
                .apply(Arc::clone(clip))
                .unwrap_or_else(|_| Arc::clone(clip))
        };
        out.insert(id.clone(), wrapped);
    }
    out
}

fn visit(
    id: &str,
    t: MediaTime,
    nodes: &BTreeMap<&str, &RenderNode>,
    keep: &mut BTreeSet<String>,
    bypass: &mut BTreeMap<String, String>,
    keep_inputs: &mut BTreeMap<String, Vec<usize>>,
) -> Result<()> {
    if keep.contains(id) || bypass.contains_key(id) {
        return Ok(());
    }
    let node = nodes
        .get(id)
        .ok_or_else(|| IoError::message(format!("preview plan: unknown node {id}")))?;
    if is_bypass_op(node) {
        if let Some(inp) = node.inputs.first() {
            visit(inp.0.as_str(), t, nodes, keep, bypass, keep_inputs)?;
            let resolved = bypass.get(&inp.0).cloned().unwrap_or_else(|| inp.0.clone());
            bypass.insert(id.to_string(), resolved);
        }
        return Ok(());
    }
    keep.insert(id.to_string());
    let mut idxs = Vec::new();
    for (i, inp) in node.inputs.iter().enumerate() {
        if skip_input_at(node, i, t) {
            continue;
        }
        idxs.push(i);
        visit(inp.0.as_str(), t, nodes, keep, bypass, keep_inputs)?;
    }
    if idxs.is_empty() && !node.inputs.is_empty() {
        idxs.push(0);
        visit(
            node.inputs[0].0.as_str(),
            t,
            nodes,
            keep,
            bypass,
            keep_inputs,
        )?;
    }
    keep_inputs.insert(id.to_string(), idxs);
    Ok(())
}

fn is_bypass_op(node: &RenderNode) -> bool {
    match &node.body {
        RenderNodeKind::Op { operation, .. } => matches!(
            operation.as_str(),
            "rf.encode.h264"
                | "rf.encode.hw"
                | "rf.gpu.passthrough"
                | "rf.audio.drop"
                | "rf.audio.preserve"
                | "rf.audio.gain"
        ),
        _ => false,
    }
}

fn skip_input_at(node: &RenderNode, index: usize, t: MediaTime) -> bool {
    let RenderNodeKind::Op { operation, params } = &node.body else {
        return false;
    };
    match operation.as_str() {
        "rf.compose.layers" => layer_starts_after(params, index, t),
        "rf.audio.mix" => index > 0,
        _ => false,
    }
}

fn layer_starts_after(params: &serde_json::Value, index: usize, t: MediaTime) -> bool {
    let Some(layer) = params.get("layers").and_then(|l| l.get(index)) else {
        return false;
    };
    let Some(start) = layer.get("start").and_then(json_as_media_time) else {
        return false;
    };
    media_after(start, t)
}

fn media_after(a: MediaTime, b: MediaTime) -> bool {
    let Ok(a) = a.rebase(b.timescale.max(1)) else {
        return a.as_secs() > b.as_secs();
    };
    a.ticks > b.ticks
}

fn json_as_media_time(v: &serde_json::Value) -> Option<MediaTime> {
    if let Some(s) = v.as_f64() {
        return MediaTime::from_secs(s, MediaTime::HZ_1K).ok();
    }
    let ticks = v.get("ticks")?.as_i64()?;
    let ts = u32::try_from(v.get("timescale")?.as_u64()?).ok()?;
    MediaTime::new(ticks, ts).ok()
}

fn rewrite_body(body: &mut RenderNodeKind, idxs: &[usize], draft: bool) {
    match body {
        RenderNodeKind::Op { operation, params } if operation.as_str() == "rf.compose.layers" => {
            if let Some(layers) = params.get("layers").and_then(serde_json::Value::as_array) {
                let kept: Vec<serde_json::Value> = idxs
                    .iter()
                    .filter_map(|&i| layers.get(i).cloned())
                    .collect();
                params["layers"] = serde_json::Value::Array(kept);
            }
        }
        RenderNodeKind::Redaction { redaction } if draft => {
            redaction.masks = draft_masks(&redaction.masks);
        }
        _ => {}
    }
}

fn draft_masks(
    masks: &reelforge_render_graph::MaskTimeline,
) -> reelforge_render_graph::MaskTimeline {
    let mut out = reelforge_render_graph::MaskTimeline::new();
    out.interpolation = masks.interpolation;
    out.missing_policy = masks.missing_policy;
    for sample in &masks.samples {
        let mut s = sample.clone();
        s.asset = None;
        out.samples.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_contract::PreviewQuality;
    use reelforge_render_graph::{
        GraphOutput, MediaAsset, RENDER_GRAPH_VERSION, RenderNode, RenderNodeKind,
    };

    fn compose_graph() -> RenderGraph {
        RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![
                MediaAsset {
                    id: MediaAssetId("a".into()),
                    uri: "seed://a".into(),
                    duration: None,
                    role: Some("video".into()),
                },
                MediaAsset {
                    id: MediaAssetId("b".into()),
                    uri: "seed://b".into(),
                    duration: None,
                    role: Some("video".into()),
                },
            ],
            nodes: vec![
                RenderNode {
                    id: NodeId("src_a".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("a".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("src_b".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("b".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("comp".into()),
                    body: RenderNodeKind::Op {
                        operation: reelforge_render_graph::OperationId::new("rf.compose.layers"),
                        params: serde_json::json!({
                            "layers": [
                                { "start": { "ticks": 0, "timescale": 1000 } },
                                { "start": { "ticks": 5000, "timescale": 1000 } }
                            ]
                        }),
                    },
                    inputs: vec![NodeId("src_a".into()), NodeId("src_b".into())],
                },
                RenderNode {
                    id: NodeId("enc".into()),
                    body: RenderNodeKind::Op {
                        operation: reelforge_render_graph::OperationId::new("rf.encode.h264"),
                        params: serde_json::json!({ "path": "out.mp4" }),
                    },
                    inputs: vec![NodeId("comp".into())],
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
        }
    }

    #[test]
    fn skips_future_layer_and_bypasses_encode() {
        let g = compose_graph();
        let plan = plan_preview(
            &g,
            PreviewRequest::at(MediaTime::zero(1_000), PreviewQuality::Draft),
        )
        .unwrap();
        assert!(plan.keeps(&NodeId("src_a".into())));
        assert!(!plan.keeps(&NodeId("src_b".into())));
        assert!(plan.bypass_nodes.iter().any(|n| n.0 == "enc"));
        let sliced = slice_preview_graph(&g, &plan);
        assert_eq!(sliced.assets.len(), 1);
        assert!(sliced.nodes.iter().all(|n| n.id.0 != "enc"));
        assert_eq!(sliced.outputs[0].node.0, "out");
        let out = sliced.nodes.iter().find(|n| n.id.0 == "out").expect("out");
        assert_eq!(out.inputs, vec![NodeId("comp".into())]);
        let comp = sliced
            .nodes
            .iter()
            .find(|n| n.id.0 == "comp")
            .expect("comp");
        assert_eq!(comp.inputs.len(), 1);
        assert_eq!(comp.inputs[0].0, "src_a");
    }

    #[test]
    fn later_time_keeps_both_layers() {
        let g = compose_graph();
        let t = MediaTime::from_secs(6.0, 1_000).unwrap();
        let plan = plan_preview(&g, PreviewRequest::at(t, PreviewQuality::Proxy)).unwrap();
        assert!(plan.keeps(&NodeId("src_a".into())));
        assert!(plan.keeps(&NodeId("src_b".into())));
    }

    #[test]
    fn draft_strips_mask_assets() {
        use reelforge_render_graph::{
            MaskAsset, MaskAssetRef, MaskSample, MaskTimeline, RedactionStyle, RegionRedaction,
        };
        let mut masks = MaskTimeline::new();
        let mut sample = MaskSample::ellipse(MediaTime::zero(1_000), 8.0, 8.0, 4.0);
        sample.asset = Some(MaskAssetRef::inline(MaskAsset::Dense {
            width: 2,
            height: 2,
            data: vec![255, 255, 255, 255],
        }));
        masks.push(sample);
        let g = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "seed://a".into(),
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
                    id: NodeId("red".into()),
                    body: RenderNodeKind::Redaction {
                        redaction: RegionRedaction {
                            masks,
                            style: RedactionStyle::default(),
                        },
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("red".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: None,
            }],
        };
        let plan = plan_preview(
            &g,
            PreviewRequest::at(MediaTime::zero(1_000), PreviewQuality::Draft),
        )
        .unwrap();
        let sliced = slice_preview_graph(&g, &plan);
        let red = sliced
            .nodes
            .iter()
            .find_map(|n| match &n.body {
                RenderNodeKind::Redaction { redaction } => Some(redaction),
                _ => None,
            })
            .expect("redaction");
        assert!(red.masks.samples[0].asset.is_none());
    }
}
