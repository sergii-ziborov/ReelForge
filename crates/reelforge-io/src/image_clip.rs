//! Still-image video clips.

use crate::error::{IoError, Result};
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

    /// Load an image file (PNG, JPEG, WebP, GIF first frame, BMP) as RGB8.
    ///
    /// # Errors
    ///
    /// Returns image or timing errors.
    pub fn from_path(path: impl AsRef<Path>, duration: Duration) -> Result<Self> {
        let path = path.as_ref();
        let img = image::open(path)
            .map_err(|e| IoError::image(format!("open {}: {e}", path.display())))?
            .to_rgb8();
        let width = img.width();
        let height = img.height();
        let size = Size::new(width, height);
        let frame =
            Frame::from_raw(size, FrameFormat::Rgb8, img.into_raw()).map_err(IoError::from)?;
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
    use reelforge_core::{Rgb8, VideoClip};
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
        assert_eq!(&f.data()[0..3], &[0, 255, 0]);
    }
}
