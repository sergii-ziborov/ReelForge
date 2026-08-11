//! Text measurement and glyph placement.

use crate::error::{Result, TextError};
use crate::font::{FontFace, GlyphCoverage};
use reelforge_core::Size;

/// One positioned glyph on the canvas.
#[derive(Debug, Clone)]
pub struct PlacedGlyph {
    /// Top-left X on the output bitmap.
    pub x: i32,
    /// Top-left Y on the output bitmap.
    pub y: i32,
    /// Raster data.
    pub glyph: GlyphCoverage,
}

/// Measured layout for a text block.
#[derive(Debug, Clone)]
pub struct TextLayout {
    /// Content width (without outer padding).
    pub content_width: u32,
    /// Content height (without outer padding).
    pub content_height: u32,
    /// Placed glyphs in draw order.
    pub glyphs: Vec<PlacedGlyph>,
}

/// Layout `text` with `face`. Lines are split on `\n`.
///
/// # Errors
///
/// Returns layout errors when content is empty after processing or glyph size overflows.
pub fn layout_text(face: &FontFace, text: &str) -> Result<TextLayout> {
    if text.is_empty() {
        return Err(TextError::layout("text must not be empty"));
    }
    let line_height = face.line_height();
    let mut glyphs = Vec::new();
    let mut max_w: u32 = 0;
    let mut pen_y: i32 = 0;

    for line in text.split('\n') {
        let mut pen_x: f32 = 0.0;
        let mut line_max_h = line_height;
        let mut line_glyphs = Vec::new();

        for ch in line.chars() {
            if ch == '\r' {
                continue;
            }
            let g = face.glyph(ch);
            line_max_h = line_max_h.max(g.height.max(1));
            // Baseline-ish: bitmap draws from top; TTF uses ymin relative to baseline.
            let y = match face {
                FontFace::Bitmap { .. } => pen_y,
                FontFace::TrueType { .. } => {
                    // Place so glyph top is near pen_y: baseline at pen_y + ascent-ish
                    pen_y + (line_height.cast_signed() - g.height.cast_signed() - g.ymin)
                }
            };
            #[allow(clippy::cast_possible_truncation)]
            let x = pen_x as i32 + g.xmin;
            pen_x += g.advance;
            line_glyphs.push(PlacedGlyph { x, y, glyph: g });
        }

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let line_w = pen_x.ceil().max(0.0) as u32;
        max_w = max_w.max(line_w);
        glyphs.extend(line_glyphs);
        pen_y += line_max_h.cast_signed();
    }

    let content_height = u32::try_from(pen_y.max(0)).unwrap_or(u32::MAX).max(1);
    let content_width = max_w.max(1);
    Ok(TextLayout {
        content_width,
        content_height,
        glyphs,
    })
}

/// Resolve final canvas size from layout + options.
///
/// # Errors
///
/// Returns an error when the forced size is zero.
pub fn resolve_canvas(layout: &TextLayout, forced: Option<Size>, padding: u32) -> Result<Size> {
    if let Some(size) = forced {
        return size
            .require_positive()
            .map_err(|e| TextError::layout(e.to_string()));
    }
    let w = layout
        .content_width
        .saturating_add(padding.saturating_mul(2));
    let h = layout
        .content_height
        .saturating_add(padding.saturating_mul(2));
    Size::new(w.max(1), h.max(1))
        .require_positive()
        .map_err(|e| TextError::layout(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::load_face;
    use crate::options::BITMAP_FONT;

    #[test]
    fn layout_two_lines() {
        let face = load_face(BITMAP_FONT, 7).unwrap();
        let layout = layout_text(&face, "Hi\nYo").unwrap();
        assert!(layout.content_height >= 14);
        assert!(layout.glyphs.len() >= 4);
    }
}
