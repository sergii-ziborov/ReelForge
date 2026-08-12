//! Apply remainder [`PlanOp`]s on a Rust clip graph (`reelforge-fx`).

use super::ops::PlanOp;
use crate::error::{IoError, Result};
use reelforge_core::{Duration, Size, Time, VideoClip, VideoEffect, subclip_video};
use reelforge_fx::{
    BlackAndWhite, Crop, EvenSize, FadeIn, FadeOut, InvertColors, MirrorX, MirrorY, MultiplyColor,
    Painting, Resize, Rotate,
};
use std::sync::Arc;

/// Fold `ops` onto `clip` (Rust / custom path).
///
/// # Errors
///
/// Invalid parameters or unknown custom op names.
pub fn apply_plan_ops(
    mut clip: Arc<dyn VideoClip>,
    ops: &[PlanOp],
) -> Result<Arc<dyn VideoClip>> {
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
        PlanOp::Crop { x, y, w, h } => Crop::new(*x, *y, *w, *h)
            .apply(clip)
            .map_err(IoError::from),
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
        other => Err(IoError::message(format!(
            "unknown custom plan op '{other}' (supported: black_and_white, invert, painting, multiply_color, identity)"
        ))),
    }
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
        assert!(validate_remainder(&[PlanOp::Custom {
            name: "head_blur".into(),
            params: None,
        }])
        .is_err());
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
