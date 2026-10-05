//! Execute [`RenderGraph`] / [`ExecutionPlan`] (M3 hybrid runner).
//!
//! ```text
//! RenderGraph --schedule--> ExecutionPlan --run--> outputs on disk
//! ```
//!
//! Linear DAGs and multi-input `rf.compose.layers` are supported. Adapter
//! stages (`rf.adapter.sightloom`) materialize masks via [`crate::AdapterHost`]
//! or exported tracks JSON. GPU stages run via [`crate::GpuHost`] /
//! [`crate::GpuRegistry`] (`passthrough`, `rf.encode.hw`). `FFmpeg` stages that
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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How a source with incomplete metadata is admitted.
///
/// Legacy callers keep their historical defaults. Strict production must not
/// turn those defaults into authored timeline decisions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub enum SourceAdmission {
    /// A still with no positive duration holds for [`LEGACY_STILL_HOLD`].
    /// GIF and other animated containers open as one still frame.
    #[default]
    Legacy,
    /// A still must declare a positive duration. An animated file is rejected
    /// instead of being opened as a still, and the extension must match the
    /// sniffed signature. This mode does not take the `FFmpeg` prefix, so the
    /// declared hold is the duration that is sampled.
    Strict,
}

/// Hold used only by [`SourceAdmission::Legacy`] when a still has no positive
/// duration. Strict admission never substitutes this value.
pub const LEGACY_STILL_HOLD: reelforge_core::Duration = reelforge_core::Duration::from_secs(1.0);

/// Bytes of a PNG prefix scanned for an `acTL` animation chunk.
const RASTER_SCAN_BYTES: usize = 65_536;
/// PNG file signature.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

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
    /// Adapter executors (`SightLoom` JSON by default).
    pub adapter_registry: crate::AdapterRegistry,
    /// Optional GPU device host.
    pub gpu_host: Option<std::sync::Arc<dyn crate::GpuHost>>,
    /// GPU executors (passthrough + hw encode by default).
    pub gpu_registry: crate::GpuRegistry,
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
            .field("adapter_registry", &self.adapter_registry.len())
            .field("gpu_host", &self.gpu_host.is_some())
            .field("gpu_registry", &self.gpu_registry.len())
            .finish()
    }
}

/// Encode settings that belong to one output branch.
///
/// Unset fields fall back to the run-level [`GraphEncodeHints`]. A sibling
/// branch does not fill them in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputEncodeOptions {
    /// Frames per second for this output.
    pub fps: Option<f64>,
    /// Video codec name (`libx264`, …).
    pub video_codec: Option<String>,
    /// Constant-rate factor.
    pub crf: Option<u8>,
    /// Mux this output's audio when `Some(true)`.
    pub preserve_audio: Option<bool>,
    /// Path recorded on this branch's encode node.
    pub path: Option<String>,
}

/// One graph output bound to the media that node actually produced.
#[derive(Clone)]
pub struct GraphOutputMedia {
    /// Destination path.
    pub uri: String,
    /// Picture for this output.
    pub video: Arc<dyn VideoClip>,
    /// Audio for this output. Absent means a silent file, not another output's mix.
    pub audio: Option<Arc<dyn AudioClip>>,
    /// Codec and rate for this output only.
    pub encode: OutputEncodeOptions,
}

/// Materialized video (+ optional audio) from a [`RenderGraph`].
#[derive(Clone)]
pub struct GraphBundle {
    /// Video stream of the primary output.
    pub video: Arc<dyn VideoClip>,
    /// Optional audio (source companion or graph audio ops).
    pub audio: Option<Arc<dyn AudioClip>>,
    /// Encode / output hints for the primary output.
    pub hints: GraphEncodeHints,
    /// Hints from the walk, before the primary branch's encode was copied on top.
    pub(crate) base_hints: GraphEncodeHints,
    /// Every `GraphOutput` that has a URI, each with its own media.
    pub outputs: Vec<GraphOutputMedia>,
}

/// One node product while walking the DAG.
#[derive(Clone)]
pub(crate) struct NodeMedia {
    pub(crate) video: Arc<dyn VideoClip>,
    pub(crate) audio: Option<Arc<dyn AudioClip>>,
    pub(crate) masks: Option<reelforge_render_graph::MaskTimeline>,
    pub(crate) encode: OutputEncodeOptions,
}

impl NodeMedia {
    pub(crate) fn new(video: Arc<dyn VideoClip>, audio: Option<Arc<dyn AudioClip>>) -> Self {
        Self {
            video,
            audio,
            masks: None,
            encode: OutputEncodeOptions::default(),
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
    /// Adapter executors (builtins by default).
    pub adapter_registry: crate::AdapterRegistry,
    /// Optional GPU device host.
    pub gpu_host: Option<Arc<dyn crate::GpuHost>>,
    /// GPU executors (builtins by default).
    pub gpu_registry: crate::GpuRegistry,
    /// Skip in-process eval for stages before this index (job resume).
    pub resume_from_stage: u32,
    /// Restored node clips from validated stage artifacts.
    pub restored_video: HashMap<String, Arc<dyn VideoClip>>,
    /// Audio restored with each node, when the artifact had a stream.
    pub restored_audio: HashMap<String, Arc<dyn AudioClip>>,
    /// Masks from the artifact sidecar. A missing sidecar leaves this empty.
    pub restored_masks: HashMap<String, reelforge_render_graph::MaskTimeline>,
    /// Branch encode settings from the sidecar. A missing sidecar leaves this empty.
    pub restored_encode: HashMap<String, crate::StageEncodeState>,
    /// Persist each completed stage under this directory.
    pub persist_stage_dir: Option<PathBuf>,
    /// Invoked after a stage is committed (fingerprint + artifacts).
    pub on_stage_committed: Option<std::sync::Arc<dyn Fn(crate::StageCommit) + Send + Sync>>,
    /// How incomplete source metadata is admitted. Defaults to legacy.
    pub source_admission: SourceAdmission,
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
            adapter_registry: crate::AdapterRegistry::with_builtins(),
            gpu_host: None,
            gpu_registry: crate::GpuRegistry::with_builtins(),
            resume_from_stage: 0,
            restored_video: HashMap::new(),
            restored_audio: HashMap::new(),
            restored_masks: HashMap::new(),
            restored_encode: HashMap::new(),
            persist_stage_dir: None,
            on_stage_committed: None,
            source_admission: SourceAdmission::Legacy,
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

    /// Choose legacy defaults or strict production admission.
    #[must_use]
    pub fn with_source_admission(mut self, admission: SourceAdmission) -> Self {
        self.source_admission = admission;
        self
    }

    /// Install a vision adapter host (`SightLoom` / tests).
    #[must_use]
    pub fn with_adapter_host(mut self, host: Arc<dyn crate::AdapterHost>) -> Self {
        self.adapter_host = Some(host);
        self
    }

    /// Replace the adapter executor registry.
    #[must_use]
    pub fn with_adapter_registry(mut self, registry: crate::AdapterRegistry) -> Self {
        self.adapter_registry = registry;
        self
    }

    /// Install a GPU device host.
    #[must_use]
    pub fn with_gpu_host(mut self, host: Arc<dyn crate::GpuHost>) -> Self {
        self.gpu_host = Some(host);
        self
    }

    /// Replace the GPU executor registry.
    #[must_use]
    pub fn with_gpu_registry(mut self, registry: crate::GpuRegistry) -> Self {
        self.gpu_registry = registry;
        self
    }

    /// Resume from a validated prefix (skip completed stages).
    #[must_use]
    pub fn with_stage_resume(mut self, plan: crate::StageResumePlan) -> Self {
        self.resume_from_stage = plan.start_stage;
        self.restored_video = plan.restored_video;
        self.restored_audio = plan.restored_audio;
        self.restored_masks = plan.restored_masks;
        self.restored_encode = plan.restored_encode;
        self
    }

    /// Persist each completed stage under `dir`.
    #[must_use]
    pub fn with_stage_persist_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.persist_stage_dir = Some(dir.into());
        self
    }

    /// Hooks passed into the stage walker.
    #[must_use]
    pub fn stage_hooks(&self) -> crate::StageRunHooks {
        crate::StageRunHooks {
            start_stage: self.resume_from_stage,
            restored_video: self.restored_video.clone(),
            restored_audio: self.restored_audio.clone(),
            restored_masks: self.restored_masks.clone(),
            restored_encode: self.restored_encode.clone(),
            persist_dir: self.persist_stage_dir.clone(),
            on_committed: self.on_stage_committed.clone(),
        }
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
            .field("adapter_registry", &self.adapter_registry.len())
            .field("gpu_host", &self.gpu_host.is_some())
            .field("gpu_registry", &self.gpu_registry.len())
            .field("resume_from_stage", &self.resume_from_stage)
            .field("restored_video", &self.restored_video.len())
            .field("restored_audio", &self.restored_audio.len())
            .field("restored_masks", &self.restored_masks.len())
            .field("restored_encode", &self.restored_encode.len())
            .field("persist_stage_dir", &self.persist_stage_dir)
            .field("on_stage_committed", &self.on_stage_committed.is_some())
            .field("source_admission", &self.source_admission)
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

/// Run a graph and return the sealed [`reelforge_render_graph::ArtifactManifest`] (output URIs + file hashes).
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

    let multi_output = graph.outputs.len() > 1;
    let run_fp = options
        .cache
        .as_ref()
        .filter(|_| !multi_output)
        .map(|_| execution_cache_key(graph, plan, options))
        .transpose()?;

    if let (Some(cache), Some(fp)) = (&options.cache, &run_fp)
        && let Some(cached) = cache.hit(fp, "mp4")
    {
        restore_cached_outputs(graph, &cached)?;
        control.report(WriteProgress::new(WriteStage::Done, 1, 1));
        return finish_manifest(compiled, plan, None);
    }

    // Hybrid FFmpeg prefix is video-only and does not apply declared still
    // holds. Strict admission stays on the in-process source path.
    if options.source_admission == SourceAdmission::Legacy
        && !options.with_audio
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
    let mut bundle = materialize_plan_admitted(
        graph,
        plan,
        &options.registry,
        &seeds,
        &audio_seeds,
        options.with_audio,
        Some(control),
        options.cache.as_ref(),
        crate::AdapterContext {
            host: options.adapter_host.clone(),
            registry: options.adapter_registry.clone(),
        },
        crate::GpuContext {
            host: options.gpu_host.clone(),
            registry: options.gpu_registry.clone(),
        },
        Some(&options.stage_hooks()),
        options.source_admission,
    )?;
    merge_option_hints(&mut bundle.hints, options);
    write_bundle_outputs(graph, &bundle, options, control, true)?;
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
/// Resolves video files via [`open_video`] and still images via [`crate::ImageClip`].
/// For tests, prefer [`materialize_graph_with_seeds`].
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
    materialize_graph_bundle_admitted(
        graph,
        registry,
        video_seeds,
        audio_seeds,
        with_audio,
        SourceAdmission::Legacy,
    )
}

fn materialize_graph_bundle_admitted<S: BuildHasher, A: BuildHasher>(
    graph: &RenderGraph,
    registry: &OperationRegistry,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
    admission: SourceAdmission,
) -> Result<GraphBundle> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    let order = graph
        .topo_order()
        .map_err(|e| IoError::message(e.to_string()))?;
    let mut ctx = MaterializeCtx::new(
        graph,
        registry,
        with_audio,
        crate::AdapterContext::default(),
        crate::GpuContext::default(),
        admission,
    );
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
    materialize_execution_plan_with_adapters(
        graph,
        plan,
        registry,
        video_seeds,
        audio_seeds,
        with_audio,
        control,
        cache,
        crate::AdapterContext::default(),
        crate::GpuContext::default(),
        None,
    )
}

/// Like [`materialize_execution_plan`] with an explicit [`crate::AdapterContext`].
///
/// # Errors
///
/// Same as [`materialize_execution_plan`].
#[allow(clippy::too_many_arguments)]
pub fn materialize_execution_plan_with_adapters<S: BuildHasher, A: BuildHasher>(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    registry: &OperationRegistry,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
    control: Option<&WriteControl>,
    cache: Option<&StageCache>,
    adapters: crate::AdapterContext,
    gpu: crate::GpuContext,
    hooks: Option<&crate::StageRunHooks>,
) -> Result<GraphBundle> {
    materialize_plan_admitted(
        graph,
        plan,
        registry,
        video_seeds,
        audio_seeds,
        with_audio,
        control,
        cache,
        adapters,
        gpu,
        hooks,
        SourceAdmission::Legacy,
    )
}

#[allow(clippy::too_many_arguments)]
fn materialize_plan_admitted<S: BuildHasher, A: BuildHasher>(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    registry: &OperationRegistry,
    video_seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
    audio_seeds: &HashMap<MediaAssetId, Arc<dyn AudioClip>, A>,
    with_audio: bool,
    control: Option<&WriteControl>,
    cache: Option<&StageCache>,
    adapters: crate::AdapterContext,
    gpu: crate::GpuContext,
    hooks: Option<&crate::StageRunHooks>,
    admission: SourceAdmission,
) -> Result<GraphBundle> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;

    if plan.stages.is_empty() {
        // Empty plan: fall back to full topo (tests / hand-built plans).
        return materialize_graph_bundle_admitted(
            graph,
            registry,
            video_seeds,
            audio_seeds,
            with_audio,
            admission,
        );
    }

    let mut ctx = MaterializeCtx::new(graph, registry, with_audio, adapters, gpu, admission);
    if let Some(h) = hooks {
        inject_restored(&mut ctx.produced, h);
    }
    let mut upstream_fp = asset_input_fingerprint(graph);
    let total_stages = plan.stages.len();
    #[allow(clippy::cast_possible_truncation)]
    let total_u = total_stages as u64;
    let start_stage = hooks.map_or(0, |h| h.start_stage as usize);

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

        if si < start_stage {
            upstream_fp = stage_fp;
            continue;
        }

        for id in node_ids {
            ctx.eval_node(id, video_seeds, audio_seeds)?;
        }

        if let Some(h) = hooks {
            let mut artifacts = Vec::new();
            if let Some(dir) = h.persist_dir.as_ref() {
                let ids = stage_frontier_ids(plan, si, node_ids);
                artifacts = persist_finished_stage(dir, si, &stage_fp, &ids, &ctx.produced)?;
            }
            if let Some(cb) = &h.on_committed {
                cb(crate::StageCommit {
                    index: u32::try_from(si).unwrap_or(u32::MAX),
                    fingerprint: stage_fp.clone(),
                    artifacts,
                });
            }
        }

        // Next stage inputs depend on this stage's work.
        upstream_fp = stage_fp;
    }

    ctx.finish_bundle()
}

fn inject_restored(produced: &mut HashMap<String, NodeMedia>, hooks: &crate::StageRunHooks) {
    for (id, clip) in &hooks.restored_video {
        let mut media = NodeMedia::new(Arc::clone(clip), hooks.restored_audio.get(id).cloned());
        if let Some(masks) = hooks.restored_masks.get(id) {
            media.masks = Some(masks.clone());
        }
        if let Some(encode) = hooks.restored_encode.get(id) {
            media.encode = encode_from_state(encode);
        }
        produced.insert(id.clone(), media);
    }
}

fn encode_from_state(state: &crate::StageEncodeState) -> OutputEncodeOptions {
    OutputEncodeOptions {
        fps: state.fps,
        video_codec: state.video_codec.clone(),
        crf: state.crf,
        preserve_audio: state.preserve_audio,
        path: state.path.clone(),
    }
}

fn encode_state_of(encode: &OutputEncodeOptions) -> crate::StageEncodeState {
    crate::StageEncodeState {
        fps: encode.fps,
        video_codec: encode.video_codec.clone(),
        crf: encode.crf,
        preserve_audio: encode.preserve_audio,
        path: encode.path.clone(),
    }
}

/// Nodes that leave the stage: a later stage or a graph output consumes them.
///
/// Plans without per-stage ports persist every node, which is the older path.
pub(crate) fn stage_frontier_ids(
    plan: &ExecutionPlan,
    stage_index: usize,
    node_ids: &[NodeId],
) -> Vec<String> {
    let Some(io) = plan.stage_io(stage_index) else {
        return node_ids.iter().map(|id| id.0.clone()).collect();
    };
    if io.nodes.len() != node_ids.len() {
        return node_ids.iter().map(|id| id.0.clone()).collect();
    }
    let mut out = Vec::new();
    for port in &io.outputs {
        let Some(pos) = io.nodes.iter().position(|node| *node == port.node) else {
            continue;
        };
        if let Some(id) = node_ids.get(pos) {
            out.push(id.0.clone());
        }
    }
    out
}

fn persist_finished_stage(
    dir: &Path,
    stage_index: usize,
    fingerprint: &str,
    ids: &[String],
    produced: &HashMap<String, NodeMedia>,
) -> Result<Vec<crate::StageArtifactRecord>> {
    let mut artifacts = Vec::new();
    for id in ids {
        let Some(media) = produced.get(id) else {
            continue;
        };
        let encode = encode_state_of(&media.encode);
        artifacts.push(crate::stage_resume::persist_stage_media(
            dir,
            u32::try_from(stage_index).unwrap_or(u32::MAX),
            fingerprint,
            id,
            &crate::stage_resume::StageMediaParts {
                video: media.video.as_ref(),
                audio: media.audio.as_deref(),
                masks: media.masks.as_ref(),
                encode: &encode,
            },
        )?);
    }
    Ok(artifacts)
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
    source_admission: SourceAdmission,
}

impl<'a> MaterializeCtx<'a> {
    fn new(
        graph: &'a RenderGraph,
        registry: &'a OperationRegistry,
        with_audio: bool,
        adapters: crate::AdapterContext,
        gpu: crate::GpuContext,
        source_admission: SourceAdmission,
    ) -> Self {
        Self {
            graph,
            registry,
            node_map: graph.nodes.iter().map(|n| (n.id.0.as_str(), n)).collect(),
            asset_map: graph.assets.iter().map(|a| (a.id.0.as_str(), a)).collect(),
            produced: HashMap::new(),
            hints: GraphEncodeHints {
                preserve_audio: with_audio,
                adapter_host: adapters.host,
                adapter_registry: adapters.registry,
                gpu_host: gpu.host,
                gpu_registry: gpu.registry,
                ..GraphEncodeHints::default()
            },
            primary_out: None,
            last_stage_fingerprint: None,
            stage_fingerprints: Vec::new(),
            source_admission,
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
                self.source_admission,
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
                    encode: input.encode,
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
        let mut outputs = Vec::new();
        for out in &self.graph.outputs {
            let Some(uri) = out.uri.clone() else {
                continue;
            };
            let Some(c) = self.produced.get(&out.node.0) else {
                return Err(IoError::message(format!(
                    "output '{}' node '{}' was not produced",
                    out.name, out.node.0
                )));
            };
            outputs.push(GraphOutputMedia {
                uri,
                video: Arc::clone(&c.video),
                audio: c.audio.clone(),
                encode: c.encode.clone(),
            });
        }
        let base_hints = self.hints.clone();
        if let Some(first) = outputs.first() {
            overlay_encode(&mut self.hints, &first.encode);
            self.hints
                .output_path
                .get_or_insert_with(|| first.uri.clone());
            return Ok(GraphBundle {
                video: Arc::clone(&first.video),
                audio: first.audio.clone(),
                hints: self.hints,
                base_hints,
                outputs,
            });
        }
        if let Some(c) = self.primary_out {
            overlay_encode(&mut self.hints, &c.encode);
            return Ok(GraphBundle {
                video: c.video,
                audio: c.audio,
                hints: self.hints,
                base_hints,
                outputs,
            });
        }
        Err(IoError::message(
            "RenderGraph produced no output clip (missing Output node?)",
        ))
    }
}

pub(crate) fn execution_cache_key(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    options: &GraphRunOptions,
) -> Result<String> {
    let base = StageCache::run_fingerprint(graph, plan)?;
    let sources = crate::stage_cache::source_set_fingerprint(
        graph
            .assets
            .iter()
            .map(|asset| (asset.id.0.as_str(), asset.uri.as_str())),
    );
    let admission = match options.source_admission {
        SourceAdmission::Legacy => "legacy",
        SourceAdmission::Strict => "strict",
    };
    Ok(format!(
        "{base}|fps={}|codec={}|crf={}|audio={}|admit={admission}|src={sources}",
        options
            .fps
            .map(|fps| format!("{fps:.6}"))
            .unwrap_or_default(),
        options.video_codec.as_deref().unwrap_or(""),
        options.crf.map(|crf| crf.to_string()).unwrap_or_default(),
        u8::from(options.with_audio)
    ))
}

fn asset_input_fingerprint(graph: &RenderGraph) -> String {
    crate::stage_cache::source_set_fingerprint(
        graph
            .assets
            .iter()
            .map(|asset| (asset.id.0.as_str(), asset.uri.as_str())),
    )
}

/// Live fingerprints for each plan stage (same keys the runner uses).
///
/// # Errors
///
/// Unknown ops / compile failures.
pub fn plan_stage_fingerprints(
    graph: &RenderGraph,
    plan: &ExecutionPlan,
    registry: &OperationRegistry,
) -> Result<Vec<String>> {
    let mut upstream_fp = asset_input_fingerprint(graph);
    let mut out = Vec::with_capacity(plan.stages.len());
    for stage in &plan.stages {
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
        out.push(stage_fp.clone());
        upstream_fp = stage_fp;
    }
    Ok(out)
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
    admission: SourceAdmission,
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
    if let Some(still) = open_admitted_still(path, meta, admission)? {
        return Ok(still);
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RasterClass {
    Still,
    Animated,
    Other,
}

fn extension_raster(path: &Path) -> RasterClass {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return RasterClass::Other;
    };
    match ext.to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" => RasterClass::Still,
        "gif" => RasterClass::Animated,
        _ => RasterClass::Other,
    }
}

fn open_admitted_still(
    path: &Path,
    meta: &reelforge_render_graph::MediaAsset,
    admission: SourceAdmission,
) -> Result<Option<NodeMedia>> {
    let by_ext = extension_raster(path);
    match admission {
        SourceAdmission::Legacy => {
            if by_ext == RasterClass::Other {
                return Ok(None);
            }
            Ok(Some(still_node(path, meta, admission)?))
        }
        SourceAdmission::Strict => match (by_ext, sniff_raster(path)?) {
            (RasterClass::Animated, _) | (_, RasterClass::Animated) => {
                Err(IoError::message(format!(
                    "strict source admission does not open animated file '{}' as a still",
                    meta.uri
                )))
            }
            (RasterClass::Still, RasterClass::Still) => {
                Ok(Some(still_node(path, meta, admission)?))
            }
            (RasterClass::Still, RasterClass::Other) | (RasterClass::Other, RasterClass::Still) => {
                Err(IoError::message(format!(
                    "strict source admission rejects '{}': extension and image signature disagree",
                    meta.uri
                )))
            }
            (RasterClass::Other, RasterClass::Other) => Ok(None),
        },
    }
}

fn still_node(
    path: &Path,
    meta: &reelforge_render_graph::MediaAsset,
    admission: SourceAdmission,
) -> Result<NodeMedia> {
    let clip = crate::ImageClip::from_path(path, admitted_still_hold(meta, admission)?)?;
    Ok(NodeMedia::new(Arc::new(clip), None))
}

fn admitted_still_hold(
    meta: &reelforge_render_graph::MediaAsset,
    admission: SourceAdmission,
) -> Result<reelforge_core::Duration> {
    if let Some(time) = meta.duration {
        let duration = time.to_duration();
        if duration.is_positive() {
            return Ok(duration);
        }
    }
    match admission {
        SourceAdmission::Legacy => Ok(LEGACY_STILL_HOLD),
        SourceAdmission::Strict => Err(IoError::message(format!(
            "strict source admission rejects still '{}' with no positive duration",
            meta.id.0
        ))),
    }
}

fn sniff_raster(path: &Path) -> Result<RasterClass> {
    let mut file = std::fs::File::open(path)
        .map_err(|err| IoError::message(format!("open source {}: {err}", path.display())))?;
    let mut header = [0_u8; 32];
    let read = file
        .read(&mut header)
        .map_err(|err| IoError::message(format!("read source {}: {err}", path.display())))?;
    let header = &header[..read];
    if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        return Ok(RasterClass::Animated);
    }
    if header.starts_with(&PNG_SIGNATURE) {
        let mut buf = vec![0_u8; RASTER_SCAN_BYTES];
        let copy = read.min(buf.len());
        buf[..copy].copy_from_slice(&header[..copy]);
        let rest = file
            .read(&mut buf[copy..])
            .map_err(|err| IoError::message(format!("read source {}: {err}", path.display())))?;
        let total = copy + rest;
        if png_declares_animation(&buf[..total]) {
            return Ok(RasterClass::Animated);
        }
        return Ok(RasterClass::Still);
    }
    if header.starts_with(&[0xFF, 0xD8, 0xFF]) || header.starts_with(b"BM") {
        return Ok(RasterClass::Still);
    }
    if header.len() >= 12 && header.starts_with(b"RIFF") && &header[8..12] == b"WEBP" {
        if header.len() >= 21 && &header[12..16] == b"VP8X" && (header[20] & 0x02) != 0 {
            return Ok(RasterClass::Animated);
        }
        return Ok(RasterClass::Still);
    }
    Ok(RasterClass::Other)
}

fn png_declares_animation(bytes: &[u8]) -> bool {
    if bytes.len() < PNG_SIGNATURE.len() || bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
        return false;
    }
    let mut index = PNG_SIGNATURE.len();
    while index + 8 <= bytes.len() {
        let Ok(len_bytes) = bytes[index..index + 4].try_into() else {
            return false;
        };
        let len = usize::try_from(u32::from_be_bytes(len_bytes)).unwrap_or(usize::MAX);
        let kind = &bytes[index + 4..index + 8];
        if kind == b"acTL" {
            return true;
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return false;
        }
        let Some(next) = index.checked_add(12).and_then(|pos| pos.checked_add(len)) else {
            return false;
        };
        if next > bytes.len() {
            return false;
        }
        index = next;
    }
    false
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
    Ok(NodeMedia::new(Arc::new(video), Some(Arc::new(audio))))
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

fn overlay_encode(hints: &mut GraphEncodeHints, encode: &OutputEncodeOptions) {
    if let Some(fps) = encode.fps {
        hints.fps = Some(fps);
    }
    if let Some(codec) = &encode.video_codec {
        hints.video_codec = Some(codec.clone());
    }
    if let Some(crf) = encode.crf {
        hints.crf = Some(crf);
    }
    if let Some(keep) = encode.preserve_audio {
        hints.preserve_audio = keep;
    }
    if hints.output_path.is_none()
        && let Some(path) = &encode.path
    {
        hints.output_path = Some(path.clone());
    }
}

fn hints_for_output(
    base: &GraphEncodeHints,
    encode: &OutputEncodeOptions,
    options: &GraphRunOptions,
) -> GraphEncodeHints {
    let mut hints = base.clone();
    overlay_encode(&mut hints, encode);
    // Call-site fps, codec, and crf win over the branch. preserve_audio stays
    // on the branch because the run flag is already in `base`.
    let preserve = hints.preserve_audio;
    merge_option_hints(&mut hints, options);
    hints.preserve_audio = preserve;
    hints
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

/// Write each materialized branch.
///
/// `include_audio` stays false on the video-only hybrid prefix. That prefix
/// has already dropped companion audio, and one branch must not reuse another's.
fn write_bundle_outputs(
    graph: &RenderGraph,
    bundle: &GraphBundle,
    options: &GraphRunOptions,
    control: &WriteControl,
    include_audio: bool,
) -> Result<()> {
    if bundle.outputs.is_empty() {
        let audio = if include_audio {
            bundle.audio.as_deref()
        } else {
            None
        };
        return write_graph_outputs(graph, bundle.video.as_ref(), audio, &bundle.hints, control);
    }
    for out in &bundle.outputs {
        control.check_cancel()?;
        let hints = hints_for_output(&bundle.base_hints, &out.encode, options);
        let audio = if include_audio {
            out.audio.as_deref()
        } else {
            None
        };
        write_one_output(&out.uri, out.video.as_ref(), audio, &hints, control)?;
    }
    control.report(WriteProgress::new(WriteStage::Done, 1, 1));
    Ok(())
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

    for path in paths {
        control.check_cancel()?;
        write_one_output(&path, clip, audio, hints, control)?;
    }
    control.report(WriteProgress::new(WriteStage::Done, 1, 1));
    Ok(())
}

fn write_one_output(
    path: &str,
    clip: &dyn VideoClip,
    audio: Option<&dyn AudioClip>,
    hints: &GraphEncodeHints,
    control: &WriteControl,
) -> Result<()> {
    let fps = resolve_fps(hints, clip)?;
    let mut opts = WriteVideoOptions::new(path, fps);
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
    let linear = plan
        .stages
        .first()
        .is_some_and(|stage| ffmpeg_stage_is_line(graph, stage.node_ids()));
    has_rust && first_ffmpeg && graph.assets.len() == 1 && linear
}

/// True when the stage is a single path. A shared upstream with two consumers
/// is a legal DAG and must not be rewritten into one `FFmpeg` chain.
fn ffmpeg_stage_is_line(graph: &RenderGraph, nodes: &[reelforge_render_graph::NodeId]) -> bool {
    if nodes.is_empty() {
        return false;
    }
    let set: HashSet<&str> = nodes.iter().map(|n| n.0.as_str()).collect();
    let mut consumers: HashMap<&str, u32> = HashMap::new();
    for node in &graph.nodes {
        for input in &node.inputs {
            if set.contains(input.0.as_str()) {
                *consumers.entry(input.0.as_str()).or_default() += 1;
            }
        }
    }
    let mut heads = 0_u32;
    for node in &graph.nodes {
        if !set.contains(node.id.0.as_str()) {
            continue;
        }
        let indeg = node
            .inputs
            .iter()
            .filter(|input| set.contains(input.0.as_str()))
            .count();
        if indeg > 1 {
            return false;
        }
        if indeg == 0 {
            heads += 1;
        }
        if consumers.get(node.id.0.as_str()).copied().unwrap_or(0) > 1 {
            return false;
        }
    }
    heads <= 1
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
        write_bundle_outputs(graph, &bundle, options, control, false)
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
    use crate::ImageClip;
    use reelforge_core::{
        AlphaMode, AudioFormat, ColorClip, Duration, Frame, FrameFormat, MediaTime, Rgb8, Rgba8,
        SilenceClip, Size, Time,
    };
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

    fn rust_chain_graph() -> RenderGraph {
        chain_graph(vec![
            RenderNode {
                id: NodeId("invert".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.color.invert"),
                    params: serde_json::json!({}),
                },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("bw".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.color.black_and_white"),
                    params: serde_json::json!({}),
                },
                inputs: vec![NodeId("invert".into())],
            },
        ])
    }

    fn mask_resume_graph() -> RenderGraph {
        chain_graph(vec![RenderNode {
            id: NodeId("redact".into()),
            body: RenderNodeKind::Redaction {
                redaction: RegionRedaction {
                    masks: MaskTimeline::new(),
                    style: RedactionStyle::Solid {
                        color: Rgba8::BLACK,
                    },
                },
            },
            inputs: vec![NodeId("src".into())],
        }])
    }

    fn chain_graph(middle: Vec<RenderNode>) -> RenderGraph {
        let mut nodes = vec![RenderNode {
            id: NodeId("src".into()),
            body: RenderNodeKind::Source {
                asset: MediaAssetId("a".into()),
            },
            inputs: vec![],
        }];
        let last = if let Some(node) = middle.last() {
            node.id.0.clone()
        } else {
            "src".into()
        };
        nodes.extend(middle);
        nodes.push(RenderNode {
            id: NodeId("out".into()),
            body: RenderNodeKind::Output {
                name: "main".into(),
            },
            inputs: vec![NodeId(last)],
        });
        RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "seed://color".into(),
                duration: None,
                role: Some("video".into()),
            }],
            nodes,
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
    fn resume_skips_prefix_when_nodes_restored() {
        let g = linear_redaction_graph();
        let registry = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &registry).unwrap();
        assert!(plan.stage_count() >= 2);
        let seed: Arc<dyn VideoClip> = Arc::new(
            ColorClip::new(Size::new(16, 16), Rgb8::RED, Duration::from_secs(1.0)).with_fps(15.0),
        );
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), Arc::clone(&seed));
        let audio: HashMap<MediaAssetId, Arc<dyn AudioClip>> = HashMap::new();
        let mut restored = HashMap::new();
        for id in plan.stages[0].node_ids() {
            restored.insert(id.0.clone(), Arc::clone(&seed));
        }
        let hooks = crate::StageRunHooks {
            start_stage: 1,
            restored_video: restored,
            ..crate::StageRunHooks::default()
        };
        let bundle = materialize_execution_plan_with_adapters(
            &g,
            &plan,
            &registry,
            &seeds,
            &audio,
            true,
            None,
            None,
            crate::AdapterContext::default(),
            crate::GpuContext::default(),
            Some(&hooks),
        )
        .unwrap();
        assert!(bundle.video.duration().as_secs() > 0.0);
        let _ = bundle.video.frame_at(Time::ZERO).unwrap();
    }

    #[test]
    fn checkpoint_frontier_omits_internal_nodes() {
        let g = rust_chain_graph();
        let registry = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &registry).unwrap();
        let (index, stage) = plan
            .stages
            .iter()
            .enumerate()
            .find(|(_, stage)| stage.backend_tag() == "rust")
            .expect("rust stage");
        let ids: Vec<&str> = stage.node_ids().iter().map(|id| id.0.as_str()).collect();
        assert!(ids.contains(&"invert"));
        assert!(ids.contains(&"bw"));
        let frontier = stage_frontier_ids(&plan, index, stage.node_ids());
        assert!(frontier.iter().any(|id| id == "bw"));
        assert!(!frontier.iter().any(|id| id == "invert"));
    }

    #[test]
    fn checkpoint_persist_writes_frontier_only() {
        if !crate::ffmpeg_available() {
            return;
        }
        let g = rust_chain_graph();
        let registry = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &registry).unwrap();
        let seed: Arc<dyn VideoClip> = Arc::new(
            ColorClip::new(Size::new(32, 32), Rgb8::WHITE, Duration::from_secs(0.4)).with_fps(10.0),
        );
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let audio: HashMap<MediaAssetId, Arc<dyn AudioClip>> = HashMap::new();
        let dir = tempfile::tempdir().unwrap();
        let written = Arc::new(std::sync::Mutex::new(Vec::new()));
        let written2 = Arc::clone(&written);
        let hooks = crate::StageRunHooks {
            persist_dir: Some(dir.path().to_path_buf()),
            on_committed: Some(Arc::new(move |commit| {
                written2
                    .lock()
                    .unwrap()
                    .extend(commit.artifacts.iter().map(|rec| rec.node_id.clone()));
            })),
            ..crate::StageRunHooks::default()
        };
        materialize_execution_plan_with_adapters(
            &g,
            &plan,
            &registry,
            &seeds,
            &audio,
            true,
            None,
            None,
            crate::AdapterContext::default(),
            crate::GpuContext::default(),
            Some(&hooks),
        )
        .unwrap();
        let ids = written.lock().unwrap().clone();
        assert!(ids.iter().any(|id| id == "bw"));
        assert!(ids.iter().any(|id| id == "src"));
        assert!(!ids.iter().any(|id| id == "invert"));
    }

    #[test]
    fn checkpoint_resume_keeps_audio_masks_and_encode() {
        if !crate::ffmpeg_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let g = mask_resume_graph();
        let registry = OperationRegistry::with_builtins();
        let plan = schedule_graph(&g, &registry).unwrap();
        let expected = plan_stage_fingerprints(&g, &plan, &registry).unwrap();
        assert!(plan.stage_count() >= 2);
        let duration = Duration::from_secs(0.4);
        let video = ColorClip::new(Size::new(32, 32), Rgb8::WHITE, duration).with_fps(10.0);
        let audio = SilenceClip::new(AudioFormat::STEREO_48K, duration);
        let mut masks = MaskTimeline::new();
        masks.push(MaskSample::ellipse(
            MediaTime::new(0, 30).unwrap(),
            16.0,
            16.0,
            8.0,
        ));
        let encode = crate::StageEncodeState {
            crf: Some(21),
            fps: Some(12.0),
            preserve_audio: Some(true),
            ..crate::StageEncodeState::default()
        };
        let rec = crate::stage_resume::persist_stage_media(
            dir.path(),
            0,
            &expected[0],
            "src",
            &crate::stage_resume::StageMediaParts {
                video: &video,
                audio: Some(&audio),
                masks: Some(&masks),
                encode: &encode,
            },
        )
        .unwrap();
        let resume = crate::restore_validated_prefix(&[rec], &expected).unwrap();
        assert_eq!(resume.start_stage, 1);
        assert!(resume.restored_audio.contains_key("src"));
        assert_eq!(resume.restored_masks.get("src"), Some(&masks));
        assert_eq!(
            resume
                .restored_encode
                .get("src")
                .and_then(|state| state.crf),
            Some(21)
        );
        let hooks = GraphRunOptions::new()
            .with_stage_resume(resume)
            .stage_hooks();
        let seeds: HashMap<MediaAssetId, Arc<dyn VideoClip>> = HashMap::new();
        let audio_seeds: HashMap<MediaAssetId, Arc<dyn AudioClip>> = HashMap::new();
        let bundle = materialize_execution_plan_with_adapters(
            &g,
            &plan,
            &registry,
            &seeds,
            &audio_seeds,
            true,
            None,
            None,
            crate::AdapterContext::default(),
            crate::GpuContext::default(),
            Some(&hooks),
        )
        .unwrap();
        assert!(bundle.audio.is_some());
        assert_eq!(bundle.hints.crf, Some(21));
        let frame = bundle.video.frame_at(Time::ZERO).unwrap();
        assert_eq!(rgb_at(&frame, 32, 16, 16), [0, 0, 0]);
        assert_eq!(rgb_at(&frame, 32, 0, 0), [255, 255, 255]);
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
        assert!(is_executable_op("rf.transform.freeze"));
        assert!(is_executable_op("rf.transform.loop"));
        assert!(is_executable_op("rf.adapter.sightloom"));
        assert!(is_executable_op("rf.gpu.passthrough"));
        assert!(is_executable_op("rf.encode.hw"));
        assert!(!is_executable_op("rf.not.real"));
    }

    #[test]
    fn gpu_stage_passthrough_and_hw_hint() {
        let registry = OperationRegistry::with_builtins();
        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(16, 16),
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
                    id: NodeId("gpu".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.gpu.passthrough"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("enc".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.encode.hw"),
                        params: serde_json::json!({ "codec": "libx264" }),
                    },
                    inputs: vec![NodeId("gpu".into())],
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
                uri: None,
            }],
        };
        let plan = schedule_graph(&g, &registry).unwrap();
        assert!(
            plan.stages
                .iter()
                .any(|s| matches!(s, ExecutionStage::Gpu(_))),
            "gpu ops must schedule as GPU: {plan:?}"
        );
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let (clip, hints) = materialize_graph_with_seeds(&g, &registry, &seeds).unwrap();
        assert!((clip.duration().as_secs() - 1.0).abs() < 1e-9);
        assert_eq!(hints.video_codec.as_deref(), Some("libx264"));
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

    #[test]
    fn drop_then_mix_keeps_audio_on_the_output() {
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
                    id: NodeId("drop".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.audio.drop"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("mix".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.audio.mix"),
                        params: serde_json::json!({ "tracks": [{}, {}] }),
                    },
                    inputs: vec![NodeId("drop".into()), NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("mix".into())],
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
        assert!(bundle.audio.is_some(), "mix must restore audio after drop");
        assert!(bundle.hints.preserve_audio);
        assert_eq!(bundle.outputs.len(), 1);
        assert!(bundle.outputs[0].audio.is_some());
    }

    #[test]
    fn cache_key_changes_with_fps() {
        let graph = RenderGraph::default();
        let plan = ExecutionPlan::default();
        let slow = GraphRunOptions {
            fps: Some(5.0),
            ..GraphRunOptions::default()
        };
        let fast = GraphRunOptions {
            fps: Some(10.0),
            ..GraphRunOptions::default()
        };
        let a = execution_cache_key(&graph, &plan, &slow).unwrap();
        let b = execution_cache_key(&graph, &plan, &fast).unwrap();
        let again = execution_cache_key(&graph, &plan, &slow).unwrap();
        assert_ne!(a, b);
        assert_eq!(a, again);
    }

    #[test]
    fn cache_key_tracks_source_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.mp4");
        std::fs::write(&src, b"frame-a").unwrap();
        let graph = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: src.to_string_lossy().into_owned(),
                duration: None,
                role: None,
            }],
            nodes: vec![],
            outputs: vec![],
        };
        let plan = ExecutionPlan::default();
        let options = GraphRunOptions::default();
        let first = execution_cache_key(&graph, &plan, &options).unwrap();
        let again = execution_cache_key(&graph, &plan, &options).unwrap();
        assert_eq!(first, again);
        std::fs::write(&src, b"frame-b").unwrap();
        let changed = execution_cache_key(&graph, &plan, &options).unwrap();
        assert_ne!(first, changed);
        std::fs::write(&src, b"frame-a").unwrap();
        let restored = execution_cache_key(&graph, &plan, &options).unwrap();
        assert_eq!(first, restored);
    }

    #[test]
    fn distinct_outputs_keep_their_picture_and_encode() {
        let registry = OperationRegistry::with_builtins();
        let video: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
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
                    id: NodeId("inv".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.color.invert"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("enc_orig".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.encode.h264"),
                        params: serde_json::json!({ "crf": 18, "fps": 10.0, "path": "orig.mp4" }),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("enc_inv".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.encode.h264"),
                        params: serde_json::json!({ "crf": 40, "fps": 24.0, "path": "inv.mp4" }),
                    },
                    inputs: vec![NodeId("inv".into())],
                },
                RenderNode {
                    id: NodeId("out_orig".into()),
                    body: RenderNodeKind::Output {
                        name: "original".into(),
                    },
                    inputs: vec![NodeId("enc_orig".into())],
                },
                RenderNode {
                    id: NodeId("out_inv".into()),
                    body: RenderNodeKind::Output {
                        name: "invert".into(),
                    },
                    inputs: vec![NodeId("enc_inv".into())],
                },
            ],
            outputs: vec![
                GraphOutput {
                    name: "original".into(),
                    node: NodeId("out_orig".into()),
                    uri: Some("orig.mp4".into()),
                },
                GraphOutput {
                    name: "invert".into(),
                    node: NodeId("out_inv".into()),
                    uri: Some("inv.mp4".into()),
                },
            ],
        };
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), video);
        let audio = HashMap::new();
        let bundle = materialize_graph_bundle(&g, &registry, &seeds, &audio, true).unwrap();
        assert_eq!(bundle.outputs.len(), 2);
        let orig = &bundle.outputs[0];
        let inv = &bundle.outputs[1];
        assert_eq!(
            &orig.video.frame_at(Time::ZERO).unwrap().data()[0..3],
            &[255, 255, 255]
        );
        assert_eq!(
            &inv.video.frame_at(Time::ZERO).unwrap().data()[0..3],
            &[0, 0, 0]
        );
        assert_eq!(orig.encode.crf, Some(18));
        assert_eq!(inv.encode.crf, Some(40));
        assert!((orig.encode.fps.unwrap() - 10.0).abs() < 1e-9);
        assert!((inv.encode.fps.unwrap() - 24.0).abs() < 1e-9);
        assert_eq!(bundle.hints.crf, Some(18));
        assert!((bundle.hints.fps.unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn run_options_override_branch_crf_only() {
        let base = GraphEncodeHints::default();
        let branch = OutputEncodeOptions {
            crf: Some(40),
            fps: Some(24.0),
            ..OutputEncodeOptions::default()
        };
        let options = GraphRunOptions {
            crf: Some(30),
            ..GraphRunOptions::default()
        };
        let forced = hints_for_output(&base, &branch, &options);
        assert_eq!(forced.crf, Some(30));
        assert!((forced.fps.unwrap() - 24.0).abs() < 1e-9);
    }

    fn split_seed() -> Arc<dyn VideoClip> {
        let mut data = vec![0_u8; 4 * 2 * 3];
        for y in 0..2 {
            for x in 0..4 {
                let i = (y * 4 + x) * 3;
                if x < 2 {
                    data[i] = 255;
                } else {
                    data[i + 2] = 255;
                }
            }
        }
        let frame = Frame::from_raw(Size::new(4, 2), FrameFormat::Rgb8, data).unwrap();
        Arc::new(ImageClip::from_frame(frame, Duration::from_secs(1.0)).unwrap())
    }

    fn rgb_at(frame: &Frame, width: u32, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * width as usize + x as usize) * 3;
        let data = frame.data();
        [data[i], data[i + 1], data[i + 2]]
    }

    fn source_node() -> RenderNode {
        RenderNode {
            id: NodeId("src".into()),
            body: RenderNodeKind::Source {
                asset: MediaAssetId("a".into()),
            },
            inputs: vec![],
        }
    }

    #[test]
    fn diamond_branches_keep_distinct_pixels() {
        let registry = OperationRegistry::with_builtins();
        let g = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "seed://a".into(),
                duration: None,
                role: Some("video".into()),
            }],
            nodes: vec![
                source_node(),
                RenderNode {
                    id: NodeId("h".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.hflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("v".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.vflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("mix".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.compose.layers"),
                        params: serde_json::json!({
                            "w": 8,
                            "h": 2,
                            "layers": [{ "x": 0 }, { "x": 4 }]
                        }),
                    },
                    inputs: vec![NodeId("v".into()), NodeId("h".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("mix".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        };
        g.validate().unwrap();
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), split_seed());
        let audio = HashMap::new();
        let bundle = materialize_graph_bundle(&g, &registry, &seeds, &audio, true).unwrap();
        let frame = bundle.video.frame_at(Time::ZERO).unwrap();
        assert_eq!(rgb_at(&frame, 8, 0, 0), [255, 0, 0]);
        assert_eq!(rgb_at(&frame, 8, 2, 0), [0, 0, 255]);
        assert_eq!(rgb_at(&frame, 8, 4, 0), [0, 0, 255]);
        assert_eq!(rgb_at(&frame, 8, 6, 0), [255, 0, 0]);
    }

    #[test]
    fn repeated_upstream_paints_both_ports() {
        let registry = OperationRegistry::with_builtins();
        let g = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "seed://a".into(),
                duration: None,
                role: Some("video".into()),
            }],
            nodes: vec![
                source_node(),
                RenderNode {
                    id: NodeId("mix".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.compose.layers"),
                        params: serde_json::json!({
                            "w": 8,
                            "h": 2,
                            "layers": [{ "x": 0 }, { "x": 4 }]
                        }),
                    },
                    inputs: vec![NodeId("src".into()), NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("mix".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: Some("out.mp4".into()),
            }],
        };
        g.validate().unwrap();
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), split_seed());
        let audio = HashMap::new();
        let bundle = materialize_graph_bundle(&g, &registry, &seeds, &audio, true).unwrap();
        let frame = bundle.video.frame_at(Time::ZERO).unwrap();
        assert_eq!(rgb_at(&frame, 8, 0, 0), [255, 0, 0]);
        assert_eq!(rgb_at(&frame, 8, 4, 0), [255, 0, 0]);
        assert_eq!(rgb_at(&frame, 8, 6, 0), [0, 0, 255]);
    }

    #[test]
    fn stripped_diamond_keeps_a_repeated_upstream() {
        let g = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "seed://a".into(),
                duration: None,
                role: Some("video".into()),
            }],
            nodes: vec![
                source_node(),
                RenderNode {
                    id: NodeId("h".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.hflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("v".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.vflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("mix".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.compose.layers"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("v".into()), NodeId("h".into())],
                },
            ],
            outputs: vec![],
        };
        let reduced = strip_and_rewire(&g, &HashSet::from(["h".into(), "v".into()]));
        let mix = reduced.nodes.iter().find(|n| n.id.0 == "mix").unwrap();
        assert_eq!(mix.inputs[0].0, "src");
        assert_eq!(mix.inputs[1].0, "src");
        reduced.validate().unwrap();
    }

    #[test]
    fn branched_stage_is_not_a_line() {
        let graph = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("a".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("h".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.hflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
                RenderNode {
                    id: NodeId("v".into()),
                    body: RenderNodeKind::Op {
                        operation: OperationId::new("rf.transform.vflip"),
                        params: serde_json::json!({}),
                    },
                    inputs: vec![NodeId("src".into())],
                },
            ],
            outputs: vec![],
        };
        let nodes = vec![NodeId("src".into()), NodeId("h".into()), NodeId("v".into())];
        assert!(!ffmpeg_stage_is_line(&graph, &nodes));
        // `src` still feeds `v`, so an FFmpeg prefix must not consume it for `h` alone.
        assert!(!ffmpeg_stage_is_line(
            &graph,
            &[NodeId("src".into()), NodeId("h".into())]
        ));
        let mut line = graph.clone();
        line.nodes.pop();
        assert!(ffmpeg_stage_is_line(
            &line,
            &[NodeId("src".into()), NodeId("h".into())]
        ));
    }

    #[test]
    fn still_source_holds_its_duration_and_alpha() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.png");
        let mut img = image::RgbaImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
        img.put_pixel(1, 0, image::Rgba([255, 0, 0, 0]));
        img.save(&path).unwrap();
        let graph = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("photo".into()),
                uri: path.to_string_lossy().into_owned(),
                duration: Some(MediaTime::from_secs(1.5, 1_000).unwrap()),
                role: Some("image".into()),
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("photo".into()),
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
                uri: None,
            }],
        };
        let clip = materialize_graph(&graph).unwrap();
        let held = clip.duration().as_secs();
        assert!((held - 1.5).abs() < 1e-9, "held {held}");
        let frame = clip.frame_at(Time::from_secs(1.4)).unwrap();
        assert_eq!(frame.format(), FrameFormat::Rgba8);
        assert_eq!(frame.alpha_mode(), AlphaMode::Straight);
        assert_eq!(&frame.data()[0..8], &[255, 0, 0, 255, 255, 0, 0, 0]);
        assert!(clip.frame_at(Time::from_secs(1.5)).is_err());

        let mut unset = graph.clone();
        unset.assets[0].duration = None;
        let fallback = materialize_graph(&unset).unwrap().duration().as_secs();
        assert!((LEGACY_STILL_HOLD.as_secs() - 1.0).abs() < 1e-9);
        assert!(
            (fallback - LEGACY_STILL_HOLD.as_secs()).abs() < 1e-9,
            "fallback {fallback}"
        );
        unset.assets[0].duration = Some(MediaTime::zero(1_000));
        let zero = materialize_graph(&unset).unwrap().duration().as_secs();
        assert!(
            (zero - LEGACY_STILL_HOLD.as_secs()).abs() < 1e-9,
            "zero {zero}"
        );
    }

    fn still_photo_graph(path: &std::path::Path, duration: Option<MediaTime>) -> RenderGraph {
        RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("photo".into()),
                uri: path.to_string_lossy().into_owned(),
                duration,
                role: Some("image".into()),
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("photo".into()),
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
                uri: None,
            }],
        }
    }

    fn materialize_admitted(
        graph: &RenderGraph,
        admission: SourceAdmission,
    ) -> Result<Arc<dyn VideoClip>> {
        let registry = OperationRegistry::with_builtins();
        let video = HashMap::<MediaAssetId, Arc<dyn VideoClip>>::new();
        let audio = HashMap::<MediaAssetId, Arc<dyn AudioClip>>::new();
        Ok(
            materialize_graph_bundle_admitted(graph, &registry, &video, &audio, true, admission)?
                .video,
        )
    }

    fn admission_error(graph: &RenderGraph, admission: SourceAdmission) -> String {
        let Err(err) = materialize_admitted(graph, admission) else {
            panic!("source admission accepted a source that must be rejected");
        };
        err.to_string()
    }

    #[test]
    fn strict_still_requires_a_positive_duration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.png");
        image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]))
            .save(&path)
            .unwrap();
        let missing = still_photo_graph(&path, None);
        let missing_text = admission_error(&missing, SourceAdmission::Strict);
        assert!(
            missing_text.contains("no positive duration"),
            "{missing_text}"
        );
        assert!(!missing_text.contains("1.0"), "{missing_text}");

        let mut zero = missing.clone();
        zero.assets[0].duration = Some(MediaTime::zero(1_000));
        let zero_text = admission_error(&zero, SourceAdmission::Strict);
        assert!(zero_text.contains("no positive duration"), "{zero_text}");

        let declared = still_photo_graph(&path, Some(MediaTime::from_secs(1.5, 1_000).unwrap()));
        let held = materialize_admitted(&declared, SourceAdmission::Strict)
            .unwrap()
            .duration()
            .as_secs();
        assert!((held - 1.5).abs() < 1e-9, "held {held}");

        let plan = schedule_graph(&missing, &OperationRegistry::with_builtins()).unwrap();
        let video = HashMap::<MediaAssetId, Arc<dyn VideoClip>>::new();
        let audio = HashMap::<MediaAssetId, Arc<dyn AudioClip>>::new();
        let Err(err) = materialize_plan_admitted(
            &missing,
            &plan,
            &OperationRegistry::with_builtins(),
            &video,
            &audio,
            true,
            None,
            None,
            crate::AdapterContext::default(),
            crate::GpuContext::default(),
            None,
            SourceAdmission::Strict,
        ) else {
            panic!("strict plan accepted a still with no duration");
        };
        let planned = err.to_string();
        assert!(planned.contains("no positive duration"), "{planned}");
    }

    #[test]
    fn strict_still_rejects_animated_sources() {
        let dir = tempfile::tempdir().unwrap();
        let gif = dir.path().join("loop.gif");
        let frame = image::Frame::new(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([0, 255, 0, 255]),
        ));
        let file = std::fs::File::create(&gif).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        encoder.encode_frame(frame).unwrap();

        let legacy =
            materialize_admitted(&still_photo_graph(&gif, None), SourceAdmission::Legacy).unwrap();
        assert!((legacy.duration().as_secs() - LEGACY_STILL_HOLD.as_secs()).abs() < 1e-9);

        let gif_text = admission_error(&still_photo_graph(&gif, None), SourceAdmission::Strict);
        assert!(gif_text.contains("animated"), "{gif_text}");

        let disguised = dir.path().join("still.png");
        std::fs::copy(&gif, &disguised).unwrap();
        let disguised_text = admission_error(
            &still_photo_graph(&disguised, None),
            SourceAdmission::Strict,
        );
        assert!(disguised_text.contains("animated"), "{disguised_text}");

        let webp = dir.path().join("move.webp");
        let mut header = vec![0_u8; 30];
        header[0..4].copy_from_slice(b"RIFF");
        header[8..12].copy_from_slice(b"WEBP");
        header[12..16].copy_from_slice(b"VP8X");
        header[20] = 0x02;
        std::fs::write(&webp, &header).unwrap();
        let webp_text = admission_error(&still_photo_graph(&webp, None), SourceAdmission::Strict);
        assert!(webp_text.contains("animated"), "{webp_text}");

        let apng = dir.path().join("anim.png");
        let mut bytes = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        bytes.extend_from_slice(&13_u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0_u8; 17]);
        bytes.extend_from_slice(&8_u32.to_be_bytes());
        bytes.extend_from_slice(b"acTL");
        bytes.extend_from_slice(&[0_u8; 12]);
        std::fs::write(&apng, &bytes).unwrap();
        let apng_text = admission_error(&still_photo_graph(&apng, None), SourceAdmission::Strict);
        assert!(apng_text.contains("animated"), "{apng_text}");

        let junk = dir.path().join("junk.png");
        std::fs::write(&junk, b"not-a-png").unwrap();
        let junk_text = admission_error(&still_photo_graph(&junk, None), SourceAdmission::Strict);
        assert!(junk_text.contains("signature"), "{junk_text}");
        let legacy_junk = admission_error(&still_photo_graph(&junk, None), SourceAdmission::Legacy);
        assert!(
            !legacy_junk.contains("strict source admission"),
            "{legacy_junk}"
        );
    }

    #[test]
    fn still_admission_changes_the_cache_key() {
        let graph = RenderGraph::default();
        let plan = ExecutionPlan::default();
        let legacy = GraphRunOptions::new();
        let strict = legacy
            .clone()
            .with_source_admission(SourceAdmission::Strict);
        let legacy_key = execution_cache_key(&graph, &plan, &legacy).unwrap();
        let strict_key = execution_cache_key(&graph, &plan, &strict).unwrap();
        assert_ne!(legacy_key, strict_key);
        assert!(legacy_key.contains("admit=legacy"), "{legacy_key}");
        assert!(strict_key.contains("admit=strict"), "{strict_key}");
    }
}
