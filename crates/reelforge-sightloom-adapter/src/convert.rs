//! Document → [`TrackTimeline`] (no `SightLoom` types).

use crate::document::{SampleEntry, TrackDocument, TrackEntry};
use crate::error::{AdapterError, Result};
use reelforge_core::MediaTime;
use reelforge_render_graph::{
    AppearanceId, Geometry, MaskLifecycle, MaskProvenance, MaskRef, ObservationId, OcclusionState,
    SubjectId, TrackId, TrackSample, TrackTimeline,
};

/// Default ticks/sec for JSON seconds (`t`).
pub const DEFAULT_TIMESCALE: u32 = 1_000;

/// Convert a parsed document into track timelines.
///
/// # Errors
///
/// Missing geometry on a sample.
pub fn document_to_timelines(doc: &TrackDocument) -> Result<Vec<TrackTimeline>> {
    doc.tracks.iter().map(entry_to_timeline).collect()
}

fn entry_to_timeline(entry: &TrackEntry) -> Result<TrackTimeline> {
    let track_id = TrackId::new(entry.id.clone());
    let subject = entry.subject.as_deref().map(SubjectId::new);
    let mut tl = TrackTimeline::new(track_id.clone());
    if let Some(s) = subject.clone() {
        tl = tl.with_subject(s);
    }
    let mut prov = MaskProvenance::sightloom(entry.id.clone());
    if let Some(kind) = &entry.kind {
        prov.model = Some(kind.clone());
    }
    tl.provenance = Some(prov.clone());
    for sample in &entry.samples {
        tl.push(sample_to_track(
            track_id.clone(),
            subject.clone(),
            sample,
            &prov,
        )?);
    }
    Ok(tl)
}

fn sample_to_track(
    track: TrackId,
    subject: Option<SubjectId>,
    raw: &SampleEntry,
    track_prov: &MaskProvenance,
) -> Result<TrackSample> {
    let t = MediaTime::from_secs(raw.t, DEFAULT_TIMESCALE)
        .map_err(|e| AdapterError::Sample(format!("t={} is not a valid media time: {e}", raw.t)))?;
    let geometry = geometry_from_sample(raw)?;
    let (cx, cy) = geometry.center();
    let mut sample = TrackSample::ellipse(track, t, cx, cy, geometry.radius());
    sample.geometry = geometry;
    sample.conf = raw.conf.unwrap_or(1.0).clamp(0.0, 1.0);
    sample.subject = subject;
    if let Some(occ) = raw.occlusion.as_deref() {
        sample.occlusion = parse_occlusion(occ)?;
        if sample.occlusion == OcclusionState::Occluded {
            sample.lifecycle = MaskLifecycle::Occluded;
        }
    }
    if let Some(ap) = &raw.appearance {
        sample.appearance = Some(AppearanceId::new(ap.clone()));
    }
    if let Some(obs) = &raw.observation {
        sample.observation = Some(ObservationId::new(obs.clone()));
    }
    if let Some(mask) = &raw.mask {
        let obs = mask
            .observation
            .clone()
            .or_else(|| raw.observation.clone())
            .unwrap_or_else(|| format!("obs-{}", raw.t));
        let mut href = MaskRef::new(ObservationId::new(obs));
        if let Some(uri) = &mask.uri {
            href = href.with_uri(uri.clone());
        }
        sample.mask = Some(href);
    }
    sample.provenance = Some(track_prov.clone());
    Ok(sample)
}

fn geometry_from_sample(s: &SampleEntry) -> Result<Geometry> {
    if let (Some(cx), Some(cy), Some(radius)) = (s.cx, s.cy, s.radius) {
        return Ok(Geometry::ellipse(cx, cy, radius));
    }
    if let (Some(x), Some(y), Some(w), Some(h)) = (s.x, s.y, s.w, s.h) {
        return Ok(Geometry::aabb(x, y, x + w, y + h));
    }
    if let (Some(left), Some(top), Some(right), Some(bottom)) = (s.left, s.top, s.right, s.bottom) {
        return Ok(Geometry::aabb(left, top, right, bottom));
    }
    Err(AdapterError::Sample(format!(
        "sample at t={} needs cx/cy/radius or x/y/w/h or left/top/right/bottom",
        s.t
    )))
}

fn parse_occlusion(raw: &str) -> Result<OcclusionState> {
    match raw.to_ascii_lowercase().as_str() {
        "visible" => Ok(OcclusionState::Visible),
        "partial" => Ok(OcclusionState::Partial),
        "occluded" => Ok(OcclusionState::Occluded),
        "unknown" => Ok(OcclusionState::Unknown),
        other => Err(AdapterError::Sample(format!(
            "unknown occlusion `{other}` (visible|partial|occluded|unknown)"
        ))),
    }
}
