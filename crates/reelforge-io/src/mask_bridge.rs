//! Bridge `MaskTimeline` / `RegionRedaction` contracts into runtime fx.

use reelforge_core::{MediaTime, VideoClip, VideoEffect};
use reelforge_fx::{RegionSample, RegionTrack, TrackSet, TrackedBlur};
use reelforge_render_graph::{MaskTimeline, RedactionStyle, RegionRedaction};
use crate::error::{IoError, Result};
use std::sync::Arc;

/// Convert a [`MaskTimeline`] into a [`TrackSet`] for [`TrackedBlur`].
#[must_use]
pub fn mask_timeline_to_track_set(masks: &MaskTimeline) -> TrackSet {
    let mut set = TrackSet::new();
    let mut track = RegionTrack::new("mask_timeline");
    for s in &masks.samples {
        let sample = if let (Some(l), Some(t), Some(r), Some(b)) =
            (s.left, s.top, s.right, s.bottom)
        {
            RegionSample::from_bbox(s.t.as_secs(), l, t, r, b, s.conf)
        } else {
            RegionSample {
                t: s.t.as_secs(),
                cx: s.cx,
                cy: s.cy,
                radius: s.radius,
                conf: s.conf,
            }
        };
        track.push(sample);
    }
    set.push(track);
    set
}

/// Apply [`RegionRedaction`] to a clip when style is Gaussian (M2 preview path).
///
/// Pixelate / Solid return an error until those kernels land.
///
/// # Errors
///
/// Unsupported style or empty masks.
pub fn apply_region_redaction(
    clip: Arc<dyn VideoClip>,
    redaction: &RegionRedaction,
) -> Result<Arc<dyn VideoClip>> {
    if redaction.masks.samples.is_empty() {
        return Err(IoError::message("RegionRedaction masks are empty"));
    }
    match &redaction.style {
        RedactionStyle::Gaussian { sigma } => {
            let tracks = mask_timeline_to_track_set(&redaction.masks);
            let blur = TrackedBlur::new(tracks).with_intensity(*sigma);
            blur.apply(clip).map_err(IoError::from)
        }
        RedactionStyle::Pixelate { .. } | RedactionStyle::Solid { .. } => Err(IoError::message(
            "RegionRedaction style pixelate/solid not implemented yet; use gaussian",
        )),
    }
}

/// Build a single-sample [`MaskTimeline`] at media time for tests / adapters.
#[must_use]
pub fn mask_timeline_from_box(
    t: MediaTime,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) -> MaskTimeline {
    let mut tl = MaskTimeline::new();
    tl.push(reelforge_render_graph::MaskSample::from_box(
        t, left, top, right, bottom, 1.0,
    ));
    tl
}
