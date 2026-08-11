//! Loaded font face abstraction.

use super::bitmap::{BITMAP_CELL_H, BITMAP_CELL_W, bitmap_glyph};
use crate::error::{Result, TextError};
use crate::options::BITMAP_FONT;
use std::path::Path;
use std::sync::Arc;

/// Coverage raster for one glyph.
#[derive(Debug, Clone)]
pub struct GlyphCoverage {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// Horizontal advance for layout.
    pub advance: f32,
    /// Horizontal bearing (left offset).
    pub xmin: i32,
    /// Vertical bearing (bottom of bitmap relative to baseline in fontdue; 0 for bitmap).
    pub ymin: i32,
    /// Row-major coverage `0.0..=1.0`.
    pub coverage: Vec<f32>,
}

/// Font backend used by the text rasterizer.
#[derive(Clone)]
pub enum FontFace {
    /// Built-in 5×7 ASCII face.
    Bitmap {
        /// Requested pixel scale (integer multiplier of the 7px cell).
        scale: u32,
    },
    /// TrueType/OpenType via `fontdue`.
    TrueType {
        /// Shared font data.
        font: Arc<fontdue::Font>,
        /// Pixel size.
        px: f32,
    },
}

/// Load a face from options (`bitmap` or a font file path).
///
/// # Errors
///
/// Returns font errors when the file cannot be read or parsed.
pub fn load_face(font_path: &str, font_size: u32) -> Result<FontFace> {
    if font_size == 0 {
        return Err(TextError::font("font_size must be > 0"));
    }
    if font_path.is_empty() || font_path.eq_ignore_ascii_case(BITMAP_FONT) {
        let scale = (font_size / BITMAP_CELL_H).max(1);
        return Ok(FontFace::Bitmap { scale });
    }
    let path = Path::new(font_path);
    let bytes = std::fs::read(path)
        .map_err(|e| TextError::font(format!("read {}: {e}", path.display())))?;
    let font = fontdue::Font::from_bytes(bytes.as_slice(), fontdue::FontSettings::default())
        .map_err(|e| TextError::font(format!("parse font: {e}")))?;
    #[allow(clippy::cast_precision_loss)]
    let px = font_size as f32;
    Ok(FontFace::TrueType {
        font: Arc::new(font),
        px,
    })
}

impl FontFace {
    /// Line height in pixels.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn line_height(&self) -> u32 {
        match self {
            Self::Bitmap { scale } => BITMAP_CELL_H * *scale,
            Self::TrueType { font, px } => {
                let metrics = font.horizontal_line_metrics(*px);
                metrics
                    .map_or_else(|| px.ceil() as u32, |m| m.new_line_size.ceil() as u32)
                    .max(1)
            }
        }
    }

    /// Rasterize one Unicode scalar.
    #[must_use]
    pub fn glyph(&self, ch: char) -> GlyphCoverage {
        match self {
            Self::Bitmap { scale } => scale_bitmap(ch, *scale),
            Self::TrueType { font, px } => {
                let (metrics, bitmap) = font.rasterize(ch, *px);
                let coverage = bitmap.into_iter().map(|b| f32::from(b) / 255.0).collect();
                #[allow(clippy::cast_possible_truncation)]
                let width = metrics.width as u32;
                #[allow(clippy::cast_possible_truncation)]
                let height = metrics.height as u32;
                GlyphCoverage {
                    width,
                    height,
                    advance: metrics.advance_width,
                    xmin: metrics.xmin,
                    ymin: metrics.ymin,
                    coverage,
                }
            }
        }
    }
}

fn scale_bitmap(ch: char, scale: u32) -> GlyphCoverage {
    let base = bitmap_glyph(ch);
    let w = BITMAP_CELL_W * scale;
    let h = BITMAP_CELL_H * scale;
    let mut coverage = vec![0.0_f32; (w * h) as usize];
    for y in 0..BITMAP_CELL_H {
        for x in 0..BITMAP_CELL_W {
            let v = base[y as usize][x as usize];
            if v <= 0.0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let ix = (x * scale + dx) as usize;
                    let iy = (y * scale + dy) as usize;
                    coverage[iy * w as usize + ix] = v;
                }
            }
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let advance = f32::from(u16::try_from(w.saturating_add(scale)).unwrap_or(u16::MAX));
    GlyphCoverage {
        width: w,
        height: h,
        advance,
        xmin: 0,
        ymin: 0,
        coverage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_face_loads() {
        let face = load_face(BITMAP_FONT, 14).unwrap();
        assert!(matches!(face, FontFace::Bitmap { scale: 2 }));
        let g = face.glyph('H');
        assert!(g.coverage.iter().any(|&c| c > 0.0));
    }
}
