//! Generic region redaction (privacy) driven by a mask view of tracks.

use crate::mask::MaskTimeline;
use crate::track::{TrackTimeline, mask_timeline_from_tracks};
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

/// Fused multi-ROI redaction node input (one mask view, not N `HeadBlur` nodes).
///
/// Author [`TrackTimeline`]s, then [`Self::from_tracks`]. `masks` is the
/// serialized ROI view so existing graphs stay valid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionRedaction {
    /// Materialized ROI view (merge of tracks when “blur everyone”).
    pub masks: MaskTimeline,
    /// Redaction appearance.
    pub style: RedactionStyle,
}

impl RegionRedaction {
    /// Gaussian helper from an already-materialized view.
    #[must_use]
    pub fn gaussian(masks: MaskTimeline, sigma: f32) -> Self {
        Self {
            masks,
            style: RedactionStyle::Gaussian {
                sigma: sigma.max(0.5),
            },
        }
    }

    /// One track → fused redaction.
    #[must_use]
    pub fn from_track(track: &TrackTimeline, style: RedactionStyle) -> Self {
        Self {
            masks: track.to_mask_timeline(),
            style,
        }
    }

    /// Many tracks → one fused redaction node.
    #[must_use]
    pub fn from_tracks<'a>(
        tracks: impl IntoIterator<Item = &'a TrackTimeline>,
        style: RedactionStyle,
    ) -> Self {
        Self {
            masks: mask_timeline_from_tracks(tracks),
            style,
        }
    }

    /// Gaussian helper from tracks.
    #[must_use]
    pub fn gaussian_tracks<'a>(
        tracks: impl IntoIterator<Item = &'a TrackTimeline>,
        sigma: f32,
    ) -> Self {
        Self::from_tracks(
            tracks,
            RedactionStyle::Gaussian {
                sigma: sigma.max(0.5),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{SubjectId, TrackId};
    use crate::track::TrackSample;
    use reelforge_core::MediaTime;

    #[test]
    fn from_tracks_fuses_subjects() {
        let t0 = MediaTime::new(0, 30).unwrap();
        let mut a = TrackTimeline::new(TrackId::new("a")).with_subject(SubjectId::new("sa"));
        a.push(TrackSample::ellipse(TrackId::new("a"), t0, 1.0, 1.0, 4.0));
        let mut b = TrackTimeline::new(TrackId::new("b")).with_subject(SubjectId::new("sb"));
        b.push(TrackSample::ellipse(TrackId::new("b"), t0, 9.0, 9.0, 4.0));
        let r = RegionRedaction::gaussian_tracks([&a, &b], 8.0);
        assert_eq!(r.masks.subjects().len(), 2);
        assert!(matches!(r.style, RedactionStyle::Gaussian { .. }));
    }
}
