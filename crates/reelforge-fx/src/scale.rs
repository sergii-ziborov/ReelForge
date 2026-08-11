//! Frame scaling kernels (nearest / bilinear).

use reelforge_core::{CoreError, Frame, Result, Size};
use rayon::prelude::*;

/// Sampling kernel used when resizing frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ResizeFilter {
    /// Fast blocky sample (point sampling).
    Nearest,
    /// Linear interpolation in X and Y (default quality path).
    #[default]
    Bilinear,
}

/// Bilinear resize to `new_size` (pixel-center mapping, fixed-point weights).
///
/// # Errors
///
/// Returns size / buffer errors when `new_size` is invalid or allocation fails.
pub fn resize_bilinear(frame: &Frame, new_size: Size) -> Result<Frame> {
    new_size.require_positive()?;
    let src = frame.size();
    if src == new_size {
        return Ok(frame.clone());
    }
    let bpp = frame.format().bytes_per_pixel();
    let data = frame.data();
    let pixels = usize::try_from(new_size.pixel_count())
        .map_err(|_| CoreError::invalid_frame("resize pixel count exceeds usize"))?;
    let mut out = vec![
        0_u8;
        pixels
            .checked_mul(bpp)
            .ok_or_else(|| CoreError::invalid_frame("resize buffer overflow"))?
    ];

    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = new_size.width as usize;
    let dh = new_size.height as usize;
    let row_src = sw * bpp;
    let row_dst = dw * bpp;

    // Precompute horizontal lerps: x0, x1, w0 (w1 = 256 - w0).
    let x_lerps = build_axis_lerps(sw, dw);
    let y_lerps = build_axis_lerps(sh, dh);

    out.par_chunks_mut(row_dst)
        .zip(y_lerps.par_iter())
        .for_each(|(dst_row, y)| {
            let y0 = y.i0;
            let y1 = y.i1;
            let wy0 = u32::from(y.w0);
            let wy1 = 256 - wy0;
            let row0 = y0 * row_src;
            let row1 = y1 * row_src;
            for (dx, x) in x_lerps.iter().enumerate() {
                let x0 = x.i0;
                let x1 = x.i1;
                let wx0 = u32::from(x.w0);
                let wx1 = 256 - wx0;
                let dst_i = dx * bpp;
                for c in 0..bpp {
                    let p00 = u32::from(data[row0 + x0 * bpp + c]);
                    let p01 = u32::from(data[row0 + x1 * bpp + c]);
                    let p10 = u32::from(data[row1 + x0 * bpp + c]);
                    let p11 = u32::from(data[row1 + x1 * bpp + c]);
                    let top = p00 * wx0 + p01 * wx1;
                    let bot = p10 * wx0 + p11 * wx1;
                    // top/bot already scaled by 256; multiply by y weights → / 256^2
                    let v = (top * wy0 + bot * wy1) >> 16;
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        dst_row[dst_i + c] = v.min(255) as u8;
                    }
                }
            }
        });

    Frame::from_raw(new_size, frame.format(), out)
}

/// Axis sample: indices and weight of the left/top sample (`w0` in 0..=256).
#[derive(Clone, Copy)]
struct AxisLerp {
    i0: usize,
    i1: usize,
    /// Weight for `i0` in 0..=256 (`i1` gets `256 - w0`).
    w0: u16,
}

/// Map destination index `d` in `0..dst` onto continuous source in `[0, src-1]`.
///
/// Pixel-center convention: `s = (d + 0.5) * src / dst - 0.5`, clamped.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn build_axis_lerps(src: usize, dst: usize) -> Vec<AxisLerp> {
    debug_assert!(src >= 1 && dst >= 1);
    if src == 1 {
        return (0..dst)
            .map(|_| AxisLerp {
                i0: 0,
                i1: 0,
                w0: 256,
            })
            .collect();
    }
    let src_f = src as f64;
    let dst_f = dst as f64;
    let max_i = src - 1;
    (0..dst)
        .map(|d| {
            let s = (d as f64 + 0.5) * src_f / dst_f - 0.5;
            let s = s.clamp(0.0, max_i as f64);
            let i0 = s.floor() as usize;
            let i1 = (i0 + 1).min(max_i);
            let frac = s - i0 as f64;
            // w0 = weight of i0 = 1 - frac
            let w0 = ((1.0 - frac) * 256.0).round().clamp(0.0, 256.0) as u16;
            AxisLerp { i0, i1, w0 }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{FrameFormat, Rgb8};

    #[test]
    fn bilinear_midpoint_is_gray() {
        // 2×1: black | white → 1×1 should be ~128
        let data = vec![0, 0, 0, 255, 255, 255];
        let frame = Frame::from_raw(Size::new(2, 1), FrameFormat::Rgb8, data).unwrap();
        let out = resize_bilinear(&frame, Size::new(1, 1)).unwrap();
        let v = out.data()[0];
        assert!((120..=135).contains(&v), "got {v}");
        assert_eq!(out.data()[0], out.data()[1]);
        assert_eq!(out.data()[1], out.data()[2]);
    }

    #[test]
    fn bilinear_solid_unchanged() {
        let frame = Frame::solid_rgb(Size::new(8, 6), Rgb8::new(10, 20, 30)).unwrap();
        let out = resize_bilinear(&frame, Size::new(3, 2)).unwrap();
        assert_eq!(&out.data()[0..3], &[10, 20, 30]);
    }

    #[test]
    fn bilinear_upscale_dims() {
        let frame = Frame::solid_rgb(Size::new(2, 2), Rgb8::RED).unwrap();
        let out = resize_bilinear(&frame, Size::new(5, 7)).unwrap();
        assert_eq!(out.size(), Size::new(5, 7));
    }

    #[test]
    fn bilinear_identity() {
        let frame = Frame::solid_rgb(Size::new(4, 4), Rgb8::GREEN).unwrap();
        let out = resize_bilinear(&frame, Size::new(4, 4)).unwrap();
        assert_eq!(out.data(), frame.data());
    }
}
