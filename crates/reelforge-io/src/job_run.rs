//! Run / resume a [`RenderJob`] against a [`RenderGraph`].

use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::error::{IoError, Result};
use crate::graph_run::{GraphRunOptions, plan_stage_fingerprints, run_render_graph_with_manifest};
use crate::job::{JobState, RenderJob};
use crate::job_store::JobStore;
use crate::stage_cache::StageCache;
use crate::stage_resume::{StageCommit, restore_validated_prefix};
use reelforge_render_graph::{ArtifactManifest, ExecutionPlan, RenderGraph, schedule_graph};
use std::path::Path;

/// Create a queued job with the graph+plan fingerprint filled in.
///
/// # Errors
///
/// Invalid graph, schedule, or store write.
pub fn submit_render_job(
    store: &JobStore,
    graph: &RenderGraph,
    options: &GraphRunOptions,
) -> Result<RenderJob> {
    graph
        .validate()
        .map_err(|e| IoError::message(e.to_string()))?;
    let plan =
        schedule_graph(graph, &options.registry).map_err(|e| IoError::message(e.to_string()))?;
    let fp = StageCache::run_fingerprint(graph, &plan)?;
    let mut job = RenderJob::new(crate::job::JobId::generate()).with_fingerprint(fp);
    job.checkpoint.total_stages = u32::try_from(plan.stages.len()).unwrap_or(u32::MAX);
    if let Some(uri) = first_output_uri(graph) {
        job.output_uri = Some(uri);
    }
    store.save(&job)?;
    Ok(job)
}

/// Execute or resume `job`. A `Done` job with the same fingerprint is a no-op.
///
/// Cancel (`IoError::Cancelled`) persists [`JobState::Paused`]. Other errors
/// persist [`JobState::Failed`]. Success persists [`JobState::Done`].
///
/// Completed stages with a valid on-disk artifact are skipped. In-process
/// stages without a file still re-run. A matching full-run [`StageCache`]
/// hit skips encode. Capture owns retry / queue policy.
///
/// # Errors
///
/// Graph / encode / store failures, or cancel.
pub fn run_render_job(
    store: &JobStore,
    job: &mut RenderJob,
    graph: &RenderGraph,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<ArtifactManifest> {
    let plan =
        schedule_graph(graph, &options.registry).map_err(|e| IoError::message(e.to_string()))?;
    let fp = StageCache::run_fingerprint(graph, &plan)?;
    if job.state == JobState::Done
        && job.run_fingerprint.as_deref() == Some(fp.as_str())
        && output_ready(job)
    {
        return already_done_manifest(graph, &plan);
    }
    if job.run_fingerprint.as_deref() != Some(fp.as_str()) {
        job.checkpoint.next_stage = 0;
        job.checkpoint.stage_fingerprints.clear();
        job.checkpoint.stage_artifacts.clear();
    }
    job.run_fingerprint = Some(fp);
    job.checkpoint.total_stages = u32::try_from(plan.stages.len()).unwrap_or(u32::MAX);
    job.state = JobState::Running;
    job.error = None;
    job.touch();
    store.save(job)?;

    let expected = plan_stage_fingerprints(graph, &plan, &options.registry)?;
    let resume = restore_validated_prefix(&job.checkpoint.stage_artifacts, &expected)?;
    job.checkpoint.next_stage = resume.start_stage;

    let mut options = options.clone();
    options.persist_stage_dir = Some(store.stages_dir(&job.id));
    let store_cb = store.clone();
    let job_id = job.id.clone();
    options.on_stage_committed = Some(std::sync::Arc::new(move |commit: StageCommit| {
        if let Ok(mut live) = store_cb.load(&job_id) {
            live.checkpoint.next_stage = commit.index.saturating_add(1);
            live.checkpoint
                .stage_fingerprints
                .truncate(commit.index as usize);
            live.checkpoint.stage_fingerprints.push(commit.fingerprint);
            live.checkpoint
                .stage_artifacts
                .retain(|a| a.stage_index != commit.index);
            live.checkpoint.stage_artifacts.extend(commit.artifacts);
            live.touch();
            let _ = store_cb.save(&live);
        }
    }));
    options = options.with_stage_resume(resume);

    let control = checkpointing_control(store, job, control);
    match run_render_graph_with_manifest(graph, &control, &options) {
        Ok(manifest) => {
            if let Ok(live) = store.load(&job.id) {
                job.checkpoint = live.checkpoint;
            }
            job.state = JobState::Done;
            job.checkpoint.next_stage = job.checkpoint.total_stages;
            job.output_uri = first_output_uri(graph).or(job.output_uri.clone());
            job.error = None;
            job.touch();
            store.save(job)?;
            Ok(manifest)
        }
        Err(e) => {
            if let Ok(live) = store.load(&job.id) {
                job.checkpoint = live.checkpoint;
            }
            if matches!(e, IoError::Cancelled) {
                job.state = JobState::Paused;
            } else {
                job.state = JobState::Failed;
            }
            job.error = Some(e.to_string());
            job.touch();
            store.save(job)?;
            Err(e)
        }
    }
}

/// Resume alias: same as [`run_render_job`].
///
/// # Errors
///
/// Same as [`run_render_job`].
pub fn resume_render_job(
    store: &JobStore,
    job: &mut RenderJob,
    graph: &RenderGraph,
    control: &WriteControl,
    options: &GraphRunOptions,
) -> Result<ArtifactManifest> {
    run_render_job(store, job, graph, control, options)
}

fn checkpointing_control(
    store: &JobStore,
    job: &RenderJob,
    control: &WriteControl,
) -> WriteControl {
    let store = store.clone();
    let id = job.id.clone();
    let prev = control.clone();
    WriteControl {
        cancel: control.cancel.clone(),
        max_in_flight: control.max_in_flight,
        on_progress: Some(std::sync::Arc::new(move |p: WriteProgress| {
            prev.report(p);
            if p.stage != WriteStage::Plan {
                return;
            }
            if let Ok(mut live) = store.load(&id) {
                #[allow(clippy::cast_possible_truncation)]
                {
                    live.checkpoint.next_stage =
                        u32::try_from(p.index.saturating_add(1)).unwrap_or(u32::MAX);
                    live.checkpoint.total_stages =
                        u32::try_from(p.total).unwrap_or(live.checkpoint.total_stages);
                }
                live.touch();
                let _ = store.save(&live);
            }
        })),
    }
}

fn first_output_uri(graph: &RenderGraph) -> Option<String> {
    graph.outputs.iter().find_map(|o| o.uri.clone())
}

fn output_ready(job: &RenderJob) -> bool {
    job.output_uri
        .as_ref()
        .is_some_and(|u| Path::new(u).is_file())
}

fn already_done_manifest(graph: &RenderGraph, plan: &ExecutionPlan) -> Result<ArtifactManifest> {
    let compiled = reelforge_render_graph::compile_graph(
        graph,
        &reelforge_render_graph::OperationRegistry::with_builtins(),
    )
    .map_err(|e| IoError::message(e.to_string()))?;
    Ok(reelforge_render_graph::artifact_manifest(&compiled, plan))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::CancelToken;
    use crate::job::JobId;
    use reelforge_render_graph::{
        GraphOutput, MediaAsset, MediaAssetId, NodeId, RENDER_GRAPH_VERSION, RenderNode,
        RenderNodeKind,
    };

    fn tiny_graph() -> RenderGraph {
        RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "missing.mp4".into(),
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
        }
    }

    #[test]
    fn submit_then_cancel_pauses() {
        let dir = tempfile::tempdir().unwrap();
        let store = JobStore::open(dir.path()).unwrap();
        let g = tiny_graph();
        let opts = GraphRunOptions::default();
        let mut job = submit_render_job(&store, &g, &opts).unwrap();
        assert_eq!(job.state, JobState::Queued);
        assert!(job.run_fingerprint.is_some());

        let token = CancelToken::new();
        token.cancel();
        let control = WriteControl::new().with_cancel(token);
        let err = run_render_job(&store, &mut job, &g, &control, &opts).unwrap_err();
        assert!(matches!(err, IoError::Cancelled));
        let live = store.load(&job.id).unwrap();
        assert_eq!(live.state, JobState::Paused);
    }

    #[test]
    fn done_with_matching_fp_skips_when_output_exists() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.mp4");
        std::fs::write(&out, b"x").unwrap();
        let store = JobStore::open(dir.path().join("jobs")).unwrap();
        let mut g = tiny_graph();
        g.outputs[0].uri = Some(out.to_string_lossy().into());
        let opts = GraphRunOptions::default();
        let mut job = submit_render_job(&store, &g, &opts).unwrap();
        job.state = JobState::Done;
        job.output_uri = g.outputs[0].uri.clone();
        store.save(&job).unwrap();
        let manifest =
            run_render_job(&store, &mut job, &g, &WriteControl::default(), &opts).unwrap();
        assert!(!manifest.outputs.is_empty());
        assert_eq!(store.load(&job.id).unwrap().state, JobState::Done);
        let _ = JobId::new("x");
    }

    #[test]
    fn resume_plan_stops_at_missing_artifact() {
        let rec = crate::StageArtifactRecord::new(0, "fp", "n", "no-such-stage.mp4");
        assert_eq!(crate::first_invalid_stage(&[rec], 4), 0);
    }
}
