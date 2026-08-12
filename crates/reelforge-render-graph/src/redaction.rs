//! Generic region redaction (privacy) driven by [`crate::MaskTimeline`].

use crate::mask::MaskTimeline;
use reelforge_core::Rgba8;
use serde::{Deserialize, Serialize};

/// How to fill / obscure a masked region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "style", rename_all = "snake_case")]
pub enum RedactionStyle {
    /// Gaussian-style blur (sigma in pixels).
    Gaussian {
        /// Blur strength.
        sigma: f32,
    },
    /// Blocky pixelation.
    Pixelate {
        /// Block size in pixels.
        block_size: u16,
    },
    /// Solid fill.
    Solid {
        /// Fill color.
        color: Rgba8,
    },
}

impl Default for RedactionStyle {
    fn default() -> Self {
        Self::Gaussian { sigma: 12.0 }
    }
}

/// Fused multi-ROI redaction node input (one mask timeline, not N `HeadBlur` nodes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionRedaction {
    /// Masks to redact (already merged when “blur everyone”).
    pub masks: MaskTimeline,
    /// Redaction appearance.
    pub style: RedactionStyle,
}

impl RegionRedaction {
    /// Gaussian helper.
    #[must_use]
    pub fn gaussian(masks: MaskTimeline, sigma: f32) -> Self {
        Self {
            masks,
            style: RedactionStyle::Gaussian {
                sigma: sigma.max(0.5),
            },
        }
    }
}
