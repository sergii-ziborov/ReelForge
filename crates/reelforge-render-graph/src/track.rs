//! [`TrackTimeline`] — identity source; [`crate::MaskTimeline`] is a view.
//!
//! A track is a trajectory (`TrackId`) with optional subject / appearance /
//! observation handles. `ReelForge` interpolates geometry; it does not query
//! identities.

use crate::geometry::{Geometry, MaskRef};
use crate::ids::{AppearanceId, ObservationId, SubjectId, TrackId};
use crate::mask::{
    MaskInterpolation, MaskLifecycle, MaskProvenance, MaskSample, MaskTimeline, MissingMaskPolicy,
};
use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// Visibility at a sample (orthogonal to [`MaskLifecycle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OcclusionState {
    /// Fully visible.
    #[default]
    Visible,
    /// Partially hidden; still redact.
    Partial,
    /// Fully occluded; skip redaction fill.
    Occluded,
    /// Unknown (adapter did not say).
    Unknown,
}

impl OcclusionState {
    /// Whether a redaction region should be emitted.
    #[must_use]
    pub const fn contributes_region(self) -> bool {
        matches!(self, Self::Visible | Self::Partial | Self::Unknown)
    }
}

/// One timed observation on a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackSample {
    /// Sample time.
    pub t: MediaTime,
    /// Owning track.
    pub track: TrackId,
    /// Optional subject handle (identity resolver may fill later).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<SubjectId>,
    /// Optional appearance / visit segment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<AppearanceId>,
    /// Optional detector observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<ObservationId>,
    /// Frame-space ROI.
    pub geometry: Geometry,
    /// Detector confidence `0..=1`.
    #[serde(default = "track_one")]
    pub conf: f32,
    /// Visibility.
    #[serde(default)]
    pub occlusion: OcclusionState,
    /// Track continuity (enter / active / exit / lost).
    #[serde(default)]
    pub lifecycle: MaskLifecycle,
    /// Optional compact-mask handle (pixels stay in the adapter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<MaskRef>,
    /// Origin metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<MaskProvenance>,
}

fn track_one() -> f32 {
    1.0
}

impl TrackSample {
    /// Ellipse sample on `track`.
    #[must_use]
    pub fn ellipse(track: TrackId, t: MediaTime, cx: f32, cy: f32, radius: f32) -> Self {
        Self {
            t,
            track,
            subject: None,
            appearance: None,
            observation: None,
            geometry: Geometry::ellipse(cx, cy, radius),
            conf: 1.0,
            occlusion: OcclusionState::Visible,
            lifecycle: MaskLifecycle::Active,
            mask: None,
            provenance: None,
        }
    }

    /// Attach a subject handle.
    #[must_use]
    pub fn with_subject(mut self, subject: SubjectId) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Attach occlusion.
    #[must_use]
    pub fn with_occlusion(mut self, occlusion: OcclusionState) -> Self {
        self.occlusion = occlusion;
        if occlusion == OcclusionState::Occluded {
            self.lifecycle = MaskLifecycle::Occluded;
        }
        self
    }

    /// Materialize a [`MaskSample`] (ROI view; ids flatten into subject + provenance).
    #[must_use]
    pub fn to_mask_sample(&self) -> MaskSample {
        let (cx, cy) = self.geometry.center();
        let mut sample = match self.geometry.as_box() {
            Some((left, top, right, bottom)) => {
                MaskSample::from_box(self.t, left, top, right, bottom, self.conf)
            }
            None => MaskSample::ellipse(self.t, cx, cy, self.geometry.radius()),
        };
        sample.conf = self.conf;
        sample.lifecycle = self.lifecycle;
        sample.occluded =
            !self.occlusion.contributes_region() || self.lifecycle == MaskLifecycle::Occluded;
        if let Some(subject) = self.subject.clone() {
            sample.subject = Some(subject);
        } else {
            sample.subject = Some(SubjectId::new(self.track.as_str()));
        }
        if let Some(mask) = &self.mask {
            sample.asset = Some(crate::mask_asset::MaskAssetRef::external(
                mask.uri
                    .clone()
                    .unwrap_or_else(|| "sightloom".into()),
                mask_ref_id(mask),
            ));
        }
        let mut prov = self.provenance.clone().unwrap_or_default();
        if prov.track_id.is_none() {
            prov.track_id = Some(self.track.0.clone());
        }
        sample.provenance = Some(prov);
        sample
    }
}

fn mask_ref_id(mask: &MaskRef) -> u64 {
    if let Ok(n) = mask.observation.as_str().parse::<u64>() {
        return n;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    mask.observation.as_str().hash(&mut hasher);
    if let Some(uri) = &mask.uri {
        uri.hash(&mut hasher);
    }
    hasher.finish()
}

/// One tracker trajectory. Identity source for redaction.
///
/// [`MaskTimeline`] is produced via [`Self::to_mask_timeline`] — a view, not
/// the place where tracks are authored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackTimeline {
    /// Track handle.
    pub track: TrackId,
    /// Optional subject (may be unset until Intelligence / re-id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<SubjectId>,
    /// Time-ordered samples.
    #[serde(default)]
    pub samples: Vec<TrackSample>,
    /// Interpolation (applied on the materialized mask view).
    #[serde(default)]
    pub interpolation: MaskInterpolation,
    /// Missing-sample policy for the view.
    #[serde(default)]
    pub missing_policy: MissingMaskPolicy,
    /// Track-level provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<MaskProvenance>,
}

impl TrackTimeline {
    /// Empty track.
    #[must_use]
    pub fn new(track: TrackId) -> Self {
        Self {
            track,
            subject: None,
            samples: Vec::new(),
            interpolation: MaskInterpolation::default(),
            missing_policy: MissingMaskPolicy::default(),
            provenance: None,
        }
    }

    /// Attach a subject handle.
    #[must_use]
    pub fn with_subject(mut self, subject: SubjectId) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Append a sample (keeps time order).
    pub fn push(&mut self, mut sample: TrackSample) {
        sample.track = self.track.clone();
        if sample.subject.is_none() {
            sample.subject.clone_from(&self.subject);
        }
        self.samples.push(sample);
        self.samples.sort_by(|a, b| {
            a.t.as_secs()
                .partial_cmp(&b.t.as_secs())
                .unwrap_or(core::cmp::Ordering::Equal)
        });
    }

    /// Materialize a per-track [`MaskTimeline`] (redaction view).
    #[must_use]
    pub fn to_mask_timeline(&self) -> MaskTimeline {
        let mut tl = MaskTimeline::new();
        tl.interpolation = self.interpolation;
        tl.missing_policy = self.missing_policy;
        for sample in &self.samples {
            let mut mask = sample.to_mask_sample();
            if mask.subject.is_none() {
                mask.subject = self
                    .subject
                    .clone()
                    .or_else(|| Some(SubjectId::new(self.track.as_str())));
            }
            tl.push(mask);
        }
        tl
    }
}

/// Merge many tracks into one fused [`MaskTimeline`] (one redaction node).
#[must_use]
pub fn mask_timeline_from_tracks<'a>(
    tracks: impl IntoIterator<Item = &'a TrackTimeline>,
) -> MaskTimeline {
    MaskTimeline::merge_by_subject(tracks.into_iter().map(TrackTimeline::to_mask_timeline))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> MediaTime {
        MediaTime::new(0, 30).unwrap()
    }

    #[test]
    fn track_materializes_mask_with_track_id() {
        let mut track =
            TrackTimeline::new(TrackId::new("tr_1")).with_subject(SubjectId::new("person_a"));
        track.push(TrackSample::ellipse(
            TrackId::new("tr_1"),
            t0(),
            10.0,
            20.0,
            5.0,
        ));
        let view = track.to_mask_timeline();
        assert_eq!(view.samples.len(), 1);
        assert_eq!(view.samples[0].subject_id(), SubjectId::new("person_a"));
        assert_eq!(
            view.samples[0]
                .provenance
                .as_ref()
                .and_then(|p| p.track_id.as_deref()),
            Some("tr_1")
        );
        assert_eq!(view.regions_at(t0()).len(), 1);
    }

    #[test]
    fn occluded_sample_drops_from_view() {
        let mut track = TrackTimeline::new(TrackId::new("tr"));
        track.push(
            TrackSample::ellipse(TrackId::new("tr"), t0(), 1.0, 1.0, 4.0)
                .with_occlusion(OcclusionState::Occluded),
        );
        let view = track.to_mask_timeline();
        assert!(view.regions_at(t0()).is_empty());
    }

    #[test]
    fn fuse_two_tracks() {
        let mut a = TrackTimeline::new(TrackId::new("a")).with_subject(SubjectId::new("sa"));
        a.push(TrackSample::ellipse(TrackId::new("a"), t0(), 0.0, 0.0, 3.0));
        let mut b = TrackTimeline::new(TrackId::new("b")).with_subject(SubjectId::new("sb"));
        b.push(TrackSample::ellipse(
            TrackId::new("b"),
            t0(),
            80.0,
            80.0,
            3.0,
        ));
        let fused = mask_timeline_from_tracks([&a, &b]);
        assert_eq!(fused.subjects().len(), 2);
        assert_eq!(fused.regions_at(t0()).len(), 2);
    }
}
