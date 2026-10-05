//! Stage resume: persist / validate / restore intermediate job artifacts.

use crate::error::{IoError, Result};
use crate::job::StageArtifactRecord;
use crate::manifest_seal::fingerprint_file;
use crate::options::WriteVideoOptions;
use crate::video_file::open_video;
use crate::write::{write_av, write_video};
use reelforge_core::{AudioClip, VideoClip};
use reelforge_render_graph::MaskTimeline;
use serde::{Deserialize, Serialize};
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

/// Encode settings stored beside a stage artifact.
///
/// Absent fields fall back to the run. A checkpoint with no sidecar restores
/// as [`Self::default`], so older records stay valid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StageEncodeState {
    /// Frames per second for this branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    /// Video codec name (`libx264`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<String>,
    /// Constant-rate factor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crf: Option<u8>,
    /// Mux companion audio when `Some(true)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preserve_audio: Option<bool>,
    /// Path recorded on the branch's encode node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl StageEncodeState {
    fn is_empty(&self) -> bool {
        self.fps.is_none()
            && self.video_codec.is_none()
            && self.crf.is_none()
            && self.preserve_audio.is_none()
            && self.path.is_none()
    }
}

/// Picture, audio, masks, and encode settings for one persisted node.
#[derive(Clone, Copy)]
pub(crate) struct StageMediaParts<'a> {
    pub(crate) video: &'a dyn VideoClip,
    pub(crate) audio: Option<&'a dyn AudioClip>,
    pub(crate) masks: Option<&'a MaskTimeline>,
    pub(crate) encode: &'a StageEncodeState,
}

/// Sidecar schema. Records that store [`StageArtifactRecord::sidecar_fingerprint`]
/// were written at this version.
const STAGE_MEDIA_SIDECAR_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StageMediaSidecar {
    /// `0` on checkpoints written before the field existed.
    #[serde(default)]
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    masks: Option<MaskTimeline>,
    #[serde(default, skip_serializing_if = "StageEncodeState::is_empty")]
    encode: StageEncodeState,
}

/// Optional resume / persist hooks for [`crate::materialize_execution_plan`].
#[derive(Clone, Default)]
pub struct StageRunHooks {
    /// Skip eval for stages `[0, start_stage)`.
    pub start_stage: u32,
    /// Restored clips keyed by graph node id.
    pub restored_video: HashMap<String, Arc<dyn VideoClip>>,
    /// Audio restored with each node, when the artifact had a stream.
    pub restored_audio: HashMap<String, Arc<dyn AudioClip>>,
    /// Masks from the artifact sidecar. A missing sidecar leaves this empty.
    pub restored_masks: HashMap<String, MaskTimeline>,
    /// Branch encode settings from the sidecar. A missing sidecar leaves this empty.
    pub restored_encode: HashMap<String, StageEncodeState>,
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
    /// Audio restored with each node, when the artifact had a stream.
    pub restored_audio: HashMap<String, Arc<dyn AudioClip>>,
    /// Masks from the artifact sidecar. A missing sidecar leaves this empty.
    pub restored_masks: HashMap<String, MaskTimeline>,
    /// Branch encode settings from the sidecar. A missing sidecar leaves this empty.
    pub restored_encode: HashMap<String, StageEncodeState>,
}

/// True when the video exists, is non-empty, and matches every stored hash.
///
/// A record with [`StageArtifactRecord::sidecar_fingerprint`] also requires
/// the sidecar file. A record without that hash is legacy: a missing sidecar
/// is allowed, and a present sidecar must still parse.
#[must_use]
pub fn artifact_is_valid(rec: &StageArtifactRecord) -> bool {
    let path = Path::new(&rec.uri);
    if !file_matches_hash(path, rec.file_fingerprint.as_deref()) {
        return false;
    }
    match rec.sidecar_fingerprint.as_deref() {
        Some(expected) => file_matches_hash(&sidecar_path(path), Some(expected)),
        None => sidecar_legacy_ok(&rec.uri),
    }
}

fn file_matches_hash(path: &Path, expected: Option<&str>) -> bool {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() && meta.len() > 0 => {}
        _ => return false,
    }
    let Some(expected) = expected else {
        return true;
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
/// A readable sidecar restores masks and encode settings. A record that
/// stores a sidecar hash must still have those bytes. A record without that
/// hash is a checkpoint from before sidecars were sealed: a missing file
/// restores the picture only. A sidecar that does not parse, or whose hash
/// does not match, stops the prefix.
///
/// `required_nodes` is empty here, so any non-empty valid record set can
/// certify a stage. [`restore_validated_prefix_members`] checks the frontier.
///
/// # Errors
///
/// `open_video` failures on a record that passed [`artifact_is_valid`].
pub fn restore_validated_prefix(
    artifacts: &[StageArtifactRecord],
    expected_by_stage: &[String],
) -> Result<StageResumePlan> {
    restore_validated_prefix_members(artifacts, expected_by_stage, &[])
}

/// Restore a prefix whose stages contain every required frontier node.
///
/// `required_nodes[i]` is the node set stage `i` must have persisted. An
/// empty entry keeps the older rule: any non-empty set of valid records.
/// A missing required node stops the prefix even when the other files match.
///
/// # Errors
///
/// `open_video` failures on a record that passed [`artifact_is_valid`].
pub fn restore_validated_prefix_members(
    artifacts: &[StageArtifactRecord],
    expected_by_stage: &[String],
    required_nodes: &[Vec<String>],
) -> Result<StageResumePlan> {
    let mut plan = StageResumePlan::default();
    for (si, expected) in expected_by_stage.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let index = si as u32;
        let recs: Vec<&StageArtifactRecord> = artifacts
            .iter()
            .filter(|a| a.stage_index == index && a.fingerprint == *expected)
            .collect();
        let required = match required_nodes.get(si) {
            Some(nodes) => nodes.as_slice(),
            None => &[],
        };
        if !stage_records_complete(&recs, required) {
            plan.start_stage = index;
            return Ok(plan);
        }
        for rec in recs {
            restore_one(&mut plan, rec)?;
        }
        plan.start_stage = index.saturating_add(1);
    }
    Ok(plan)
}

fn stage_records_complete(recs: &[&StageArtifactRecord], required: &[String]) -> bool {
    if recs.is_empty() || recs.iter().any(|record| !artifact_is_valid(record)) {
        return false;
    }
    required
        .iter()
        .all(|id| recs.iter().any(|record| &record.node_id == id))
}

fn restore_one(plan: &mut StageResumePlan, rec: &StageArtifactRecord) -> Result<()> {
    let clip = open_video(&crate::OpenVideoOptions::new(&rec.uri))?;
    let audio = clip
        .audio()
        .cloned()
        .map(|track| Arc::new(track) as Arc<dyn AudioClip>);
    let side = read_sidecar(Path::new(&rec.uri))?.unwrap_or_default();
    if let Some(track) = audio {
        plan.restored_audio.insert(rec.node_id.clone(), track);
    }
    if let Some(masks) = side.masks {
        plan.restored_masks.insert(rec.node_id.clone(), masks);
    }
    if !side.encode.is_empty() {
        plan.restored_encode
            .insert(rec.node_id.clone(), side.encode);
    }
    plan.restored_video
        .insert(rec.node_id.clone(), Arc::new(clip));
    Ok(())
}

/// Write one stage output to `dir` and return its record.
///
/// Video only: no audio, masks, or encode settings. The runner stores the
/// full frontier node, including audio and the sidecar.
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
    let encode = StageEncodeState::default();
    persist_stage_media(
        dir,
        stage_index,
        fingerprint,
        node_id,
        &StageMediaParts {
            video: clip,
            audio: None,
            masks: None,
            encode: &encode,
        },
    )
}

/// Write one frontier node, including audio and a media sidecar.
///
/// The picture is `CRF` 30. Companion audio is muxed when present. Masks and
/// encode settings go in `{uri}.media.json`. The record stores that file's
/// hash, so a later resume does not treat a missing sidecar as "no masks".
///
/// # Errors
///
/// Encode, sidecar, or hash I/O.
pub(crate) fn persist_stage_media(
    dir: impl AsRef<Path>,
    stage_index: u32,
    fingerprint: &str,
    node_id: &str,
    media: &StageMediaParts<'_>,
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
    let fps = media
        .video
        .fps()
        .filter(|f| f.is_finite() && *f > 0.0)
        .unwrap_or(15.0);
    let uri = path.to_string_lossy().into_owned();
    let opts = WriteVideoOptions::new(&uri, fps).with_crf(30);
    if let Some(audio) = media.audio {
        write_av(media.video, audio, &opts)?;
    } else {
        write_video(media.video, &opts)?;
    }
    write_sidecar(
        &path,
        &StageMediaSidecar {
            version: STAGE_MEDIA_SIDECAR_VERSION,
            masks: media.masks.cloned(),
            encode: media.encode.clone(),
        },
    )?;
    let file_fp = fingerprint_file(&path)?;
    let sidecar_fp = fingerprint_file(sidecar_path(&path))?;
    Ok(
        StageArtifactRecord::new(stage_index, fingerprint, node_id, uri)
            .with_file_fingerprint(file_fp)
            .with_sidecar_fingerprint(sidecar_fp),
    )
}

fn sidecar_path(video: &Path) -> PathBuf {
    PathBuf::from(format!("{}.media.json", video.display()))
}

fn sidecar_legacy_ok(uri: &str) -> bool {
    let path = sidecar_path(Path::new(uri));
    if !path.is_file() {
        return true;
    }
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StageMediaSidecar>(&bytes).ok())
        .is_some()
}

fn read_sidecar(video: &Path) -> Result<Option<StageMediaSidecar>> {
    let path = sidecar_path(video);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path)
        .map_err(|e| IoError::message(format!("stage sidecar read {}: {e}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| IoError::message(format!("stage sidecar {}: {e}", path.display())))
}

fn write_sidecar(video: &Path, body: &StageMediaSidecar) -> Result<()> {
    let path = sidecar_path(video);
    let tmp = video.with_file_name(format!(
        ".{}.media.json.partial",
        video
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("stage")
    ));
    let text = serde_json::to_string(body)
        .map_err(|e| IoError::message(format!("stage sidecar encode: {e}")))?;
    fs::write(&tmp, text)
        .map_err(|e| IoError::message(format!("stage sidecar write {}: {e}", tmp.display())))?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| {
            IoError::message(format!("stage sidecar replace {}: {e}", path.display()))
        })?;
    }
    fs::rename(&tmp, &path)
        .map_err(|e| IoError::message(format!("stage sidecar rename {}: {e}", path.display())))?;
    Ok(())
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

    #[test]
    fn checkpoint_corrupt_sidecar_stops_the_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let p0 = dir.path().join("s0.mp4");
        fs::write(&p0, b"hello").unwrap();
        let hash = fingerprint_file(&p0).unwrap();
        fs::write(sidecar_path(&p0), b"{not-json").unwrap();
        let rec = StageArtifactRecord::new(0, "fp0", "n0", p0.to_string_lossy())
            .with_file_fingerprint(hash);
        let plan = restore_validated_prefix(&[rec], &["fp0".into()]).unwrap();
        assert_eq!(plan.start_stage, 0);
        assert!(plan.restored_video.is_empty());
    }

    fn sealed_record(
        dir: &Path,
        name: &str,
        node: &str,
        stage: u32,
        fp: &str,
    ) -> StageArtifactRecord {
        let video = dir.join(name);
        fs::write(&video, b"video-bytes").unwrap();
        let side = sidecar_path(&video);
        fs::write(&side, br#"{"version":1,"encode":{"fps":5.0}}"#).unwrap();
        StageArtifactRecord::new(stage, fp, node, video.to_string_lossy())
            .with_file_fingerprint(fingerprint_file(&video).unwrap())
            .with_sidecar_fingerprint(fingerprint_file(&side).unwrap())
    }

    #[test]
    fn legacy_record_without_sidecar_hash_stays_valid() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("old.mp4");
        fs::write(&video, b"video").unwrap();
        let rec = StageArtifactRecord::new(0, "fp", "n", video.to_string_lossy())
            .with_file_fingerprint(fingerprint_file(&video).unwrap());
        assert!(artifact_is_valid(&rec));
    }

    #[test]
    fn sealed_sidecar_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let rec = sealed_record(dir.path(), "vision.mp4", "vision", 0, "fp0");
        assert!(artifact_is_valid(&rec));
        fs::remove_file(sidecar_path(Path::new(&rec.uri))).unwrap();
        assert!(!artifact_is_valid(&rec));
        let plan = restore_validated_prefix(&[rec], &["fp0".into()]).unwrap();
        assert_eq!(plan.start_stage, 0);
        assert!(plan.restored_masks.is_empty());
    }

    #[test]
    fn changed_sidecar_does_not_resume() {
        let dir = tempfile::tempdir().unwrap();
        let rec = sealed_record(dir.path(), "out_a.mp4", "out_a", 0, "fp0");
        fs::write(
            sidecar_path(Path::new(&rec.uri)),
            br#"{"version":1,"encode":{"fps":3.0}}"#,
        )
        .unwrap();
        assert!(!artifact_is_valid(&rec));
        let plan = restore_validated_prefix(&[rec], &["fp0".into()]).unwrap();
        assert_eq!(plan.start_stage, 0);
        assert!(plan.restored_encode.is_empty());
    }

    #[test]
    fn incomplete_frontier_stops_the_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let other = sealed_record(dir.path(), "other.mp4", "other", 0, "fp0");
        let plan = restore_validated_prefix_members(
            &[other],
            &["fp0".into()],
            &[vec!["inv_again".into(), "other".into()]],
        )
        .unwrap();
        assert_eq!(plan.start_stage, 0);
        assert!(plan.restored_video.is_empty());
    }
}
