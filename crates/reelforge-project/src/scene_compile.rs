//! Compile an existing [`SceneDocument`] into a posed `RenderGraph`.
//!
//! The document has no camera, so placement stays in canvas pixels. Groups are
//! checked by [`SceneDocument::validate`] and are not drawn. A coverage mask is
//! refused here instead of being painted as an unmasked picture.

use crate::emit_clip::media_time_json;
use crate::error::{ProjectError, Result};
use crate::ids::{MediaRefId, PartId, SceneId};
use crate::scene::{MotionProperty, PartAnchor, SceneDocument, ScenePoint, motion_curve};
use reelforge_core::MediaTime;
use reelforge_render_graph::{
    Animated, GraphOutput, MediaAsset, MediaAssetId, NodeId, OperationId, OperationRegistry,
    RENDER_GRAPH_VERSION, RenderGraph, RenderNode, RenderNodeKind, compile_graph,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// One library entry the scene may reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneMedia {
    /// Library id named by a part.
    pub id: MediaRefId,
    /// Host-resolved URI. Compilation does not open it.
    pub uri: String,
}

/// Canvas the scene is placed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneCanvas {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How long the posed composite stays open.
    pub duration: MediaTime,
}

/// One visible part after validation, in draw order.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPart {
    /// Part id.
    pub id: PartId,
    /// Picture media. A replacement wins over the original source.
    pub media: MediaRefId,
    /// Hierarchy parent.
    pub parent: Option<PartId>,
    /// Contact binding. Not set together with [`Self::parent`].
    pub anchor: Option<PartAnchor>,
    /// Local pivot in source pixels.
    pub pivot: ScenePoint,
    /// Draw order.
    pub order: i32,
    /// Local translation.
    pub x: Animated<f32>,
    /// Local translation.
    pub y: Animated<f32>,
    /// Clockwise degrees.
    pub rotation: Animated<f32>,
    /// Uniform scale about the pivot.
    pub scale: Animated<f32>,
    /// Opacity from 0 to 1.
    pub opacity: Animated<f32>,
}

/// Scene links resolved to sampled channels.
///
/// There is no second scene document here. Hidden parts stay in the source
/// document and are absent from this pose.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedScene {
    /// Document id.
    pub id: SceneId,
    /// Canvas duration. This is not a nominal frame grid.
    pub duration: MediaTime,
    /// Visible parts, low draw order first.
    pub parts: Vec<ResolvedPart>,
}

/// Executable scene: resolved pose plus the graph that samples it.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneCompile {
    /// Channels and links.
    pub resolved: ResolvedScene,
    /// `rf.compose.layers` graph. Layer objects carry parent, pivot, and rotation.
    pub graph: RenderGraph,
}

/// Validate `scene` and compile its visible parts.
///
/// # Errors
///
/// The document is invalid, the canvas is unusable, a visible part names media
/// that was not supplied, or a part still carries a coverage mask.
pub fn compile_scene(
    scene: &SceneDocument,
    media: &[SceneMedia],
    canvas: SceneCanvas,
) -> Result<SceneCompile> {
    scene.validate()?;
    if canvas.width == 0 || canvas.height == 0 {
        return Err(ProjectError::message("scene canvas must be positive"));
    }
    if canvas.duration.ticks <= 0 || !canvas.duration.as_secs().is_finite() {
        return Err(ProjectError::message(
            "scene duration must be finite and positive",
        ));
    }
    let library = index_media(media)?;
    let resolved = resolve(scene, &library, canvas.duration)?;
    let graph = emit_graph(&resolved, &library, canvas)?;
    compile_graph(&graph, &OperationRegistry::with_builtins())
        .map_err(|err| ProjectError::Graph(err.to_string()))?;
    Ok(SceneCompile { resolved, graph })
}

fn index_media(media: &[SceneMedia]) -> Result<BTreeMap<&str, &str>> {
    let mut library = BTreeMap::new();
    for item in media {
        if item.id.as_str().is_empty() {
            return Err(ProjectError::message("scene media id is empty"));
        }
        if item.uri.is_empty() {
            return Err(ProjectError::message(format!(
                "scene media {} uri is empty",
                item.id.as_str()
            )));
        }
        if library
            .insert(item.id.as_str(), item.uri.as_str())
            .is_some()
        {
            return Err(ProjectError::message(format!(
                "scene media {} is listed twice",
                item.id.as_str()
            )));
        }
    }
    Ok(library)
}

fn resolve(
    scene: &SceneDocument,
    library: &BTreeMap<&str, &str>,
    duration: MediaTime,
) -> Result<ResolvedScene> {
    let mut visible: Vec<(usize, &crate::scene::ScenePart)> = scene
        .parts
        .iter()
        .enumerate()
        .filter(|(_, part)| !part.hidden)
        .collect();
    if visible.is_empty() {
        return Err(ProjectError::message("scene has no visible parts"));
    }
    visible.sort_by_key(|(index, part)| (part.order, *index));
    let mut parts = Vec::with_capacity(visible.len());
    for (_, part) in visible {
        let source = part.effective_source();
        if let Some(mask) = &source.mask {
            return Err(ProjectError::message(format!(
                "scene part {} has mask {mask}; coverage is not compiled",
                part.id.as_str()
            )));
        }
        if !library.contains_key(source.media.as_str()) {
            return Err(ProjectError::message(format!(
                "scene part {} needs media {}",
                part.id.as_str(),
                source.media.as_str()
            )));
        }
        parts.push(ResolvedPart {
            id: part.id.clone(),
            media: source.media.clone(),
            parent: part.parent.clone(),
            anchor: part.anchor.clone(),
            pivot: part.pivot,
            order: part.order,
            x: motion_curve(scene, &part.id, MotionProperty::PositionX, 0.0),
            y: motion_curve(scene, &part.id, MotionProperty::PositionY, 0.0),
            rotation: motion_curve(scene, &part.id, MotionProperty::Rotation, 0.0),
            scale: motion_curve(scene, &part.id, MotionProperty::Scale, 1.0),
            opacity: motion_curve(scene, &part.id, MotionProperty::Opacity, 1.0),
        });
    }
    let visible_ids: BTreeSet<&str> = parts.iter().map(|part| part.id.as_str()).collect();
    for part in &parts {
        if let Some(parent) = &part.parent
            && !visible_ids.contains(parent.as_str())
        {
            return Err(ProjectError::message(format!(
                "scene part {} parent {} is not a visible part",
                part.id.as_str(),
                parent.as_str()
            )));
        }
        if let Some(anchor) = &part.anchor
            && !visible_ids.contains(anchor.target.as_str())
        {
            return Err(ProjectError::message(format!(
                "scene part {} anchor {} is not a visible part",
                part.id.as_str(),
                anchor.target.as_str()
            )));
        }
    }
    Ok(ResolvedScene {
        id: scene.id.clone(),
        duration,
        parts,
    })
}

fn emit_graph(
    scene: &ResolvedScene,
    library: &BTreeMap<&str, &str>,
    canvas: SceneCanvas,
) -> Result<RenderGraph> {
    let mut assets = BTreeMap::<String, MediaAsset>::new();
    let mut nodes = Vec::new();
    let mut inputs = Vec::new();
    let mut layers = Vec::new();
    for part in &scene.parts {
        let uri = library.get(part.media.as_str()).copied().ok_or_else(|| {
            ProjectError::message(format!(
                "scene part {} needs media {}",
                part.id.as_str(),
                part.media.as_str()
            ))
        })?;
        let asset_key = format!("m_{}", part.media.as_str());
        assets.entry(asset_key.clone()).or_insert(MediaAsset {
            id: MediaAssetId(asset_key.clone()),
            uri: uri.to_string(),
            duration: Some(canvas.duration),
            role: Some("video".into()),
        });
        let source = NodeId(format!("n_src_{}", part.id.as_str()));
        nodes.push(RenderNode {
            id: source.clone(),
            body: RenderNodeKind::Source {
                asset: MediaAssetId(asset_key),
            },
            inputs: Vec::new(),
        });
        inputs.push(source);
        layers.push(layer_json(part)?);
    }
    let scene_node = NodeId("n_scene".into());
    nodes.push(RenderNode {
        id: scene_node.clone(),
        body: RenderNodeKind::Op {
            operation: OperationId::new("rf.compose.layers"),
            params: json!({
                "w": canvas.width,
                "h": canvas.height,
                "duration": media_time_json(canvas.duration),
                "layers": layers,
            }),
        },
        inputs,
    });
    let out = NodeId("n_out".into());
    nodes.push(RenderNode {
        id: out.clone(),
        body: RenderNodeKind::Output {
            name: "main".into(),
        },
        inputs: vec![scene_node],
    });
    Ok(RenderGraph {
        version: RENDER_GRAPH_VERSION,
        assets: assets.into_values().collect(),
        nodes,
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: out,
            uri: None,
        }],
    })
}

fn layer_json(part: &ResolvedPart) -> Result<Value> {
    let mut layer = Map::new();
    layer.insert("id".into(), json!(part.id.as_str()));
    layer.insert(
        "pivot".into(),
        json!({ "x": part.pivot.x, "y": part.pivot.y }),
    );
    layer.insert("x".into(), curve_json(&part.x)?);
    layer.insert("y".into(), curve_json(&part.y)?);
    layer.insert("rotation".into(), curve_json(&part.rotation)?);
    layer.insert("scale".into(), curve_json(&part.scale)?);
    layer.insert("opacity".into(), curve_json(&part.opacity)?);
    layer.insert("layer_index".into(), json!(part.order));
    if let Some(parent) = &part.parent {
        layer.insert("parent".into(), json!(parent.as_str()));
    }
    if let Some(anchor) = &part.anchor {
        layer.insert(
            "anchor".into(),
            json!({
                "target": anchor.target.as_str(),
                "x": anchor.point.x,
                "y": anchor.point.y,
            }),
        );
    }
    Ok(Value::Object(layer))
}

fn curve_json(curve: &Animated<f32>) -> Result<Value> {
    match curve {
        Animated::Constant { value } => Ok(json!(value)),
        Animated::Keyframes { .. } => serde_json::to_value(curve).map_err(|err| {
            ProjectError::message(format!("scene motion could not be written: {err}"))
        }),
    }
}
