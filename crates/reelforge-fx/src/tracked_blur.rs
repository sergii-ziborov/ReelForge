//! Multi-region tracked blur (privacy / [`crate::HeadBlur`] stack with one Gaussian pass).

use crate::tracks::TrackSet;
use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Blur every active track region at each frame time.
///
/// One full-frame separable Gaussian, then soft-elliptical composite per region
/// (cheaper than stacking N independent [`crate::HeadBlur`] graphs).
#[derive(Clone)]
pub struct TrackedBlur {
    /// Temporal tracks (`SightLoom` / detector export).
    pub tracks: Arc<TrackSet>,
    /// Multiply per-sample radius (`1.0` = use sample radius).
    pub radius_scale: f32,
    /// Override radius for all regions when set.
    pub fixed_radius: Option<f32>,
    /// Gaussian sigma. `None` → derived from mean radius.
    pub intensity: Option<f32>,
    /// Soft edge as fraction of radius.
    pub feather: f32,
    /// Minimum confidence to blur (`0.0`–`1.0`).
    pub min_conf: f32,
}

impl TrackedBlur {
    /// Default privacy blur for `tracks`.
    #[must_use]
    pub fn new(tracks: TrackSet) -> Self {
        Self {
            tracks: Arc::new(tracks),
            radius_scale: 1.0,
            fixed_radius: None,
            intensity: None,
            feather: 0.35,
            min_conf: 0.05,
        }
    }

    /// Wrap existing arc.
    #[must_use]
    pub fn from_arc(tracks: Arc<TrackSet>) -> Self {
        Self {
            tracks,
            radius_scale: 1.0,
            fixed_radius: None,
            intensity: None,
            feather: 0.35,
            min_conf: 0.05,
        }
    }

    /// Scale sample radii.
    #[must_use]
    pub fn with_radius_scale(mut self, scale: f32) -> Self {
        self.radius_scale = scale.max(0.1);
        self
    }

    /// Force a fixed radius for every region.
    #[must_use]
    pub fn with_fixed_radius(mut self, radius: f32) -> Self {
        self.fixed_radius = Some(radius.max(1.0));
        self
    }

    /// Override blur strength.
    #[must_use]
    pub fn with_intensity(mut self, intensity: f32) -> Self {
        self.intensity = Some(intensity.max(0.5));
        self
    }

    /// Soft edge fraction of radius.
    #[must_use]
    pub fn with_feather(mut self, feather: f32) -> Self {
        self.feather = feather.clamp(0.0, 1.0);
        self
    }
}

impl VideoEffect for TrackedBlur {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(TrackedBlurVideo {
            inner: clip,
            tracks: Arc::clone(&self.tracks),
            radius_scale: self.radius_scale,
            fixed_radius: self.fixed_radius,
            intensity: self.intensity,
            feather: self.feather,
            min_conf: self.min_conf,
        }))
    }
}

struct TrackedBlurVideo {
    inner: Arc<dyn VideoClip>,
    tracks: Arc<TrackSet>,
    radius_scale: f32,
    fixed_radius: Option<f32>,
    intensity: Option<f32>,
    feather: f32,
    min_conf: f32,
}

impl VideoClip for TrackedBlurVideo {
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
        let regions: Vec<(f32, f32, f32)> = self
            .tracks
            .regions_at(t.as_secs())
            .into_iter()
            .filter(|(_, _, _, conf)| *conf >= self.min_conf)
            .map(|(cx, cy, r, _)| {
                let radius = self
                    .fixed_radius
                    .unwrap_or_else(|| (r * self.radius_scale).max(1.0));
                (cx, cy, radius)
            })
            .collect();
        if regions.is_empty() {
            return Ok(frame);
        }
        let mean_r = {
            let sum: f32 = regions.iter().map(|(_, _, r)| r).sum();
            #[allow(clippy::cast_precision_loss)]
            let n = regions.len() as f32;
            (sum / n).max(1.0)
        };
        let intensity = self
            .intensity
            .unwrap_or_else(|| (2.0 * mean_r / 3.0).max(0.5));
        apply_multi_blur(&mut frame, &regions, intensity, self.feather);
        Ok(frame)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names,
    clippy::similar_names
)]
fn apply_multi_blur(frame: &mut Frame, regions: &[(f32, f32, f32)], intensity: f32, feather: f32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let src = frame.data().to_vec();
    let mut blurred = src.clone();
    let kernel = gaussian_kernel(intensity.max(0.5));
    blur_separable(&src, &mut blurred, w, h, bpp, &kernel);
    let out_px = frame.data_mut();

    for y in 0..h {
        for x in 0..w {
            let mut wgt = 0.0_f32;
            for &(cx, cy, radius) in regions {
                let r = radius.max(1.0);
                let feather_px = (feather * r).max(0.5);
                let inner = (r - feather_px).max(0.0);
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                wgt = wgt.max(soft_mask(dist, inner, r));
            }
            if wgt <= 0.0 {
                continue;
            }
            let i = (y * w + x) * bpp;
            for c in 0..bpp.min(3) {
                let a = f32::from(src[i + c]);
                let b = f32::from(blurred[i + c]);
                out_px[i + c] = (a * (1.0 - wgt) + b * wgt).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn soft_mask(dist: f32, inner: f32, outer: f32) -> f32 {
    if dist <= inner {
        1.0
    } else if dist >= outer {
        0.0
    } else {
        let t = ((dist - inner) / (outer - inner).max(1e-6)).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        1.0 - s
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius_px = ((sigma * 3.0).ceil().max(1.0) as usize).min(32);
    let mut k = Vec::with_capacity(radius_px * 2 + 1);
    let s2 = 2.0 * sigma * sigma;
    let mut sum = 0.0_f32;
    for i in 0..=radius_px * 2 {
        let offset = i as i32 - radius_px as i32;
        let v = (-(offset * offset) as f32 / s2).exp();
        k.push(v);
        sum += v;
    }
    for v in &mut k {
        *v /= sum;
    }
    k
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::similar_names
)]
fn blur_separable(
    src: &[u8],
    dst: &mut [u8],
    width: usize,
    height: usize,
    bpp: usize,
    kernel: &[f32],
) {
    let r = kernel.len() / 2;
    let mut tmp = vec![0_u8; src.len()];
    let w_i = width as isize;
    let h_i = height as isize;
    let r_i = r as isize;
    for y in 0..height {
        for x in 0..width {
            let mut acc = [0.0_f32; 4];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sx = (x as isize + ki as isize - r_i).clamp(0, w_i - 1) as usize;
                let i = (y * width + sx) * bpp;
                for c in 0..bpp.min(4) {
                    acc[c] += f32::from(src[i + c]) * kw;
                }
            }
            let di = (y * width + x) * bpp;
            for c in 0..bpp.min(4) {
                tmp[di + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    for y in 0..height {
        for x in 0..width {
            let mut acc = [0.0_f32; 4];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sy = (y as isize + ki as isize - r_i).clamp(0, h_i - 1) as usize;
                let i = (sy * width + x) * bpp;
                for c in 0..bpp.min(4) {
                    acc[c] += f32::from(tmp[i + c]) * kw;
                }
            }
            let di = (y * width + x) * bpp;
            for c in 0..bpp.min(4) {
                dst[di + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracks::{RegionSample, RegionTrack};
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn blurs_tracked_region() {
        let mut set = TrackSet::new();
        let mut tr = RegionTrack::new("face_1").with_kind("face");
        tr.push(RegionSample::new(0.0, 32.0, 32.0, 14.0));
        tr.push(RegionSample::new(1.0, 40.0, 32.0, 14.0));
        set.push(tr);
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(64, 64),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let out = TrackedBlur::new(set).apply(clip).unwrap();
        let f = out.frame_at(Time::from_secs(0.5)).unwrap();
        assert_eq!(f.size(), Size::new(64, 64));
    }
}
