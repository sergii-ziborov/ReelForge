//! Mask timelines for redaction and vision materialization.
//!
//! # Identity model
//!
//! Samples carry an optional [`SubjectId`]. Interpolation and merge are
//! **track-safe**: each subject is a separate temporal chain. Flat merges that
//! interleave unrelated boxes no longer break linear interpolation.
//!
//! Lifecycle + provenance let Capture / `SightLoom` adapters express occlusion
//! and origin without collapsing multi-person tracks into one ROI chain.

use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// Stable subject / track identity within a graph (person, plate, face, …).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SubjectId(pub String);

impl SubjectId {
    /// Construct from any string-like id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Anonymous single-track identity used when samples omit `subject`.
    #[must_use]
    pub fn anonymous() -> Self {
        Self("_anon".into())
    }

    /// As string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the anonymous bucket.
    #[must_use]
    pub fn is_anonymous(&self) -> bool {
        self.0 == "_anon"
    }
}

impl fmt::Display for SubjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<&str> for SubjectId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for SubjectId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// Lifecycle of a subject observation at a sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MaskLifecycle {
    /// First frames of a track (fade-in / soft enter).
    Entering,
    /// Normal visible tracking.
    #[default]
    Active,
    /// Temporarily occluded (still tracked; usually skip redaction fill).
    Occluded,
    /// Last frames before loss (fade-out).
    Exiting,
    /// Tracker lost / terminal sample.
    Lost,
}

impl MaskLifecycle {
    /// Whether this sample should contribute a redaction region by default.
    #[must_use]
    pub const fn contributes_region(self) -> bool {
        matches!(self, Self::Entering | Self::Active | Self::Exiting)
    }
}

/// Origin of a mask sample (adapter / model / external track id).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MaskProvenance {
    /// Source system (`sightloom`, `manual`, `capture`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// External track id from the vision pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    /// Detector / model label when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Free-form notes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl MaskProvenance {
    /// SightLoom-style provenance.
    #[must_use]
    pub fn sightloom(track_id: impl Into<String>) -> Self {
        Self {
            source: Some("sightloom".into()),
            track_id: Some(track_id.into()),
            model: None,
            notes: None,
        }
    }

    /// Manual / authoring provenance.
    #[must_use]
    pub fn manual() -> Self {
        Self {
            source: Some("manual".into()),
            track_id: None,
            model: None,
            notes: None,
        }
    }
}

/// Policy when a sample is missing at query time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MissingMaskPolicy {
    /// Treat as fully transparent (no redaction).
    #[default]
    Transparent,
    /// Hold last known sample.
    HoldLast,
    /// Fully opaque redaction region (conservative privacy).
    Opaque,
}

/// Spatial interpolation between mask samples **within one subject track**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MaskInterpolation {
    /// Hold until next sample.
    Hold,
    /// Linear box/center blend (default).
    #[default]
    Linear,
}

/// One timed mask sample (ellipse / box proxy for ROI redaction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskSample {
    /// Sample time.
    pub t: MediaTime,
    /// Subject identity (omit / null → anonymous single track).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<SubjectId>,
    /// Center X (pixels).
    pub cx: f32,
    /// Center Y (pixels).
    pub cy: f32,
    /// Radius or half-extent (pixels).
    pub radius: f32,
    /// Optional confidence `0..=1`.
    #[serde(default = "one")]
    pub conf: f32,
    /// Lifecycle at this sample.
    #[serde(default)]
    pub lifecycle: MaskLifecycle,
    /// Explicit occlusion flag (in addition to [`MaskLifecycle::Occluded`]).
    #[serde(default)]
    pub occluded: bool,
    /// Provenance / adapter metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<MaskProvenance>,
    /// Optional axis-aligned box left edge (if present, preferred for fusion).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<f32>,
    /// Optional axis-aligned box top edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<f32>,
    /// Optional axis-aligned box right edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<f32>,
    /// Optional axis-aligned box bottom edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<f32>,
}

fn one() -> f32 {
    1.0
}

impl MaskSample {
    /// Resolved subject id (anonymous when unset).
    #[must_use]
    pub fn subject_id(&self) -> SubjectId {
        self.subject.clone().unwrap_or_else(SubjectId::anonymous)
    }

    /// Whether this sample should feed a redaction region.
    #[must_use]
    pub fn contributes_region(&self) -> bool {
        !self.occluded && self.lifecycle.contributes_region() && self.conf > 0.0
    }

    /// Ellipse sample.
    #[must_use]
    pub fn ellipse(t: MediaTime, cx: f32, cy: f32, radius: f32) -> Self {
        Self {
            t,
            subject: None,
            cx,
            cy,
            radius: radius.max(1.0),
            conf: 1.0,
            lifecycle: MaskLifecycle::Active,
            occluded: false,
            provenance: None,
            left: None,
            top: None,
            right: None,
            bottom: None,
        }
    }

    /// Ellipse with subject identity.
    #[must_use]
    pub fn ellipse_subject(
        subject: SubjectId,
        t: MediaTime,
        cx: f32,
        cy: f32,
        radius: f32,
    ) -> Self {
        let mut s = Self::ellipse(t, cx, cy, radius);
        s.subject = Some(subject);
        s
    }

    /// From axis-aligned box.
    #[must_use]
    pub fn from_box(t: MediaTime, left: f32, top: f32, right: f32, bottom: f32, conf: f32) -> Self {
        let w = (right - left).abs();
        let h = (bottom - top).abs();
        Self {
            t,
            subject: None,
            cx: left + w * 0.5,
            cy: top + h * 0.5,
            radius: ((w * w + h * h).sqrt() * 0.5).max(1.0),
            conf: conf.clamp(0.0, 1.0),
            lifecycle: MaskLifecycle::Active,
            occluded: false,
            provenance: None,
            left: Some(left),
            top: Some(top),
            right: Some(right),
            bottom: Some(bottom),
        }
    }

    /// Box with subject identity.
    #[must_use]
    pub fn from_box_subject(
        subject: SubjectId,
        t: MediaTime,
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        conf: f32,
    ) -> Self {
        let mut s = Self::from_box(t, left, top, right, bottom, conf);
        s.subject = Some(subject);
        s
    }

    /// Attach subject id.
    #[must_use]
    pub fn with_subject(mut self, subject: SubjectId) -> Self {
        self.subject = Some(subject);
        self
    }

    /// Attach lifecycle.
    #[must_use]
    pub fn with_lifecycle(mut self, lifecycle: MaskLifecycle) -> Self {
        self.lifecycle = lifecycle;
        if lifecycle == MaskLifecycle::Occluded {
            self.occluded = true;
        }
        self
    }

    /// Attach provenance.
    #[must_use]
    pub fn with_provenance(mut self, provenance: MaskProvenance) -> Self {
        self.provenance = Some(provenance);
        self
    }

    /// Mark occluded.
    #[must_use]
    pub fn with_occluded(mut self, occluded: bool) -> Self {
        self.occluded = occluded;
        if occluded {
            self.lifecycle = MaskLifecycle::Occluded;
        }
        self
    }
}

/// Interpolated region at a query time (identity-aware).
#[derive(Debug, Clone, PartialEq)]
pub struct MaskRegionAt {
    /// Subject that owns this region.
    pub subject: SubjectId,
    /// Center X.
    pub cx: f32,
    /// Center Y.
    pub cy: f32,
    /// Radius.
    pub radius: f32,
    /// Confidence.
    pub conf: f32,
    /// Lifecycle at the sample used / nearest.
    pub lifecycle: MaskLifecycle,
}

impl MaskRegionAt {
    /// Tuple form used by older redaction paths: `(cx, cy, radius, conf)`.
    #[must_use]
    pub fn as_tuple(&self) -> (f32, f32, f32, f32) {
        (self.cx, self.cy, self.radius, self.conf)
    }
}

/// Timed multi-subject mask for fused redaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MaskTimeline {
    /// Ordered samples (sort key: subject, then time).
    #[serde(default)]
    pub samples: Vec<MaskSample>,
    /// Interpolation mode (applied **per subject track**).
    #[serde(default)]
    pub interpolation: MaskInterpolation,
    /// Missing sample policy (per subject).
    #[serde(default)]
    pub missing_policy: MissingMaskPolicy,
}

impl MaskTimeline {
    /// Empty timeline.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push and keep subject-then-time order.
    pub fn push(&mut self, sample: MaskSample) {
        self.samples.push(sample);
        self.sort_samples();
    }

    fn sort_samples(&mut self) {
        self.samples.sort_by(|a, b| {
            let sa = a.subject_id();
            let sb = b.subject_id();
            sa.cmp(&sb).then_with(|| {
                a.t.as_secs()
                    .partial_cmp(&b.t.as_secs())
                    .unwrap_or(core::cmp::Ordering::Equal)
            })
        });
    }

    /// Unique subject ids present (sorted).
    #[must_use]
    pub fn subjects(&self) -> Vec<SubjectId> {
        let mut ids: Vec<SubjectId> = self.samples.iter().map(MaskSample::subject_id).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Samples belonging to one subject (time-ordered).
    #[must_use]
    pub fn samples_for(&self, subject: &SubjectId) -> Vec<&MaskSample> {
        self.samples
            .iter()
            .filter(|s| &s.subject_id() == subject)
            .collect()
    }

    /// Group samples by subject (`BTreeMap` for stable iteration).
    #[must_use]
    pub fn group_by_subject(&self) -> BTreeMap<SubjectId, Vec<&MaskSample>> {
        let mut map: BTreeMap<SubjectId, Vec<&MaskSample>> = BTreeMap::new();
        for s in &self.samples {
            map.entry(s.subject_id()).or_default().push(s);
        }
        map
    }

    /// Track-safe merge: union of samples, **per-subject** chains preserved.
    ///
    /// Prefer this over dumping multi-person detections into one flat list.
    #[must_use]
    pub fn merge_by_subject(timelines: impl IntoIterator<Item = Self>) -> Self {
        let mut out = Self::new();
        for tl in timelines {
            out.interpolation = tl.interpolation;
            out.missing_policy = tl.missing_policy;
            for s in tl.samples {
                out.samples.push(s);
            }
        }
        out.sort_samples();
        out
    }

    /// Alias for [`Self::merge_by_subject`] (historical name).
    #[must_use]
    pub fn merge(timelines: impl IntoIterator<Item = Self>) -> Self {
        Self::merge_by_subject(timelines)
    }

    /// Assign a subject id to all anonymous samples (in place).
    pub fn assign_anonymous(&mut self, subject: &SubjectId) {
        for s in &mut self.samples {
            if s.subject.is_none() {
                s.subject = Some(subject.clone());
            }
        }
        self.sort_samples();
    }

    /// Drop samples that do not contribute regions (occluded / lost / conf=0).
    pub fn prune_non_contributing(&mut self) {
        self.samples.retain(MaskSample::contributes_region);
    }

    /// Identity-aware regions at time `t` (one entry per active subject).
    #[must_use]
    pub fn regions_at_identity(&self, t: MediaTime) -> Vec<MaskRegionAt> {
        if self.samples.is_empty() {
            return match self.missing_policy {
                MissingMaskPolicy::Opaque => vec![MaskRegionAt {
                    subject: SubjectId::anonymous(),
                    cx: 0.0,
                    cy: 0.0,
                    radius: 1.0e6,
                    conf: 1.0,
                    lifecycle: MaskLifecycle::Active,
                }],
                MissingMaskPolicy::Transparent | MissingMaskPolicy::HoldLast => Vec::new(),
            };
        }

        let mut out = Vec::new();
        for (subject, track) in self.group_by_subject() {
            if let Some(region) =
                interpolate_track(track.as_slice(), t, self.interpolation, self.missing_policy)
            {
                out.push(MaskRegionAt {
                    subject,
                    cx: region.0,
                    cy: region.1,
                    radius: region.2,
                    conf: region.3,
                    lifecycle: region.4,
                });
            }
        }
        out
    }

    /// Sample region list at time `t` as `(cx, cy, radius, conf)` tuples.
    ///
    /// Interpolates **per subject**, then unions. Occluded / non-contributing
    /// samples are skipped.
    #[must_use]
    pub fn regions_at(&self, t: MediaTime) -> Vec<(f32, f32, f32, f32)> {
        self.regions_at_identity(t)
            .into_iter()
            .map(|r| r.as_tuple())
            .collect()
    }
}

/// `(cx, cy, radius, conf, lifecycle)` for one subject track.
fn interpolate_track(
    track: &[&MaskSample],
    t: MediaTime,
    interpolation: MaskInterpolation,
    missing: MissingMaskPolicy,
) -> Option<(f32, f32, f32, f32, MaskLifecycle)> {
    if track.is_empty() {
        return None;
    }
    // Only samples that can contribute; keep occluded for hold gaps if HoldLast?
    // For Active redaction we skip occluded entirely.
    let active: Vec<&MaskSample> = track
        .iter()
        .copied()
        .filter(|s| s.contributes_region())
        .collect();
    if active.is_empty() {
        return None;
    }

    let ts = t.as_secs();
    match interpolation {
        MaskInterpolation::Hold => {
            let mut last = None;
            for s in &active {
                if s.t.as_secs() <= ts {
                    last = Some(*s);
                } else {
                    break;
                }
            }
            match last {
                Some(s) => Some((s.cx, s.cy, s.radius, s.conf, s.lifecycle)),
                None => match missing {
                    MissingMaskPolicy::Opaque => {
                        Some((0.0, 0.0, 1.0e6, 1.0, MaskLifecycle::Active))
                    }
                    MissingMaskPolicy::HoldLast => {
                        let s = active[0];
                        Some((s.cx, s.cy, s.radius, s.conf, s.lifecycle))
                    }
                    MissingMaskPolicy::Transparent => None,
                },
            }
        }
        MaskInterpolation::Linear => {
            if active.len() == 1 {
                let s = active[0];
                return Some((s.cx, s.cy, s.radius, s.conf, s.lifecycle));
            }
            if ts <= active[0].t.as_secs() {
                let s = active[0];
                return Some((s.cx, s.cy, s.radius, s.conf, s.lifecycle));
            }
            let last = *active.last()?;
            if ts >= last.t.as_secs() {
                return Some((last.cx, last.cy, last.radius, last.conf, last.lifecycle));
            }
            for w in active.windows(2) {
                let a = w[0];
                let b = w[1];
                let ta = a.t.as_secs();
                let tb = b.t.as_secs();
                if ts >= ta && ts <= tb {
                    #[allow(clippy::cast_possible_truncation)]
                    let u = ((ts - ta) / (tb - ta).max(1e-12)) as f32;
                    return Some((
                        a.cx + (b.cx - a.cx) * u,
                        a.cy + (b.cy - a.cy) * u,
                        a.radius + (b.radius - a.radius) * u,
                        a.conf + (b.conf - a.conf) * u,
                        if u < 0.5 { a.lifecycle } else { b.lifecycle },
                    ));
                }
            }
            Some((last.cx, last.cy, last.radius, last.conf, last.lifecycle))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::MediaTime;

    fn t0() -> MediaTime {
        MediaTime::new(0, 30).unwrap()
    }

    fn t15() -> MediaTime {
        MediaTime::new(15, 30).unwrap() // 0.5s
    }

    fn t30() -> MediaTime {
        MediaTime::new(30, 30).unwrap() // 1s
    }

    #[test]
    fn merge_two_subjects_near_same_time() {
        let mut a = MaskTimeline::new();
        a.push(MaskSample::ellipse_subject(
            SubjectId::new("face_a"),
            t0(),
            10.0,
            10.0,
            5.0,
        ));
        let mut b = MaskTimeline::new();
        b.push(MaskSample::ellipse_subject(
            SubjectId::new("face_b"),
            t0(),
            50.0,
            50.0,
            8.0,
        ));
        let m = MaskTimeline::merge_by_subject([a, b]);
        assert_eq!(m.subjects().len(), 2);
        let r = m.regions_at(t0());
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn linear_interp_is_per_subject_not_flat() {
        // Without subject ids, two people at t=0 and t=1 would incorrectly
        // interpolate into one moving box. With subjects, each holds.
        let mut a = MaskTimeline::new();
        a.push(MaskSample::ellipse(t0(), 0.0, 0.0, 10.0).with_subject(SubjectId::new("a")));
        a.push(MaskSample::ellipse(t30(), 0.0, 0.0, 10.0).with_subject(SubjectId::new("a")));
        let mut b = MaskTimeline::new();
        b.push(MaskSample::ellipse(t0(), 100.0, 100.0, 10.0).with_subject(SubjectId::new("b")));
        b.push(MaskSample::ellipse(t30(), 100.0, 100.0, 10.0).with_subject(SubjectId::new("b")));
        let m = MaskTimeline::merge_by_subject([a, b]);
        let r = m.regions_at_identity(t15());
        assert_eq!(r.len(), 2);
        let mut by_id: BTreeMap<_, _> = r.into_iter().map(|x| (x.subject.0.clone(), x)).collect();
        let ra = by_id.remove("a").unwrap();
        let rb = by_id.remove("b").unwrap();
        assert!((ra.cx - 0.0).abs() < 1e-3);
        assert!((rb.cx - 100.0).abs() < 1e-3);
    }

    #[test]
    fn occluded_subject_skipped() {
        let mut tl = MaskTimeline::new();
        tl.push(
            MaskSample::ellipse_subject(SubjectId::new("p1"), t0(), 10.0, 10.0, 5.0)
                .with_lifecycle(MaskLifecycle::Occluded),
        );
        tl.push(MaskSample::ellipse_subject(
            SubjectId::new("p2"),
            t0(),
            40.0,
            40.0,
            5.0,
        ));
        let r = tl.regions_at(t0());
        assert_eq!(r.len(), 1);
        assert!((r[0].0 - 40.0).abs() < 1e-3);
    }

    #[test]
    fn backward_compat_json_without_subject() {
        let json = r#"{
            "samples": [{
                "t": { "ticks": 0, "timescale": 30 },
                "cx": 1.0, "cy": 2.0, "radius": 3.0
            }]
        }"#;
        let tl: MaskTimeline = serde_json::from_str(json).unwrap();
        assert_eq!(tl.samples.len(), 1);
        assert!(tl.samples[0].subject.is_none());
        assert_eq!(tl.samples[0].subject_id(), SubjectId::anonymous());
        assert_eq!(tl.regions_at(t0()).len(), 1);
    }

    #[test]
    fn provenance_sightloom() {
        let s = MaskSample::ellipse(t0(), 1.0, 1.0, 2.0)
            .with_subject(SubjectId::new("42"))
            .with_provenance(MaskProvenance::sightloom("42"));
        assert_eq!(
            s.provenance.as_ref().unwrap().source.as_deref(),
            Some("sightloom")
        );
    }
}
