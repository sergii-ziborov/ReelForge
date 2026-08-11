//! Paint laid-out glyphs into an RGBA frame + optional mask.

use crate::error::Result;
use crate::layout::TextLayout;
use reelforge_core::{Frame, FrameFormat, Mask, Rgba8, Size};

/// Render `layout` into an RGBA frame of `canvas`, with `padding` offset for content.
///
/// # Errors
///
/// Propagates frame construction errors.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names
)]
pub fn rasterize_rgba(
    layout: &TextLayout,
    canvas: Size,
    padding: u32,
    color: Rgba8,
) -> Result<(Frame, Mask)> {
    let pixels = usize::try_from(canvas.pixel_count())
        .map_err(|_| reelforge_core::CoreError::invalid_frame("canvas too large"))?;
    let mut rgba = vec![0_u8; pixels * 4];
    let mut mask = vec![0.0_f32; pixels];
    let canvas_w = canvas.width as usize;
    let canvas_h = canvas.height as usize;
    let pad = padding.cast_signed();

    for placed in &layout.glyphs {
        let glyph = &placed.glyph;
        let glyph_w = glyph.width as usize;
        let glyph_h = glyph.height as usize;
        for row in 0..glyph_h {
            for col in 0..glyph_w {
                let cov = glyph.coverage[row * glyph_w + col];
                if cov <= 0.0 {
                    continue;
                }
                let px = placed.x + pad + col as i32;
                let py = placed.y + pad + row as i32;
                if px < 0 || py < 0 {
                    continue;
                }
                let xu = px as usize;
                let yu = py as usize;
                if xu >= canvas_w || yu >= canvas_h {
                    continue;
                }
                let idx = yu * canvas_w + xu;
                // Max coverage wins for overlapping strokes.
                if cov > mask[idx] {
                    mask[idx] = cov;
                    let alpha = (f32::from(color.a) * cov).round().clamp(0.0, 255.0) as u8;
                    let off = idx * 4;
                    rgba[off] = color.r;
                    rgba[off + 1] = color.g;
                    rgba[off + 2] = color.b;
                    rgba[off + 3] = alpha;
                }
            }
        }
    }

    let frame = Frame::from_raw(canvas, FrameFormat::Rgba8, rgba)?;
    let mask = Mask::from_raw(canvas, mask)?;
    Ok((frame, mask))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::load_face;
    use crate::layout::{layout_text, resolve_canvas};
    use crate::options::BITMAP_FONT;

    #[test]
    fn raster_has_ink() {
        let face = load_face(BITMAP_FONT, 7).unwrap();
        let layout = layout_text(&face, "OK").unwrap();
        let size = resolve_canvas(&layout, None, 2).unwrap();
        let (frame, mask) = rasterize_rgba(&layout, size, 2, Rgba8::WHITE).unwrap();
        assert_eq!(frame.format(), FrameFormat::Rgba8);
        assert!(mask.data().iter().any(|&c| c > 0.5));
    }
}
