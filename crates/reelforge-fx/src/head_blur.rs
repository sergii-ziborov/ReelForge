//! Regional gaussian blur with soft elliptical mask (tracking-friendly head blur).

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Blur a circular/elliptical region whose center can move over time.
///
/// Quality path: full-frame separable Gaussian, then soft-masked composite
/// (better than a hard box-blur patch). Default `intensity` follows
/// `2 * radius / 3` when left as `None` at construction via [`HeadBlur::auto`].
#[derive(Clone)]
pub struct HeadBlur {
    /// Blur region radius in pixels.
    pub radius: f32,
    /// Gaussian blur strength (sigma). `None` → `2 * radius / 3`.
    pub intensity: Option<f32>,
    /// Soft edge width as a fraction of `radius` (`0.0` = hard, `0.35` default).
    pub feather: f32,
    /// Center path: media time (seconds) → `(x, y)` in pixels.
    pub center: Arc<dyn Fn(f64) -> (f32, f32) + Send + Sync>,
}

impl HeadBlur {
    /// Static center; intensity auto (`2r/3`), feather `0.35`.
    #[must_use]
    pub fn fixed(cx: f32, cy: f32, radius: f32) -> Self {
        Self::auto(radius, move |_| (cx, cy))
    }

    /// Static center with explicit intensity.
    #[must_use]
    pub fn fixed_intensity(cx: f32, cy: f32, radius: f32, intensity: f32) -> Self {
        Self {
            radius: radius.max(1.0),
            intensity: Some(intensity.max(0.5)),
            feather: 0.35,
            center: Arc::new(move |_| (cx, cy)),
        }
    }

    /// Moving center with auto intensity.
    #[must_use]
    pub fn auto<F>(radius: f32, center: F) -> Self
    where
        F: Fn(f64) -> (f32, f32) + Send + Sync + 'static,
    {
        Self {
            radius: radius.max(1.0),
            intensity: None,
            feather: 0.35,
            center: Arc::new(center),
        }
    }

    /// Moving center, full control.
    #[must_use]
    pub fn moving<F>(radius: f32, intensity: f32, feather: f32, center: F) -> Self
    where
        F: Fn(f64) -> (f32, f32) + Send + Sync + 'static,
    {
        Self {
            radius: radius.max(1.0),
            intensity: Some(intensity.max(0.5)),
            feather: feather.clamp(0.0, 1.0),
            center: Arc::new(center),
        }
    }

    /// Override feather (0–1 of radius).
    #[must_use]
    pub fn with_feather(mut self, feather: f32) -> Self {
        self.feather = feather.clamp(0.0, 1.0);
        self
    }
}

impl VideoEffect for HeadBlur {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        let intensity = self
            .intensity
            .unwrap_or_else(|| (2.0 * self.radius / 3.0).max(0.5));
        Ok(Arc::new(HeadBlurVideo {
            inner: clip,
            radius: self.radius,
            intensity,
            feather: self.feather,
            center: Arc::clone(&self.center),
        }))
    }
}

struct HeadBlurVideo {
    inner: Arc<dyn VideoClip>,
    radius: f32,
    intensity: f32,
    feather: f32,
    center: Arc<dyn Fn(f64) -> (f32, f32) + Send + Sync>,
}

impl VideoClip for HeadBlurVideo {
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
        let (cx, cy) = (self.center)(t.as_secs());
        apply_head_blur(&mut frame, cx, cy, self.radius, self.intensity, self.feather);
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
fn apply_head_blur(frame: &mut Frame, cx: f32, cy: f32, radius: f32, intensity: f32, feather: f32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let src = frame.data().to_vec();
    let mut blurred = src.clone();

    // Separable gaussian on full frame (intensity as sigma).
    let sigma = intensity.max(0.5);
    let kernel = gaussian_kernel(sigma);
    blur_separable(&src, &mut blurred, w, h, bpp, &kernel);

    let r = radius.max(1.0);
    let feather_px = (feather * r).max(0.5);
    let inner = (r - feather_px).max(0.0);
    let out_px = frame.data_mut();

    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let wgt = soft_mask(dist, inner, r);
            if wgt <= 0.0 {
                continue;
            }
            let i = (y * w + x) * bpp;
            for c in 0..bpp.min(3) {
                let a = f32::from(src[i + c]);
                let b = f32::from(blurred[i + c]);
                out_px[i + c] = (a * (1.0 - wgt) + b * wgt).round().clamp(0.0, 255.0) as u8;
            }
            // alpha untouched if present
        }
    }
}

fn soft_mask(dist: f32, inner: f32, outer: f32) -> f32 {
    if dist <= inner {
        1.0
    } else if dist >= outer {
        0.0
    } else {
        // smoothstep falloff
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
fn blur_separable(src: &[u8], dst: &mut [u8], width: usize, height: usize, bpp: usize, kernel: &[f32]) {
    let r = kernel.len() / 2;
    let mut tmp = vec![0_u8; src.len()];
    let w_i = width as isize;
    let h_i = height as isize;
    let r_i = r as isize;
    // horizontal
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
    // vertical
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
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn applies_gaussian() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(64, 64),
            Rgb8::WHITE,
            Duration::from_secs(0.5),
        ));
        let out = HeadBlur::fixed(32.0, 32.0, 12.0).apply(clip).unwrap();
        assert_eq!(out.frame_at(Time::ZERO).unwrap().size(), Size::new(64, 64));
    }

    #[test]
    fn soft_mask_falloff() {
        assert!((soft_mask(0.0, 5.0, 10.0) - 1.0).abs() < 1e-5);
        assert!(soft_mask(10.0, 5.0, 10.0) <= 1e-5);
        let mid = soft_mask(7.5, 5.0, 10.0);
        assert!(mid > 0.0 && mid < 1.0);
    }
}
