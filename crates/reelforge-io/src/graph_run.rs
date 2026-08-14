//! Execute [`RenderGraph`] / [`ExecutionPlan`] (M3 hybrid runner).
//!
//! ```text
//! RenderGraph --schedule--> ExecutionPlan --run--> outputs on disk
//! ```
//!
//! Linear DAGs and multi-input `rf.compose.layers` are supported. Adapter
//! stages (`rf.adapter.sightloom`) materialize masks via [`crate::AdapterHost`]
//! or exported tracks JSON. GPU stages still fail clearly. `FFmpeg` stages that
//! only carry encode/output markers finalize via Rust pixel encode
//! (`write_video`); geometry/`trim` prefixes use host filtergraph when a later
//! stage needs Rust.

use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::error::{IoError, Result};
use crate::filtergraph::{FilterGraph, FilterOp};
use crate::mask_bridge::apply_region_redaction;
use crate::options::{OpenVideoOptions, WriteVideoOptions};
use crate::stage_cache::StageCache;
use crate::video_file::open_video;
use crate::{run_filtergraph, write_av_with, write_video_with};
use reelforge_core::{AudioClip, VideoClip};
use reelforge_render_graph::{
    BackendClass, CompiledOp, ExecutionPlan, ExecutionStage, MediaAssetId, NodeId, OperationId,
    OperationRegistry, RENDER_GRAPH_VERSION, RenderGraph, RenderNode, RenderNodeKind,
    StageCacheKey, TypedParams, artifact_manifest, compile_graph, compile_op,
    fingerprint_stage_key, is_executable_op_id, schedule_compiled, schedule_graph,
};
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Encode / output hints collected while walking the graph.
#[derive(Clone, Default)]
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
    /// Optional vision adapter host (`SightLoom` / tests).
    pub adapter_host: Option<std::sync::Arc<dyn crate::AdapterHost>>,
}

impl core::fmt::Debug for GraphEncodeHints {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GraphEncodeHints")
            .field("fps", &self.fps)
            .field("video_codec", &self.video_codec)
            .field("crf", &self.crf)
            .field("output_path", &self.output_path)
            .field("preserve_audio", &self.preserve_audio)
            .field("adapter_host", &self.adapter_host.is_some())
            .finish()
    }
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
pub(crate) struct NodeMedia {
    pub(crate) video: Arc<dyn VideoClip>,
    pub(crate) audio: Option<Arc<dyn AudioClip>>,
    pub(crate) masks: Option<reelforge_render_graph::MaskTimeline>,
}

impl NodeMedia {
    pub(crate) fn new(video: Arc<dyn VideoClip>, audio: Option<Arc<dyn AudioClip>>) -> Self {
        Self {
            video,
            audio,
            masks: None,
        }
    }
}

/// Options for [`run_render_graph_with`].
#[derive(Clone)]
pub struct GraphRunOptions {
    /// Operation registry (builtins by default).
    pub registry: OperationRegistry,
    /// Override encode FPS.
    pub fps: Option<f64>,
    /// Override video codec.
    pub video_codec: Option<String>,
    /// Override CRF.
    pub crf: Option<u8>,
    /// Optional full-run stage cache (fingerprint â†’ artifact).
    pub cache: Option<StageCache>,
    /// Open source files with audio and mux when present (default `true`).
    ///
    /// When `true`, pure in-process materialize is preferred over hybrid
    /// `FFmpeg` prefixes so companion audio stays aligned.
    pub with_audio: bool,
    /// Optional `SightLoom` / test adapter host.
    pub adapter_host: Option<Arc<dyn crate::AdapterHost>>,
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
            adapter_host: None,
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

    /// Install a vision adapter host (`SightLoom` / tests).
    #[must_use]
    pub fn with_adapter_host(mut self, host: Arc<dyn crate::AdapterHost>) -> Self {
        self.adapter_host = Some(host);
        self
    }
}

impl core::fmt::Debug for GraphRunOptions {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GraphRunOptions")
            .field("registry", &self.registry.len())
            .field("fps", &self.fps)
            .field("video_codec", &self.video_codec)
            .field("crf", &self.crf)
            .field("cache", &self.cache.is_some())
            .field("with_audio", &self.with_audio)
            .field("adapter_host", &self.adapter_host.is_some())
            .finish()
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
    run_render_graph_with_manifest(graph, control, options).map(|_| ())
}

/// Run a graph and return the sealed [`ArtifactManifest`] (output URIs + file hashes).
///
/// # Errors
///
/// Same as [`run_render_graph_with`].
pub fn run_render_graph_with_manifest(
    graph: &RenderGraph,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<reelforge_render_graph::ArtifactManifest> {
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
    let compiled =
        compile_graph(graph, &options.registry).map_err(|e| IoError::message(e.to_string()))?;
    let plan = schedule_compiled(&compiled).map_err(|e| IoError::message(e.to_string()))?;
    execute_plan_and_seal(graph, &compiled, &plan, control, options)
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
/// Walks **plan stages in order** (mandatory stage boundaries). Each stage
/// evaluates only its node set; media products carry forward. Optional hybrid
/// `FFmpeg` prefix optimizes the first filter stage on disk when video-only.
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
    run_execution_plan_with_manifest(graph, plan, control, options).map(|_| ())
}

/// Execute a plan and return the sealed [`reelforge_render_graph::ArtifactManifest`].
///
/// # Errors
///
/// Same as [`run_execution_plan_with`].
pub fn run_execution_plan_with_manifest(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<reelforge_render_graph::ArtifactManifest> {
    let compiled =
        compile_graph(graph, &options.registry).map_err(|e| IoError::message(e.to_string()))?;
    execute_plan_and_seal(graph, &compiled, plan, control, options)
}

fn execute_plan_and_seal(
    graph: &RenderGraph,
    compiled: &reelforge_render_graph::CompiledGraph,
    plan: &ExecutionPlan,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<reelforge_render_graph::ArtifactManifest> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    control.check_cancel()?;
    reject_gpu_stages(plan)?;

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
        return finish_manifest(compiled, plan, None);
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
        return finish_manifest(compiled, plan, resolve_output_path(graph));
    }

    let seeds = HashMap::new();
    let audio_seeds = HashMap::new();
    let mut bundle = materialize_execution_plan_with_host(
        graph,
        plan,
        &options.registry,
        &seeds,
        &audio_seeds,
        options.with_audio,
        Some(control),
        options.cache.as_ref(),
        options.adapter_host.clone(),
    )?;
    merge_option_hints(&mut bundle.hints, options);
    write_graph_outputs(
        graph,
        bundle.video.as_ref(),
        bundle.audio.as_deref(),
        &bundle.hints,
        control,
    )?;
    let written = resolve_output_path(graph).or(bundle.hints.output_path.clone());
    if let (Some(cache), Some(fp)) = (&options.cache, &run_fp)
        && let Some(out) = &written
    {
        let _ = cache.store_copy(fp, "mp4", out);
    }
    finish_manifest(compiled, plan, written)
}

fn finish_manifest(
    compiled: &reelforge_render_graph::CompiledGraph,
    plan: &ExecutionPlan,
    extra_uri: Option<String>,
) -> Result<reelforge_render_graph::ArtifactManifest> {
    let mut manifest = artifact_manifest(compiled, plan);
    if let Some(uri) = extra_uri {
        for art in manifest.outputs.iter_mut().chain(
            manifest
                .stages
                .iter_mut()
                .flat_map(|s| s.artifacts.iter_mut()),
        ) {
            if art.uri.is_none() && matches!(art.kind, reelforge_render_graph::ArtifactKind::Output)
            {
                art.uri = Some(uri.clone());
            }
        }
    }
    crate::manifest_seal::seal_manifest_on_disk(&mut manifest)?;
    Ok(manifest)
}

fn reject_gpu_stages(plan: &ExecutionPlan) -> Result<()> {
    for stage in &plan.stages {
        if matches!(stage, ExecutionStage::Gpu(_)) {
            return Err(IoError::message("GPU stages not implemented in M3 runner"));
        }
    }
    Ok(())
}

fn restore_cached_outputs(graph: &RenderGraph, cached: &Path) -> Result<()> {
    let paths: Vec<String> = graph.outputs.iter().filter_map(|o| o.uri.clone()).collect();
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
/// Walks the full topological order (ignores stage boundaries). Prefer
/// [`materialize_execution_plan`] when an [`ExecutionPlan`] is available.
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
    let mut ctx = MaterializeCtx::new(graph, registry, with_audio, None);
    for id in &order {
        ctx.eval_node(id, video_seeds, audio_seeds)?;
    }
    ctx.finish_bundle()
}

/// Materialize by walking [`ExecutionPlan`] stages in order.
///
/// Each stage evaluates only its node ids; products from earlier stages feed
/// later ones. This is the runtime contract for [`run_execution_plan_with`].
///
/// When `cache` is set, a strong per-stage fingerprint is computed (inputs +
/// compiled ops + backend + host `FFmpeg`) for intermediate keying / diagnostics.
///
/// # Errors
///
/// Unsupported stages, graph structure, unknown ops, open/decode failures.
#[allow(clippy::too_many_arguments)]
pub fn materialize_execution_plan<S: BuildHasher, A: BuildHasher>(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    registry: &OperationRegistry,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
    control: Option<&WriteControl>,
    cache: Option<&StageCache>,
) -> Result<GraphBundle> {
    materialize_execution_plan_with_host(
        graph,
        plan,
        registry,
        video_seeds,
        audio_seeds,
        with_audio,
        control,
        cache,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn materialize_execution_plan_with_host<S: BuildHasher, A: BuildHasher>(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    registry: &OperationRegistry,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
    control: Option<&WriteControl>,
    cache: Option<&StageCache>,
    adapter_host: Option<Arc<dyn crate::AdapterHost>>,
) -> Result<GraphBundle> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    reject_gpu_stages(plan)?;

    if plan.stages.is_empty() {
        // Empty plan: fall back to full topo (tests / hand-built plans).
        return materialize_graph_bundle(graph, registry, video_seeds, audio_seeds, with_audio);
    }

    let mut ctx = MaterializeCtx::new(graph, registry, with_audio, adapter_host);
    let mut upstream_fp = asset_input_fingerprint(graph);
    let total_stages = plan.stages.len();
    #[allow(clippy::cast_possible_truncation)]
    let total_u = total_stages as u64;

    for (si, stage) in plan.stages.iter().enumerate() {
        if let Some(c) = control {
            c.check_cancel()?;
            #[allow(clippy::cast_possible_truncation)]
            c.report(WriteProgress::new(WriteStage::Plan, si as u64, total_u));
        }

        let node_ids = stage.node_ids();
        let compiled = compile_stage_ops(graph, registry, node_ids)?;
        let node_id_strs: Vec<String> = node_ids.iter().map(|n| n.0.clone()).collect();
        let stage_fp = fingerprint_stage_key(&StageCacheKey {
            backend: stage.backend_tag(),
            node_ids: &node_id_strs,
            input_fingerprint: &upstream_fp,
            compiled: &compiled,
            ffmpeg_version: crate::stage_cache::probe_ffmpeg_version_cached(),
            host_tag: std::env::consts::OS,
        });

        // Stage cache: intermediate file hits are only meaningful for FFmpeg
        // disk stages (hybrid prefix). Here we record the key on the context
        // so hosts / tests can assert stage boundaries were honored.
        ctx.last_stage_fingerprint = Some(stage_fp.clone());
        if cache.is_some() {
            ctx.stage_fingerprints.push(stage_fp.clone());
        }

        for id in node_ids {
            ctx.eval_node(id, video_seeds, audio_seeds)?;
        }

        // Next stage inputs depend on this stage's work.
        upstream_fp = stage_fp;
    }

    ctx.finish_bundle()
}

/// In-process materialize state shared by full-topo and stage runners.
struct MaterializeCtx<'a> {
    graph: &'a RenderGraph,
    registry: &'a OperationRegistry,
    node_map: HashMap<&'a str, &'a RenderNode>,
    asset_map: HashMap<&'a str, &'a reelforge_render_graph::MediaAsset>,
    produced: HashMap<String, NodeMedia>,
    hints: GraphEncodeHints,
    primary_out: Option<NodeMedia>,
    last_stage_fingerprint: Option<String>,
    stage_fingerprints: Vec<String>,
}

impl<'a> MaterializeCtx<'a> {
    fn new(
        graph: &'a RenderGraph,
        registry: &'a OperationRegistry,
        with_audio: bool,
        adapter_host: Option<Arc<dyn crate::AdapterHost>>,
    ) -> Self {
        Self {
            graph,
            registry,
            node_map: graph.nodes.iter().map(|n| (n.id.0.as_str(), n)).collect(),
            asset_map: graph.assets.iter().map(|a| (a.id.0.as_str(), a)).collect(),
            produced: HashMap::new(),
            hints: GraphEncodeHints {
                preserve_audio: with_audio,
                adapter_host,
                ..GraphEncodeHints::default()
            },
            primary_out: None,
            last_stage_fingerprint: None,
            stage_fingerprints: Vec::new(),
        }
    }

    fn eval_node<S: BuildHasher, A: BuildHasher>(
        &mut self,
        id: &NodeId,
        video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
        audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    ) -> Result<()> {
        let node = self
            .node_map
            .get(id.0.as_str())
            .ok_or_else(|| IoError::message(format!("missing node {}", id.0)))?;
        let media = match &node.body {
            RenderNodeKind::Source { asset } => resolve_source(
                asset,
                &self.asset_map,
                video_seeds,
                audio_seeds,
                self.hints.preserve_audio,
            )?,
            RenderNodeKind::Op { operation, params } => {
                let compiled = compile_op(self.registry, operation, params)
                    .map_err(|e| IoError::message(e.to_string()))?;
                let inputs = match compiled.params.executor_kind() {
                    reelforge_render_graph::ExecutorKind::Nary => {
                        multi_input_media(node, &self.produced)?
                    }
                    reelforge_render_graph::ExecutorKind::Unary => {
                        vec![single_input_media(node, &self.produced)?]
                    }
                };
                crate::exec::execute_compiled(&compiled, inputs, &mut self.hints)?
            }
            RenderNodeKind::Redaction { redaction } => {
                let input = single_input_media(node, &self.produced)?;
                let resolved = if redaction.masks.samples.is_empty() {
                    let Some(masks) = input.masks.clone() else {
                        return Err(IoError::message(
                            "RegionRedaction masks are empty (adapter did not materialize any)",
                        ));
                    };
                    let mut r = redaction.clone();
                    r.masks = masks;
                    apply_region_redaction(input.video, &r)?
                } else {
                    apply_region_redaction(input.video, redaction)?
                };
                NodeMedia {
                    video: resolved,
                    audio: input.audio,
                    masks: input.masks,
                }
            }
            RenderNodeKind::Output { .. } => {
                let input = single_input_media(node, &self.produced)?;
                self.primary_out = Some(input.clone());
                input
            }
        };
        self.produced.insert(id.0.clone(), media);
        Ok(())
    }

    fn finish_bundle(mut self) -> Result<GraphBundle> {
        if let Some(out) = self.graph.outputs.first() {
            if let Some(uri) = &out.uri {
                self.hints.output_path.get_or_insert_with(|| uri.clone());
            }
            if let Some(c) = self.produced.get(&out.node.0) {
                return Ok(GraphBundle {
                    video: Arc::clone(&c.video),
                    audio: c.audio.clone(),
                    hints: self.hints,
                });
            }
        }
        if let Some(c) = self.primary_out {
            return Ok(GraphBundle {
                video: c.video,
                audio: c.audio,
                hints: self.hints,
            });
        }
        Err(IoError::message(
            "RenderGraph produced no output clip (missing Output node?)",
        ))
    }
}

fn asset_input_fingerprint(graph: &RenderGraph) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    for a in &graph.assets {
        a.id.0.hash(&mut h);
        a.uri.hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn compile_stage_ops(
    graph: &RenderGraph,
    registry: &OperationRegistry,
    node_ids: &[NodeId],
) -> Result<Vec<CompiledOp>> {
    let mut out = Vec::new();
    for id in node_ids {
        let Some(node) = graph.nodes.iter().find(|n| n.id == *id) else {
            continue;
        };
        match &node.body {
            RenderNodeKind::Op { operation, params } => {
                let c = compile_op(registry, operation, params)
                    .map_err(|e| IoError::message(e.to_string()))?;
                out.push(c);
            }
            RenderNodeKind::Redaction { .. } => {
                let c = compile_op(
                    registry,
                    &OperationId::new("rf.redaction.region"),
                    &serde_json::json!({}),
                )
                .map_err(|e| IoError::message(e.to_string()))?;
                out.push(c);
            }
            _ => {}
        }
    }
    Ok(out)
}

fn resolve_source<S: BuildHasher, A: BuildHasher>(
    asset: &MediaAssetId,
    asset_map: &HashMap<&str, &reelforge_render_graph::MediaAsset>,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
) -> Result<NodeMedia> {
    if let Some(clip) = video_seeds.get(asset) {
        return Ok(NodeMedia::new(
            Arc::clone(clip),
            audio_seeds.get(asset).cloned(),
        ));
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
    if meta.role.as_deref() == Some("audio") {
        return resolve_audio_source(meta);
    }
    let mut opts = OpenVideoOptions::new(&meta.uri);
    if !with_audio {
        opts = opts.video_only();
    }
    match open_video(&opts) {
        Ok(opened) => {
            let audio = opened
                .audio()
                .map(|a| Arc::new(a.clone()) as Arc<dyn AudioClip>);
            Ok(NodeMedia::new(Arc::new(opened), audio))
        }
        Err(_) => resolve_audio_source(meta),
    }
}

fn resolve_audio_source(meta: &reelforge_render_graph::MediaAsset) -> Result<NodeMedia> {
    use crate::audio_file::open_audio;
    use crate::options::OpenAudioOptions;
    use reelforge_core::{ColorClip, Duration, Rgb8, Size};

    let audio = open_audio(&OpenAudioOptions::new(&meta.uri))?;
    let duration = audio.duration();
    let duration = if duration.is_positive() {
        duration
    } else {
        Duration::from_secs(0.04)
    };
    let video = ColorClip::new(Size::new(2, 2), Rgb8::BLACK, duration);
    Ok(NodeMedia::new(
        Arc::new(video),
        Some(Arc::new(audio)),
    ))
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
        let c = produced
            .get(&up.0)
            .cloned()
            .ok_or_else(|| IoError::message(format!("upstream {} not produced yet", up.0)))?;
        clips.push(c);
    }
    Ok(clips)
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
    let mut paths: Vec<String> = graph.outputs.iter().filter_map(|o| o.uri.clone()).collect();
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
            RenderNodeKind::Op { operation, params } => {
                let compiled = compile_op(&options.registry, operation, params)
                    .map_err(|e| IoError::message(e.to_string()))?;
                match compiled.params {
                    TypedParams::Trim { start, duration } => {
                        filter = filter.then(FilterOp::Trim {
                            start: start.as_secs(),
                            duration: duration.as_secs(),
                        });
                        saw_trim = true;
                        strip_ids.insert(nid.0.clone());
                    }
                    TypedParams::HFlip => {
                        filter = filter.then(FilterOp::HFlip);
                        strip_ids.insert(nid.0.clone());
                    }
                    TypedParams::VFlip => {
                        filter = filter.then(FilterOp::VFlip);
                        strip_ids.insert(nid.0.clone());
                    }
                    TypedParams::Scale { w, h } => {
                        filter = filter.then(FilterOp::Scale { w, h });
                        strip_ids.insert(nid.0.clone());
                    }
                    TypedParams::Crop { x, y, w, h } => {
                        filter = filter.then(FilterOp::Crop { w, h, x, y });
                        strip_ids.insert(nid.0.clone());
                    }
                    TypedParams::EvenDims => {
                        filter = filter.then(FilterOp::EvenDims);
                        strip_ids.insert(nid.0.clone());
                    }
                    _ => return Ok(None),
                }
            }
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

    let out_path = resolve_output_path(graph)
        .ok_or_else(|| IoError::message("hybrid run needs GraphOutput.uri or encode path"))?;

    control.check_cancel()?;
    #[allow(clippy::cast_possible_truncation)]
    let plan_total = plan.stages.len() as u64;
    control.report(WriteProgress::new(WriteStage::Plan, 0, plan_total));
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
    if plan_total > 1 {
        control.report(WriteProgress::new(WriteStage::Plan, 1, plan_total));
    }

    let mut reduced = strip_and_rewire(graph, &strip_ids);
    if let Some(asset) = reduced.assets.first_mut() {
        asset.uri = mid.to_string_lossy().into_owned();
    }

    let result = (|| {
        let seeds = HashMap::new();
        let audio_seeds = HashMap::new();
        let mut bundle =
            materialize_graph_bundle(&reduced, &options.registry, &seeds, &audio_seeds, false)?;
        bundle.hints.output_path = Some(out_path);
        merge_option_hints(&mut bundle.hints, options);
        write_graph_outputs(graph, bundle.video.as_ref(), None, &bundle.hints, control)
    })();

    let _ = std::fs::remove_file(&mid);
    result.map(Some)
}

fn resolve_output_path(graph: &RenderGraph) -> Option<String> {
    if let Some(uri) = graph.outputs.iter().find_map(|o| o.uri.clone()) {
        return Some(uri);
    }
    let registry = OperationRegistry::with_builtins();
    graph.nodes.iter().find_map(|n| {
        let RenderNodeKind::Op { operation, params } = &n.body else {
            return None;
        };
        let compiled = compile_op(&registry, operation, params).ok()?;
        match compiled.params {
            TypedParams::EncodeH264 { path, .. } => path,
            _ => None,
        }
    })
}

/// Remove applied nodes and rewire consumers to each removed node's single input.
fn strip_and_rewire(graph: &RenderGraph, strip: &HashSet<String>) -> RenderGraph {
    let mut g = graph.clone();
    // Map stripped id â†’ its upstream (single input).
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
///
/// Delegates to [`is_executable_op_id`] so registry and executor share one list.
#[must_use]
pub fn is_executable_op(id: &str) -> bool {
    is_executable_op_id(id)
}

/// Backend class for a graph node (for hosts / debug).
#[must_use]
pub fn node_backend(node: &RenderNode, registry: &OperationRegistry) -> Option<BackendClass> {
    match &node.body {
        RenderNodeKind::Source { .. } | RenderNodeKind::Output { .. } => Some(BackendClass::Ffmpeg),
        RenderNodeKind::Redaction { .. } => Some(BackendClass::Rust),
        RenderNodeKind::Op { operation, .. } => registry.get(operation).ok().map(|d| d.backend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Duration, MediaTime, Rgb8, Rgba8, Size, Time};
    use reelforge_render_graph::{
        GraphOutput, MaskSample, MaskTimeline, MediaAsset, MediaAssetId, RENDER_GRAPH_VERSION,
        RedactionStyle, RegionRedaction, RenderNode,
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
    fn stage_materialize_matches_full_topo() {
        let g = linear_redaction_graph();
        let registry = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &registry).unwrap();
        assert!(
            plan.stage_count() >= 2,
            "hybrid graph should fuse multiple stages, got {}",
            plan.stage_count()
        );
        // Stages cover every node exactly once.
        let mut covered: HashSet<String> = HashSet::new();
        for stage in &plan.stages {
            for n in stage.node_ids() {
                assert!(
                    covered.insert(n.0.clone()),
                    "node {} appeared in multiple stages",
                    n.0
                );
            }
            assert!(!stage.backend_tag().is_empty());
        }
        assert_eq!(covered.len(), g.nodes.len());

        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(2.0),
        ));
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), Arc::clone(&seed));
        let audio: HashMap<MediaAssetId, Arc<dyn AudioClip>> = HashMap::new();

        let full = materialize_graph_bundle(&g, &registry, &seeds, &audio, true).unwrap();
        let staged =
            materialize_execution_plan(&g, &plan, &registry, &seeds, &audio, true, None, None)
                .unwrap();

        assert!((full.video.duration().as_secs() - staged.video.duration().as_secs()).abs() < 1e-9);
        assert_eq!(full.hints.crf, staged.hints.crf);
        assert_eq!(full.hints.output_path, staged.hints.output_path);
        let _ = staged.video.frame_at(Time::ZERO).unwrap();
    }

    #[test]
    fn execution_plan_reports_plan_stage_progress() {
        let g = linear_redaction_graph();
        let registry = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &registry).unwrap();
        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(16, 16),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let audio: HashMap<MediaAssetId, Arc<dyn AudioClip>> = HashMap::new();

        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let hits2 = Arc::clone(&hits);
        let control = WriteControl::new().with_progress(move |p| {
            hits2.lock().unwrap().push((p.stage, p.index, p.total));
        });

        materialize_execution_plan(
            &g,
            &plan,
            &registry,
            &seeds,
            &audio,
            true,
            Some(&control),
            None,
        )
        .unwrap();

        let events = hits.lock().unwrap().clone();
        let plan_events: Vec<_> = events
            .iter()
            .copied()
            .filter(|(s, _, _)| *s == WriteStage::Plan)
            .collect();
        assert_eq!(plan_events.len(), plan.stage_count());
        assert_eq!(plan_events[0].0, WriteStage::Plan);
        assert_eq!(plan_events[0].1, 0);
        assert_eq!(
            usize::try_from(plan_events[0].2).unwrap(),
            plan.stage_count()
        );
        assert!(events.iter().all(|(s, _, _)| *s != WriteStage::Video));
    }

    #[test]
    fn stage_fingerprints_chain_when_cache_present() {
        let g = linear_redaction_graph();
        let registry = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &registry).unwrap();
        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(16, 16),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let audio: HashMap<MediaAssetId, Arc<dyn AudioClip>> = HashMap::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = StageCache::open(dir.path()).unwrap();

        // Smoke: stage path runs with cache hook (keys computed, no panic).
        let bundle = materialize_execution_plan(
            &g,
            &plan,
            &registry,
            &seeds,
            &audio,
            true,
            None,
            Some(&cache),
        )
        .unwrap();
        assert!((bundle.video.duration().as_secs() - 0.5).abs() < 1e-9);
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
        assert!(is_executable_op("rf.adapter.sightloom"));
        assert!(!is_executable_op("rf.not.real"));
    }

    #[test]
    fn adapter_materializes_masks_then_redacts() {
        let registry = OperationRegistry::with_builtins();
        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let g = RenderGraph {
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
                    id: NodeId("vision".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.adapter.sightloom"),
                        params: serde_json::json!({
                            "tracks": [{
                                "id": "person_a",
                                "samples": [{"t": 0.0, "cx": 16.0, "cy": 16.0, "radius": 8.0}]
                            }]
                        }),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("blur".into()),
                    body: RenderNodeKind::Redaction {
                        redaction: RegionRedaction {
                            masks: MaskTimeline::new(),
                            style: RedactionStyle::Solid {
                                color: Rgba8::new(0, 0, 0, 255),
                            },
                        },
                    },
                    inputs: vec![NodeId("vision".into())],
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
                uri: None,
            }],
        };
        let plan = schedule_graph(&g, &registry).unwrap();
        assert!(
            plan.stages
                .iter()
                .any(|s| matches!(s, ExecutionStage::Adapter(_))),
            "sightloom op must schedule as adapter: {plan:?}"
        );
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let (clip, _) = materialize_graph_with_seeds(&g, &registry, &seeds).unwrap();
        let f = clip.frame_at(Time::ZERO).unwrap();
        let i = (16 * 32 + 16) * 3;
        assert!(f.data()[i] < 250, "adapter masks must feed redaction");
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
            AudioFormat::STEREO_48K,
            Duration::from_secs(1.0),
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
