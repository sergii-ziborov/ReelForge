//! Stage resume: persist / validate / restore intermediate job artifacts.

use crate::error::{IoError, Result};
use crate::job::StageArtifactRecord;
use crate::manifest_seal::fingerprint_file;
use crate::options::WriteVideoOptions;
use crate::video_file::open_video;
use crate::write::write_video;
use reelforge_core::VideoClip;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One committed stage (persist + checkpoint).
#[derive(Debug, Clone)]
pub struct StageCommit {
    /// Stage index that just finished.
    pub index: u32,
    /// Strong fingerprint.
    pub fingerprint: String,
    /// Files written for this stage's live nodes.
    pub artifacts: Vec<StageArtifactRecord>,
}

/// Optional resume / persist hooks for [`crate::materialize_execution_plan`].
#[derive(Clone, Default)]
pub struct StageRunHooks {
    /// Skip eval for stages `[0, start_stage)`.
    pub start_stage: u32,
    /// Restored clips keyed by graph node id.
    pub restored_video: HashMap<String, Arc<dyn VideoClip>>,
    /// When set, each completed stage is encoded under this directory.
    pub persist_dir: Option<PathBuf>,
    /// Called after a stage is evaluated (and persisted, when configured).
    pub on_committed: Option<std::sync::Arc<dyn Fn(StageCommit) + Send + Sync>>,
}

/// How far a job can skip, plus clips to inject for completed nodes.
#[derive(Clone, Default)]
pub struct StageResumePlan {
    /// First stage that must run (`plan.stages.len()` = only encode remains).
    pub start_stage: u32,
    /// Node id → restored video from a validated artifact.
    pub restored_video: HashMap<String, Arc<dyn VideoClip>>,
}

/// True when the file exists, is non-empty, and matches the stored hash.
#[must_use]
pub fn artifact_is_valid(rec: &StageArtifactRecord) -> bool {
    let path = Path::new(&rec.uri);
    match fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() > 0 => {}
        _ => return false,
    }
    let Some(expected) = rec.file_fingerprint.as_deref() else {
        return path.is_file();
    };
    fingerprint_file(path).is_ok_and(|got| got == expected)
}

/// Walk completed records in stage order. Stop at the first missing / stale slot.
#[must_use]
pub fn first_invalid_stage(artifacts: &[StageArtifactRecord], total_stages: u32) -> u32 {
    let mut expected = 0_u32;
    while expected < total_stages {
        let recs: Vec<&StageArtifactRecord> = artifacts
            .iter()
            .filter(|a| a.stage_index == expected)
            .collect();
        if recs.is_empty() || recs.iter().any(|r| !artifact_is_valid(r)) {
            return expected;
        }
        expected += 1;
    }
    total_stages
}

/// Open validated artifacts whose fingerprint still matches `expected_by_stage`.
///
/// `expected_by_stage[i]` is the live fingerprint for stage `i`. A mismatch
/// means the graph/plan/host changed and that stage must run again.
///
/// # Errors
///
/// `open_video` failures on a record that passed [`artifact_is_valid`].
pub fn restore_validated_prefix(
    artifacts: &[StageArtifactRecord],
    expected_by_stage: &[String],
) -> Result<StageResumePlan> {
    let mut plan = StageResumePlan::default();
    for (si, expected) in expected_by_stage.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let index = si as u32;
        let recs: Vec<&StageArtifactRecord> = artifacts
            .iter()
            .filter(|a| a.stage_index == index && a.fingerprint == *expected)
            .collect();
        if recs.is_empty() || recs.iter().any(|r| !artifact_is_valid(r)) {
            plan.start_stage = index;
            return Ok(plan);
        }
        for rec in recs {
            let clip = open_video(&crate::OpenVideoOptions::new(&rec.uri))?;
            plan.restored_video
                .insert(rec.node_id.clone(), Arc::new(clip));
        }
        plan.start_stage = index.saturating_add(1);
    }
    Ok(plan)
}

/// Write one stage output to `dir` and return its record.
///
/// # Errors
///
/// Encode or hash I/O.
pub fn persist_stage_video(
    dir: impl AsRef<Path>,
    stage_index: u32,
    fingerprint: &str,
    node_id: &str,
    clip: &dyn VideoClip,
) -> Result<StageArtifactRecord> {
    let dir = dir.as_ref();
    fs::create_dir_all(dir)
        .map_err(|e| IoError::message(format!("stage persist mkdir {}: {e}", dir.display())))?;
    let stem: String = fingerprint.chars().take(16).collect();
    let safe_node: String = node_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let path: PathBuf = dir.join(format!("s{stage_index}-{safe_node}-{stem}.mp4"));
    let fps = clip
        .fps()
        .filter(|f| f.is_finite() && *f > 0.0)
        .unwrap_or(15.0);
    let uri = path.to_string_lossy().into_owned();
    write_video(clip, &WriteVideoOptions::new(&uri, fps).with_crf(30))?;
    let file_fp = fingerprint_file(&path)?;
    Ok(
        StageArtifactRecord::new(stage_index, fingerprint, node_id, uri)
            .with_file_fingerprint(file_fp),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_invalid() {
        let rec = StageArtifactRecord::new(0, "abc", "n", "definitely-missing-rf-stage.mp4");
        assert!(!artifact_is_valid(&rec));
        assert_eq!(first_invalid_stage(&[rec], 3), 0);
    }

    #[test]
    fn valid_prefix_then_gap() {
        let dir = tempfile::tempdir().unwrap();
        let p0 = dir.path().join("s0.mp4");
        fs::write(&p0, b"hello").unwrap();
        let hash = fingerprint_file(&p0).unwrap();
        let a0 = StageArtifactRecord::new(0, "fp0", "n0", p0.to_string_lossy())
            .with_file_fingerprint(hash);
        let a1 = StageArtifactRecord::new(
            1,
            "fp1",
            "n1",
            dir.path().join("missing.mp4").to_string_lossy(),
        );
        assert!(artifact_is_valid(&a0));
        assert_eq!(first_invalid_stage(&[a0, a1], 3), 1);
    }

    #[test]
    fn stale_hash_invalidates() {
        let dir = tempfile::tempdir().unwrap();
        let p0 = dir.path().join("s0.mp4");
        fs::write(&p0, b"hello").unwrap();
        let rec = StageArtifactRecord::new(0, "fp0", "n0", p0.to_string_lossy())
            .with_file_fingerprint("deadbeef");
        assert!(!artifact_is_valid(&rec));
    }
}
