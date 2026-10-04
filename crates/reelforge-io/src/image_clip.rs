//! Still-image video clips.

use crate::error::{IoError, Result};
use image::ImageDecoder;
use reelforge_core::{CoreError, Duration, Frame, FrameFormat, Size, Time, VideoClip};
use std::path::Path;

/// Video clip that shows a single raster for its full duration.
#[derive(Debug, Clone)]
pub struct ImageClip {
    frame: Frame,
    duration: Duration,
    fps: Option<f64>,
}

impl ImageClip {
    /// Build from an already-decoded frame.
    ///
    /// # Errors
    ///
    /// Returns [`IoError::Core`] when `duration` is not positive or the frame size is invalid.
    pub fn from_frame(frame: Frame, duration: Duration) -> Result<Self> {
        frame.size().require_positive().map_err(IoError::from)?;
        if !duration.is_positive() {
            return Err(IoError::from(CoreError::invalid_timing(
                "image clip duration must be > 0",
            )));
        }
        Ok(Self {
            frame,
            duration,
            fps: None,
        })
    }

    /// Load an image file (PNG, JPEG, WebP, GIF first frame, BMP).
    ///
    /// Files with an alpha channel stay [`FrameFormat::Rgba8`] straight alpha.
    /// Opaque files stay [`FrameFormat::Rgb8`]. The decoder's orientation is
    /// applied once, so preview and export share the same pixels.
    ///
    /// # Errors
    ///
    /// Returns image or timing errors.
    pub fn from_path(path: impl AsRef<Path>, duration: Duration) -> Result<Self> {
        let path = path.as_ref();
        let img = load_oriented(path)?;
        let size = Size::new(img.width(), img.height());
        let frame = if img.color().has_alpha() {
            Frame::from_raw(size, FrameFormat::Rgba8, img.to_rgba8().into_raw())
        } else {
            Frame::from_raw(size, FrameFormat::Rgb8, img.to_rgb8().into_raw())
        }
        .map_err(IoError::from)?;
        Self::from_frame(frame, duration)
    }

    /// Attach a nominal FPS for writers / previews.
    #[must_use]
    pub fn with_fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Borrow the stored frame.
    #[must_use]
    pub fn frame(&self) -> &Frame {
        &self.frame
    }
}

fn load_oriented(path: &Path) -> Result<image::DynamicImage> {
    let reader = image::ImageReader::open(path)
        .map_err(|e| IoError::image(format!("open {}: {e}", path.display())))?
        .with_guessed_format()
        .map_err(|e| IoError::image(format!("format {}: {e}", path.display())))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| IoError::image(format!("decode {}: {e}", path.display())))?;
    let orientation = decoder
        .orientation()
        .map_err(|e| IoError::image(format!("orientation {}: {e}", path.display())))?;
    let mut img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| IoError::image(format!("decode {}: {e}", path.display())))?;
    img.apply_orientation(orientation);
    Ok(img)
}

impl VideoClip for ImageClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.frame.size()
    }

    fn fps(&self) -> Option<f64> {
        self.fps
    }

    fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        Ok(self.frame.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{FrameFormat, Rgb8, VideoClip};
    use std::io::Write;

    #[test]
    fn from_frame_samples() {
        let frame = Frame::solid_rgb(Size::new(4, 4), Rgb8::RED).unwrap();
        let clip = ImageClip::from_frame(frame, Duration::from_secs(2.0)).unwrap();
        assert_eq!(clip.size(), Size::new(4, 4));
        let f = clip.frame_at(Time::from_secs(1.0)).unwrap();
        assert_eq!(&f.data()[0..3], &[255, 0, 0]);
        assert!(clip.frame_at(Time::from_secs(2.0)).is_err());
    }

    #[test]
    fn from_png_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dot.png");
        {
            let mut img = image::RgbImage::new(2, 2);
            for p in img.pixels_mut() {
                *p = image::Rgb([0, 255, 0]);
            }
            let mut file = std::fs::File::create(&path).unwrap();
            let mut cursor = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(img)
                .write_to(&mut cursor, image::ImageFormat::Png)
                .unwrap();
            file.write_all(cursor.get_ref()).unwrap();
        }
        let clip = ImageClip::from_path(&path, Duration::from_secs(0.5)).unwrap();
        let f = clip.frame_at(Time::ZERO).unwrap();
        assert_eq!(f.size(), Size::new(2, 2));
        assert_eq!(f.format(), FrameFormat::Rgb8);
        assert_eq!(&f.data()[0..3], &[0, 255, 0]);
    }

    #[test]
    fn transparent_png_keeps_the_blue_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cut.png");
        write_rgba(&path, 2, 1, &[[255, 0, 0, 255], [255, 0, 0, 0]]);
        let clip = ImageClip::from_path(&path, Duration::from_secs(0.5)).unwrap();
        let stored = clip.frame_at(Time::ZERO).unwrap();
        assert_eq!(stored.format(), FrameFormat::Rgba8);
        assert_eq!(stored.alpha_mode(), reelforge_core::AlphaMode::Straight);
        assert_eq!(&stored.data()[0..8], &[255, 0, 0, 255, 255, 0, 0, 0]);

        let layer = reelforge_compose::CompositeLayer::new(std::sync::Arc::new(clip));
        let canvas = reelforge_compose::CompositeVideo::with_background(
            Size::new(2, 1),
            Rgb8::BLUE,
            vec![layer],
        )
        .unwrap();
        let painted = canvas.frame_at(Time::ZERO).unwrap();
        assert_eq!(&painted.data()[0..3], &[255, 0, 0]);
        assert_eq!(&painted.data()[3..6], &[0, 0, 255]);
    }

    fn write_rgba(path: &std::path::Path, width: u32, height: u32, pixels: &[[u8; 4]]) {
        let mut img = image::RgbaImage::new(width, height);
        for (i, px) in pixels.iter().enumerate() {
            let x = u32::try_from(i).unwrap_or(u32::MAX) % width;
            let y = u32::try_from(i).unwrap_or(u32::MAX) / width;
            img.put_pixel(x, y, image::Rgba(*px));
        }
        img.save(path).unwrap();
    }
}
