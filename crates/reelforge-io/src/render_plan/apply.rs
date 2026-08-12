//! Apply remainder [`PlanOp`]s on a Rust clip graph (`reelforge-fx`).

use super::ops::PlanOp;
use crate::error::{IoError, Result};
use crate::mask_bridge::{apply_region_redaction, region_redaction_from_value};
use crate::tracks_json::{load_track_set, track_set_from_value};
use reelforge_core::{Duration, Size, Time, VideoClip, VideoEffect, subclip_video};
use reelforge_fx::{
    BlackAndWhite, Crop, EvenSize, FadeIn, FadeOut, HeadBlur, InvertColors, MirrorX, MirrorY,
    MultiplyColor, Painting, Resize, Rotate, TrackedBlur,
};
use std::sync::Arc;

/// Fold `ops` onto `clip` (Rust / custom path).
///
/// # Errors
///
/// Invalid parameters or unknown custom op names.
pub fn apply_plan_ops(mut clip: Arc<dyn VideoClip>, ops: &[PlanOp]) -> Result<Arc<dyn VideoClip>> {
    for op in ops {
        clip = apply_one(clip, op)?;
    }
    Ok(clip)
}

fn apply_one(clip: Arc<dyn VideoClip>, op: &PlanOp) -> Result<Arc<dyn VideoClip>> {
    match op {
        PlanOp::Identity => Ok(clip),
        PlanOp::Trim { start, duration } => {
            let start = Time::from_secs(*start);
            let duration = Duration::from_secs(*duration);
            subclip_video(clip, start, duration).map_err(IoError::from)
        }
        PlanOp::Crop { x, y, w, h } => Crop::new(*x, *y, *w, *h).apply(clip).map_err(IoError::from),
        PlanOp::Scale { w, h } => Resize::to(Size::new(*w, *h))
            .apply(clip)
            .map_err(IoError::from),
        PlanOp::HFlip => MirrorX.apply(clip).map_err(IoError::from),
        PlanOp::VFlip => MirrorY.apply(clip).map_err(IoError::from),
        PlanOp::TransposeCw => Rotate::cw90().apply(clip).map_err(IoError::from),
        PlanOp::EvenDims => EvenSize.apply(clip).map_err(IoError::from),
        PlanOp::FadeIn { duration } => FadeIn::new(Duration::from_secs(*duration))
            .apply(clip)
            .map_err(IoError::from),
        PlanOp::FadeOut { duration, total: _ } => FadeOut::new(Duration::from_secs(*duration))
            .apply(clip)
            .map_err(IoError::from),
        PlanOp::Custom { name, params } => apply_custom(clip, name, params.as_ref()),
    }
}

fn apply_custom(
    clip: Arc<dyn VideoClip>,
    name: &str,
    params: Option<&serde_json::Value>,
) -> Result<Arc<dyn VideoClip>> {
    let key = name.trim().to_ascii_lowercase();
    match key.as_str() {
        "identity" | "noop" | "pass" => Ok(clip),
        "black_and_white" | "bw" | "grayscale" | "grey" => {
            BlackAndWhite.apply(clip).map_err(IoError::from)
        }
        "invert" | "invert_colors" | "negate" => InvertColors.apply(clip).map_err(IoError::from),
        "painting" | "paint" => Painting::default().apply(clip).map_err(IoError::from),
        "multiply_color" | "multiply" => {
            let factor = params
                .and_then(|p| p.get("factor"))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(1.0);
            #[allow(clippy::cast_possible_truncation)]
            MultiplyColor::new(factor as f32)
                .apply(clip)
                .map_err(IoError::from)
        }
        "head_blur" | "tracked_blur" | "privacy_blur" | "face_blur" => {
            apply_tracked_blur(clip, params)
        }
        "region_redaction" | "redaction" | "rf.redaction.region" => {
            let params = params.ok_or_else(|| {
                IoError::message(
                    "region_redaction requires params with masks (and optional style/sigma)",
                )
            })?;
            let redaction = region_redaction_from_value(params)?;
            apply_region_redaction(clip, &redaction)
        }
        other => Err(IoError::message(format!(
            "unknown custom plan op '{other}' (supported: black_and_white, invert, painting, multiply_color, head_blur/tracked_blur, region_redaction, identity)"
        ))),
    }
}

#[allow(clippy::cast_possible_truncation)]
fn apply_tracked_blur(
    clip: Arc<dyn VideoClip>,
    params: Option<&serde_json::Value>,
) -> Result<Arc<dyn VideoClip>> {
    let params = params.ok_or_else(|| {
        IoError::message(
            "head_blur/tracked_blur requires params: tracks (object/array) or tracks_path, optional radius/feather",
        )
    })?;

    let tracks = if let Some(path) = params.get("tracks_path").and_then(|v| v.as_str()) {
        load_track_set(path)?
    } else if let Some(tracks_val) = params.get("tracks") {
        track_set_from_value(tracks_val)?
    } else {
        // Allow params itself to be the tracks document.
        track_set_from_value(params).map_err(|_| {
            IoError::message("head_blur params must include \"tracks\" or \"tracks_path\"")
        })?
    };

    if tracks.is_empty() {
        return Err(IoError::message("head_blur tracks list is empty"));
    }

    // Single static region shortcut: radius + cx/cy without multi-track.
    if tracks.len() == 1 && tracks.tracks[0].samples.len() == 1 {
        let s = &tracks.tracks[0].samples[0];
        let radius = params
            .get("radius")
            .and_then(serde_json::Value::as_f64)
            .map_or(s.radius, |r| r as f32);
        let feather = params
            .get("feather")
            .and_then(serde_json::Value::as_f64)
            .map_or(0.35, |f| f as f32);
        let mut blur = HeadBlur::fixed(s.cx, s.cy, radius).with_feather(feather);
        if let Some(intensity) = params.get("intensity").and_then(serde_json::Value::as_f64) {
            blur.intensity = Some(intensity as f32);
        }
        return blur.apply(clip).map_err(IoError::from);
    }

    let mut blur = TrackedBlur::new(tracks);
    if let Some(scale) = params
        .get("radius_scale")
        .and_then(serde_json::Value::as_f64)
    {
        blur = blur.with_radius_scale(scale as f32);
    }
    if let Some(radius) = params.get("radius").and_then(serde_json::Value::as_f64) {
        blur = blur.with_fixed_radius(radius as f32);
    }
    if let Some(feather) = params.get("feather").and_then(serde_json::Value::as_f64) {
        blur = blur.with_feather(feather as f32);
    }
    if let Some(intensity) = params.get("intensity").and_then(serde_json::Value::as_f64) {
        blur = blur.with_intensity(intensity as f32);
    }
    blur.apply(clip).map_err(IoError::from)
}

/// Whether a custom name is registered for the hybrid runner.
#[must_use]
pub fn is_known_custom(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "identity"
            | "noop"
            | "pass"
            | "black_and_white"
            | "bw"
            | "grayscale"
            | "grey"
            | "invert"
            | "invert_colors"
            | "negate"
            | "painting"
            | "paint"
            | "multiply_color"
            | "multiply"
            | "head_blur"
            | "tracked_blur"
            | "privacy_blur"
            | "face_blur"
            | "region_redaction"
            | "redaction"
            | "rf.redaction.region"
    )
}

/// Validate that every remainder op can be applied (incl. known customs).
///
/// # Errors
///
/// Returns the first unknown custom name.
pub fn validate_remainder(ops: &[PlanOp]) -> Result<()> {
    for op in ops {
        if let PlanOp::Custom { name, .. } = op
            && !is_known_custom(name)
        {
            return Err(IoError::message(format!(
                "unknown custom plan op '{name}' cannot run on hybrid Rust path"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8, Time};

    #[test]
    fn applies_bw_and_scale() {
        let base: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 24),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let out = apply_plan_ops(
            base,
            &[
                PlanOp::Custom {
                    name: "black_and_white".into(),
                    params: None,
                },
                PlanOp::Scale { w: 16, h: 12 },
            ],
        )
        .unwrap();
        assert_eq!(out.size(), Size::new(16, 12));
        let f = out.frame_at(Time::from_secs(0.0)).unwrap();
        // Grayscale of red is not pure red.
        assert_eq!(f.data()[0], f.data()[1]);
    }

    #[test]
    fn rejects_unknown_custom() {
        assert!(
            validate_remainder(&[PlanOp::Custom {
                name: "not_a_real_effect".into(),
                params: None,
            }])
            .is_err()
        );
    }

    #[test]
    fn head_blur_is_known() {
        assert!(is_known_custom("head_blur"));
        assert!(is_known_custom("tracked_blur"));
        assert!(is_known_custom("region_redaction"));
        assert!(is_known_custom("rf.redaction.region"));
    }

    #[test]
    fn region_redaction_from_plan_params() {
        let base: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(64, 64),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let params = serde_json::json!({
            "sigma": 10.0,
            "masks": {
                "samples": [{
                    "t": { "ticks": 0, "timescale": 30 },
                    "cx": 32.0,
                    "cy": 32.0,
                    "radius": 12.0
                }]
            }
        });
        let out = apply_plan_ops(
            base,
            &[PlanOp::Custom {
                name: "region_redaction".into(),
                params: Some(params),
            }],
        )
        .unwrap();
        let _ = out.frame_at(Time::from_secs(0.0)).unwrap();
    }

    #[test]
    fn tracked_blur_from_inline_tracks() {
        let base: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(64, 64),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let params = serde_json::json!({
            "tracks": [{
                "id": "face_1",
                "kind": "face",
                "samples": [
                    {"t": 0.0, "cx": 32.0, "cy": 32.0, "radius": 12.0},
                    {"t": 1.0, "cx": 40.0, "cy": 30.0, "radius": 14.0}
                ]
            }],
            "feather": 0.3
        });
        let out = apply_plan_ops(
            base,
            &[PlanOp::Custom {
                name: "tracked_blur".into(),
                params: Some(params),
            }],
        )
        .unwrap();
        let _ = out.frame_at(Time::from_secs(0.5)).unwrap();
    }

    #[test]
    fn timed_subclip_via_trim() {
        let base: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::BLUE,
            Duration::from_secs(4.0),
        ));
        let out = apply_plan_ops(
            base,
            &[PlanOp::Trim {
                start: 1.0,
                duration: 1.5,
            }],
        )
        .unwrap();
        assert!((out.duration().as_secs() - 1.5).abs() < 1e-9);
    }
}
