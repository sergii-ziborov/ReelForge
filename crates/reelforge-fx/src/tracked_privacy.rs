//! Multi-region privacy redaction: gaussian blur, pixelate, or solid fill.

use crate::privacy_roi::{apply_fused_blur, stamp_coverage, union_roi};
use crate::tracks::{RegionAt, TrackSet};
use reelforge_core::{Duration, Frame, Result, Rgba8, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// How to obscure tracked regions.
#[derive(Debug, Clone, PartialEq)]
pub enum PrivacyStyle {
    /// Separable gaussian (sigma in pixels).
    Gaussian {
        /// Blur strength.
        sigma: f32,
    },
    /// Blocky pixelation inside each ROI.
    Pixelate {
        /// Block size in pixels.
        block_size: u16,
    },
    /// Solid fill with soft edge.
    Solid {
        /// Fill color.
        color: Rgba8,
    },
}

impl Default for PrivacyStyle {
    fn default() -> Self {
        Self::Gaussian { sigma: 12.0 }
    }
}

/// Privacy redaction driven by a [`TrackSet`] (fused multi-ROI pass).
#[derive(Clone)]
pub struct TrackedPrivacy {
    /// Temporal tracks.
    pub tracks: Arc<TrackSet>,
    /// Redaction appearance.
    pub style: PrivacyStyle,
    /// Soft edge as fraction of radius.
    pub feather: f32,
    /// Minimum confidence to redact.
    pub min_conf: f32,
}

impl TrackedPrivacy {
    /// Construct with tracks + style.
    #[must_use]
    pub fn new(tracks: TrackSet, style: PrivacyStyle) -> Self {
        Self {
            tracks: Arc::new(tracks),
            style,
            feather: 0.35,
            min_conf: 0.05,
        }
    }

    /// Gaussian helper.
    #[must_use]
    pub fn gaussian(tracks: TrackSet, sigma: f32) -> Self {
        Self::new(
            tracks,
            PrivacyStyle::Gaussian {
                sigma: sigma.max(0.5),
            },
        )
    }

    /// Pixelate helper.
    #[must_use]
    pub fn pixelate(tracks: TrackSet, block_size: u16) -> Self {
        Self::new(
            tracks,
            PrivacyStyle::Pixelate {
                block_size: block_size.max(2),
            },
        )
    }

    /// Solid fill helper.
    #[must_use]
    pub fn solid(tracks: TrackSet, color: Rgba8) -> Self {
        Self::new(tracks, PrivacyStyle::Solid { color })
    }

    /// Soft edge fraction of radius.
    #[must_use]
    pub fn with_feather(mut self, feather: f32) -> Self {
        self.feather = feather.clamp(0.0, 1.0);
        self
    }
}

impl VideoEffect for TrackedPrivacy {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(TrackedPrivacyVideo {
            inner: clip,
            tracks: Arc::clone(&self.tracks),
            style: self.style.clone(),
            feather: self.feather,
            min_conf: self.min_conf,
        }))
    }
}

struct TrackedPrivacyVideo {
    inner: Arc<dyn VideoClip>,
    tracks: Arc<TrackSet>,
    style: PrivacyStyle,
    feather: f32,
    min_conf: f32,
}

impl VideoClip for TrackedPrivacyVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let mut frame = self.inner.frame_at(t)?;
        let regions: Vec<RegionAt> = self
            .tracks
            .regions_with_coverage_at(t.as_secs())
            .into_iter()
            .filter(|r| r.conf >= self.min_conf)
            .map(|mut r| {
                r.radius = r.radius.max(1.0);
                r
            })
            .collect();
        if regions.is_empty() {
            return Ok(frame);
        }
        match &self.style {
            PrivacyStyle::Gaussian { sigma } => {
                apply_fused_blur(&mut frame, &regions, sigma.max(0.5), self.feather);
            }
            PrivacyStyle::Pixelate { block_size } => {
                apply_multi_pixelate(&mut frame, &regions, (*block_size).max(2), self.feather);
            }
            PrivacyStyle::Solid { color } => {
                apply_multi_solid(&mut frame, &regions, *color, self.feather);
            }
        }
        Ok(frame)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]
fn apply_multi_pixelate(frame: &mut Frame, regions: &[RegionAt], block: u16, feather: f32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let Some(roi) = union_roi(regions, w, h, 1) else {
        return;
    };
    let rw = roi.x1 - roi.x0;
    let rh = roi.y1 - roi.y0;
    let mut cov = vec![0.0_f32; rw * rh];
    stamp_coverage(&mut cov, roi, regions, feather);
    let block = usize::from(block).max(2);
    let src = frame.data().to_vec();
    let out_px = frame.data_mut();

    for y in roi.y0..roi.y1 {
        for x in roi.x0..roi.x1 {
            let wgt = cov[(y - roi.y0) * rw + (x - roi.x0)];
            if wgt <= 0.0 {
                continue;
            }
            let bx = (x / block) * block;
            let by = (y / block) * block;
            let mut acc = [0.0_f32; 3];
            let mut n = 0.0_f32;
            let x1 = (bx + block).min(w);
            let y1 = (by + block).min(h);
            for py in by..y1 {
                for px in bx..x1 {
                    let si = (py * w + px) * bpp;
                    for c in 0..3.min(bpp) {
                        acc[c] += f32::from(src[si + c]);
                    }
                    n += 1.0;
                }
            }
            if n <= 0.0 {
                continue;
            }
            let i = (y * w + x) * bpp;
            for c in 0..3.min(bpp) {
                let a = f32::from(src[i + c]);
                let b = acc[c] / n;
                out_px[i + c] = (a * (1.0 - wgt) + b * wgt).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]
fn apply_multi_solid(frame: &mut Frame, regions: &[RegionAt], color: Rgba8, feather: f32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let Some(roi) = union_roi(regions, w, h, 1) else {
        return;
    };
    let rw = roi.x1 - roi.x0;
    let rh = roi.y1 - roi.y0;
    let mut cov = vec![0.0_f32; rw * rh];
    stamp_coverage(&mut cov, roi, regions, feather);
    let src = frame.data().to_vec();
    let out_px = frame.data_mut();
    let fill = [color.r, color.g, color.b];
    #[allow(clippy::cast_lossless)]
    let alpha = f32::from(color.a) / 255.0;
    let _ = rh;

    for y in roi.y0..roi.y1 {
        for x in roi.x0..roi.x1 {
            let wgt = cov[(y - roi.y0) * rw + (x - roi.x0)] * alpha;
            if wgt <= 0.0 {
                continue;
            }
            let i = (y * w + x) * bpp;
            for c in 0..3.min(bpp) {
                let a = f32::from(src[i + c]);
                let b = f32::from(fill[c]);
                out_px[i + c] = (a * (1.0 - wgt) + b * wgt).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::{RegionSample, RegionTrack};
    use reelforge_core::{ColorClip, Rgb8};

    fn face_tracks() -> TrackSet {
        let mut set = TrackSet::new();
        let mut tr = RegionTrack::new("face");
        tr.push(RegionSample::new(0.0, 32.0, 32.0, 14.0));
        set.push(tr);
        set
    }

    #[test]
    fn pixelate_and_solid_apply() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(64, 64),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let pix = TrackedPrivacy::pixelate(face_tracks(), 8)
            .apply(Arc::clone(&clip))
            .unwrap();
        let _ = pix.frame_at(Time::ZERO).unwrap();
        let sol = TrackedPrivacy::solid(face_tracks(), Rgba8::new(0, 0, 0, 255))
            .apply(clip)
            .unwrap();
        let f = sol.frame_at(Time::ZERO).unwrap();
        // Center of solid black region should not stay pure white.
        let i = (32 * 64 + 32) * 3;
        assert!(f.data()[i] < 250);
    }

    #[test]
    fn dense_mask_redacts_silhouette_not_ellipse() {
        use crate::tracks::CoverageMask;
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let mut data = vec![0_u8; 32 * 32];
        for y in 8..24 {
            data[y * 32 + 8] = 255;
        }
        let mut tr = RegionTrack::new("bar");
        tr.push(RegionSample::from_bbox(0.0, 8.0, 8.0, 9.0, 24.0, 1.0).with_coverage(
            CoverageMask {
                left: 0,
                top: 0,
                width: 32,
                height: 32,
                data: Arc::new(data),
            },
        ));
        let mut set = TrackSet::new();
        set.push(tr);
        let out = TrackedPrivacy::solid(set, Rgba8::new(0, 0, 0, 255))
            .apply(clip)
            .unwrap()
            .frame_at(Time::ZERO)
            .unwrap();
        let bar = (16 * 32 + 8) * 3;
        let far = (16 * 32 + 24) * 3;
        assert!(out.data()[bar] < 250, "dense column must be filled");
        assert_eq!(out.data()[far], 255, "outside silhouette stays white");
    }
}
