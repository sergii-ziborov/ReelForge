//! Circular soft blur around a moving center (MoviePy-style head blur).

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Blur a circular region whose center moves as a function of time.
///
/// `center` maps media time (seconds) → `(fx, fy)` pixel coordinates.
#[derive(Clone)]
pub struct HeadBlur {
    /// Circle radius in pixels.
    pub radius: u32,
    /// Blur strength (box radius, 1–8 typical).
    pub intensity: u32,
    /// Center path: `t_secs → (x, y)`.
    pub center: Arc<dyn Fn(f64) -> (f32, f32) + Send + Sync>,
}

impl HeadBlur {
    /// Static center blur.
    #[must_use]
    pub fn fixed(cx: f32, cy: f32, radius: u32, intensity: u32) -> Self {
        Self {
            radius,
            intensity: intensity.max(1),
            center: Arc::new(move |_| (cx, cy)),
        }
    }

    /// Moving center from a callable.
    #[must_use]
    pub fn moving<F>(radius: u32, intensity: u32, center: F) -> Self
    where
        F: Fn(f64) -> (f32, f32) + Send + Sync + 'static,
    {
        Self {
            radius,
            intensity: intensity.max(1),
            center: Arc::new(center),
        }
    }
}

impl VideoEffect for HeadBlur {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(HeadBlurVideo {
            inner: clip,
            radius: self.radius,
            intensity: self.intensity,
            center: Arc::clone(&self.center),
        }))
    }
}

struct HeadBlurVideo {
    inner: Arc<dyn VideoClip>,
    radius: u32,
    intensity: u32,
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
        blur_circle(&mut frame, cx, cy, self.radius, self.intensity);
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
fn blur_circle(frame: &mut Frame, cx: f32, cy: f32, radius: u32, intensity: u32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as i32;
    let h = size.height as i32;
    let r2 = (radius as f32) * (radius as f32);
    let k = intensity as i32;
    let src = frame.data().to_vec();
    let dst = frame.data_mut();
    let sw = w as usize;

    let x0 = (cx - radius as f32).floor().max(0.0) as i32;
    let y0 = (cy - radius as f32).floor().max(0.0) as i32;
    let x1 = (cx + radius as f32).ceil().min((size.width - 1) as f32) as i32;
    let y1 = (cy + radius as f32).ceil().min((size.height - 1) as f32) as i32;

    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy > r2 {
                continue;
            }
            let mut acc = [0_u32; 4];
            let mut n = 0_u32;
            for oy in -k..=k {
                for ox in -k..=k {
                    let sx = (x + ox).clamp(0, w - 1) as usize;
                    let sy = (y + oy).clamp(0, h - 1) as usize;
                    let i = (sy * sw + sx) * bpp;
                    for c in 0..bpp.min(4) {
                        acc[c] += u32::from(src[i + c]);
                    }
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            let di = (y as usize * sw + x as usize) * bpp;
            for c in 0..bpp.min(4) {
                dst[di + c] = (acc[c] / n) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn applies() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(0.5),
        ));
        let out = HeadBlur::fixed(16.0, 16.0, 6, 2).apply(clip).unwrap();
        assert_eq!(out.frame_at(Time::ZERO).unwrap().size(), Size::new(32, 32));
    }
}
