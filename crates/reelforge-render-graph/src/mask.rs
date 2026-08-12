//! Mask timelines for redaction and vision materialization.

use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};

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

/// Spatial interpolation between mask samples.
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
    /// Center X (pixels).
    pub cx: f32,
    /// Center Y (pixels).
    pub cy: f32,
    /// Radius or half-extent (pixels).
    pub radius: f32,
    /// Optional confidence `0..=1`.
    #[serde(default = "one")]
    pub conf: f32,
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
    /// Ellipse sample.
    #[must_use]
    pub fn ellipse(t: MediaTime, cx: f32, cy: f32, radius: f32) -> Self {
        Self {
            t,
            cx,
            cy,
            radius: radius.max(1.0),
            conf: 1.0,
            left: None,
            top: None,
            right: None,
            bottom: None,
        }
    }

    /// From axis-aligned box.
    #[must_use]
    pub fn from_box(t: MediaTime, left: f32, top: f32, right: f32, bottom: f32, conf: f32) -> Self {
        let w = (right - left).abs();
        let h = (bottom - top).abs();
        Self {
            t,
            cx: left + w * 0.5,
            cy: top + h * 0.5,
            radius: ((w * w + h * h).sqrt() * 0.5).max(1.0),
            conf: conf.clamp(0.0, 1.0),
            left: Some(left),
            top: Some(top),
            right: Some(right),
            bottom: Some(bottom),
        }
    }
}

/// Timed mask for fused multi-subject redaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MaskTimeline {
    /// Ordered samples.
    #[serde(default)]
    pub samples: Vec<MaskSample>,
    /// Interpolation mode.
    #[serde(default)]
    pub interpolation: MaskInterpolation,
    /// Missing sample policy.
    #[serde(default)]
    pub missing_policy: MissingMaskPolicy,
}

impl MaskTimeline {
    /// Empty timeline.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push and keep time order (by seconds).
    pub fn push(&mut self, sample: MaskSample) {
        self.samples.push(sample);
        self.samples.sort_by(|a, b| {
            a.t.as_secs()
                .partial_cmp(&b.t.as_secs())
                .unwrap_or(core::cmp::Ordering::Equal)
        });
    }

    /// Merge many subject timelines into one (union of samples for fused redaction).
    #[must_use]
    pub fn merge(timelines: impl IntoIterator<Item = Self>) -> Self {
        let mut out = Self::new();
        for tl in timelines {
            out.interpolation = tl.interpolation;
            out.missing_policy = tl.missing_policy;
            for s in tl.samples {
                out.push(s);
            }
        }
        out
    }

    /// Sample region list active near time `t` (all samples with matching tick key or hold).
    ///
    /// For linear mode returns interpolated center/radius when two neighbors exist.
    #[must_use]
    pub fn regions_at(&self, t: MediaTime) -> Vec<(f32, f32, f32, f32)> {
        if self.samples.is_empty() {
            return match self.missing_policy {
                MissingMaskPolicy::Opaque => vec![(0.0, 0.0, 1.0e6, 1.0)],
                MissingMaskPolicy::Transparent | MissingMaskPolicy::HoldLast => Vec::new(),
            };
        }
        // Group: for multi-track merged timelines, return all samples closest bucket.
        // Simple approach: interpolate global envelope for single chain; for multi,
        // return every sample within half-frame of t, else hold each track-less sample.
        let ts = t.as_secs();
        let mut regions = Vec::new();
        // If samples look like multi-subject (same-ish times), emit all near t.
        let window = 1.0 / 30.0;
        for s in &self.samples {
            if (s.t.as_secs() - ts).abs() <= window {
                regions.push((s.cx, s.cy, s.radius, s.conf));
            }
        }
        if !regions.is_empty() {
            return regions;
        }
        // Fall back to single-path interpolation of sorted unique times.
        match self.interpolation {
            MaskInterpolation::Hold => {
                let mut last = None;
                for s in &self.samples {
                    if s.t.as_secs() <= ts {
                        last = Some(s);
                    } else {
                        break;
                    }
                }
                match last {
                    Some(s) => vec![(s.cx, s.cy, s.radius, s.conf)],
                    None => match self.missing_policy {
                        MissingMaskPolicy::Opaque => vec![(0.0, 0.0, 1.0e6, 1.0)],
                        _ => Vec::new(),
                    },
                }
            }
            MaskInterpolation::Linear => {
                if self.samples.len() == 1 {
                    let s = &self.samples[0];
                    return vec![(s.cx, s.cy, s.radius, s.conf)];
                }
                for w in self.samples.windows(2) {
                    let a = &w[0];
                    let b = &w[1];
                    let ta = a.t.as_secs();
                    let tb = b.t.as_secs();
                    if ts >= ta && ts <= tb {
                        #[allow(clippy::cast_possible_truncation)]
                        let u = ((ts - ta) / (tb - ta).max(1e-12)) as f32;
                        return vec![(
                            a.cx + (b.cx - a.cx) * u,
                            a.cy + (b.cy - a.cy) * u,
                            a.radius + (b.radius - a.radius) * u,
                            a.conf + (b.conf - a.conf) * u,
                        )];
                    }
                }
                let s = if ts < self.samples[0].t.as_secs() {
                    &self.samples[0]
                } else {
                    self.samples.last().unwrap()
                };
                vec![(s.cx, s.cy, s.radius, s.conf)]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::MediaTime;

    #[test]
    fn merge_and_near() {
        let mut a = MaskTimeline::new();
        a.push(MaskSample::ellipse(
            MediaTime::new(0, 30).unwrap(),
            10.0,
            10.0,
            5.0,
        ));
        let mut b = MaskTimeline::new();
        b.push(MaskSample::ellipse(
            MediaTime::new(0, 30).unwrap(),
            50.0,
            50.0,
            8.0,
        ));
        let m = MaskTimeline::merge([a, b]);
        let r = m.regions_at(MediaTime::new(0, 30).unwrap());
        assert_eq!(r.len(), 2);
    }
}
