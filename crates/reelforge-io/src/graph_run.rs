//! Execute [`RenderGraph`] / [`ExecutionPlan`] (M3 hybrid runner).
//!
//! ```text
//! RenderGraph ──schedule──► ExecutionPlan ──run──► outputs on disk
//! ```
//!
//! Linear DAGs and multi-input `rf.compose.layers` are supported. Adapter /
//! GPU stages fail clearly until host adapters land. `FFmpeg` stages that only
//! carry encode/output markers finalize via Rust pixel encode (`write_video`);
//! geometry/`trim` prefixes use host filtergraph when a later stage needs Rust.

use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::error::{IoError, Result};
use crate::filtergraph::{FilterGraph, FilterOp};
use crate::mask_bridge::{apply_region_redaction, region_redaction_from_value};
use crate::options::{OpenVideoOptions, WriteVideoOptions};
use crate::stage_cache::StageCache;
use crate::video_file::open_video;
use crate::{run_filtergraph, write_av_with, write_video_with};
use reelforge_compose::{CompositeLayer, composite_video, composite_video_with_background};
use reelforge_core::{
    AudioClip, AudioEffect, Duration, MediaTime, Position, Rgb8, Size, Time, VideoClip,
    VideoEffect, subclip_audio, subclip_video,
};
use reelforge_fx::{
    BlackAndWhite, Crop, EvenSize, FadeIn, FadeOut, InvertColors, MirrorX, MirrorY, Painting,
    Resize, Rotate, VolumeGain,
};
use reelforge_render_graph::{
    BackendClass, ExecutionPlan, ExecutionStage, MediaAssetId, NodeId, OperationId,
    OperationRegistry, RENDER_GRAPH_VERSION, RenderGraph, RenderNode, RenderNodeKind,
    schedule_graph,
};
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Encode / output hints collected while walking the graph.
#[derive(Debug, Clone, Default)]
pub struct GraphEncodeHints {
    /// Output FPS override.
    pub fps: Option<f64>,
    /// Video codec (e.g. `libx264`).
    pub video_codec: Option<String>,
    /// CRF when using x264-style encodes.
    pub crf: Option<u8>,
    /// Primary output path (from first `GraphOutput.uri` or encode params).
    pub output_path: Option<String>,
    /// Mux companion audio when present (`true` by default once audio attaches).
    pub preserve_audio: bool,
}

/// Materialized video (+ optional audio) from a [`RenderGraph`].
#[derive(Clone)]
pub struct GraphBundle {
    /// Video stream.
    pub video: Arc<dyn VideoClip>,
    /// Optional audio (source companion or graph audio ops).
    pub audio: Option<Arc<dyn AudioClip>>,
    /// Encode / output hints.
    pub hints: GraphEncodeHints,
}

/// One node product while walking the DAG.
#[derive(Clone)]
struct NodeMedia {
    video: Arc<dyn VideoClip>,
    audio: Option<Arc<dyn AudioClip>>,
}

/// Options for [`run_render_graph_with`].
#[derive(Debug, Clone)]
pub struct GraphRunOptions {
    /// Operation registry (builtins by default).
    pub registry: OperationRegistry,
    /// Override encode FPS.
    pub fps: Option<f64>,
    /// Override video codec.
    pub video_codec: Option<String>,
    /// Override CRF.
    pub crf: Option<u8>,
    /// Optional full-run stage cache (fingerprint → artifact).
    pub cache: Option<StageCache>,
    /// Open source files with audio and mux when present (default `true`).
    ///
    /// When `true`, pure in-process materialize is preferred over hybrid
    /// `FFmpeg` prefixes so companion audio stays aligned.
    pub with_audio: bool,
}

impl Default for GraphRunOptions {
    fn default() -> Self {
        Self {
            registry: OperationRegistry::with_builtins(),
            fps: None,
            video_codec: None,
            crf: None,
            cache: None,
            with_audio: true,
        }
    }
}

impl GraphRunOptions {
    /// Default builtins registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace registry.
    #[must_use]
    pub fn with_registry(mut self, registry: OperationRegistry) -> Self {
        self.registry = registry;
        self
    }

    /// Enable directory stage cache.
    #[must_use]
    pub fn with_cache(mut self, cache: StageCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Prefer video-only hybrid prefixes (drops companion audio path).
    #[must_use]
    pub fn video_only(mut self) -> Self {
        self.with_audio = false;
        self
    }
}

/// Schedule + human-readable routing without writing files.
///
/// # Errors
///
/// Invalid graph or unknown operations.
pub fn explain_render_graph(graph: &RenderGraph) -> Result<String> {
    explain_render_graph_with(graph, &OperationRegistry::with_builtins())
}

/// Like [`explain_render_graph`] with a custom registry.
///
/// # Errors
///
/// Invalid graph or unknown operations.
pub fn explain_render_graph_with(
    graph: &RenderGraph,
    registry: &OperationRegistry,
) -> Result<String> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    let plan = schedule_graph(graph, registry).map_err(|e| IoError::message(e.to_string()))?;
    let mut lines = Vec::new();
    lines.push(format!(
        "render_graph version={} assets={} nodes={} outputs={}",
        graph.version,
        graph.assets.len(),
        graph.nodes.len(),
        graph.outputs.len()
    ));
    lines.push(format!("execution_stages: {}", plan.stages.len()));
    if let Some(notes) = &plan.notes {
        lines.push(format!("notes: {notes}"));
    }
    for (i, stage) in plan.stages.iter().enumerate() {
        lines.push(format!("  [{i}] {}", stage_summary(stage)));
    }
    for o in &graph.outputs {
        lines.push(format!(
            "output: name={} node={} uri={}",
            o.name,
            o.node.0,
            o.uri.as_deref().unwrap_or("<unset>")
        ));
    }
    Ok(lines.join("\n"))
}

fn stage_summary(stage: &ExecutionStage) -> String {
    match stage {
        ExecutionStage::Ffmpeg(s) => format!(
            "ffmpeg nodes=[{}]",
            s.nodes
                .iter()
                .map(|n| n.0.as_str())
                .collect::<Vec<_>>()
                .join(",")
        ),
        ExecutionStage::Rust(s) => format!(
            "rust nodes=[{}] ops=[{}]",
            s.nodes
                .iter()
                .map(|n| n.0.as_str())
                .collect::<Vec<_>>()
                .join(","),
            s.operations
                .iter()
                .map(OperationId::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ),
        ExecutionStage::Adapter(s) => format!("adapter={} nodes={}", s.adapter, s.nodes.len()),
        ExecutionStage::Gpu(s) => format!("gpu nodes={}", s.nodes.len()),
    }
}

/// Run a graph: schedule → hybrid materialize → write outputs.
///
/// # Errors
///
/// Validation, missing sources/outputs, unsupported stages, I/O, encode.
pub fn run_render_graph(graph: &RenderGraph) -> Result<()> {
    run_render_graph_with(graph, &WriteControl::default(), &GraphRunOptions::default())
}

/// Run a graph with control + options.
///
/// # Errors
///
/// Same as [`run_render_graph`], plus cancel.
pub fn run_render_graph_with(
    graph: &RenderGraph,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<()> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    if graph.version == 0 || graph.version > RENDER_GRAPH_VERSION {
        return Err(IoError::message(format!(
            "unsupported RenderGraph version {}",
            graph.version
        )));
    }
    if graph.outputs.is_empty() {
        return Err(IoError::message("RenderGraph has no outputs"));
    }
    let plan =
        schedule_graph(graph, &options.registry).map_err(|e| IoError::message(e.to_string()))?;
    run_execution_plan_with(graph, &plan, control, options)
}

/// Execute a pre-built [`ExecutionPlan`] against its source graph.
///
/// # Errors
///
/// Unsupported stages, missing assets, decode/encode failures.
pub fn run_execution_plan(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    control: &WriteControl,
) -> Result<()> {
    run_execution_plan_with(graph, plan, control, &GraphRunOptions::default())
}

/// Execute plan with options.
///
/// # Errors
///
/// Same as [`run_execution_plan`].
pub fn run_execution_plan_with(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<()> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    control.check_cancel()?;

    // Reject adapter / GPU until host support lands.
    for stage in &plan.stages {
        match stage {
            ExecutionStage::Adapter(s) => {
                return Err(IoError::message(format!(
                    "adapter stage '{}' not implemented in M3 runner",
                    s.adapter
                )));
            }
            ExecutionStage::Gpu(_) => {
                return Err(IoError::message("GPU stages not implemented in M3 runner"));
            }
            ExecutionStage::Ffmpeg(_) | ExecutionStage::Rust(_) => {}
        }
    }

    let run_fp = options
        .cache
        .as_ref()
        .map(|_| StageCache::run_fingerprint(graph, plan))
        .transpose()?;

    if let (Some(cache), Some(fp)) = (&options.cache, &run_fp)
        && let Some(cached) = cache.hit(fp, "mp4")
    {
        restore_cached_outputs(graph, &cached)?;
        control.report(WriteProgress::new(WriteStage::Done, 1, 1));
        return Ok(());
    }

    // Hybrid FFmpeg prefix is video-only; skip when we want companion audio.
    if !options.with_audio
        && can_use_ffmpeg_prefix(graph, plan)
        && let Some(()) = try_hybrid_ffmpeg_prefix(graph, plan, control, options)?
    {
        if let (Some(cache), Some(fp)) = (&options.cache, &run_fp)
            && let Some(out) = resolve_output_path(graph)
        {
            let _ = cache.store_copy(fp, "mp4", out);
        }
        return Ok(());
    }

    let seeds = HashMap::new();
    let audio_seeds = HashMap::new();
    let mut bundle = materialize_graph_bundle(
        graph,
        &options.registry,
        &seeds,
        &audio_seeds,
        options.with_audio,
    )?;
    merge_option_hints(&mut bundle.hints, options);
    write_graph_outputs(
        graph,
        bundle.video.as_ref(),
        bundle.audio.as_deref(),
        &bundle.hints,
        control,
    )?;
    if let (Some(cache), Some(fp)) = (&options.cache, &run_fp)
        && let Some(out) = resolve_output_path(graph).or(bundle.hints.output_path.clone())
    {
        let _ = cache.store_copy(fp, "mp4", out);
    }
    Ok(())
}

fn restore_cached_outputs(graph: &RenderGraph, cached: &Path) -> Result<()> {
    let paths: Vec<String> = graph
        .outputs
        .iter()
        .filter_map(|o| o.uri.clone())
        .collect();
    if paths.is_empty() {
        return Err(IoError::message(
            "cache hit but graph has no GraphOutput.uri to restore",
        ));
    }
    for path in paths {
        if let Some(parent) = Path::new(&path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| IoError::message(format!("cache restore mkdir: {e}")))?;
        }
        std::fs::copy(cached, &path)
            .map_err(|e| IoError::message(format!("cache restore copy: {e}")))?;
    }
    Ok(())
}

/// Materialize the primary output clip in-process (no encode).
///
/// Resolves file sources via [`open_video`]. For tests, prefer
/// [`materialize_graph_with_seeds`].
///
/// # Errors
///
/// Graph structure, unknown ops, open/decode failures.
pub fn materialize_graph(graph: &RenderGraph) -> Result<Arc<dyn VideoClip>> {
    let registry = OperationRegistry::with_builtins();
    let seeds = HashMap::new();
    Ok(materialize_graph_with_seeds(graph, &registry, &seeds)?.0)
}

/// Materialize with optional in-memory asset seeds (tests / preview hosts).
///
/// Seeds are keyed by [`MediaAssetId`] and bypass file open when present.
///
/// # Errors
///
/// Graph structure, unknown ops, open/decode failures.
pub fn materialize_graph_with_seeds<S: BuildHasher>(
    graph: &RenderGraph,
    registry: &OperationRegistry,
    seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
) -> Result<(Arc<dyn VideoClip>, GraphEncodeHints)> {
    let audio_seeds: HashMap<MediaAssetId, Arc<dyn AudioClip>> = HashMap::new();
    let bundle = materialize_graph_bundle(graph, registry, seeds, &audio_seeds, true)?;
    Ok((bundle.video, bundle.hints))
}

/// Full materialize: video + optional audio + encode hints.
///
/// # Errors
///
/// Graph structure, unknown ops, open/decode failures.
pub fn materialize_graph_bundle<S: BuildHasher, A: BuildHasher>(
    graph: &RenderGraph,
    registry: &OperationRegistry,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
) -> Result<GraphBundle> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    let order = graph
        .topo_order()
        .map_err(|e| IoError::message(e.to_string()))?;
    let node_map: HashMap<&str, &RenderNode> =
        graph.nodes.iter().map(|n| (n.id.0.as_str(), n)).collect();
    let asset_map: HashMap<&str, &reelforge_render_graph::MediaAsset> = graph
        .assets
        .iter()
        .map(|a| (a.id.0.as_str(), a))
        .collect();

    let mut produced: HashMap<String, NodeMedia> = HashMap::new();
    let mut hints = GraphEncodeHints {
        preserve_audio: with_audio,
        ..GraphEncodeHints::default()
    };
    let mut primary_out: Option<NodeMedia> = None;

    for id in order {
        let node = node_map
            .get(id.0.as_str())
            .ok_or_else(|| IoError::message(format!("missing node {}", id.0)))?;
        let media = match &node.body {
            RenderNodeKind::Source { asset } => {
                resolve_source(asset, &asset_map, video_seeds, audio_seeds, with_audio)?
            }
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.compose.layers" =>
            {
                let inputs = multi_input_media(node, &produced)?;
                let videos: Vec<_> = inputs.iter().map(|m| Arc::clone(&m.video)).collect();
                let audio = inputs.first().and_then(|m| m.audio.clone());
                let video = apply_compose_layers(videos, params, registry)?;
                NodeMedia { video, audio }
            }
            RenderNodeKind::Op { operation, params } => {
                let input = single_input_media(node, &produced)?;
                apply_registered_op_media(input, operation, params, registry, &mut hints)?
            }
            RenderNodeKind::Redaction { redaction } => {
                let input = single_input_media(node, &produced)?;
                let video = apply_region_redaction(input.video, redaction)?;
                NodeMedia {
                    video,
                    audio: input.audio,
                }
            }
            RenderNodeKind::Output { .. } => {
                let input = single_input_media(node, &produced)?;
                primary_out = Some(input.clone());
                input
            }
        };
        produced.insert(id.0.clone(), media);
    }

    if let Some(out) = graph.outputs.first() {
        if let Some(uri) = &out.uri {
            hints.output_path.get_or_insert_with(|| uri.clone());
        }
        if let Some(c) = produced.get(&out.node.0) {
            return Ok(GraphBundle {
                video: Arc::clone(&c.video),
                audio: c.audio.clone(),
                hints,
            });
        }
    }
    if let Some(c) = primary_out {
        return Ok(GraphBundle {
            video: c.video,
            audio: c.audio,
            hints,
        });
    }
    Err(IoError::message(
        "RenderGraph produced no output clip (missing Output node?)",
    ))
}

fn resolve_source<S: BuildHasher, A: BuildHasher>(
    asset: &MediaAssetId,
    asset_map: &HashMap<&str, &reelforge_render_graph::MediaAsset>,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
) -> Result<NodeMedia> {
    if let Some(clip) = video_seeds.get(asset) {
        return Ok(NodeMedia {
            video: Arc::clone(clip),
            audio: audio_seeds.get(asset).cloned(),
        });
    }
    let meta = asset_map
        .get(asset.0.as_str())
        .ok_or_else(|| IoError::message(format!("unknown asset {}", asset.0)))?;
    let path = Path::new(&meta.uri);
    if !path.is_file() {
        return Err(IoError::message(format!(
            "source asset {} not found: {}",
            asset.0, meta.uri
        )));
    }
    let mut opts = OpenVideoOptions::new(&meta.uri);
    if !with_audio {
        opts = opts.video_only();
    }
    let opened = open_video(&opts)?;
    let audio = opened
        .audio()
        .map(|a| Arc::new(a.clone()) as Arc<dyn AudioClip>);
    Ok(NodeMedia {
        video: Arc::new(opened),
        audio,
    })
}

fn single_input_media(
    node: &RenderNode,
    produced: &HashMap<String, NodeMedia>,
) -> Result<NodeMedia> {
    if node.inputs.len() != 1 {
        return Err(IoError::message(format!(
            "node {} requires exactly one input (got {})",
            node.id.0,
            node.inputs.len()
        )));
    }
    let up = &node.inputs[0];
    produced
        .get(&up.0)
        .cloned()
        .ok_or_else(|| IoError::message(format!("upstream {} not produced yet", up.0)))
}

fn multi_input_media(
    node: &RenderNode,
    produced: &HashMap<String, NodeMedia>,
) -> Result<Vec<NodeMedia>> {
    if node.inputs.is_empty() {
        return Err(IoError::message(format!(
            "node {} requires at least one input",
            node.id.0
        )));
    }
    let mut clips = Vec::with_capacity(node.inputs.len());
    for up in &node.inputs {
        let c = produced.get(&up.0).cloned().ok_or_else(|| {
            IoError::message(format!("upstream {} not produced yet", up.0))
        })?;
        clips.push(c);
    }
    Ok(clips)
}

fn apply_compose_layers(
    inputs: Vec<Arc<dyn VideoClip>>,
    params: &serde_json::Value,
    registry: &OperationRegistry,
) -> Result<Arc<dyn VideoClip>> {
    let _ = registry
        .get(&OperationId::new("rf.compose.layers"))
        .map_err(|e| IoError::message(e.to_string()))?;
    if inputs.is_empty() {
        return Err(IoError::message("rf.compose.layers needs inputs"));
    }

    let size = if let (Some(w), Some(h)) = (
        params.get("w").and_then(serde_json::Value::as_u64),
        params.get("h").and_then(serde_json::Value::as_u64),
    ) {
        #[allow(clippy::cast_possible_truncation)]
        Size::new(w as u32, h as u32)
    } else {
        inputs[0].size()
    };

    let layer_params = params.get("layers").and_then(|v| v.as_array());
    let mut layers = Vec::with_capacity(inputs.len());
    for (i, clip) in inputs.into_iter().enumerate() {
        let mut layer = CompositeLayer::new(clip).with_layer_index(
            i32::try_from(i).unwrap_or(i32::MAX),
        );
        if let Some(arr) = layer_params
            && let Some(lp) = arr.get(i)
        {
            let x = lp
                .get("x")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let y = lp
                .get("y")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            #[allow(clippy::cast_possible_truncation)]
            {
                layer = layer.with_position(Position::absolute(x as i32, y as i32));
            }
            if let Some(op) = lp.get("opacity").and_then(serde_json::Value::as_f64) {
                #[allow(clippy::cast_possible_truncation)]
                {
                    layer = layer.with_opacity(op as f32);
                }
            }
            if let Some(start) = lp.get("start").and_then(serde_json::Value::as_f64) {
                layer = layer.with_start(Time::from_secs(start));
            }
            if let Some(idx) = lp.get("layer_index").and_then(serde_json::Value::as_i64) {
                #[allow(clippy::cast_possible_truncation)]
                {
                    layer = layer.with_layer_index(idx as i32);
                }
            }
        }
        layers.push(layer);
    }

    if let Some(bg) = params.get("background") {
        let r = bg.get("r").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let g = bg.get("g").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let b = bg.get("b").and_then(serde_json::Value::as_u64).unwrap_or(0);
        #[allow(clippy::cast_possible_truncation)]
        let color = Rgb8::new(r as u8, g as u8, b as u8);
        composite_video_with_background(size, color, layers)
            .map_err(|e| IoError::message(e.to_string()))
    } else {
        composite_video(size, layers).map_err(|e| IoError::message(e.to_string()))
    }
}

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
fn apply_registered_op_media(
    input: NodeMedia,
    operation: &OperationId,
    params: &serde_json::Value,
    registry: &OperationRegistry,
    hints: &mut GraphEncodeHints,
) -> Result<NodeMedia> {
    let _desc = registry
        .get(operation)
        .map_err(|e| IoError::message(e.to_string()))?;

    match operation.as_str() {
        "rf.audio.gain" => {
            let factor = params
                .get("factor")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0);
            let audio = match input.audio {
                Some(a) => Some(
                    VolumeGain::new(factor as f32)
                        .apply(a)
                        .map_err(IoError::from)?,
                ),
                None => None,
            };
            Ok(NodeMedia {
                video: input.video,
                audio,
            })
        }
        "rf.audio.drop" => {
            hints.preserve_audio = false;
            Ok(NodeMedia {
                video: input.video,
                audio: None,
            })
        }
        "rf.audio.preserve" => {
            hints.preserve_audio = true;
            Ok(input)
        }
        op if op.starts_with("rf.audio.") => Err(IoError::message(format!(
            "audio operation '{op}' is registered but has no executor yet"
        ))),
        _ => {
            let video = apply_registered_op(
                Arc::clone(&input.video),
                operation,
                params,
                registry,
                hints,
            )?;
            let audio = match operation.as_str() {
                "rf.transform.trim" => match input.audio {
                    Some(a) => Some(apply_trim_audio(a, params)?),
                    None => None,
                },
                _ => input.audio,
            };
            Ok(NodeMedia { video, audio })
        }
    }
}

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
fn apply_registered_op(
    clip: Arc<dyn VideoClip>,
    operation: &OperationId,
    params: &serde_json::Value,
    registry: &OperationRegistry,
    hints: &mut GraphEncodeHints,
) -> Result<Arc<dyn VideoClip>> {
    // Typed registry: unknown ids are rejected (not open Custom).
    let _desc = registry
        .get(operation)
        .map_err(|e| IoError::message(e.to_string()))?;

    match operation.as_str() {
        "rf.transform.trim" => apply_trim(clip, params),
        "rf.transform.hflip" => MirrorX.apply(clip).map_err(IoError::from),
        "rf.transform.vflip" => MirrorY.apply(clip).map_err(IoError::from),
        "rf.transform.even_dims" => EvenSize.apply(clip).map_err(IoError::from),
        "rf.transform.scale" => {
            let w = params
                .get("w")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| IoError::message("rf.transform.scale requires w"))?;
            let h = params
                .get("h")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| IoError::message("rf.transform.scale requires h"))?;
            Resize::to(Size::new(w as u32, h as u32))
                .apply(clip)
                .map_err(IoError::from)
        }
        "rf.transform.crop" => {
            let x = params
                .get("x")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let y = params
                .get("y")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let w = params
                .get("w")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| IoError::message("rf.transform.crop requires w"))?;
            let h = params
                .get("h")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| IoError::message("rf.transform.crop requires h"))?;
            Crop::new(x as u32, y as u32, w as u32, h as u32)
                .apply(clip)
                .map_err(IoError::from)
        }
        "rf.transform.rotate" => apply_rotate(clip, params),
        "rf.transform.fade_in" => {
            let d = param_seconds(params, "duration")?.unwrap_or(0.5);
            FadeIn::new(Duration::from_secs(d))
                .apply(clip)
                .map_err(IoError::from)
        }
        "rf.transform.fade_out" => {
            let d = param_seconds(params, "duration")?.unwrap_or(0.5);
            FadeOut::new(Duration::from_secs(d))
                .apply(clip)
                .map_err(IoError::from)
        }
        "rf.color.black_and_white" => BlackAndWhite.apply(clip).map_err(IoError::from),
        "rf.color.invert" => InvertColors.apply(clip).map_err(IoError::from),
        "rf.color.painting" => {
            let sat = params
                .get("saturation")
                .and_then(serde_json::Value::as_f64)
                .map(|v| v as f32);
            let black = params
                .get("black")
                .and_then(serde_json::Value::as_f64)
                .map(|v| v as f32);
            let paint = match (sat, black) {
                (Some(s), Some(b)) => Painting::with(s, b),
                (Some(s), None) => Painting {
                    saturation: s,
                    ..Painting::new()
                },
                (None, Some(b)) => Painting {
                    black: b,
                    ..Painting::new()
                },
                (None, None) => Painting::new(),
            };
            paint.apply(clip).map_err(IoError::from)
        }
        "rf.redaction.region" => {
            let empty = params.is_null()
                || params
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty);
            if empty {
                return Err(IoError::message(
                    "rf.redaction.region requires masks params (or use Redaction node)",
                ));
            }
            let redaction = region_redaction_from_value(params)?;
            apply_region_redaction(clip, &redaction)
        }
        "rf.compose.layers" => Err(IoError::message(
            "rf.compose.layers must be applied with multi-input materialize path",
        )),
        "rf.audio.gain" | "rf.audio.drop" | "rf.audio.preserve" => {
            // Handled in apply_registered_op_media; video passthrough if called here.
            Ok(clip)
        }
        "rf.encode.h264" => {
            if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                hints.output_path = Some(path.to_string());
            }
            if let Some(crf) = params.get("crf").and_then(serde_json::Value::as_u64) {
                hints.crf = Some(crf.min(51) as u8);
            }
            if let Some(codec) = params.get("codec").and_then(|v| v.as_str()) {
                hints.video_codec = Some(codec.to_string());
            } else {
                hints.video_codec.get_or_insert_with(|| "libx264".into());
            }
            if let Some(fps) = params.get("fps").and_then(serde_json::Value::as_f64) {
                hints.fps = Some(fps);
            }
            if let Some(pa) = params.get("preserve_audio").and_then(serde_json::Value::as_bool)
            {
                hints.preserve_audio = pa;
            }
            Ok(clip)
        }
        other => Err(IoError::message(format!(
            "operation '{other}' is registered but has no executor yet"
        ))),
    }
}

fn apply_rotate(clip: Arc<dyn VideoClip>, params: &serde_json::Value) -> Result<Arc<dyn VideoClip>> {
    let mode = params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("cw90");
    let rot = match mode {
        "cw90" | "90" => Rotate::cw90(),
        "cw180" | "180" => Rotate::half(),
        "cw270" | "270" | "ccw90" => Rotate::cw270(),
        "degrees" => {
            let d = params
                .get("degrees")
                .and_then(serde_json::Value::as_f64)
                .ok_or_else(|| IoError::message("rotate mode=degrees needs degrees"))?;
            #[allow(clippy::cast_possible_truncation)]
            Rotate::degrees(d as f32)
        }
        other => {
            return Err(IoError::message(format!(
                "unknown rotate mode '{other}' (cw90|cw180|cw270|degrees)"
            )));
        }
    };
    rot.apply(clip).map_err(IoError::from)
}

fn apply_trim(clip: Arc<dyn VideoClip>, params: &serde_json::Value) -> Result<Arc<dyn VideoClip>> {
    let start = param_seconds(params, "start")?.unwrap_or(0.0);
    let duration = param_seconds(params, "duration")?.ok_or_else(|| {
        IoError::message("rf.transform.trim requires duration (seconds or MediaTime)")
    })?;
    subclip_video(
        clip,
        Time::from_secs(start),
        Duration::from_secs(duration),
    )
    .map_err(IoError::from)
}

fn apply_trim_audio(
    clip: Arc<dyn AudioClip>,
    params: &serde_json::Value,
) -> Result<Arc<dyn AudioClip>> {
    let start = param_seconds(params, "start")?.unwrap_or(0.0);
    let duration = param_seconds(params, "duration")?.ok_or_else(|| {
        IoError::message("rf.transform.trim requires duration (seconds or MediaTime)")
    })?;
    subclip_audio(
        clip,
        Time::from_secs(start),
        Duration::from_secs(duration),
    )
    .map_err(IoError::from)
}

fn param_seconds(params: &serde_json::Value, key: &str) -> Result<Option<f64>> {
    let Some(v) = params.get(key) else {
        return Ok(None);
    };
    if let Some(n) = v.as_f64() {
        return Ok(Some(n));
    }
    if let Some(n) = v.as_i64() {
        #[allow(clippy::cast_precision_loss)]
        return Ok(Some(n as f64));
    }
    if let (Some(ticks), Some(ts)) = (
        v.get("ticks").and_then(serde_json::Value::as_i64),
        v.get("timescale").and_then(serde_json::Value::as_u64),
    ) {
        #[allow(clippy::cast_possible_truncation)]
        let mt = MediaTime::new(ticks, ts as u32).map_err(IoError::from)?;
        return Ok(Some(mt.as_secs()));
    }
    Err(IoError::message(format!(
        "param '{key}' must be number or MediaTime object"
    )))
}

fn merge_option_hints(hints: &mut GraphEncodeHints, options: &GraphRunOptions) {
    if options.fps.is_some() {
        hints.fps = options.fps;
    }
    if options.video_codec.is_some() {
        hints.video_codec.clone_from(&options.video_codec);
    }
    if options.crf.is_some() {
        hints.crf = options.crf;
    }
}

fn write_graph_outputs(
    graph: &RenderGraph,
    clip: &dyn VideoClip,
    audio: Option<&dyn AudioClip>,
    hints: &GraphEncodeHints,
    control: &WriteControl,
) -> Result<()> {
    let mut paths: Vec<String> = graph
        .outputs
        .iter()
        .filter_map(|o| o.uri.clone())
        .collect();
    if paths.is_empty()
        && let Some(p) = &hints.output_path
    {
        paths.push(p.clone());
    }
    if paths.is_empty() {
        return Err(IoError::message(
            "no output path: set GraphOutput.uri or rf.encode.h264 path",
        ));
    }

    let fps = resolve_fps(hints, clip)?;
    for path in paths {
        control.check_cancel()?;
        let mut opts = WriteVideoOptions::new(&path, fps);
        if let Some(codec) = &hints.video_codec {
            opts = opts.with_video_codec(codec.clone());
        }
        if let Some(crf) = hints.crf {
            opts = opts.with_crf(crf);
        } else if hints.video_codec.is_none() {
            opts = opts.with_crf(23);
        }
        if hints.preserve_audio
            && let Some(a) = audio
        {
            write_av_with(clip, a, &opts, control)?;
        } else {
            write_video_with(clip, &opts, control)?;
        }
    }
    control.report(WriteProgress::new(WriteStage::Done, 1, 1));
    Ok(())
}

fn resolve_fps(hints: &GraphEncodeHints, clip: &dyn VideoClip) -> Result<f64> {
    if let Some(fps) = hints.fps {
        if fps.is_finite() && fps > 0.0 {
            return Ok(fps);
        }
        return Err(IoError::message(format!("invalid encode fps {fps}")));
    }
    if let Some(fps) = clip.fps()
        && fps.is_finite()
        && fps > 0.0
    {
        return Ok(fps);
    }
    Ok(24.0)
}

fn can_use_ffmpeg_prefix(graph: &RenderGraph, plan: &ExecutionPlan) -> bool {
    let has_rust = plan
        .stages
        .iter()
        .any(|s| matches!(s, ExecutionStage::Rust(_)));
    let first_ffmpeg = plan
        .stages
        .first()
        .is_some_and(|s| matches!(s, ExecutionStage::Ffmpeg(_)));
    has_rust && first_ffmpeg && graph.assets.len() == 1
}

/// Returns `Ok(Some(()))` when hybrid path fully finished, `Ok(None)` to fall back.
#[allow(clippy::too_many_lines)]
fn try_hybrid_ffmpeg_prefix(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<Option<()>> {
    let Some(ExecutionStage::Ffmpeg(first)) = plan.stages.first() else {
        return Ok(None);
    };

    let mut filter = FilterGraph::new();
    let mut saw_trim = false;
    let mut strip_ids: HashSet<String> = HashSet::new();

    for nid in &first.nodes {
        let node = graph
            .nodes
            .iter()
            .find(|n| n.id == *nid)
            .ok_or_else(|| IoError::message(format!("missing node {}", nid.0)))?;
        match &node.body {
            RenderNodeKind::Source { .. } | RenderNodeKind::Output { .. } => {}
            RenderNodeKind::Op { operation, params } => match operation.as_str() {
                "rf.transform.trim" => {
                    let start = param_seconds(params, "start")?.unwrap_or(0.0);
                    let duration = param_seconds(params, "duration")?.ok_or_else(|| {
                        IoError::message("trim duration required for FFmpeg prefix")
                    })?;
                    filter = filter.then(FilterOp::Trim { start, duration });
                    saw_trim = true;
                    strip_ids.insert(nid.0.clone());
                }
                "rf.transform.hflip" => {
                    filter = filter.then(FilterOp::HFlip);
                    strip_ids.insert(nid.0.clone());
                }
                "rf.transform.vflip" => {
                    filter = filter.then(FilterOp::VFlip);
                    strip_ids.insert(nid.0.clone());
                }
                "rf.transform.scale" => {
                    let w = params
                        .get("w")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| IoError::message("scale w required"))?;
                    let h = params
                        .get("h")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| IoError::message("scale h required"))?;
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        filter = filter.then(FilterOp::Scale {
                            w: w as u32,
                            h: h as u32,
                        });
                    }
                    strip_ids.insert(nid.0.clone());
                }
                "rf.transform.crop" => {
                    let x = params
                        .get("x")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let y = params
                        .get("y")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let w = params
                        .get("w")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| IoError::message("crop w required"))?;
                    let h = params
                        .get("h")
                        .and_then(serde_json::Value::as_u64)
                        .ok_or_else(|| IoError::message("crop h required"))?;
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        filter = filter.then(FilterOp::Crop {
                            w: w as u32,
                            h: h as u32,
                            x: x as u32,
                            y: y as u32,
                        });
                    }
                    strip_ids.insert(nid.0.clone());
                }
                "rf.transform.even_dims" => {
                    filter = filter.then(FilterOp::EvenDims);
                    strip_ids.insert(nid.0.clone());
                }
                _ => return Ok(None),
            },
            RenderNodeKind::Redaction { .. } => return Ok(None),
        }
    }
    // Prefix must apply at least one real filter (trim and/or geometry).
    if filter.is_empty() && !saw_trim {
        return Ok(None);
    }
    if filter.is_empty() {
        return Ok(None);
    }

    let source_uri = graph
        .assets
        .first()
        .map(|a| a.uri.as_str())
        .ok_or_else(|| IoError::message("hybrid prefix needs an asset"))?;
    if !Path::new(source_uri).is_file() {
        return Ok(None);
    }

    let out_path = resolve_output_path(graph).ok_or_else(|| {
        IoError::message("hybrid run needs GraphOutput.uri or encode path")
    })?;

    control.check_cancel()?;
    let mid = temp_graph_path(Path::new(&out_path), "rf-g-pfx");
    let vf = filter.to_vf().map_err(IoError::message)?;
    let node_id_strs: Vec<String> = first.nodes.iter().map(|n| n.0.clone()).collect();
    let stage_fp = StageCache::ffmpeg_prefix_key(source_uri, &vf, &node_id_strs);

    let mut used_cache = false;
    if let Some(cache) = &options.cache
        && cache.restore_to(&stage_fp, "mp4", &mid)?
    {
        used_cache = true;
    }
    if !used_cache {
        if let Err(e) = run_filtergraph(source_uri, &mid, &filter) {
            let _ = std::fs::remove_file(&mid);
            return Err(e);
        }
        if let Some(cache) = &options.cache {
            let _ = cache.store_copy(&stage_fp, "mp4", &mid);
        }
    }
    control.report(WriteProgress::new(WriteStage::Video, 0, 1));

    let mut reduced = strip_and_rewire(graph, &strip_ids);
    if let Some(asset) = reduced.assets.first_mut() {
        asset.uri = mid.to_string_lossy().into_owned();
    }

    let result = (|| {
        let seeds = HashMap::new();
        let audio_seeds = HashMap::new();
        let mut bundle = materialize_graph_bundle(
            &reduced,
            &options.registry,
            &seeds,
            &audio_seeds,
            false,
        )?;
        bundle.hints.output_path = Some(out_path);
        merge_option_hints(&mut bundle.hints, options);
        write_graph_outputs(
            graph,
            bundle.video.as_ref(),
            None,
            &bundle.hints,
            control,
        )
    })();

    let _ = std::fs::remove_file(&mid);
    result.map(Some)
}

fn resolve_output_path(graph: &RenderGraph) -> Option<String> {
    if let Some(uri) = graph.outputs.iter().find_map(|o| o.uri.clone()) {
        return Some(uri);
    }
    graph.nodes.iter().find_map(|n| match &n.body {
        RenderNodeKind::Op { operation, params } if operation.as_str() == "rf.encode.h264" => {
            params
                .get("path")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }
        _ => None,
    })
}

/// Remove applied nodes and rewire consumers to each removed node's single input.
fn strip_and_rewire(graph: &RenderGraph, strip: &HashSet<String>) -> RenderGraph {
    let mut g = graph.clone();
    // Map stripped id → its upstream (single input).
    let mut replace: HashMap<String, String> = HashMap::new();
    for n in &graph.nodes {
        if strip.contains(&n.id.0)
            && let Some(up) = n.inputs.first()
        {
            replace.insert(n.id.0.clone(), up.0.clone());
        }
    }
    // Flatten replace chains.
    let resolve = |mut id: String| -> String {
        let mut guard = 0;
        while let Some(next) = replace.get(&id) {
            id = next.clone();
            guard += 1;
            if guard > 64 {
                break;
            }
        }
        id
    };

    g.nodes.retain(|n| !strip.contains(&n.id.0));
    for n in &mut g.nodes {
        for inp in &mut n.inputs {
            inp.0 = resolve(inp.0.clone());
        }
    }
    for o in &mut g.outputs {
        o.node = NodeId(resolve(o.node.0.clone()));
    }
    g
}

fn temp_graph_path(output: &Path, tag: &str) -> PathBuf {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let stem = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("reelforge");
    parent.join(format!(".{stem}.{tag}.{}.mp4", std::process::id()))
}

/// Whether the registry backend for `id` is known to the M3 runner.
#[must_use]
pub fn is_executable_op(id: &str) -> bool {
    matches!(
        id,
        "rf.transform.trim"
            | "rf.transform.hflip"
            | "rf.transform.vflip"
            | "rf.transform.scale"
            | "rf.transform.crop"
            | "rf.transform.even_dims"
            | "rf.transform.rotate"
            | "rf.transform.fade_in"
            | "rf.transform.fade_out"
            | "rf.color.black_and_white"
            | "rf.color.invert"
            | "rf.color.painting"
            | "rf.compose.layers"
            | "rf.redaction.region"
            | "rf.audio.gain"
            | "rf.audio.drop"
            | "rf.audio.preserve"
            | "rf.encode.h264"
    )
}

/// Backend class for a graph node (for hosts / debug).
#[must_use]
pub fn node_backend(node: &RenderNode, registry: &OperationRegistry) -> Option<BackendClass> {
    match &node.body {
        RenderNodeKind::Source { .. } | RenderNodeKind::Output { .. } => {
            Some(BackendClass::Ffmpeg)
        }
        RenderNodeKind::Redaction { .. } => Some(BackendClass::Rust),
        RenderNodeKind::Op { operation, .. } => registry.get(operation).ok().map(|d| d.backend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8, Size};
    use reelforge_render_graph::{
        GraphOutput, MaskSample, MaskTimeline, MediaAsset, MediaAssetId, RegionRedaction,
        RenderNode, RENDER_GRAPH_VERSION,
    };

    fn linear_redaction_graph() -> RenderGraph {
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
                uri: "seed://color".into(),
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
                    id: NodeId("trim".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.trim"),
                        params: serde_json::json!({ "start": 0.0, "duration": 0.5 }),
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
                    id: NodeId("enc".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.encode.h264"),
                        params: serde_json::json!({ "crf": 28, "path": "out.mp4" }),
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
        }
    }

    #[test]
    fn explain_lists_hybrid_stages() {
        let g = linear_redaction_graph();
        let text = explain_render_graph(&g).unwrap();
        assert!(text.contains("execution_stages"));
        assert!(text.contains("rust") || text.contains("ffmpeg"));
    }

    #[test]
    fn materialize_with_seed_applies_trim_and_redaction() {
        let g = linear_redaction_graph();
        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(2.0),
        ));
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let registry = OperationRegistry::with_builtins();
        let (clip, hints) = materialize_graph_with_seeds(&g, &registry, &seeds).unwrap();
        assert!((clip.duration().as_secs() - 0.5).abs() < 1e-9);
        assert_eq!(hints.crf, Some(28));
        assert_eq!(hints.output_path.as_deref(), Some("out.mp4"));
        let _ = clip.frame_at(Time::ZERO).unwrap();
    }

    #[test]
    fn rejects_unknown_op_even_if_forced_on_graph() {
        let mut g = linear_redaction_graph();
        g.nodes[1].body = RenderNodeKind::Op {
            operation: OperationId::new("rf.not.real"),
            params: serde_json::json!({}),
        };
        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let err = materialize_graph_with_seeds(&g, &OperationRegistry::with_builtins(), &seeds);
        assert!(err.is_err());
    }

    #[test]
    fn strip_rewires_consumers() {
        let g = linear_redaction_graph();
        let mut strip = HashSet::new();
        strip.insert("trim".into());
        let reduced = strip_and_rewire(&g, &strip);
        assert!(!reduced.nodes.iter().any(|n| n.id.0 == "trim"));
        let blur = reduced.nodes.iter().find(|n| n.id.0 == "blur").unwrap();
        assert_eq!(blur.inputs[0].0, "src");
    }

    #[test]
    fn is_executable_builtins() {
        assert!(is_executable_op("rf.transform.trim"));
        assert!(is_executable_op("rf.redaction.region"));
        assert!(is_executable_op("rf.compose.layers"));
        assert!(is_executable_op("rf.transform.fade_in"));
        assert!(!is_executable_op("rf.not.real"));
    }

    #[test]
    fn materialize_fade_and_compose() {
        let registry = OperationRegistry::with_builtins();
        let base: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(16, 16),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let overlay: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let g = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![
                MediaAsset {
                    id: MediaAssetId("a".into()),
                    uri: "seed://a".into(),
                    duration: None,
                    role: None,
                },
                MediaAsset {
                    id: MediaAssetId("b".into()),
                    uri: "seed://b".into(),
                    duration: None,
                    role: None,
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
                    id: NodeId("fade".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.fade_in"),
                        params: serde_json::json!({ "duration": 0.2 }),
                    },
                    inputs: vec![NodeId("src_a".into())],
                },
                RenderNode {
                    id: NodeId("comp".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.compose.layers"),
                        params: serde_json::json!({
                            "w": 16,
                            "h": 16,
                            "layers": [
                                { "x": 0, "y": 0 },
                                { "x": 4, "y": 4, "opacity": 1.0 }
                            ]
                        }),
                    },
                    inputs: vec![NodeId("fade".into()), NodeId("src_b".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("comp".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        };
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), base);
        seeds.insert(MediaAssetId("b".into()), overlay);
        let (clip, _) = materialize_graph_with_seeds(&g, &registry, &seeds).unwrap();
        assert_eq!(clip.size(), Size::new(16, 16));
        let _ = clip.frame_at(Time::ZERO).unwrap();
    }

    #[test]
    fn materialize_audio_gain_and_drop() {
        use reelforge_core::{AudioFormat, SilenceClip};
        let registry = OperationRegistry::with_builtins();
        let video: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::BLUE,
            Duration::from_secs(1.0),
        ));
        let audio: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            Duration::from_secs(1.0),
            AudioFormat::mono_f32(48_000),
        ));
        let g = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "seed://a".into(),
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
                    id: NodeId("gain".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.audio.gain"),
                        params: serde_json::json!({ "factor": 0.5 }),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("gain".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        };
        let mut vseeds = HashMap::new();
        vseeds.insert(MediaAssetId("a".into()), video);
        let mut aseeds = HashMap::new();
        aseeds.insert(MediaAssetId("a".into()), audio);
        let bundle = materialize_graph_bundle(&g, &registry, &vseeds, &aseeds, true).unwrap();
        assert!(bundle.audio.is_some());
        assert!(bundle.hints.preserve_audio);
    }
}
