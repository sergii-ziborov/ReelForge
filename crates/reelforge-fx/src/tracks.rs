//! Temporal region tracks for object-aware effects (`SightLoom`-compatible intermediate).
//!
//! `ReelForge` does **not** depend on `SightLoom`. Vision pipelines export samples in this
//! shape (JSON via `reelforge-io` / plan params); effects only need interpolated centers.

use std::sync::Arc;

/// Cropped coverage (`0..=255`) for silhouette redaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageMask {
    /// Left origin in the source frame.
    pub left: u32,
    /// Top origin in the source frame.
    pub top: u32,
    /// Crop width.
    pub width: u32,
    /// Crop height.
    pub height: u32,
    /// `width * height` coverage bytes.
    pub data: Arc<Vec<u8>>,
}

impl CoverageMask {
    /// Sample coverage at frame pixel `(x, y)` as `0..=1`.
    #[must_use]
    pub fn sample(&self, x: usize, y: usize) -> f32 {
        let ox = x.checked_sub(self.left as usize);
        let oy = y.checked_sub(self.top as usize);
        let (Some(ox), Some(oy)) = (ox, oy) else {
            return 0.0;
        };
        if ox >= self.width as usize || oy >= self.height as usize {
            return 0.0;
        }
        let i = oy * self.width as usize + ox;
        self.data
            .get(i)
            .copied()
            .map_or(0.0, |v| f32::from(v) / 255.0)
    }

    /// Inclusive pixel bounds `(x0, y0, x1, y1)` clipped later by the caller.
    #[must_use]
    pub fn bounds(&self) -> (u32, u32, u32, u32) {
        (
            self.left,
            self.top,
            self.left.saturating_add(self.width),
            self.top.saturating_add(self.height),
        )
    }
}

/// One timed region sample (bbox center + radius).
#[derive(Debug, Clone, PartialEq)]
pub struct RegionSample {
    /// Media time in seconds.
    pub t: f64,
    /// Region center X (pixels).
    pub cx: f32,
    /// Region center Y (pixels).
    pub cy: f32,
    /// Soft elliptical radius in pixels (typically half bbox diagonal).
    pub radius: f32,
    /// Optional confidence `0..=1` (default 1 when omitted at construction).
    pub conf: f32,
    /// Optional silhouette coverage (preferred over the ellipse).
    pub coverage: Option<CoverageMask>,
}

impl RegionSample {
    /// Sample with full confidence.
    #[must_use]
    pub fn new(t: f64, cx: f32, cy: f32, radius: f32) -> Self {
        Self {
            t,
            cx,
            cy,
            radius: radius.max(1.0),
            conf: 1.0,
            coverage: None,
        }
    }

    /// From axis-aligned box `left,top,right,bottom` at time `t`.
    #[must_use]
    pub fn from_bbox(t: f64, left: f32, top: f32, right: f32, bottom: f32, conf: f32) -> Self {
        let w = (right - left).abs();
        let h = (bottom - top).abs();
        let cx = left + w * 0.5;
        let cy = top + h * 0.5;
        let radius = ((w * w + h * h).sqrt() * 0.5).max(1.0);
        Self {
            t,
            cx,
            cy,
            radius,
            conf: conf.clamp(0.0, 1.0),
            coverage: None,
        }
    }

    /// Attach silhouette coverage.
    #[must_use]
    pub fn with_coverage(mut self, coverage: CoverageMask) -> Self {
        self.coverage = Some(coverage);
        self
    }

    /// Half-open rect fields (`x,y,w,h` — `SightLoom` / detector style).
    #[must_use]
    #[allow(clippy::many_single_char_names)]
    pub fn from_xywh(t: f64, x: f32, y: f32, w: f32, h: f32, conf: f32) -> Self {
        Self::from_bbox(t, x, y, x + w, y + h, conf)
    }
}

/// One tracked object over time.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionTrack {
    /// Stable id (e.g. track id or `"face_12"`).
    pub id: String,
    /// Optional kind (`face`, `plate`, `person`, …).
    pub kind: Option<String>,
    /// Samples sorted by `t` (unsorted input is sorted on insert helpers).
    pub samples: Vec<RegionSample>,
}

impl RegionTrack {
    /// New empty track.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: None,
            samples: Vec::new(),
        }
    }

    /// With kind label.
    #[must_use]
    pub fn with_kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    /// Append sample and keep time order.
    pub fn push(&mut self, sample: RegionSample) {
        self.samples.push(sample);
        self.samples
            .sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Interpolated center + radius at media time `t`, or `None` if empty track.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn region_at(&self, t: f64) -> Option<(f32, f32, f32, f32)> {
        if self.samples.is_empty() {
            return None;
        }
        if self.samples.len() == 1 {
            let s = &self.samples[0];
            return Some((s.cx, s.cy, s.radius, s.conf));
        }
        // Before first / after last: clamp to endpoint (hold).
        if t <= self.samples[0].t {
            let s = &self.samples[0];
            return Some((s.cx, s.cy, s.radius, s.conf));
        }
        let last = self.samples.last()?;
        if t >= last.t {
            return Some((last.cx, last.cy, last.radius, last.conf));
        }
        for w in self.samples.windows(2) {
            let a = &w[0];
            let b = &w[1];
            if t >= a.t && t <= b.t {
                let span = (b.t - a.t).max(1e-9);
                let u = ((t - a.t) / span) as f32;
                let cx = a.cx + (b.cx - a.cx) * u;
                let cy = a.cy + (b.cy - a.cy) * u;
                let radius = a.radius + (b.radius - a.radius) * u;
                let conf = a.conf + (b.conf - a.conf) * u;
                return Some((cx, cy, radius.max(1.0), conf));
            }
        }
        None
    }

    /// Hold the last coverage whose sample time is `<= t` (no pixel interpolation).
    #[must_use]
    pub fn coverage_at(&self, t: f64) -> Option<CoverageMask> {
        let mut last = None;
        for s in &self.samples {
            if s.t <= t {
                if s.coverage.is_some() {
                    last.clone_from(&s.coverage);
                }
            } else {
                break;
            }
        }
        last.or_else(|| self.samples.iter().find_map(|s| s.coverage.clone()))
    }

    /// Center path for [`crate::HeadBlur`].
    #[must_use]
    pub fn center_fn(&self) -> Arc<dyn Fn(f64) -> (f32, f32) + Send + Sync> {
        let samples = self.samples.clone();
        Arc::new(move |t| {
            let track = RegionTrack {
                id: String::new(),
                kind: None,
                samples: samples.clone(),
            };
            track
                .region_at(t)
                .map_or((-10_000.0, -10_000.0), |(cx, cy, _, _)| (cx, cy))
        })
    }
}

/// Set of tracks (adapter boundary with vision).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackSet {
    /// Tracks by id.
    pub tracks: Vec<RegionTrack>,
}

impl TrackSet {
    /// Empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a track.
    pub fn push(&mut self, track: RegionTrack) {
        self.tracks.push(track);
    }

    /// Number of tracks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    /// Whether empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// All active regions at time `t` (center, radius, conf).
    #[must_use]
    pub fn regions_at(&self, t: f64) -> Vec<(f32, f32, f32, f32)> {
        self.tracks
            .iter()
            .filter_map(|tr| tr.region_at(t))
            .filter(|(_, _, _, conf)| *conf > 0.0)
            .collect()
    }

    /// Regions plus optional silhouette coverage at `t`.
    #[must_use]
    pub fn regions_with_coverage_at(&self, t: f64) -> Vec<RegionAt> {
        self.tracks
            .iter()
            .filter_map(|tr| {
                let (cx, cy, radius, conf) = tr.region_at(t)?;
                if conf <= 0.0 {
                    return None;
                }
                Some(RegionAt {
                    cx,
                    cy,
                    radius,
                    conf,
                    coverage: tr.coverage_at(t),
                })
            })
            .collect()
    }
}

/// Interpolated region plus optional dense coverage.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionAt {
    /// Center X.
    pub cx: f32,
    /// Center Y.
    pub cy: f32,
    /// Ellipse radius fallback.
    pub radius: f32,
    /// Confidence.
    pub conf: f32,
    /// Silhouette when the adapter provided pixels.
    pub coverage: Option<CoverageMask>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_center() {
        let mut tr = RegionTrack::new("a");
        tr.push(RegionSample::new(0.0, 0.0, 0.0, 10.0));
        tr.push(RegionSample::new(1.0, 100.0, 50.0, 20.0));
        let (cx, cy, r, _) = tr.region_at(0.5).unwrap();
        assert!((cx - 50.0).abs() < 1e-3);
        assert!((cy - 25.0).abs() < 1e-3);
        assert!((r - 15.0).abs() < 1e-3);
    }

    #[test]
    fn from_bbox_center() {
        let s = RegionSample::from_bbox(0.0, 10.0, 20.0, 30.0, 40.0, 0.9);
        assert!((s.cx - 20.0).abs() < 1e-5);
        assert!((s.cy - 30.0).abs() < 1e-5);
        assert!((s.conf - 0.9).abs() < 1e-5);
    }
}
