//! Bridge `MaskTimeline` / `RegionRedaction` contracts into runtime fx.

use crate::error::{IoError, Result};
use reelforge_core::{MediaTime, VideoClip, VideoEffect};
use reelforge_fx::{RegionSample, RegionTrack, TrackSet, TrackedBlur};
use reelforge_render_graph::{MaskSample, MaskTimeline, RedactionStyle, RegionRedaction};
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
    tl.push(MaskSample::from_box(t, left, top, right, bottom, 1.0));
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
}
