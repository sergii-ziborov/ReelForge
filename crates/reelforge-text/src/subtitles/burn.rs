//! Build composite layers that burn subtitles onto video.

use super::srt::SubtitleCue;
use crate::clip::TextClip;
use crate::error::Result;
use crate::options::{BITMAP_FONT, TextClipOptions};
use reelforge_compose::CompositeLayer;
use reelforge_core::{Anchor, Position, Rgba8, VideoClip};
use std::sync::Arc;

/// Options for burn-in text appearance.
#[derive(Debug, Clone)]
pub struct BurnInOptions {
    /// Font path or [`BITMAP_FONT`].
    pub font_path: String,
    /// Pixel font size.
    pub font_size: u32,
    /// Text color.
    pub color: Rgba8,
    /// Placement on the canvas.
    pub position: Position,
    /// Layer index for subtitle stack (default high).
    pub layer_index: i32,
}

impl Default for BurnInOptions {
    fn default() -> Self {
        Self {
            font_path: BITMAP_FONT.to_string(),
            font_size: 18,
            color: Rgba8::WHITE,
            position: Position::anchored(Anchor::Bottom, 0, -24),
            layer_index: 100,
        }
    }
}

/// Create one [`CompositeLayer`] per cue (caller composites with base video).
///
/// # Errors
///
/// Propagates text rasterization errors.
pub fn burn_in_layers(cues: &[SubtitleCue], opts: &BurnInOptions) -> Result<Vec<CompositeLayer>> {
    let mut layers = Vec::with_capacity(cues.len());
    for cue in cues {
        let d = cue.duration();
        if !d.is_positive() {
            continue;
        }
        let text_opts = TextClipOptions::new(cue.text.clone(), opts.font_size, d)
            .with_font_path(opts.font_path.clone())
            .with_color(opts.color);
        let clip: Arc<dyn VideoClip> = Arc::new(TextClip::new(&text_opts)?);
        layers.push(
            CompositeLayer::new(clip)
                .with_start(cue.start)
                .with_position(opts.position)
                .with_layer_index(opts.layer_index),
        );
    }
    Ok(layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitles::srt::parse_srt;

    #[test]
    fn builds_layers() {
        let srt = "1\n00:00:00,000 --> 00:00:01,000\nHi\n";
        let cues = parse_srt(srt).unwrap();
        let layers = burn_in_layers(&cues, &BurnInOptions::default()).unwrap();
        assert_eq!(layers.len(), 1);
        assert!((layers[0].clip.duration().as_secs() - 1.0).abs() < 1e-9);
    }
}
