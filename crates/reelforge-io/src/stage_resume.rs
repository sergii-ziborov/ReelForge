//! Stage resume: persist / validate / restore intermediate job artifacts.

use crate::error::{IoError, Result};
use crate::job::StageArtifactRecord;
use crate::manifest_seal::fingerprint_file;
use crate::options::WriteVideoOptions;
use crate::video_file::open_video;
use crate::write::{write_av, write_video};
use reelforge_core::{
    AlphaMode, AudioBuffer, AudioClip, AudioFormat, CoreError, Duration, Frame, FrameFormat,
    MediaTime, SampleLayout, Size, Time, VideoClip,
};
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
    /// Preview writes CRF 30. Final writes PNG frames and PCM instead.
    pub(crate) fidelity: CheckpointFidelity,
}

/// Sidecar schema. Records that store [`StageArtifactRecord::sidecar_fingerprint`]
/// were written at this version.
const STAGE_MEDIA_SIDECAR_VERSION: u32 = 1;

/// Final checkpoints refuse a longer picture than this before writing files.
const FINAL_MAX_FRAMES: usize = 120;

/// Final checkpoints refuse a longer PCM stream than this before writing files.
const FINAL_MAX_AUDIO_FRAMES: u64 = 240_000;

/// Non-empty stub stored at the record URI. Pixels live in the PNG siblings.
const FINAL_BUNDLE_STUB: &[u8] = b"reelforge-final-bundle\n";

/// What a checkpoint is allowed to stand in for.
///
/// Preview writes CRF 30 and labels it [`Self::PreviewLossy`]. Final writes
/// PNG frames plus PCM and is admitted only when every declared file hash
/// matches. A preview file is never treated as one of those pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointFidelity {
    /// Lossy preview. Safe to resume a preview. Not a final master.
    #[default]
    PreviewLossy,
    /// PNG frames and PCM. Emitted only when a run asks for a final checkpoint.
    FinalLossless,
}

/// Picture and PCM proof required before a checkpoint may resume a final render.
///
/// Every file is a sibling of the bundle stub. Hashes use the same fingerprint
/// the resume gate stores on stage records. An empty `frames` list means
/// `picture` alone is the proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalMediaComponents {
    /// File name in the checkpoint directory. Not a path.
    pub picture: String,
    /// [`fingerprint_file`](crate::fingerprint_file) of `picture`.
    pub picture_sha256: String,
    /// `straight`, `premultiplied`, or `none`.
    pub alpha_mode: String,
    /// Length of the restored clip. Zero on a picture-only proof.
    #[serde(default)]
    pub duration_ticks: i64,
    /// Timescale for `duration_ticks` and each frame PTS. Zero if omitted.
    #[serde(default)]
    pub timescale: u32,
    /// Every frame, in order. Empty means `picture` is the whole proof.
    #[serde(default)]
    pub frames: Vec<FinalFrameRef>,
    /// PCM sibling. Absent when the stage had no audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<FinalAudioRef>,
}

/// One lossless frame inside a final checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalFrameRef {
    /// File name in the checkpoint directory. Not a path.
    pub file: String,
    /// [`fingerprint_file`](crate::fingerprint_file) of `file`.
    pub sha256: String,
    /// Presentation time in `timescale` ticks.
    pub pts_ticks: i64,
    /// Ticks per second for `pts_ticks`.
    pub timescale: u32,
}

/// Interleaved little-endian `f32` PCM stored beside a final checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalAudioRef {
    /// File name in the checkpoint directory. Not a path.
    pub file: String,
    /// [`fingerprint_file`](crate::fingerprint_file) of `file`.
    pub sha256: String,
    /// Samples per second.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StageMediaSidecar {
    /// `0` on checkpoints written before the field existed.
    #[serde(default)]
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    masks: Option<MaskTimeline>,
    #[serde(default, skip_serializing_if = "StageEncodeState::is_empty")]
    encode: StageEncodeState,
    /// Missing on older sidecars, which are preview encodes.
    #[serde(default)]
    fidelity: CheckpointFidelity,
    /// Set only by a final bundle. Preview writes omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    final_components: Option<FinalMediaComponents>,
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
    /// Preview or final bytes written into `persist_dir`.
    pub checkpoint_fidelity: CheckpointFidelity,
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
    artifact_serves(rec, CheckpointFidelity::PreviewLossy)
}

/// True when `rec` is intact and may resume a render of `required` fidelity.
///
/// A legacy record or a CRF 30 sidecar resumes only a preview. An intact final
/// bundle resumes a preview or a final render. A final sidecar whose component
/// hash does not match resumes neither. The bundle stub itself cannot be the
/// picture.
#[must_use]
pub fn artifact_serves(rec: &StageArtifactRecord, required: CheckpointFidelity) -> bool {
    if !preview_bytes_match(rec) {
        return false;
    }
    match read_sidecar(Path::new(&rec.uri)) {
        Ok(Some(side)) if side.fidelity == CheckpointFidelity::FinalLossless => {
            final_components_match(Path::new(&rec.uri), &side)
        }
        _ => required == CheckpointFidelity::PreviewLossy,
    }
}

fn preview_bytes_match(rec: &StageArtifactRecord) -> bool {
    let path = Path::new(&rec.uri);
    if !file_matches_hash(path, rec.file_fingerprint.as_deref()) {
        return false;
    }
    match rec.sidecar_fingerprint.as_deref() {
        Some(expected) => file_matches_hash(&sidecar_path(path), Some(expected)),
        None => sidecar_legacy_ok(&rec.uri),
    }
}

fn final_components_match(video: &Path, side: &StageMediaSidecar) -> bool {
    let Some(parts) = side.final_components.as_ref() else {
        return false;
    };
    if !known_alpha_mode(&parts.alpha_mode) {
        return false;
    }
    let Some(picture) = component_file(video, &parts.picture) else {
        return false;
    };
    if fingerprint_file(&picture).ok().as_deref() != Some(parts.picture_sha256.as_str()) {
        return false;
    }
    if parts.frames.is_empty() {
        return true;
    }
    if parts.duration_ticks <= 0 || parts.timescale == 0 {
        return false;
    }
    let mut saw_picture = false;
    for frame in &parts.frames {
        if frame.timescale == 0 {
            return false;
        }
        let Some(path) = component_file(video, &frame.file) else {
            return false;
        };
        if fingerprint_file(&path).ok().as_deref() != Some(frame.sha256.as_str()) {
            return false;
        }
        if frame.file == parts.picture && frame.sha256 == parts.picture_sha256 {
            saw_picture = true;
        }
    }
    if !saw_picture {
        return false;
    }
    let Some(audio) = parts.audio.as_ref() else {
        return true;
    };
    if audio.sample_rate == 0 || audio.channels == 0 {
        return false;
    }
    let Some(path) = component_file(video, &audio.file) else {
        return false;
    };
    fingerprint_file(&path).is_ok_and(|got| got == audio.sha256)
}

fn known_alpha_mode(mode: &str) -> bool {
    matches!(mode, "straight" | "premultiplied" | "none")
}

fn component_file(video: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    let relative = Path::new(name);
    let single = relative.components().count() == 1
        && relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
    if !single {
        return None;
    }
    if relative.file_name() == video.file_name() {
        return None;
    }
    Some(video.parent()?.join(relative))
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
    first_invalid_stage_for(artifacts, total_stages, CheckpointFidelity::PreviewLossy)
}

/// Like [`first_invalid_stage`] for a preview or a final resume.
#[must_use]
pub fn first_invalid_stage_for(
    artifacts: &[StageArtifactRecord],
    total_stages: u32,
    fidelity: CheckpointFidelity,
) -> u32 {
    let mut expected = 0_u32;
    while expected < total_stages {
        let recs: Vec<&StageArtifactRecord> = artifacts
            .iter()
            .filter(|a| a.stage_index == expected)
            .collect();
        if recs.is_empty() || recs.iter().any(|record| !artifact_serves(record, fidelity)) {
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
    restore_validated_prefix_for(
        artifacts,
        expected_by_stage,
        required_nodes,
        CheckpointFidelity::PreviewLossy,
    )
}

/// Like [`restore_validated_prefix_members`] for a preview or a final resume.
///
/// A final resume stops at the first preview or legacy record and leaves that
/// stage to be computed again.
///
/// # Errors
///
/// `open_video` failures on a record that passed [`artifact_serves`].
pub fn restore_validated_prefix_for(
    artifacts: &[StageArtifactRecord],
    expected_by_stage: &[String],
    required_nodes: &[Vec<String>],
    fidelity: CheckpointFidelity,
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
        if !stage_records_complete(&recs, required, fidelity) {
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

fn stage_records_complete(
    recs: &[&StageArtifactRecord],
    required: &[String],
    fidelity: CheckpointFidelity,
) -> bool {
    if recs.is_empty() || recs.iter().any(|record| !artifact_serves(record, fidelity)) {
        return false;
    }
    required
        .iter()
        .all(|id| recs.iter().any(|record| &record.node_id == id))
}

fn restore_one(plan: &mut StageResumePlan, rec: &StageArtifactRecord) -> Result<()> {
    let side = read_sidecar(Path::new(&rec.uri))?.unwrap_or_default();
    let (video, audio) = if side.fidelity == CheckpointFidelity::FinalLossless {
        let parts = side.final_components.as_ref().ok_or_else(|| {
            IoError::message("final checkpoint is missing its picture and pcm list")
        })?;
        let clip = FinalPictureClip::open(Path::new(&rec.uri), parts)?;
        let audio = match &parts.audio {
            Some(audio) => {
                Some(Arc::new(FinalPcmClip::open(Path::new(&rec.uri), audio)?) as Arc<dyn AudioClip>)
            }
            None => None,
        };
        (Arc::new(clip) as Arc<dyn VideoClip>, audio)
    } else {
        let clip = open_video(&crate::OpenVideoOptions::new(&rec.uri))?;
        let audio = clip
            .audio()
            .cloned()
            .map(|track| Arc::new(track) as Arc<dyn AudioClip>);
        (Arc::new(clip) as Arc<dyn VideoClip>, audio)
    };
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
    plan.restored_video.insert(rec.node_id.clone(), video);
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
            fidelity: CheckpointFidelity::PreviewLossy,
        },
    )
}

/// Write one frontier node, including audio and a media sidecar.
///
/// Preview writes CRF 30 and [`CheckpointFidelity::PreviewLossy`]. Final writes
/// a bundle stub, PNG frames, and PCM, and does not call `ffmpeg`. A final clip
/// past the frame or audio cap is refused before any file is written. Masks and
/// encode settings go in `{uri}.media.json`.
///
/// # Errors
///
/// Encode, bundle, sidecar, or hash I/O. Final also errors when the clip
/// exceeds the lossless cap.
pub(crate) fn persist_stage_media(
    dir: impl AsRef<Path>,
    stage_index: u32,
    fingerprint: &str,
    node_id: &str,
    media: &StageMediaParts<'_>,
) -> Result<StageArtifactRecord> {
    match media.fidelity {
        CheckpointFidelity::FinalLossless => {
            persist_final_bundle(dir, stage_index, fingerprint, node_id, media)
        }
        CheckpointFidelity::PreviewLossy => {
            persist_preview_checkpoint(dir, stage_index, fingerprint, node_id, media)
        }
    }
}

fn persist_preview_checkpoint(
    dir: impl AsRef<Path>,
    stage_index: u32,
    fingerprint: &str,
    node_id: &str,
    media: &StageMediaParts<'_>,
) -> Result<StageArtifactRecord> {
    let dir = dir.as_ref();
    ensure_persist_dir(dir)?;
    let (safe_node, stem) = checkpoint_name(node_id, fingerprint);
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
    write_media_sidecar(stage_index, fingerprint, node_id, &path, media, None)
}

fn persist_final_bundle(
    dir: impl AsRef<Path>,
    stage_index: u32,
    fingerprint: &str,
    node_id: &str,
    media: &StageMediaParts<'_>,
) -> Result<StageArtifactRecord> {
    let frames = collect_final_frames(media.video)?;
    let audio = collect_final_audio(media.audio)?;
    let duration = MediaTime::from_secs(media.video.duration().as_secs(), MediaTime::HZ_1M)?;
    if frames.is_empty() || duration.ticks <= 0 {
        return Err(IoError::message(
            "final bundle refuses a clip with no positive duration",
        ));
    }
    let dir = dir.as_ref();
    ensure_persist_dir(dir)?;
    let (safe_node, stem) = checkpoint_name(node_id, fingerprint);
    let path = dir.join(format!("s{stage_index}-{safe_node}-{stem}.bundle"));
    fs::write(&path, FINAL_BUNDLE_STUB)
        .map_err(|err| IoError::message(format!("final bundle {}: {err}", path.display())))?;
    let mut listed = Vec::with_capacity(frames.len());
    let mut picture = String::new();
    let mut picture_sha256 = String::new();
    let alpha_mode = alpha_label(frames[0].1.alpha_mode());
    for (index, (time, frame)) in frames.iter().enumerate() {
        let name = format!("s{stage_index}-{safe_node}-{stem}-f{index:04}.png");
        let png = dir.join(&name);
        write_png_frame(&png, frame)?;
        let sha256 = fingerprint_file(&png)?;
        let pts = MediaTime::from_secs(time.as_secs(), MediaTime::HZ_1M)?;
        if index == 0 {
            picture.clone_from(&name);
            picture_sha256.clone_from(&sha256);
        }
        listed.push(FinalFrameRef {
            file: name,
            sha256,
            pts_ticks: pts.ticks,
            timescale: pts.timescale,
        });
    }
    let audio = match audio {
        Some((format, samples)) => {
            let name = format!("s{stage_index}-{safe_node}-{stem}.pcm");
            let pcm = dir.join(&name);
            write_pcm(&pcm, samples.samples())?;
            Some(FinalAudioRef {
                file: name,
                sha256: fingerprint_file(&pcm)?,
                sample_rate: format.sample_rate,
                channels: format.channels(),
            })
        }
        None => None,
    };
    let parts = FinalMediaComponents {
        picture,
        picture_sha256,
        alpha_mode: alpha_mode.to_string(),
        duration_ticks: duration.ticks,
        timescale: MediaTime::HZ_1M,
        frames: listed,
        audio,
    };
    write_media_sidecar(stage_index, fingerprint, node_id, &path, media, Some(parts))
}

fn write_media_sidecar(
    stage_index: u32,
    fingerprint: &str,
    node_id: &str,
    path: &Path,
    media: &StageMediaParts<'_>,
    final_components: Option<FinalMediaComponents>,
) -> Result<StageArtifactRecord> {
    write_sidecar(
        path,
        &StageMediaSidecar {
            version: STAGE_MEDIA_SIDECAR_VERSION,
            masks: media.masks.cloned(),
            encode: media.encode.clone(),
            fidelity: media.fidelity,
            final_components,
        },
    )?;
    let uri = path.to_string_lossy().into_owned();
    Ok(
        StageArtifactRecord::new(stage_index, fingerprint, node_id, uri)
            .with_file_fingerprint(fingerprint_file(path)?)
            .with_sidecar_fingerprint(fingerprint_file(sidecar_path(path))?),
    )
}

fn ensure_persist_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)
        .map_err(|err| IoError::message(format!("stage persist mkdir {}: {err}", dir.display())))
}

fn checkpoint_name(node_id: &str, fingerprint: &str) -> (String, String) {
    let stem: String = fingerprint.chars().take(16).collect();
    let safe_node: String = node_id
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    (safe_node, stem)
}

fn collect_final_frames(video: &dyn VideoClip) -> Result<Vec<(Time, Frame)>> {
    let times = final_sample_times(video)?;
    let mut frames = Vec::with_capacity(times.len());
    let mut label: Option<&str> = None;
    let mut size: Option<Size> = None;
    for time in times {
        let frame = video.frame_at(time)?;
        let next = alpha_label(frame.alpha_mode());
        if let Some(prev) = label
            && prev != next
        {
            return Err(IoError::message(
                "final bundle refuses frames that do not share one alpha mode",
            ));
        }
        if let Some(expected) = size
            && frame.size() != expected
        {
            return Err(IoError::message(
                "final bundle refuses frames that change size",
            ));
        }
        label = Some(next);
        size = Some(frame.size());
        frames.push((time, frame));
    }
    Ok(frames)
}

fn collect_final_audio(
    audio: Option<&dyn AudioClip>,
) -> Result<Option<(AudioFormat, AudioBuffer)>> {
    let Some(audio) = audio else {
        return Ok(None);
    };
    let format = audio.format();
    let frames = format.frames_for_duration(audio.duration());
    if frames == 0 {
        return Ok(None);
    }
    if frames > FINAL_MAX_AUDIO_FRAMES {
        return Err(IoError::message(format!(
            "final bundle refuses {frames} audio frames; the limit is {FINAL_MAX_AUDIO_FRAMES}"
        )));
    }
    let Ok(count) = usize::try_from(frames) else {
        return Err(IoError::message(
            "final bundle refuses an audio length that does not fit in memory",
        ));
    };
    Ok(Some((format, audio.samples_at(Time::ZERO, count)?)))
}

fn final_sample_times(video: &dyn VideoClip) -> Result<Vec<Time>> {
    let duration = video.duration().as_secs();
    if !(duration.is_finite() && duration > 0.0) {
        return Err(IoError::message(
            "final bundle refuses a clip with no positive duration",
        ));
    }
    let Some(fps) = video.fps().filter(|fps| fps.is_finite() && *fps > 0.0) else {
        return Ok(vec![Time::ZERO]);
    };
    let count = frame_count_at(duration, fps);
    let Ok(count_usize) = usize::try_from(count) else {
        return Err(too_many_frames(count));
    };
    if count_usize > FINAL_MAX_FRAMES {
        return Err(too_many_frames(count));
    }
    let mut times = Vec::new();
    for index in 0..count {
        let time = Time::from_secs(index_seconds(index, fps));
        if time.as_secs() < duration {
            times.push(time);
        }
    }
    if times.is_empty() {
        times.push(Time::ZERO);
    }
    Ok(times)
}

fn too_many_frames(count: u64) -> IoError {
    IoError::message(format!(
        "final bundle refuses {count} frames; the limit is {FINAL_MAX_FRAMES}"
    ))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn frame_count_at(duration_secs: f64, fps: f64) -> u64 {
    let count = (duration_secs * fps).round();
    if count < 1.0 { 1 } else { count as u64 }
}

#[allow(clippy::cast_precision_loss)]
fn index_seconds(index: u64, fps: f64) -> f64 {
    index as f64 / fps
}

fn alpha_label(mode: AlphaMode) -> &'static str {
    match mode {
        AlphaMode::Opaque => "none",
        AlphaMode::Straight => "straight",
        AlphaMode::Premultiplied => "premultiplied",
    }
}

fn alpha_mode_from_label(label: &str) -> Result<AlphaMode> {
    match label {
        "none" => Ok(AlphaMode::Opaque),
        "straight" => Ok(AlphaMode::Straight),
        "premultiplied" => Ok(AlphaMode::Premultiplied),
        _ => Err(IoError::message(format!(
            "final bundle refuses unknown alpha mode {label}"
        ))),
    }
}

fn write_png_frame(path: &Path, frame: &Frame) -> Result<()> {
    let size = frame.size();
    let data = frame.data().to_vec();
    let saved = match frame.format() {
        FrameFormat::Rgba8 => {
            let image = image::RgbaImage::from_raw(size.width, size.height, data)
                .ok_or_else(|| IoError::image("rgba frame does not match its size"))?;
            image.save(path)
        }
        FrameFormat::Rgb8 => {
            let image = image::RgbImage::from_raw(size.width, size.height, data)
                .ok_or_else(|| IoError::image("rgb frame does not match its size"))?;
            image.save(path)
        }
    };
    saved.map_err(|err| IoError::image(format!("png {}: {err}", path.display())))
}

fn read_png_frame(path: &Path, mode: AlphaMode) -> Result<Frame> {
    let image = image::ImageReader::open(path)
        .map_err(|err| IoError::image(format!("open {}: {err}", path.display())))?
        .with_guessed_format()
        .map_err(|err| IoError::image(format!("format {}: {err}", path.display())))?
        .decode()
        .map_err(|err| IoError::image(format!("decode {}: {err}", path.display())))?;
    let (size, format, data) = match (mode, image) {
        (AlphaMode::Opaque, image::DynamicImage::ImageRgb8(buffer)) => (
            Size::new(buffer.width(), buffer.height()),
            FrameFormat::Rgb8,
            buffer.into_raw(),
        ),
        (
            AlphaMode::Straight | AlphaMode::Premultiplied,
            image::DynamicImage::ImageRgba8(buffer),
        ) => (
            Size::new(buffer.width(), buffer.height()),
            FrameFormat::Rgba8,
            buffer.into_raw(),
        ),
        _ => {
            return Err(IoError::image(format!(
                "png {} does not match alpha mode {mode:?}",
                path.display()
            )));
        }
    };
    Ok(Frame::from_raw(size, format, data)?.with_alpha_mode(mode)?)
}

fn write_pcm(path: &Path, samples: &[f32]) -> Result<()> {
    let mut bytes = Vec::with_capacity(samples.len().saturating_mul(4));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(path, bytes)
        .map_err(|err| IoError::message(format!("final pcm {}: {err}", path.display())))
}

fn read_pcm(path: &Path) -> Result<Vec<f32>> {
    let bytes = fs::read(path)
        .map_err(|err| IoError::message(format!("final pcm {}: {err}", path.display())))?;
    if !bytes.len().is_multiple_of(4) {
        return Err(IoError::message(
            "final bundle pcm length is not a multiple of 4",
        ));
    }
    let mut samples = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        let Ok(chunk) = <[u8; 4]>::try_from(chunk) else {
            return Err(IoError::message("final bundle pcm chunk is short"));
        };
        samples.push(f32::from_le_bytes(chunk));
    }
    Ok(samples)
}

struct HeldFrame {
    pts: Time,
    frame: Frame,
}

struct FinalPictureClip {
    frames: Vec<HeldFrame>,
    duration: Duration,
    size: Size,
}

impl FinalPictureClip {
    fn open(bundle: &Path, parts: &FinalMediaComponents) -> Result<Self> {
        if parts.frames.is_empty() {
            return Err(IoError::message("final bundle has no frames to restore"));
        }
        let mode = alpha_mode_from_label(&parts.alpha_mode)?;
        let duration = MediaTime::new(parts.duration_ticks, parts.timescale)?.to_duration();
        if duration.as_secs() <= 0.0 {
            return Err(IoError::message(
                "final bundle refuses a non-positive duration",
            ));
        }
        let mut frames = Vec::with_capacity(parts.frames.len());
        for frame_ref in &parts.frames {
            let path = component_file(bundle, &frame_ref.file)
                .ok_or_else(|| IoError::message("final bundle frame path is not a sibling file"))?;
            let got = fingerprint_file(&path)?;
            if got != frame_ref.sha256 {
                return Err(IoError::message("final bundle frame hash does not match"));
            }
            let frame = read_png_frame(&path, mode)?;
            let pts = MediaTime::new(frame_ref.pts_ticks, frame_ref.timescale)?.to_time();
            frames.push(HeldFrame { pts, frame });
        }
        frames.sort_by(|left, right| left.pts.as_secs().total_cmp(&right.pts.as_secs()));
        let size = frames[0].frame.size();
        Ok(Self {
            frames,
            duration,
            size,
        })
    }
}

impl VideoClip for FinalPictureClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.size
    }

    fn fps(&self) -> Option<f64> {
        None
    }

    fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let mut selected = &self.frames[0];
        for held in &self.frames {
            if held.pts.as_secs() <= t.as_secs() {
                selected = held;
            }
        }
        Ok(selected.frame.clone())
    }
}

struct FinalPcmClip {
    format: AudioFormat,
    samples: Vec<f32>,
    duration: Duration,
}

impl FinalPcmClip {
    fn open(bundle: &Path, audio: &FinalAudioRef) -> Result<Self> {
        if audio.sample_rate == 0 || audio.channels == 0 {
            return Err(IoError::message(
                "final bundle refuses pcm with a zero rate or channel count",
            ));
        }
        let path = component_file(bundle, &audio.file)
            .ok_or_else(|| IoError::message("final bundle pcm path is not a sibling file"))?;
        let got = fingerprint_file(&path)?;
        if got != audio.sha256 {
            return Err(IoError::message("final bundle pcm hash does not match"));
        }
        let samples = read_pcm(&path)?;
        let format = AudioFormat::new(
            audio.sample_rate,
            SampleLayout::from_channels(audio.channels),
        )?;
        let channels = usize::from(format.channels());
        if channels == 0 || !samples.len().is_multiple_of(channels) {
            return Err(IoError::message(
                "final bundle pcm does not match its channel count",
            ));
        }
        let frame_count = u64::try_from(samples.len() / channels).unwrap_or(0);
        Ok(Self {
            format,
            samples,
            duration: format.duration_of_frames(frame_count),
        })
    }
}

impl AudioClip for FinalPcmClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> reelforge_core::Result<AudioBuffer> {
        if frame_count > 0 && (t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs()) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let channels = usize::from(self.format.channels());
        let start = pcm_origin(t, self.format.sample_rate).saturating_mul(channels);
        let Some(need) = frame_count.checked_mul(channels) else {
            return Err(CoreError::invalid_audio("pcm read length overflow"));
        };
        let mut out = vec![0.0; need];
        if start < self.samples.len() {
            let copy = (self.samples.len() - start).min(need);
            out[..copy].copy_from_slice(&self.samples[start..start + copy]);
        }
        AudioBuffer::from_interleaved(self.format, out)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn pcm_origin(t: Time, sample_rate: u32) -> usize {
    let frames = (t.as_secs().max(0.0) * f64::from(sample_rate)).floor();
    if frames <= 0.0 { 0 } else { frames as usize }
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
    use reelforge_core::{
        AlphaMode, AudioBuffer, AudioClip, AudioFormat, ColorClip, CoreError, Duration, Frame,
        FrameFormat, MediaTime, Rgb8, SampleLayout, Size, Time, VideoClip,
    };
    use reelforge_render_graph::{MaskSample, MaskTimeline};

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

    fn write_sidecar(video: &Path, body: impl AsRef<[u8]>) -> StageArtifactRecord {
        fs::write(video, b"preview-mp4").unwrap();
        let side = sidecar_path(video);
        fs::write(&side, body).unwrap();
        StageArtifactRecord::new(0, "fp0", "n0", video.to_string_lossy())
            .with_file_fingerprint(fingerprint_file(video).unwrap())
            .with_sidecar_fingerprint(fingerprint_file(&side).unwrap())
    }

    #[test]
    fn preview_checkpoint_does_not_resume_a_final_render() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("stage.mp4");
        let legacy = write_sidecar(&video, br#"{"version":1,"encode":{"fps":5.0}}"#);
        assert!(artifact_serves(&legacy, CheckpointFidelity::PreviewLossy));
        assert!(!artifact_serves(&legacy, CheckpointFidelity::FinalLossless));
        assert_eq!(
            first_invalid_stage_for(
                std::slice::from_ref(&legacy),
                1,
                CheckpointFidelity::FinalLossless
            ),
            0
        );
        let plan = restore_validated_prefix_for(
            &[legacy],
            &["fp0".into()],
            &[],
            CheckpointFidelity::FinalLossless,
        )
        .unwrap();
        assert_eq!(plan.start_stage, 0);
        assert!(plan.restored_video.is_empty());

        let labeled = write_sidecar(
            &video,
            br#"{"version":1,"fidelity":"preview_lossy","encode":{"fps":5.0}}"#,
        );
        assert!(artifact_serves(&labeled, CheckpointFidelity::PreviewLossy));
        assert!(!artifact_serves(
            &labeled,
            CheckpointFidelity::FinalLossless
        ));

        let bare_label = write_sidecar(&video, br#"{"version":1,"fidelity":"final_lossless"}"#);
        assert!(!artifact_serves(
            &bare_label,
            CheckpointFidelity::FinalLossless
        ));

        let self_picture = write_sidecar(
            &video,
            br#"{"version":1,"fidelity":"final_lossless","final_components":{"picture":"stage.mp4","picture_sha256":"ignored","alpha_mode":"straight"}}"#,
        );
        assert!(!artifact_serves(
            &self_picture,
            CheckpointFidelity::FinalLossless
        ));
    }

    #[test]
    fn final_resume_accepts_a_matching_sibling_picture() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("stage.mp4");
        let picture = dir.path().join("stage.png");
        fs::write(&picture, b"rgba-frame").unwrap();
        let picture_hash = fingerprint_file(&picture).unwrap();
        let body = format!(
            r#"{{"version":1,"fidelity":"final_lossless","final_components":{{"picture":"stage.png","picture_sha256":"{picture_hash}","alpha_mode":"straight"}}}}"#
        );
        let rec = write_sidecar(&video, body.as_bytes());
        assert!(artifact_serves(&rec, CheckpointFidelity::FinalLossless));
        assert_eq!(
            first_invalid_stage_for(
                std::slice::from_ref(&rec),
                2,
                CheckpointFidelity::FinalLossless
            ),
            1
        );

        assert!(artifact_serves(&rec, CheckpointFidelity::PreviewLossy));

        fs::write(&picture, b"changed").unwrap();
        assert!(!artifact_serves(&rec, CheckpointFidelity::FinalLossless));
        assert!(!artifact_serves(&rec, CheckpointFidelity::PreviewLossy));
    }

    struct StepVideo {
        frames: Vec<Frame>,
        fps: f64,
        duration: Duration,
    }

    impl VideoClip for StepVideo {
        fn duration(&self) -> Duration {
            self.duration
        }

        fn size(&self) -> Size {
            self.frames.first().map_or(Size::new(1, 1), Frame::size)
        }

        fn fps(&self) -> Option<f64> {
            Some(self.fps)
        }

        fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
            if t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs() {
                return Err(CoreError::TimeOutOfRange {
                    time: t,
                    range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
                });
            }
            let index = step_frame_index(t.as_secs(), self.fps, self.frames.len());
            self.frames
                .get(index)
                .cloned()
                .ok_or_else(|| CoreError::invalid_frame("step video has no frames"))
        }
    }

    struct StepAudio {
        samples: Vec<f32>,
        format: AudioFormat,
        duration: Duration,
    }

    impl AudioClip for StepAudio {
        fn duration(&self) -> Duration {
            self.duration
        }

        fn format(&self) -> AudioFormat {
            self.format
        }

        fn samples_at(&self, t: Time, frame_count: usize) -> reelforge_core::Result<AudioBuffer> {
            if frame_count > 0 && (t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs()) {
                return Err(CoreError::TimeOutOfRange {
                    time: t,
                    range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
                });
            }
            let channels = usize::from(self.format.channels());
            let Some(need) = frame_count.checked_mul(channels) else {
                return Err(CoreError::invalid_audio("step audio length overflow"));
            };
            let mut out = vec![0.0; need];
            let copy = need.min(self.samples.len());
            out[..copy].copy_from_slice(&self.samples[..copy]);
            AudioBuffer::from_interleaved(self.format, out)
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn step_frame_index(secs: f64, fps: f64, len: usize) -> usize {
        let index = (secs * fps).floor();
        let index = if index <= 0.0 { 0 } else { index as usize };
        index.min(len.saturating_sub(1))
    }

    fn rgba_step(rgb: [u8; 3], alpha: u8) -> Frame {
        let data = vec![
            rgb[0],
            rgb[1],
            rgb[2],
            alpha,
            rgb[0].wrapping_add(30),
            rgb[1].wrapping_add(30),
            rgb[2].wrapping_add(30),
            alpha,
        ];
        Frame::from_raw(Size::new(2, 1), FrameFormat::Rgba8, data).unwrap()
    }

    fn final_parts<'a>(
        video: &'a dyn VideoClip,
        audio: Option<&'a dyn AudioClip>,
        masks: Option<&'a MaskTimeline>,
        encode: &'a StageEncodeState,
    ) -> StageMediaParts<'a> {
        StageMediaParts {
            video,
            audio,
            masks,
            encode,
            fidelity: CheckpointFidelity::FinalLossless,
        }
    }

    #[test]
    fn final_bundle_roundtrips_rgba_pcm_and_masks() {
        let dir = tempfile::tempdir().unwrap();
        let video = StepVideo {
            frames: vec![rgba_step([10, 20, 30], 128), rgba_step([70, 80, 90], 64)],
            fps: 2.0,
            duration: Duration::from_secs(1.0),
        };
        let audio_format = AudioFormat::new(4, SampleLayout::Mono).unwrap();
        let audio = StepAudio {
            samples: vec![0.25, -0.5, 0.125, 0.0],
            format: audio_format,
            duration: Duration::from_secs(1.0),
        };
        let mut masks = MaskTimeline::new();
        masks.push(MaskSample::ellipse(
            MediaTime::new(0, 30).unwrap(),
            1.0,
            0.0,
            2.0,
        ));
        let encode = StageEncodeState::default();
        let rec = persist_stage_media(
            dir.path(),
            0,
            "bundle-fp",
            "src",
            &final_parts(&video, Some(&audio), Some(&masks), &encode),
        )
        .unwrap();
        assert!(rec.uri.ends_with(".bundle"));
        assert!(artifact_serves(&rec, CheckpointFidelity::FinalLossless));
        assert!(artifact_serves(&rec, CheckpointFidelity::PreviewLossy));
        assert!(
            fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .all(|entry| entry.path().extension().is_none_or(|ext| ext != "mp4"))
        );

        let plan = restore_validated_prefix_for(
            std::slice::from_ref(&rec),
            &["bundle-fp".into()],
            &[],
            CheckpointFidelity::FinalLossless,
        )
        .unwrap();
        assert_eq!(plan.start_stage, 1);
        let clip = plan.restored_video.get("src").unwrap();
        assert!(clip.fps().is_none());
        let first = clip.frame_at(Time::ZERO).unwrap();
        assert_eq!(first.alpha_mode(), AlphaMode::Straight);
        assert_eq!(first.data(), rgba_step([10, 20, 30], 128).data());
        let held = clip.frame_at(Time::from_secs(0.25)).unwrap();
        assert_eq!(held.data(), first.data());
        let second = clip.frame_at(Time::from_secs(0.5)).unwrap();
        assert_eq!(second.data()[3], 64);
        assert_eq!(second.data(), rgba_step([70, 80, 90], 64).data());
        let again = clip.frame_at(Time::ZERO).unwrap();
        assert_eq!(again.data(), first.data());
        let restored_audio = plan.restored_audio.get("src").unwrap();
        let samples = restored_audio.samples_at(Time::ZERO, 4).unwrap();
        assert_eq!(
            samples
                .samples()
                .iter()
                .copied()
                .map(f32::to_bits)
                .collect::<Vec<_>>(),
            [0.25_f32, -0.5, 0.125, 0.0].map(f32::to_bits)
        );
        assert_eq!(plan.restored_masks.get("src"), Some(&masks));

        let preview = restore_validated_prefix_for(
            std::slice::from_ref(&rec),
            &["bundle-fp".into()],
            &[],
            CheckpointFidelity::PreviewLossy,
        )
        .unwrap();
        assert_eq!(preview.start_stage, 1);
        assert_eq!(
            preview
                .restored_video
                .get("src")
                .unwrap()
                .frame_at(Time::ZERO)
                .unwrap()
                .data()[3],
            128
        );
    }

    #[test]
    fn final_bundle_refuses_too_many_frames() {
        let dir = tempfile::tempdir().unwrap();
        let video =
            ColorClip::new(Size::new(2, 2), Rgb8::RED, Duration::from_secs(1.0)).with_fps(1000.0);
        let encode = StageEncodeState::default();
        let err = persist_stage_media(
            dir.path(),
            0,
            "too-many",
            "src",
            &final_parts(&video, None, None, &encode),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("refuses"), "{text}");
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    #[test]
    fn changed_final_png_does_not_resume() {
        let dir = tempfile::tempdir().unwrap();
        let video = ColorClip::new(Size::new(2, 2), Rgb8::RED, Duration::from_secs(0.5));
        let encode = StageEncodeState::default();
        let rec = persist_stage_media(
            dir.path(),
            0,
            "fp0",
            "n0",
            &final_parts(&video, None, None, &encode),
        )
        .unwrap();
        let png = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "png"))
            .unwrap();
        let mut bytes = fs::read(&png).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&png, bytes).unwrap();
        assert!(!artifact_serves(&rec, CheckpointFidelity::FinalLossless));
        assert!(!artifact_serves(&rec, CheckpointFidelity::PreviewLossy));
        for fidelity in [
            CheckpointFidelity::FinalLossless,
            CheckpointFidelity::PreviewLossy,
        ] {
            let plan = restore_validated_prefix_for(
                std::slice::from_ref(&rec),
                &["fp0".into()],
                &[],
                fidelity,
            )
            .unwrap();
            assert_eq!(plan.start_stage, 0);
            assert!(plan.restored_video.is_empty());
        }
    }
}
