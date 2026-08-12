//! Multi-region privacy redaction: gaussian blur, pixelate, or solid fill.

use crate::tracks::TrackSet;
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
        let regions: Vec<(f32, f32, f32)> = self
            .tracks
            .regions_at(t.as_secs())
            .into_iter()
            .filter(|(_, _, _, conf)| *conf >= self.min_conf)
            .map(|(cx, cy, r, _)| (cx, cy, r.max(1.0)))
            .collect();
        if regions.is_empty() {
            return Ok(frame);
        }
        match &self.style {
            PrivacyStyle::Gaussian { sigma } => {
                apply_multi_blur(&mut frame, &regions, sigma.max(0.5), self.feather);
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
            let wgt = region_weight(x, y, regions, feather);
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

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]
fn apply_multi_pixelate(frame: &mut Frame, regions: &[(f32, f32, f32)], block: u16, feather: f32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let block = usize::from(block).max(2);
    let src = frame.data().to_vec();
    let out_px = frame.data_mut();

    for y in 0..h {
        for x in 0..w {
            let wgt = region_weight(x, y, regions, feather);
            if wgt <= 0.0 {
                continue;
            }
            let bx = (x / block) * block;
            let by = (y / block) * block;
            // Average block for a stable pixelate look.
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
fn apply_multi_solid(frame: &mut Frame, regions: &[(f32, f32, f32)], color: Rgba8, feather: f32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let src = frame.data().to_vec();
    let out_px = frame.data_mut();
    let fill = [color.r, color.g, color.b];
    #[allow(clippy::cast_lossless)]
    let alpha = f32::from(color.a) / 255.0;

    for y in 0..h {
        for x in 0..w {
            let wgt = region_weight(x, y, regions, feather) * alpha;
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

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn region_weight(x: usize, y: usize, regions: &[(f32, f32, f32)], feather: f32) -> f32 {
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
    wgt
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
}
