use crate::{Error, FrameReceipt, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Integer video clock. Indices address actual displayed frames, not durations of holds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Clock {
    /// Displayed frames per second.
    pub fps: u32,
    /// Exact number of displayed frames.
    pub frames: u32,
    /// Output canvas width.
    pub width: u32,
    /// Output canvas height.
    pub height: u32,
}

/// One contiguous shot, with caller-owned scene/identity memory and conditions.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShotSpec {
    /// Stable shot identifier.
    pub id: String,
    /// First displayed frame, inclusive.
    pub start: u32,
    /// First frame after this shot, exclusive.
    pub end: u32,
    /// Scene/action description shared by keyframe and intermediate requests.
    pub prompt: String,
    /// Matching tag for specialist models (landscape, realism, anime, etc.).
    pub theme: String,
    /// Appearance, location, ownership and event memory snapshot supplied to every model.
    pub continuity: serde_json::Value,
    /// Owned canonical appearance/location images, independent of temporary model caches.
    #[serde(default)]
    pub references: Vec<FrameReceipt>,
    /// Per-frame camera/pose/depth/event conditions; their content invalidates cache.
    pub conditions: BTreeMap<u32, serde_json::Value>,
    /// Explicit permission for a genuinely static shot.
    pub allow_still: bool,
}

/// Description of a complete generation, independent of any avatar or test scene.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GenerationSpec {
    /// Exact output clock.
    pub clock: Clock,
    /// Total number of expensive keyframes, distributed across shots.
    pub keyframe_count: u32,
    /// Reproducible seed.
    pub seed: u64,
    /// Ordered, gapless shot intervals.
    pub shots: Vec<ShotSpec>,
    /// Allow image-edit backends for intermediate frames as an explicit experiment.
    pub allow_image_edit_between: bool,
}

/// Backend ability, distinct from an arbitrary name containing "Qwen" or "video".
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    /// Creates a still keyframe.
    Keyframe,
    /// Jointly regenerates temporally conditioned intermediate frames.
    TemporalGeneration,
    /// Edits intermediate stills with both endpoint images and target-time conditions.
    ImageEdit,
    /// Warps/blends existing pixels; cannot replace generation.
    Interpolation,
}

/// Native model window constraints, without altering the delivered clock.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelConstraints {
    /// Largest model window including its endpoint context.
    pub max_frames: u32,
    /// Supported frame counts follow remainder + multiple * n (e.g. 1 + 4*n).
    pub frame_multiple: u32,
    /// Frame-count remainder, smaller than the multiple.
    pub frame_remainder: u32,
}

/// Explicit capability/weight/config fingerprint of a connected model adapter.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Backend {
    /// Adapter identifier.
    pub id: String,
    /// Model/configuration content fingerprint; changing weights invalidates cache.
    pub fingerprint: String,
    /// Supported task.
    pub kind: BackendKind,
    /// Empty means no theme restriction.
    pub themes: BTreeSet<String>,
    /// Backend is configured and reachable.
    pub available: bool,
    /// Conservative admitted peak host-memory estimate.
    pub ram_bytes: u64,
    /// Required accelerator memory; zero permits CPU execution.
    pub vram_bytes: u64,
    /// Model frame-window rules.
    pub constraints: ModelConstraints,
    /// Lower rank wins among admitted backends of equal capability.
    pub rank: u32,
}

/// Available memory/commit provided by the owning compute scheduler.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resources {
    /// Bytes available for this generation, after other jobs/reservations.
    pub ram_bytes: u64,
    /// Available compatible accelerator memory.
    pub vram_bytes: u64,
}

/// Type of a generated unit; native composition/effects follow these units.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// Expensive keyframe creation.
    Keyframe,
    /// Intermediate visual-content generation between two immutable anchors.
    Between,
}

/// Executable backend request. Intermediate output contains only the requested interior indices.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FrameRequest {
    /// Globally content-addressed request ID; adapters must retain it for recovery.
    pub id: String,
    /// Provider/model fingerprint snapshot.
    pub backend: Backend,
    /// Task kind.
    pub kind: TaskKind,
    /// Shot identity and remembered scene state.
    pub shot: Arc<ShotSpec>,
    /// Exact output clock.
    pub clock: Clock,
    /// Required displayed frame indices, with no repetitions or retiming.
    pub indices: Vec<u32>,
    /// Conditioning anchor indices; empty for keyframe generation.
    pub anchors: Vec<u32>,
    /// Model window after constraint padding. Extra tail frames are not displayed.
    pub model_frames: u32,
    /// Derived deterministic seed.
    pub seed: u64,
}

/// Compiled hybrid generation. No request crosses a scene cut.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GenerationPlan {
    /// Complete caller intent, including scene-memory fingerprints.
    pub spec: GenerationSpec,
    /// Ordered keyframe requests.
    pub keyframes: Vec<FrameRequest>,
    /// Ordered intermediate requests.
    pub between: Vec<FrameRequest>,
}

fn backend<'a>(
    all: &'a [Backend],
    shot: &ShotSpec,
    key: bool,
    span: u32,
    spec: &GenerationSpec,
    resources: &Resources,
) -> Result<&'a Backend> {
    all.iter()
        .filter(|b| {
            b.available
                && b.ram_bytes <= resources.ram_bytes
                && b.vram_bytes <= resources.vram_bytes
                && (b.themes.is_empty() || b.themes.contains(&shot.theme))
                && if key {
                    b.kind == BackendKind::Keyframe
                } else {
                    b.kind == BackendKind::TemporalGeneration
                        || (spec.allow_image_edit_between && b.kind == BackendKind::ImageEdit)
                }
        })
        .filter(|b| {
            if key || b.kind == BackendKind::ImageEdit {
                b.constraints.frame_remainder == 1 % b.constraints.frame_multiple
                    && b.constraints.max_frames >= 1
            } else {
                padded(span, &b.constraints).is_ok()
            }
        })
        .min_by_key(|b| (u32::from(b.kind == BackendKind::ImageEdit), b.rank, &b.id))
        .ok_or_else(|| {
            Error::Unavailable(format!(
                "{}: {} for theme {}; no interpolation/cloud fallback",
                shot.id,
                if key {
                    "keyframes"
                } else {
                    "new intermediate visual content"
                },
                shot.theme
            ))
        })
}

fn padded(span: u32, c: &ModelConstraints) -> Result<u32> {
    if c.frame_multiple == 0 || c.frame_remainder >= c.frame_multiple || c.max_frames < 1 {
        return Err(Error::Invalid("model window constraints".into()));
    }
    let n = span
        .saturating_sub(c.frame_remainder)
        .div_ceil(c.frame_multiple)
        .checked_mul(c.frame_multiple)
        .and_then(|n| n.checked_add(c.frame_remainder))
        .ok_or_else(|| Error::Invalid("model window overflow".into()))?;
    if n > c.max_frames {
        return Err(Error::Unavailable(
            "model window too small; add keyframes or select another backend".into(),
        ));
    }
    Ok(n)
}

fn initial_anchors(spec: &GenerationSpec, backends: &[Backend]) -> Result<BTreeSet<u32>> {
    let mut backend_ids = BTreeSet::new();
    for b in backends {
        if b.id.is_empty()
            || !backend_ids.insert(&b.id)
            || !crate::store::is_digest(&b.fingerprint)
            || b.constraints.frame_multiple == 0
            || b.constraints.frame_remainder >= b.constraints.frame_multiple
            || b.constraints.max_frames == 0
            || b.constraints.max_frames > 4096
        {
            return Err(Error::Invalid("backend identity/constraints".into()));
        }
    }
    if !(1..=120).contains(&spec.clock.fps)
        || !(2..=3600).contains(&spec.clock.frames)
        || !(16..=4096).contains(&spec.clock.width)
        || !(16..=4096).contains(&spec.clock.height)
        || spec.shots.is_empty()
        || spec.shots.len() > 128
        || spec.keyframe_count > spec.clock.frames
    {
        return Err(Error::Invalid("clock/canvas/shot/keyframe bounds".into()));
    }
    let mut ids = BTreeSet::new();
    let mut end = 0;
    let mut anchors = BTreeSet::new();
    for shot in &spec.shots {
        if shot.start != end
            || shot.end <= shot.start + 1
            || shot.end > spec.clock.frames
            || shot.id.is_empty()
            || !ids.insert(&shot.id)
            || shot.prompt.trim().is_empty()
            || shot.prompt.len() > 16000
            || shot.references.len() > 32
            || serde_json::to_vec(&shot.continuity)?.len() > 1_048_576
            || serde_json::to_vec(&shot.conditions)?.len() > 1_048_576
            || shot
                .conditions
                .keys()
                .any(|i| *i < shot.start || *i >= shot.end)
        {
            return Err(Error::Invalid("shot continuity/interval/conditions".into()));
        }
        end = shot.end;
        anchors.insert(shot.start);
        anchors.insert(shot.end - 1);
    }
    if end != spec.clock.frames
        || spec.keyframe_count < u32::try_from(anchors.len()).unwrap_or(u32::MAX)
    {
        return Err(Error::Invalid(
            "each shot needs start/end anchors; shots must cover the clock".into(),
        ));
    }
    Ok(anchors)
}

/// Compile sparse anchors and intermediate jobs with exact scene/clock coverage.
pub fn compile(
    spec: GenerationSpec,
    backends: &[Backend],
    resources: &Resources,
) -> Result<GenerationPlan> {
    let mut anchors = initial_anchors(&spec, backends)?;
    while anchors.len() < spec.keyframe_count as usize {
        let gap = spec
            .shots
            .iter()
            .flat_map(|s| {
                let frames: Vec<_> = anchors.range(s.start..s.end).copied().collect();
                frames
                    .windows(2)
                    .filter(|p| p[1] > p[0] + 1)
                    .map(|p| (p[1] - p[0], p[0]))
                    .collect::<Vec<_>>()
            })
            .max_by_key(|(length, start)| (*length, std::cmp::Reverse(*start)))
            .ok_or_else(|| Error::Invalid("keyframe allocation exhausted".into()))?;
        anchors.insert(gap.1 + gap.0 / 2);
    }
    let mut keyframes = vec![];
    let mut between = vec![];
    for shot in &spec.shots {
        // Hundreds of jobs share one graph/memory snapshot rather than copying
        // its potentially thousands of entities for every generated frame.
        let shot = Arc::new(shot.clone());
        let keys: Vec<_> = anchors.range(shot.start..shot.end).copied().collect();
        let key_backend = backend(backends, &shot, true, 1, &spec, resources)?;
        for &i in &keys {
            let mut r = FrameRequest {
                id: String::new(),
                backend: key_backend.clone(),
                kind: TaskKind::Keyframe,
                shot: shot.clone(),
                clock: spec.clock.clone(),
                indices: vec![i],
                anchors: if i == keys[0] { vec![] } else { vec![keys[0]] },
                model_frames: 1,
                seed: spec.seed ^ u64::from(i),
            };
            r.id = crate::store::digest(&serde_json::to_vec(&r)?);
            keyframes.push(r);
        }
        for pair in keys.windows(2).filter(|p| p[1] > p[0] + 1) {
            let span = pair[1] - pair[0] + 1;
            let b = backend(backends, &shot, false, span, &spec, resources)?;
            let indices: Vec<u32> = ((pair[0] + 1)..pair[1]).collect();
            let chunks = if b.kind == BackendKind::ImageEdit {
                indices.into_iter().map(|i| vec![i]).collect::<Vec<_>>()
            } else {
                vec![indices]
            };
            for indices in chunks {
                let seed = spec.seed ^ u64::from(indices[0]);
                let mut r = FrameRequest {
                    id: String::new(),
                    backend: b.clone(),
                    kind: TaskKind::Between,
                    shot: shot.clone(),
                    clock: spec.clock.clone(),
                    indices,
                    anchors: pair.to_vec(),
                    model_frames: if b.kind == BackendKind::ImageEdit {
                        1
                    } else {
                        padded(span, &b.constraints)?
                    },
                    seed,
                };
                r.id = crate::store::digest(&serde_json::to_vec(&r)?);
                between.push(r);
            }
        }
    }
    Ok(GenerationPlan {
        spec,
        keyframes,
        between,
    })
}
