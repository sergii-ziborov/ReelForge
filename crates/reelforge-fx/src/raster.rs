//! Packed-pixel helpers used by video effects.

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
    let mut out = Vec::with_capacity(height as usize * row_dst);
    for row in 0..height as usize {
        let src_y = (y as usize + row) * row_src;
        let start = src_y + x as usize * bpp;
        out.extend_from_slice(&data[start..start + row_dst]);
    }
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

    for dy in 0..dh {
        let sy = (dy * sh) / dh;
        for dx in 0..dw {
            let sx = (dx * sw) / dw;
            let src_i = (sy * sw + sx) * bpp;
            let dst_i = (dy * dw + dx) * bpp;
            out[dst_i..dst_i + bpp].copy_from_slice(&data[src_i..src_i + bpp]);
        }
    }
    Frame::from_raw(new_size, frame.format(), out)
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
    let h = size.height as usize;
    let data = frame.data();
    let mut out = vec![0_u8; data.len()];
    for y in 0..h {
        for x in 0..w {
            let sx = w - 1 - x;
            let src_i = (y * w + sx) * bpp;
            let dst_i = (y * w + x) * bpp;
            out[dst_i..dst_i + bpp].copy_from_slice(&data[src_i..src_i + bpp]);
        }
    }
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
    for y in 0..h {
        let sy = h - 1 - y;
        let src = sy * row;
        let dst = y * row;
        out[dst..dst + row].copy_from_slice(&data[src..src + row]);
    }
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
    for y in 0..sh {
        for x in 0..sw {
            // (x,y) -> (sh-1-y, x)
            let nx = sh - 1 - y;
            let ny = x;
            let src_i = (y * sw + x) * bpp;
            let dst_i = (ny * dw + nx) * bpp;
            out[dst_i..dst_i + bpp].copy_from_slice(&data[src_i..src_i + bpp]);
        }
    }
    let width = u32::try_from(dw).map_err(|_| CoreError::invalid_frame("rotate width"))?;
    let height = u32::try_from(dh).map_err(|_| CoreError::invalid_frame("rotate height"))?;
    Frame::from_raw(Size::new(width, height), frame.format(), out)
}

/// Rotate 180°.
///
/// # Errors
///
/// Propagates nested rotate errors.
pub fn rotate_180(frame: &Frame) -> Result<Frame> {
    rotate_90_cw(&rotate_90_cw(frame)?)
}

/// Rotate 270° clockwise (90° counter-clockwise).
///
/// # Errors
///
/// Propagates nested rotate errors.
pub fn rotate_270_cw(frame: &Frame) -> Result<Frame> {
    rotate_90_cw(&rotate_180(frame)?)
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
    let bpp = frame.format().bytes_per_pixel();
    let data = frame.data();
    let mut out = data.to_vec();
    let inv = 1.0 - amount;
    let cr = f32::from(color.r);
    let cg = f32::from(color.g);
    let cb = f32::from(color.b);

    match frame.format() {
        FrameFormat::Rgb8 | FrameFormat::Rgba8 => {
            for px in out.chunks_exact_mut(bpp) {
                px[0] = blend_u8(px[0], cr, inv, amount);
                px[1] = blend_u8(px[1], cg, inv, amount);
                px[2] = blend_u8(px[2], cb, inv, amount);
            }
        }
    }

    Frame::from_raw(frame.size(), frame.format(), out)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn blend_u8(src: u8, target: f32, inv: f32, amount: f32) -> u8 {
    let v = f32::from(src) * inv + target * amount;
    v.round().clamp(0.0, 255.0) as u8
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
}
