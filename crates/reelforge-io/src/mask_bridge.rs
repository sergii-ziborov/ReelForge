//! Bridge `MaskTimeline` / `RegionRedaction` contracts into runtime fx.

use crate::error::{IoError, Result};
use reelforge_core::{MediaTime, VideoClip, VideoEffect};
use reelforge_fx::{
    CoverageMask, PrivacyStyle, RegionSample, RegionTrack, TrackSet, TrackedPrivacy,
};
use reelforge_render_graph::{
    MaskSample, MaskTimeline, RedactionStyle, RegionRedaction, SubjectId, TrackTimeline,
    mask_timeline_from_tracks,
};
use std::sync::Arc;

/// Convert tracks to a [`TrackSet`] via the mask view.
#[must_use]
pub fn track_timelines_to_track_set<'a>(
    tracks: impl IntoIterator<Item = &'a TrackTimeline>,
) -> TrackSet {
    mask_timeline_to_track_set(&mask_timeline_from_tracks(tracks))
}

/// Convert a [`MaskTimeline`] into a [`TrackSet`] for privacy effects.
///
/// One [`RegionTrack`] per [`SubjectId`] (anonymous samples share `_anon`).
/// Occluded / non-contributing samples are skipped so runtime tracks stay clean.
#[must_use]
pub fn mask_timeline_to_track_set(masks: &MaskTimeline) -> TrackSet {
    let mut set = TrackSet::new();
    for (subject, samples) in masks.group_by_subject() {
        let mut track = RegionTrack::new(subject.as_str());
        if let Some(kind) = samples
            .iter()
            .find_map(|s| s.provenance.as_ref().and_then(|p| p.source.clone()))
        {
            track = track.with_kind(kind);
        }
        for s in samples {
            if !s.contributes_region() {
                continue;
            }
            let mut sample =
                if let (Some(l), Some(t), Some(r), Some(b)) = (s.left, s.top, s.right, s.bottom) {
                    RegionSample::from_bbox(s.t.as_secs(), l, t, r, b, s.conf)
                } else {
                    RegionSample {
                        t: s.t.as_secs(),
                        cx: s.cx,
                        cy: s.cy,
                        radius: s.radius,
                        conf: s.conf,
                        coverage: None,
                    }
                };
            if let Some(asset) = s.asset.as_ref().and_then(|a| a.asset.to_coverage()) {
                sample.coverage = Some(CoverageMask {
                    left: asset.left,
                    top: asset.top,
                    width: asset.width,
                    height: asset.height,
                    data: std::sync::Arc::new(asset.data),
                });
            }
            track.push(sample);
        }
        if !track.samples.is_empty() {
            set.push(track);
        }
    }
    // Stable order by track id (SubjectId already BTree-ordered via group_by_subject).
    set.tracks.sort_by(|a, b| a.id.cmp(&b.id));
    set
}

/// Map contract [`RedactionStyle`] → fx [`PrivacyStyle`].
#[must_use]
pub fn privacy_style_from_redaction(style: &RedactionStyle) -> PrivacyStyle {
    match style {
        RedactionStyle::Gaussian { sigma } => PrivacyStyle::Gaussian {
            sigma: sigma.max(0.5),
        },
        RedactionStyle::Pixelate { block_size } => PrivacyStyle::Pixelate {
            block_size: (*block_size).max(2),
        },
        RedactionStyle::Solid { color } => PrivacyStyle::Solid { color: *color },
    }
}

/// Apply [`RegionRedaction`] (gaussian / pixelate / solid) via [`TrackedPrivacy`].
///
/// # Errors
///
/// Empty masks or effect failure.
pub fn apply_region_redaction(
    clip: Arc<dyn VideoClip>,
    redaction: &RegionRedaction,
) -> Result<Arc<dyn VideoClip>> {
    if redaction.masks.samples.is_empty() {
        return Err(IoError::message("RegionRedaction masks are empty"));
    }
    let tracks = mask_timeline_to_track_set(&redaction.masks);
    if tracks.is_empty() {
        return Err(IoError::message(
            "RegionRedaction has no contributing samples (all occluded/lost?)",
        ));
    }
    let style = privacy_style_from_redaction(&redaction.style);
    TrackedPrivacy::new(tracks, style)
        .apply(clip)
        .map_err(IoError::from)
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
    tl.push(MaskSample::from_box(t, left, top, right, bottom, 1.0));
    tl
}

/// Build a subject-tagged single-sample timeline (vision adapter helper).
#[must_use]
pub fn mask_timeline_from_box_subject(
    subject: SubjectId,
    t: MediaTime,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) -> MaskTimeline {
    let mut tl = MaskTimeline::new();
    tl.push(MaskSample::from_box_subject(
        subject, t, left, top, right, bottom, 1.0,
    ));
    tl
}

/// Parse [`RegionRedaction`] from plan/custom JSON params.
///
/// Accepted shapes:
/// - full `{ "style": {…}, "masks": { "samples": […] } }`
/// - `{ "sigma": 12, "masks": … }` → Gaussian
/// - `{ "masks": … }` → default Gaussian
///
/// # Errors
///
/// Malformed JSON or empty masks.
pub fn region_redaction_from_value(value: &serde_json::Value) -> Result<RegionRedaction> {
    if let Ok(r) = serde_json::from_value::<RegionRedaction>(value.clone()) {
        if r.masks.samples.is_empty() {
            return Err(IoError::message("RegionRedaction masks are empty"));
        }
        return Ok(r);
    }

    let masks = value
        .get("masks")
        .ok_or_else(|| IoError::message("region_redaction requires \"masks\""))?;
    let timeline: MaskTimeline = serde_json::from_value(masks.clone())
        .map_err(|e| IoError::message(format!("invalid masks: {e}")))?;
    if timeline.samples.is_empty() {
        return Err(IoError::message("RegionRedaction masks are empty"));
    }

    #[allow(clippy::cast_possible_truncation)]
    let style = if let Some(style) = value.get("style") {
        serde_json::from_value(style.clone())
            .map_err(|e| IoError::message(format!("invalid redaction style: {e}")))?
    } else if let Some(sigma) = value.get("sigma").and_then(serde_json::Value::as_f64) {
        RedactionStyle::Gaussian {
            sigma: (sigma as f32).max(0.5),
        }
    } else {
        RedactionStyle::default()
    };

    Ok(RegionRedaction {
        masks: timeline,
        style,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Duration, Rgb8, Size, Time};
    use reelforge_render_graph::{MaskLifecycle, MaskProvenance, SubjectId};

    #[test]
    fn parse_and_apply_gaussian() {
        let params = serde_json::json!({
            "sigma": 8.0,
            "masks": {
                "samples": [{
                    "t": { "ticks": 0, "timescale": 30 },
                    "cx": 16.0,
                    "cy": 16.0,
                    "radius": 8.0
                }]
            }
        });
        let redaction = region_redaction_from_value(&params).unwrap();
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let out = apply_region_redaction(clip, &redaction).unwrap();
        let _ = out.frame_at(Time::ZERO).unwrap();
    }

    #[test]
    fn multi_subject_tracks() {
        let mut tl = MaskTimeline::new();
        tl.push(
            MaskSample::ellipse_subject(
                SubjectId::new("face_1"),
                MediaTime::new(0, 30).unwrap(),
                8.0,
                8.0,
                4.0,
            )
            .with_provenance(MaskProvenance::sightloom("face_1")),
        );
        tl.push(MaskSample::ellipse_subject(
            SubjectId::new("face_2"),
            MediaTime::new(0, 30).unwrap(),
            24.0,
            24.0,
            4.0,
        ));
        tl.push(
            MaskSample::ellipse_subject(
                SubjectId::new("face_1"),
                MediaTime::new(15, 30).unwrap(),
                10.0,
                10.0,
                4.0,
            )
            .with_lifecycle(MaskLifecycle::Occluded),
        );
        let set = mask_timeline_to_track_set(&tl);
        assert_eq!(set.len(), 2, "occluded sample does not drop the track");
        assert_eq!(set.tracks[0].id, "face_1");
        assert_eq!(set.tracks[0].samples.len(), 1, "occluded sample skipped");
        assert_eq!(set.tracks[1].id, "face_2");
        let regions = set.regions_at(0.0);
        assert_eq!(regions.len(), 2);
    }

    #[test]
    fn apply_pixelate_and_solid() {
        let masks = {
            let mut tl = MaskTimeline::new();
            tl.push(MaskSample::ellipse(
                MediaTime::new(0, 30).unwrap(),
                16.0,
                16.0,
                10.0,
            ));
            tl
        };
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let pix = RegionRedaction {
            masks: masks.clone(),
            style: RedactionStyle::Pixelate { block_size: 4 },
        };
        let _ = apply_region_redaction(Arc::clone(&clip), &pix)
            .unwrap()
            .frame_at(Time::ZERO)
            .unwrap();
        let solid = RegionRedaction {
            masks,
            style: RedactionStyle::Solid {
                color: reelforge_core::Rgba8::new(0, 0, 0, 255),
            },
        };
        let _ = apply_region_redaction(clip, &solid)
            .unwrap()
            .frame_at(Time::ZERO)
            .unwrap();
    }
}
