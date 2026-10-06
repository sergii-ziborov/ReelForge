//! Run / resume a [`RenderJob`] against a [`RenderGraph`].

use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::error::{IoError, Result};
use crate::graph_run::{
    GraphRunOptions, execution_cache_key, plan_stage_fingerprints, run_render_graph_with_manifest,
    stage_frontier_ids,
};
use crate::job::{JobOutputRecord, JobState, RenderJob};
use crate::job_store::JobStore;
use crate::manifest_seal::fingerprint_file;
use crate::stage_resume::{CheckpointFidelity, StageCommit, restore_validated_prefix_for};
use reelforge_render_graph::{ArtifactManifest, ExecutionPlan, RenderGraph, schedule_graph};
use std::fs;
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
    let fp = execution_cache_key(graph, &plan, options)?;
    let mut job = RenderJob::new(crate::job::JobId::generate()).with_fingerprint(fp);
    job.checkpoint.total_stages = u32::try_from(plan.stages.len()).unwrap_or(u32::MAX);
    if let Some(uri) = first_output_uri(graph) {
        job.output_uri = Some(uri);
    }
    store.save(&job)?;
    Ok(job)
}

/// Execute or resume `job`. A `Done` preview job with the same fingerprint and
/// a ready output is a no-op. A final job whose output manifest is empty, or
/// does not name every graph output, is recomputed.
///
/// Cancel (`IoError::Cancelled`) persists [`JobState::Paused`]. Other errors
/// persist [`JobState::Failed`]. Success persists [`JobState::Done`].
///
/// Completed stages with a valid on-disk artifact are skipped. In-process
/// stages without a file still re-run. A matching full-run [`crate::StageCache`]
/// hit skips encode. The fingerprint includes encode options and source bytes.
/// Capture owns retry / queue policy.
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
    let fp = execution_cache_key(graph, &plan, options)?;
    if job.state == JobState::Done
        && job.run_fingerprint.as_deref() == Some(fp.as_str())
        && outputs_ready(job, graph, options.checkpoint_fidelity)
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
    let required = plan
        .stages
        .iter()
        .enumerate()
        .map(|(index, stage)| stage_frontier_ids(&plan, index, stage.node_ids()))
        .collect::<Vec<_>>();
    let resume = restore_validated_prefix_for(
        &job.checkpoint.stage_artifacts,
        &expected,
        &required,
        options.checkpoint_fidelity,
    )?;
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
            let outputs = match seal_job_outputs(graph) {
                Ok(outputs) => outputs,
                Err(e) => {
                    job.state = JobState::Failed;
                    job.error = Some(e.to_string());
                    job.touch();
                    store.save(job)?;
                    return Err(e);
                }
            };
            job.outputs = outputs;
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

/// Jobs sealed with [`JobOutputRecord`] must match every declared output.
/// Older Done jobs, which have an empty manifest, keep the single-file check
/// for a preview. A final run does not treat that empty manifest as done, and
/// its sealed records must name every graph output once.
fn outputs_ready(job: &RenderJob, graph: &RenderGraph, fidelity: CheckpointFidelity) -> bool {
    if job.outputs.is_empty() {
        return fidelity != CheckpointFidelity::FinalLossless && output_ready(job);
    }
    let declared: Vec<&str> = graph
        .outputs
        .iter()
        .filter_map(|output| output.uri.as_deref())
        .collect();
    if declared.is_empty() {
        return fidelity != CheckpointFidelity::FinalLossless && output_ready(job);
    }
    let files_match = declared.iter().all(|uri| {
        job.outputs.iter().any(|record| {
            record.uri == *uri && sealed_file_matches(&record.uri, &record.file_fingerprint)
        })
    });
    if !files_match {
        return false;
    }
    fidelity != CheckpointFidelity::FinalLossless || final_output_membership(job, graph)
}

/// Every graph output has one sealed record with its name and uri.
fn final_output_membership(job: &RenderJob, graph: &RenderGraph) -> bool {
    if job.outputs.len() != graph.outputs.len() {
        return false;
    }
    graph.outputs.iter().all(|output| {
        let Some(uri) = output.uri.as_deref() else {
            return false;
        };
        job.outputs
            .iter()
            .filter(|record| {
                record.name == output.name
                    && record.uri == uri
                    && sealed_file_matches(uri, &record.file_fingerprint)
            })
            .count()
            == 1
    })
}

fn sealed_file_matches(uri: &str, expected: &str) -> bool {
    let path = Path::new(uri);
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => {}
        _ => return false,
    }
    fingerprint_file(path).is_ok_and(|got| got == expected)
}

fn seal_job_outputs(graph: &RenderGraph) -> Result<Vec<JobOutputRecord>> {
    let mut sealed = Vec::new();
    for output in &graph.outputs {
        let Some(uri) = output.uri.as_deref() else {
            continue;
        };
        let path = Path::new(uri);
        if !path.is_file() {
            return Err(IoError::message(format!("render output missing: {uri}")));
        }
        sealed.push(JobOutputRecord {
            name: output.name.clone(),
            uri: uri.to_string(),
            file_fingerprint: fingerprint_file(path)?,
        });
    }
    Ok(sealed)
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

    fn two_output_graph(primary: &std::path::Path, secondary: &std::path::Path) -> RenderGraph {
        let mut graph = tiny_graph();
        graph.nodes.push(RenderNode {
            id: NodeId("out_b".into()),
            body: RenderNodeKind::Output { name: "alt".into() },
            inputs: vec![NodeId("src".into())],
        });
        graph.outputs[0].uri = Some(primary.to_string_lossy().into());
        graph.outputs.push(GraphOutput {
            name: "alt".into(),
            node: NodeId("out_b".into()),
            uri: Some(secondary.to_string_lossy().into()),
        });
        graph
    }

    #[test]
    fn done_shortcut_rejects_a_broken_primary_and_a_missing_secondary() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary.mp4");
        let secondary = dir.path().join("secondary.mp4");
        std::fs::write(&primary, b"primary-bytes").unwrap();
        std::fs::write(&secondary, b"secondary-bytes").unwrap();
        let store = JobStore::open(dir.path().join("jobs")).unwrap();
        let graph = two_output_graph(&primary, &secondary);
        let opts = GraphRunOptions::default();
        let mut job = submit_render_job(&store, &graph, &opts).unwrap();
        job.state = JobState::Done;
        job.output_uri = Some(primary.to_string_lossy().into());
        job.outputs = vec![
            crate::JobOutputRecord {
                name: "main".into(),
                uri: primary.to_string_lossy().into(),
                file_fingerprint: crate::fingerprint_file(&primary).unwrap(),
            },
            crate::JobOutputRecord {
                name: "alt".into(),
                uri: secondary.to_string_lossy().into(),
                file_fingerprint: crate::fingerprint_file(&secondary).unwrap(),
            },
        ];
        store.save(&job).unwrap();
        let manifest =
            run_render_job(&store, &mut job, &graph, &WriteControl::default(), &opts).unwrap();
        assert!(!manifest.outputs.is_empty());
        assert_eq!(store.load(&job.id).unwrap().state, JobState::Done);

        std::fs::write(&primary, b"broken").unwrap();
        std::fs::remove_file(&secondary).unwrap();
        let err =
            run_render_job(&store, &mut job, &graph, &WriteControl::default(), &opts).unwrap_err();
        assert!(!matches!(err, IoError::Cancelled), "{err}");
        assert_eq!(std::fs::read(&primary).unwrap(), b"broken");
        assert!(!secondary.is_file());
        assert_ne!(store.load(&job.id).unwrap().state, JobState::Done);
    }

    #[test]
    fn strict_final_does_not_shortcut_an_empty_output_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.mp4");
        std::fs::write(&out, b"legacy-primary").unwrap();
        let store = JobStore::open(dir.path().join("jobs")).unwrap();
        let mut graph = tiny_graph();
        graph.outputs[0].uri = Some(out.to_string_lossy().into());
        let opts = GraphRunOptions::default()
            .with_checkpoint_fidelity(crate::CheckpointFidelity::FinalLossless);
        let mut job = submit_render_job(&store, &graph, &opts).unwrap();
        job.state = JobState::Done;
        job.output_uri = graph.outputs[0].uri.clone();
        assert!(job.outputs.is_empty());
        store.save(&job).unwrap();

        let err =
            run_render_job(&store, &mut job, &graph, &WriteControl::default(), &opts).unwrap_err();
        assert!(!matches!(err, IoError::Cancelled), "{err}");
        assert_eq!(std::fs::read(&out).unwrap(), b"legacy-primary");
        assert_ne!(store.load(&job.id).unwrap().state, JobState::Done);
    }

    #[test]
    fn strict_final_keeps_a_sealed_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary.mp4");
        let secondary = dir.path().join("secondary.mp4");
        std::fs::write(&primary, b"primary-bytes").unwrap();
        std::fs::write(&secondary, b"secondary-bytes").unwrap();
        let store = JobStore::open(dir.path().join("jobs")).unwrap();
        let graph = two_output_graph(&primary, &secondary);
        let opts = GraphRunOptions::default()
            .with_checkpoint_fidelity(crate::CheckpointFidelity::FinalLossless);
        let mut job = submit_render_job(&store, &graph, &opts).unwrap();
        job.state = JobState::Done;
        job.output_uri = Some(primary.to_string_lossy().into());
        job.outputs = vec![
            crate::JobOutputRecord {
                name: "main".into(),
                uri: primary.to_string_lossy().into(),
                file_fingerprint: crate::fingerprint_file(&primary).unwrap(),
            },
            crate::JobOutputRecord {
                name: "alt".into(),
                uri: secondary.to_string_lossy().into(),
                file_fingerprint: crate::fingerprint_file(&secondary).unwrap(),
            },
        ];
        store.save(&job).unwrap();
        let manifest =
            run_render_job(&store, &mut job, &graph, &WriteControl::default(), &opts).unwrap();
        assert!(!manifest.outputs.is_empty());
        assert_eq!(store.load(&job.id).unwrap().state, JobState::Done);
        assert_eq!(std::fs::read(&primary).unwrap(), b"primary-bytes");
        assert_eq!(std::fs::read(&secondary).unwrap(), b"secondary-bytes");
    }

    #[test]
    fn strict_final_rejects_a_manifest_that_renames_an_output() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("primary.mp4");
        let secondary = dir.path().join("secondary.mp4");
        std::fs::write(&primary, b"primary-bytes").unwrap();
        std::fs::write(&secondary, b"secondary-bytes").unwrap();
        let store = JobStore::open(dir.path().join("jobs")).unwrap();
        let graph = two_output_graph(&primary, &secondary);
        let preview = GraphRunOptions::default();
        let mut job = submit_render_job(&store, &graph, &preview).unwrap();
        job.state = JobState::Done;
        job.output_uri = Some(primary.to_string_lossy().into());
        job.outputs = vec![
            crate::JobOutputRecord {
                name: "main".into(),
                uri: primary.to_string_lossy().into(),
                file_fingerprint: crate::fingerprint_file(&primary).unwrap(),
            },
            crate::JobOutputRecord {
                name: "other".into(),
                uri: secondary.to_string_lossy().into(),
                file_fingerprint: crate::fingerprint_file(&secondary).unwrap(),
            },
        ];
        store.save(&job).unwrap();
        let manifest =
            run_render_job(&store, &mut job, &graph, &WriteControl::default(), &preview).unwrap();
        assert!(!manifest.outputs.is_empty());
        assert_eq!(store.load(&job.id).unwrap().state, JobState::Done);

        let final_run = preview.with_checkpoint_fidelity(crate::CheckpointFidelity::FinalLossless);
        job.run_fingerprint = submit_render_job(&store, &graph, &final_run)
            .unwrap()
            .run_fingerprint;
        job.state = JobState::Done;
        store.save(&job).unwrap();
        let err = run_render_job(
            &store,
            &mut job,
            &graph,
            &WriteControl::default(),
            &final_run,
        )
        .unwrap_err();
        assert!(!matches!(err, IoError::Cancelled), "{err}");
        assert_eq!(std::fs::read(&primary).unwrap(), b"primary-bytes");
        assert_eq!(std::fs::read(&secondary).unwrap(), b"secondary-bytes");
        assert_ne!(store.load(&job.id).unwrap().state, JobState::Done);
    }

    #[test]
    fn job_fingerprint_tracks_fps_and_source_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.bin");
        std::fs::write(&src, b"aaa").unwrap();
        let mut g = tiny_graph();
        g.assets[0].uri = src.to_string_lossy().into_owned();
        let store = JobStore::open(dir.path().join("jobs")).unwrap();
        let slow = GraphRunOptions {
            fps: Some(5.0),
            ..GraphRunOptions::default()
        };
        let fast = GraphRunOptions {
            fps: Some(10.0),
            ..GraphRunOptions::default()
        };
        let slow_job = submit_render_job(&store, &g, &slow).unwrap();
        let fast_job = submit_render_job(&store, &g, &fast).unwrap();
        assert_ne!(slow_job.run_fingerprint, fast_job.run_fingerprint);
        std::fs::write(&src, b"bbb").unwrap();
        let changed = submit_render_job(&store, &g, &slow).unwrap();
        assert_ne!(slow_job.run_fingerprint, changed.run_fingerprint);
        std::fs::write(&src, b"aaa").unwrap();
        let again = submit_render_job(&store, &g, &slow).unwrap();
        assert_eq!(slow_job.run_fingerprint, again.run_fingerprint);
    }

    #[test]
    fn resume_plan_stops_at_missing_artifact() {
        let rec = crate::StageArtifactRecord::new(0, "fp", "n", "no-such-stage.mp4");
        assert_eq!(crate::first_invalid_stage(&[rec], 4), 0);
    }
}
