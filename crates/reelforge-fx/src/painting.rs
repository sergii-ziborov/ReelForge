//! Painting effect: edge-enhance + ink lines (MoviePy-compatible parameters).

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Transform frames into a painted look.
///
/// Control surface: `saturation` boosts flashiness; `black` controls ink lines.
/// Internally uses an edge-enhance kernel and Sobel-style edge map, then
/// `out = saturation * enhanced - black * edges`.
#[derive(Debug, Clone, Copy)]
pub struct Painting {
    /// Color flashiness (`1.4` default).
    pub saturation: f32,
    /// Ink line amount (`0.006` default).
    pub black: f32,
}

impl Painting {
    /// Default painting look (`saturation=1.4`, `black=0.006`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            saturation: 1.4,
            black: 0.006,
        }
    }

    /// Custom saturation and ink strength.
    #[must_use]
    pub const fn with(saturation: f32, black: f32) -> Self {
        Self { saturation, black }
    }

    /// Stronger cartoon lines.
    #[must_use]
    pub const fn inky(self) -> Self {
        Self {
            saturation: self.saturation,
            black: (self.black * 2.5).min(0.05),
        }
    }
}

impl Default for Painting {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoEffect for Painting {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(PaintVideo {
            inner: clip,
            saturation: self.saturation.max(0.0),
            black: self.black.max(0.0),
        }))
    }
}

struct PaintVideo {
    inner: Arc<dyn VideoClip>,
    saturation: f32,
    black: f32,
}

impl VideoClip for PaintVideo {
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
        let frame = self.inner.frame_at(t)?;
        let painted = to_painting(frame.data(), frame.size(), frame.format().bytes_per_pixel(), self.saturation, self.black);
        Frame::from_raw(frame.size(), frame.format(), painted)
    }
}

/// EDGE_ENHANCE_MORE-style kernel (PIL-compatible spirit): center heavy, neighbors negative.
const EE_CENTER: i32 = 10;
const EE_NEIGH: i32 = -1;

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    clippy::similar_names
)]
fn to_painting(data: &[u8], size: Size, bpp: usize, saturation: f32, black: f32) -> Vec<u8> {
    let w = size.width as usize;
    let h = size.height as usize;
    let mut enhanced = vec![0_u8; data.len()];
    let mut gray = vec![0_u8; w * h];

    // Pass 1: edge-enhance RGB + grayscale for edges.
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0_i32; 3];
            for oy in -1_i32..=1 {
                for ox in -1_i32..=1 {
                    let sx = (x as i32 + ox).clamp(0, w as i32 - 1) as usize;
                    let sy = (y as i32 + oy).clamp(0, h as i32 - 1) as usize;
                    let i = (sy * w + sx) * bpp;
                    let weight = if ox == 0 && oy == 0 {
                        EE_CENTER
                    } else {
                        EE_NEIGH
                    };
                    for c in 0..3 {
                        acc[c] += i32::from(data[i + c]) * weight;
                    }
                }
            }
            let di = (y * w + x) * bpp;
            for c in 0..3 {
                enhanced[di + c] = acc[c].clamp(0, 255) as u8;
            }
            if bpp > 3 {
                enhanced[di + 3] = data[di + 3];
            }
            // BT.601 luma of enhanced
            gray[y * w + x] = ((77_u32 * u32::from(enhanced[di])
                + 150 * u32::from(enhanced[di + 1])
                + 29 * u32::from(enhanced[di + 2]))
                >> 8) as u8;
        }
    }

    // Pass 2: Sobel magnitude on gray → ink.
    let mut edges = vec![0_f32; w * h];
    let mut max_e = 1e-6_f32;
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let idx = |yy: usize, xx: usize| -> i32 { i32::from(gray[yy * w + xx]) };
            let gx = -idx(y - 1, x - 1)
                + idx(y - 1, x + 1)
                - 2 * idx(y, x - 1)
                + 2 * idx(y, x + 1)
                - idx(y + 1, x - 1)
                + idx(y + 1, x + 1);
            let gy = -idx(y - 1, x - 1)
                - 2 * idx(y - 1, x)
                - idx(y - 1, x + 1)
                + idx(y + 1, x - 1)
                + 2 * idx(y + 1, x)
                + idx(y + 1, x + 1);
            let mag = ((gx * gx + gy * gy) as f32).sqrt();
            edges[y * w + x] = mag;
            if mag > max_e {
                max_e = mag;
            }
        }
    }

    // Pass 3: saturation * enhanced - black * edges (MoviePy formula).
    let mut out = enhanced;
    for y in 0..h {
        for x in 0..w {
            let e = edges[y * w + x] / max_e; // 0..1
            let dark = black * 255.0 * e;
            let di = (y * w + x) * bpp;
            for c in 0..3 {
                let v = saturation * f32::from(out[di + c]) - dark;
                out[di + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn paints_defaults() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(16, 16),
            Rgb8::new(100, 50, 200),
            Duration::from_secs(0.5),
        ));
        let out = Painting::new().apply(clip).unwrap();
        let f = out.frame_at(Time::ZERO).unwrap();
        assert_eq!(f.size(), Size::new(16, 16));
        // Solid still roughly colorful (not all black)
        assert!(f.data()[0] > 0 || f.data()[2] > 0);
    }

    #[test]
    fn inky_stronger_param() {
        let p = Painting::new().inky();
        assert!(p.black > Painting::new().black);
    }
}
