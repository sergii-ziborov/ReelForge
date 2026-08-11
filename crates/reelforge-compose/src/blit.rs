//! Alpha compositing of child frames onto a canvas.

use reelforge_core::{CoreError, Frame, FrameFormat, Mask, Result, Rgb8, Size};

/// Paint `src` onto `dst` (RGB8 canvas) at top-left `(ox, oy)` with coverage.
///
/// `opacity` is a global multiplier in `0.0..=1.0`. When `mask` is present it
/// must match `src.size()`; values are additional per-pixel coverage.
///
/// # Errors
///
/// Returns [`CoreError::InvalidFrame`] when sizes/formats are inconsistent.
#[allow(clippy::cast_sign_loss)] // canvas coords are non-negative after bounds checks
#[allow(clippy::similar_names)] // src/dst x,y pairs
pub fn blit_over(
    dst: &mut Frame,
    src: &Frame,
    ox: i32,
    oy: i32,
    opacity: f32,
    mask: Option<&Mask>,
) -> Result<()> {
    if dst.format() != FrameFormat::Rgb8 {
        return Err(CoreError::invalid_frame(
            "composite canvas must be FrameFormat::Rgb8",
        ));
    }
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return Ok(());
    }
    if let Some(m) = mask
        && m.size() != src.size()
    {
        return Err(CoreError::invalid_frame(format!(
            "mask size {:?} does not match source {:?}",
            m.size(),
            src.size()
        )));
    }

    let canvas = dst.size();
    let child = src.size();
    let dst_data = dst.data_mut();
    let src_data = src.data();
    let cw = canvas.width as usize;
    let sw = child.width as usize;
    let canvas_w = canvas.width.cast_signed();
    let canvas_h = canvas.height.cast_signed();
    let mask_data = mask.map(Mask::data);

    match src.format() {
        FrameFormat::Rgb8 => {
            for sy in 0..child.height {
                let cy = oy + sy.cast_signed();
                if cy < 0 || cy >= canvas_h {
                    continue;
                }
                let cy_u = cy as usize;
                let sy_u = sy as usize;
                for sx in 0..child.width {
                    let cx = ox + sx.cast_signed();
                    if cx < 0 || cx >= canvas_w {
                        continue;
                    }
                    let cx_u = cx as usize;
                    let sx_u = sx as usize;
                    let mut a = opacity;
                    if let Some(md) = mask_data {
                        a *= md[sy_u * sw + sx_u].clamp(0.0, 1.0);
                    }
                    if a <= 0.0 {
                        continue;
                    }
                    let si = (sy_u * sw + sx_u) * 3;
                    let di = (cy_u * cw + cx_u) * 3;
                    blend_rgb(&mut dst_data[di..di + 3], &src_data[si..si + 3], a);
                }
            }
        }
        FrameFormat::Rgba8 => {
            for sy in 0..child.height {
                let cy = oy + sy.cast_signed();
                if cy < 0 || cy >= canvas_h {
                    continue;
                }
                let cy_u = cy as usize;
                let sy_u = sy as usize;
                for sx in 0..child.width {
                    let cx = ox + sx.cast_signed();
                    if cx < 0 || cx >= canvas_w {
                        continue;
                    }
                    let cx_u = cx as usize;
                    let sx_u = sx as usize;
                    let si = (sy_u * sw + sx_u) * 4;
                    let mut a = opacity * (f32::from(src_data[si + 3]) / 255.0);
                    if let Some(md) = mask_data {
                        a *= md[sy_u * sw + sx_u].clamp(0.0, 1.0);
                    }
                    if a <= 0.0 {
                        continue;
                    }
                    let di = (cy_u * cw + cx_u) * 3;
                    blend_rgb(&mut dst_data[di..di + 3], &src_data[si..si + 3], a);
                }
            }
        }
    }
    Ok(())
}

/// Create a solid RGB8 canvas.
///
/// # Errors
///
/// Propagates frame allocation errors.
pub fn solid_canvas(size: Size, color: Rgb8) -> Result<Frame> {
    Frame::solid_rgb(size, color)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn blend_rgb(dst: &mut [u8], src: &[u8], alpha: f32) {
    let inv = 1.0 - alpha;
    for i in 0..3 {
        let v = f32::from(src[i]) * alpha + f32::from(dst[i]) * inv;
        dst[i] = v.round().clamp(0.0, 255.0) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::Rgba8;

    #[test]
    fn blit_opaque_overwrites() {
        let mut canvas = solid_canvas(Size::new(4, 4), Rgb8::BLACK).unwrap();
        let red = Frame::solid_rgb(Size::new(2, 2), Rgb8::RED).unwrap();
        blit_over(&mut canvas, &red, 1, 1, 1.0, None).unwrap();
        let d = canvas.data();
        let i = (4 + 1) * 3;
        assert_eq!(&d[i..i + 3], &[255, 0, 0]);
        assert_eq!(&d[0..3], &[0, 0, 0]);
    }

    #[test]
    fn blit_half_opacity() {
        let mut canvas = solid_canvas(Size::new(1, 1), Rgb8::BLACK).unwrap();
        let white = Frame::solid_rgb(Size::new(1, 1), Rgb8::WHITE).unwrap();
        blit_over(&mut canvas, &white, 0, 0, 0.5, None).unwrap();
        let d = canvas.data();
        assert!(d[0] > 100 && d[0] < 160, "got {}", d[0]);
    }

    #[test]
    fn blit_rgba_alpha() {
        let mut canvas = solid_canvas(Size::new(1, 1), Rgb8::BLACK).unwrap();
        let src = Frame::solid_rgba(Size::new(1, 1), Rgba8::new(255, 0, 0, 128)).unwrap();
        blit_over(&mut canvas, &src, 0, 0, 1.0, None).unwrap();
        assert!(canvas.data()[0] > 0);
    }
}
