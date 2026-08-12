//! Proxy and thumbnail hooks for Capture / editor previews.

use crate::error::{IoError, Result};
use crate::options::WriteVideoOptions;
use crate::write::write_video;
use reelforge_core::{Size, Time, VideoClip, VideoEffect};
use reelforge_fx::Resize;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

/// Options for a low-resolution proxy encode.
#[derive(Debug, Clone)]
pub struct ProxyOptions {
    /// Output path.
    pub path: String,
    /// Max width (height scaled to preserve aspect).
    pub max_width: u32,
    /// Max height (width scaled if needed).
    pub max_height: u32,
    /// Proxy FPS (defaults to source fps or 15).
    pub fps: Option<f64>,
    /// CRF for proxy quality (default 28).
    pub crf: u8,
}

impl ProxyOptions {
    /// Proxy to `path` with max box `max_width` × `max_height`.
    #[must_use]
    pub fn new(path: impl Into<String>, max_width: u32, max_height: u32) -> Self {
        Self {
            path: path.into(),
            max_width: max_width.max(2),
            max_height: max_height.max(2),
            fps: None,
            crf: 28,
        }
    }

    /// Override proxy FPS.
    #[must_use]
    pub fn with_fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Override CRF.
    #[must_use]
    pub fn with_crf(mut self, crf: u8) -> Self {
        self.crf = crf;
        self
    }
}

/// Fit `src` into a max box, preserving aspect (even dimensions).
#[must_use]
pub fn proxy_size(src: Size, max_width: u32, max_height: u32) -> Size {
    let max_w = max_width.max(2);
    let max_h = max_height.max(2);
    let sw = src.width.max(1);
    let sh = src.height.max(1);
    #[allow(clippy::cast_precision_loss)]
    let scale = (f64::from(max_w) / f64::from(sw)).min(f64::from(max_h) / f64::from(sh));
    let scale = scale.min(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut w = ((f64::from(sw) * scale).round() as u32).max(2);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mut h = ((f64::from(sh) * scale).round() as u32).max(2);
    // Even dims for yuv420p.
    w &= !1;
    h &= !1;
    Size::new(w.max(2), h.max(2))
}

/// Write a downscaled proxy video for editor scrubbing.
///
/// # Errors
///
/// Resize or encode failures.
pub fn write_proxy(clip: Arc<dyn VideoClip>, options: &ProxyOptions) -> Result<()> {
    let size = proxy_size(clip.size(), options.max_width, options.max_height);
    let scaled = Resize::to(size).apply(clip).map_err(IoError::from)?;
    let fps = options
        .fps
        .or_else(|| scaled.fps())
        .filter(|f| f.is_finite() && *f > 0.0)
        .unwrap_or(15.0);
    let opts = WriteVideoOptions::new(&options.path, fps).with_crf(options.crf);
    write_video(scaled.as_ref(), &opts)
}

/// Write a single-frame PNG thumbnail at media time `t`.
///
/// # Errors
///
/// Frame sample or PNG encode failures.
pub fn write_thumbnail(clip: &dyn VideoClip, t: Time, path: impl AsRef<Path>) -> Result<()> {
    let frame = clip.frame_at(t).map_err(IoError::from)?;
    let size = frame.size();
    let rgb = frame.data();
    let expected = size.width as usize * size.height as usize * 3;
    if rgb.len() < expected {
        return Err(IoError::message(format!(
            "thumbnail: frame buffer too small ({} < {expected})",
            rgb.len()
        )));
    }
    let img = image::RgbImage::from_raw(size.width, size.height, rgb[..expected].to_vec())
        .ok_or_else(|| IoError::message("thumbnail: invalid RGB buffer"))?;
    let path = path.as_ref();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| IoError::message(format!("thumbnail mkdir: {e}")))?;
    }
    image::DynamicImage::ImageRgb8(img)
        .save(path)
        .map_err(|e| IoError::image(format!("thumbnail png: {e}")))?;
    Ok(())
}

/// Encode thumbnail PNG bytes in memory (for MCP / host caches).
///
/// # Errors
///
/// Frame sample or PNG encode failures.
pub fn thumbnail_png_bytes(clip: &dyn VideoClip, t: Time) -> Result<Vec<u8>> {
    let frame = clip.frame_at(t).map_err(IoError::from)?;
    let size = frame.size();
    let rgb = frame.data();
    let expected = size.width as usize * size.height as usize * 3;
    if rgb.len() < expected {
        return Err(IoError::message("thumbnail bytes: short frame"));
    }
    let img = image::RgbImage::from_raw(size.width, size.height, rgb[..expected].to_vec())
        .ok_or_else(|| IoError::message("thumbnail bytes: invalid RGB"))?;
    let mut cursor = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut cursor, image::ImageFormat::Png)
        .map_err(|e| IoError::image(format!("thumbnail encode: {e}")))?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Duration, Rgb8};

    #[test]
    fn proxy_size_fits_box() {
        let s = proxy_size(Size::new(1920, 1080), 640, 360);
        assert!(s.width <= 640 && s.height <= 360);
        assert_eq!(s.width % 2, 0);
        assert_eq!(s.height % 2, 0);
    }

    #[test]
    fn thumbnail_png_roundtrip() {
        let clip = ColorClip::new(Size::new(8, 6), Rgb8::RED, Duration::from_secs(1.0));
        let bytes = thumbnail_png_bytes(&clip, Time::ZERO).unwrap();
        assert!(bytes.starts_with(b"\x89PNG"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.png");
        write_thumbnail(&clip, Time::ZERO, &path).unwrap();
        assert!(path.is_file());
    }
}
