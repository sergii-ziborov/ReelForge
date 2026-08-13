//! Spatial primitives for [`crate::TrackSample`] (not pixel masks).

use crate::ids::ObservationId;
use serde::{Deserialize, Serialize};

/// Axis-aligned or elliptical ROI in frame pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    /// Circle / ellipse proxy (center + radius).
    Ellipse {
        /// Center X.
        cx: f32,
        /// Center Y.
        cy: f32,
        /// Radius (pixels).
        radius: f32,
    },
    /// Axis-aligned box.
    Box {
        /// Left.
        left: f32,
        /// Top.
        top: f32,
        /// Right.
        right: f32,
        /// Bottom.
        bottom: f32,
    },
}

impl Geometry {
    /// Circle at `(cx, cy)`.
    #[must_use]
    pub fn ellipse(cx: f32, cy: f32, radius: f32) -> Self {
        Self::Ellipse {
            cx,
            cy,
            radius: radius.max(1.0),
        }
    }

    /// Axis-aligned box.
    #[must_use]
    pub fn aabb(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self::Box {
            left,
            top,
            right,
            bottom,
        }
    }

    /// Center in pixels.
    #[must_use]
    pub fn center(self) -> (f32, f32) {
        match self {
            Self::Ellipse { cx, cy, .. } => (cx, cy),
            Self::Box {
                left,
                top,
                right,
                bottom,
            } => {
                let w = (right - left).abs();
                let h = (bottom - top).abs();
                (left + w * 0.5, top + h * 0.5)
            }
        }
    }

    /// Circumradius (ellipse radius, or box half-diagonal).
    #[must_use]
    pub fn radius(self) -> f32 {
        match self {
            Self::Ellipse { radius, .. } => radius.max(1.0),
            Self::Box {
                left,
                top,
                right,
                bottom,
            } => {
                let w = (right - left).abs();
                let h = (bottom - top).abs();
                ((w * w + h * h).sqrt() * 0.5).max(1.0)
            }
        }
    }

    /// Box edges when this is an AABB.
    #[must_use]
    pub fn as_box(self) -> Option<(f32, f32, f32, f32)> {
        match self {
            Self::Box {
                left,
                top,
                right,
                bottom,
            } => Some((left, top, right, bottom)),
            Self::Ellipse { .. } => None,
        }
    }
}

/// Reference to an external / compact mask blob (`SightLoom` `MaskRef` fill).
///
/// `ReelForge` does not store pixel masks here — only the handle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaskRef {
    /// Observation that produced the mask.
    pub observation: ObservationId,
    /// Optional sidecar / URI (adapter-defined).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

impl MaskRef {
    /// Handle without a sidecar URI.
    #[must_use]
    pub fn new(observation: ObservationId) -> Self {
        Self {
            observation,
            uri: None,
        }
    }

    /// Attach a sidecar URI.
    #[must_use]
    pub fn with_uri(mut self, uri: impl Into<String>) -> Self {
        self.uri = Some(uri.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_center_and_radius() {
        let g = Geometry::aabb(0.0, 0.0, 10.0, 10.0);
        let (cx, cy) = g.center();
        assert!((cx - 5.0).abs() < 1e-5);
        assert!((cy - 5.0).abs() < 1e-5);
        assert!(g.radius() > 5.0);
        assert_eq!(g.as_box(), Some((0.0, 0.0, 10.0, 10.0)));
    }
}
