//! Packed-pixel helpers used by video effects (parallel row paths where useful).

use rayon::prelude::*;
use reelforge_core::{CoreError, Frame, FrameFormat, Result, Rgb8, Size};

/// Crop a rectangular region (`x`, `y`, `width`, `height`) from `frame`.
///
/// # Errors
///
/// Returns [`CoreError::InvalidFrame`] when the crop is empty or out of bounds.
pub fn crop_frame(frame: &Frame, x: u32, y: u32, width: u32, height: u32) -> Result<Frame> {
    if width == 0 || height == 0 {
        return Err(CoreError::invalid_frame("crop size must be positive"));
    }
    let src = frame.size();
    if x.saturating_add(width) > src.width || y.saturating_add(height) > src.height {
        return Err(CoreError::invalid_frame(format!(
            "crop ({x},{y},{width}x{height}) exceeds source {src:?}"
        )));
    }
    let bpp = frame.format().bytes_per_pixel();
    let row_src = src.width as usize * bpp;
    let row_dst = width as usize * bpp;
    let data = frame.data();
    let x_off = x as usize * bpp;
    let y0 = y as usize;
    let h = height as usize;
    let mut out = vec![0_u8; h * row_dst];
    out.par_chunks_mut(row_dst)
        .enumerate()
        .for_each(|(row, dst)| {
            let start = (y0 + row) * row_src + x_off;
            dst.copy_from_slice(&data[start..start + row_dst]);
        });
    Frame::from_raw(Size::new(width, height), frame.format(), out)
}

/// Nearest-neighbor resize to `new_size`.
///
/// # Errors
///
/// Returns size errors when `new_size` is not positive.
pub fn resize_nearest(frame: &Frame, new_size: Size) -> Result<Frame> {
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

    // Precompute column source indices (shared across rows).
    let x_map: Vec<usize> = (0..dw).map(|dx| (dx * sw) / dw).collect();
    let y_map: Vec<usize> = (0..dh).map(|dy| (dy * sh) / dh).collect();

    match bpp {
        3 => resize_rows_rgb(data, &mut out, &x_map, &y_map, row_src, row_dst),
        4 => resize_rows_rgba(data, &mut out, &x_map, &y_map, row_src, row_dst),
        _ => {
            out.par_chunks_mut(row_dst)
                .zip(y_map.par_iter())
                .for_each(|(dst_row, &sy)| {
                    let src_base = sy * row_src;
                    for (dx, &sx) in x_map.iter().enumerate() {
                        let src_i = src_base + sx * bpp;
                        let dst_i = dx * bpp;
                        dst_row[dst_i..dst_i + bpp].copy_from_slice(&data[src_i..src_i + bpp]);
                    }
                });
        }
    }

    Frame::from_raw(new_size, frame.format(), out)
}

fn resize_rows_rgb(
    data: &[u8],
    out: &mut [u8],
    x_map: &[usize],
    y_map: &[usize],
    row_src: usize,
    row_dst: usize,
) {
    out.par_chunks_mut(row_dst)
        .zip(y_map.par_iter())
        .for_each(|(dst_row, &sy)| {
            let src_base = sy * row_src;
            for (dx, &sx) in x_map.iter().enumerate() {
                let src_i = src_base + sx * 3;
                let dst_i = dx * 3;
                dst_row[dst_i] = data[src_i];
                dst_row[dst_i + 1] = data[src_i + 1];
                dst_row[dst_i + 2] = data[src_i + 2];
            }
        });
}

fn resize_rows_rgba(
    data: &[u8],
    out: &mut [u8],
    x_map: &[usize],
    y_map: &[usize],
    row_src: usize,
    row_dst: usize,
) {
    out.par_chunks_mut(row_dst)
        .zip(y_map.par_iter())
        .for_each(|(dst_row, &sy)| {
            let src_base = sy * row_src;
            for (dx, &sx) in x_map.iter().enumerate() {
                let src_i = src_base + sx * 4;
                let dst_i = dx * 4;
                dst_row[dst_i] = data[src_i];
                dst_row[dst_i + 1] = data[src_i + 1];
                dst_row[dst_i + 2] = data[src_i + 2];
                dst_row[dst_i + 3] = data[src_i + 3];
            }
        });
}

/// Flip horizontally.
///
/// # Errors
///
/// Propagates frame construction errors (should not fail for valid input).
pub fn mirror_x(frame: &Frame) -> Result<Frame> {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let data = frame.data();
    let mut out = vec![0_u8; data.len()];
    let row = w * bpp;
    out.par_chunks_mut(row)
        .zip(data.par_chunks(row))
        .for_each(|(dst, src)| {
            for x in 0..w {
                let sx = w - 1 - x;
                let src_i = sx * bpp;
                let dst_i = x * bpp;
                dst[dst_i..dst_i + bpp].copy_from_slice(&src[src_i..src_i + bpp]);
            }
        });
    Frame::from_raw(size, frame.format(), out)
}

/// Flip vertically.
///
/// # Errors
///
/// Propagates frame construction errors (should not fail for valid input).
pub fn mirror_y(frame: &Frame) -> Result<Frame> {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let data = frame.data();
    let mut out = vec![0_u8; data.len()];
    let row = w * bpp;
    out.par_chunks_mut(row).enumerate().for_each(|(y, dst)| {
        let sy = h - 1 - y;
        let src = sy * row;
        dst.copy_from_slice(&data[src..src + row]);
    });
    Frame::from_raw(size, frame.format(), out)
}

/// Rotate 90° clockwise (width/height swap).
///
/// # Errors
///
/// Returns an error when dimensions do not fit `u32` or buffer construction fails.
pub fn rotate_90_cw(frame: &Frame) -> Result<Frame> {
    let src = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let sw = src.width as usize;
    let sh = src.height as usize;
    let dw = sh;
    let dh = sw;
    let data = frame.data();
    let mut out = vec![0_u8; dw * dh * bpp];
    let row_src = sw * bpp;
    let row_dst = dw * bpp;
    out.par_chunks_mut(row_dst)
        .enumerate()
        .for_each(|(ny, dst_row)| {
            // ny is destination y = source x
            let x = ny;
            for y in 0..sh {
                // (x,y) -> (sh-1-y, x)
                let nx = sh - 1 - y;
                let src_i = y * row_src + x * bpp;
                let dst_i = nx * bpp;
                dst_row[dst_i..dst_i + bpp].copy_from_slice(&data[src_i..src_i + bpp]);
            }
        });
    let width = u32::try_from(dw).map_err(|_| CoreError::invalid_frame("rotate width"))?;
    let height = u32::try_from(dh).map_err(|_| CoreError::invalid_frame("rotate height"))?;
    Frame::from_raw(Size::new(width, height), frame.format(), out)
}

/// Rotate 180° (pixel reverse in place on a copy).
///
/// # Errors
///
/// Propagates frame construction errors.
pub fn rotate_180(frame: &Frame) -> Result<Frame> {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let data = frame.data();
    let n = data.len() / bpp;
    let mut out = vec![0_u8; data.len()];
    out.par_chunks_mut(bpp).enumerate().for_each(|(i, dst)| {
        let src_i = (n - 1 - i) * bpp;
        dst.copy_from_slice(&data[src_i..src_i + bpp]);
    });
    Frame::from_raw(size, frame.format(), out)
}

/// Rotate 270° clockwise (90° counter-clockwise).
///
/// # Errors
///
/// Propagates nested rotate errors.
pub fn rotate_270_cw(frame: &Frame) -> Result<Frame> {
    // 270 CW = 90 CCW = three 90 CW, but two is enough via 90 then 180.
    rotate_90_cw(&rotate_180(frame)?)
}

/// Arbitrary-angle rotate (degrees, clockwise positive) with nearest sampling.
///
/// Canvas size is unchanged; pixels outside the rotated image are filled black.
///
/// # Errors
///
/// Propagates frame construction errors.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn rotate_degrees(frame: &Frame, degrees: f32) -> Result<Frame> {
    // Keep canvas size for free-angle path. Only 0°/180° are size-preserving orthos.
    let mut d = degrees % 360.0;
    if d < 0.0 {
        d += 360.0;
    }
    if (d - 0.0).abs() < 1e-3 || (d - 360.0).abs() < 1e-3 {
        return Ok(frame.clone());
    }
    if (d - 180.0).abs() < 1e-3 {
        return rotate_180(frame);
    }

    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let w = size.width as usize;
    let h = size.height as usize;
    let data = frame.data();
    let mut out = vec![0_u8; data.len()];

    let rad = (-degrees).to_radians(); // inverse map destination -> source
    let (sin_t, cos_t) = rad.sin_cos();
    let cx = (w as f32 - 1.0) * 0.5;
    let cy = (h as f32 - 1.0) * 0.5;
    let row = w * bpp;

    out.par_chunks_mut(row)
        .enumerate()
        .for_each(|(y, dst_row)| {
            let dy = y as f32 - cy;
            for x in 0..w {
                let dx = x as f32 - cx;
                let sx = cos_t * dx - sin_t * dy + cx;
                let sy = sin_t * dx + cos_t * dy + cy;
                if sx < 0.0 || sy < 0.0 {
                    continue;
                }
                let sx = sx.round() as isize;
                let sy = sy.round() as isize;
                if sx < 0 || sy < 0 || sx as usize >= w || sy as usize >= h {
                    continue;
                }
                let src_i = sy as usize * row + sx as usize * bpp;
                let dst_i = x * bpp;
                dst_row[dst_i..dst_i + bpp].copy_from_slice(&data[src_i..src_i + bpp]);
            }
        });

    Frame::from_raw(size, frame.format(), out)
}

/// Blend `frame` toward `color` by `amount` in `0.0..=1.0` (`1.0` = solid color).
///
/// # Errors
///
/// Propagates frame construction errors.
pub fn fade_towards(frame: &Frame, color: Rgb8, amount: f32) -> Result<Frame> {
    let amount = amount.clamp(0.0, 1.0);
    if amount <= 0.0 {
        return Ok(frame.clone());
    }
    if amount >= 1.0 {
        return match frame.format() {
            FrameFormat::Rgb8 => Frame::solid_rgb(frame.size(), color),
            FrameFormat::Rgba8 => {
                let mut f = Frame::solid_rgba(
                    frame.size(),
                    reelforge_core::Rgba8::new(color.r, color.g, color.b, 255),
                )?;
                // Preserve alpha from source when fully faded RGB.
                let bpp = 4;
                let src = frame.data();
                let dst = f.data_mut();
                for (s, d) in src.chunks_exact(bpp).zip(dst.chunks_exact_mut(bpp)) {
                    d[3] = s[3];
                }
                Ok(f)
            }
        };
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let a = (amount * 256.0).round() as u32; // 0..=256
    let inv = 256_u32.saturating_sub(a);
    let cr = u32::from(color.r);
    let cg = u32::from(color.g);
    let cb = u32::from(color.b);

    let mut out = frame.clone();
    let bpp = out.format().bytes_per_pixel();
    let data = out.data_mut();
    data.par_chunks_mut(bpp).for_each(|px| {
        px[0] = blend_fixed(px[0], cr, inv, a);
        px[1] = blend_fixed(px[1], cg, inv, a);
        px[2] = blend_fixed(px[2], cb, inv, a);
    });
    Ok(out)
}

#[inline]
fn blend_fixed(src: u8, target: u32, inv: u32, amount: u32) -> u8 {
    #[allow(clippy::cast_possible_truncation)]
    {
        ((u32::from(src) * inv + target * amount) >> 8) as u8
    }
}

/// In-place integer grayscale (ITU-R BT.601 coefficients, fixed-point).
pub fn grayscale_in_place(frame: &mut Frame) {
    let bpp = frame.format().bytes_per_pixel();
    let data = frame.data_mut();
    data.par_chunks_mut(bpp).for_each(|px| {
        // y ≈ 0.299R + 0.587G + 0.114B  →  (77R + 150G + 29B) >> 8
        let y = (77_u32 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2])) >> 8;
        #[allow(clippy::cast_possible_truncation)]
        let y = y as u8;
        px[0] = y;
        px[1] = y;
        px[2] = y;
    });
}

/// Resolve resize target from optional width/height keeping aspect when one side is set.
///
/// # Errors
///
/// Returns an error when both sides are missing or zero, or source size is invalid.
pub fn resolve_resize_size(source: Size, width: Option<u32>, height: Option<u32>) -> Result<Size> {
    source.require_positive()?;
    match (width, height) {
        (Some(w), Some(h)) => Size::new(w, h).require_positive(),
        (Some(w), None) => {
            if w == 0 {
                return Err(CoreError::InvalidSize(Size::new(0, 0)));
            }
            let h = ((u64::from(source.height) * u64::from(w)) / u64::from(source.width)).max(1);
            let h = u32::try_from(h).unwrap_or(u32::MAX).max(1);
            Ok(Size::new(w, h))
        }
        (None, Some(h)) => {
            if h == 0 {
                return Err(CoreError::InvalidSize(Size::new(0, 0)));
            }
            let w = ((u64::from(source.width) * u64::from(h)) / u64::from(source.height)).max(1);
            let w = u32::try_from(w).unwrap_or(u32::MAX).max(1);
            Ok(Size::new(w, h))
        }
        (None, None) => Err(CoreError::invalid_frame(
            "resize requires width and/or height",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_centerish() {
        let frame = Frame::solid_rgb(Size::new(4, 4), Rgb8::RED).unwrap();
        let c = crop_frame(&frame, 1, 1, 2, 2).unwrap();
        assert_eq!(c.size(), Size::new(2, 2));
        assert_eq!(&c.data()[0..3], &[255, 0, 0]);
    }

    #[test]
    fn mirror_x_swaps() {
        let mut data = vec![0_u8; 6];
        data[0] = 10;
        data[3] = 20;
        let frame = Frame::from_raw(Size::new(2, 1), FrameFormat::Rgb8, data).unwrap();
        let m = mirror_x(&frame).unwrap();
        assert_eq!(m.data()[0], 20);
        assert_eq!(m.data()[3], 10);
    }

    #[test]
    fn rotate_90_dims() {
        let frame = Frame::solid_rgb(Size::new(3, 1), Rgb8::BLUE).unwrap();
        let r = rotate_90_cw(&frame).unwrap();
        assert_eq!(r.size(), Size::new(1, 3));
    }

    #[test]
    fn rotate_degrees_keeps_canvas() {
        let frame = Frame::solid_rgb(Size::new(4, 2), Rgb8::GREEN).unwrap();
        let b = rotate_degrees(&frame, 90.0).unwrap();
        assert_eq!(b.size(), frame.size());
    }

    #[test]
    fn grayscale_fixed() {
        let mut frame = Frame::solid_rgb(Size::new(2, 2), Rgb8::RED).unwrap();
        grayscale_in_place(&mut frame);
        assert_eq!(frame.data()[0], frame.data()[1]);
        assert_eq!(frame.data()[1], frame.data()[2]);
    }
}
