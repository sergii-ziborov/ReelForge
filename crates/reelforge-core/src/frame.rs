//! Raster frames and alpha masks.

use crate::color::{Rgb8, Rgba8};
use crate::error::{CoreError, Result};
use crate::layout::Size;
use std::sync::Arc;

/// Pixel packing of a video frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FrameFormat {
    /// Packed 8-bit RGB, row-major, 3 bytes per pixel.
    #[default]
    Rgb8,
    /// Packed 8-bit RGBA, row-major, 4 bytes per pixel.
    Rgba8,
}

impl FrameFormat {
    /// Bytes per pixel for this format.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Rgb8 => 3,
            Self::Rgba8 => 4,
        }
    }
}

/// Raster frame with shared pixel buffer (`Clone` is cheap until mutation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    size: Size,
    format: FrameFormat,
    data: Arc<Vec<u8>>,
}

impl Frame {
    /// Build a frame from raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidFrame`] or [`CoreError::InvalidSize`] when the
    /// buffer length does not match `size` and `format`, or size is zero.
    pub fn from_raw(size: Size, format: FrameFormat, data: Vec<u8>) -> Result<Self> {
        size.require_positive()?;
        let expected = size
            .pixel_count()
            .checked_mul(format.bytes_per_pixel() as u64)
            .ok_or_else(|| CoreError::invalid_frame("frame byte length overflow"))?;
        if data.len() as u64 != expected {
            return Err(CoreError::invalid_frame(format!(
                "expected {expected} bytes for {size:?} {:?}, got {}",
                format,
                data.len()
            )));
        }
        Ok(Self {
            size,
            format,
            data: Arc::new(data),
        })
    }

    /// Allocate a zeroed frame.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidSize`] or [`CoreError::InvalidFrame`] when
    /// size is not positive or the allocation length overflows `usize`.
    pub fn zeros(size: Size, format: FrameFormat) -> Result<Self> {
        size.require_positive()?;
        let pixels = pixel_len(size)?;
        let len = pixels
            .checked_mul(format.bytes_per_pixel())
            .ok_or_else(|| CoreError::invalid_frame("frame byte length overflow"))?;
        Ok(Self {
            size,
            format,
            data: Arc::new(vec![0; len]),
        })
    }

    /// Solid RGB fill (stored as [`FrameFormat::Rgb8`]).
    ///
    /// Fills via a bulk buffer write (suitable for 4K/8K allocations).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidSize`] or [`CoreError::InvalidFrame`] when
    /// size is not positive or the allocation length overflows `usize`.
    pub fn solid_rgb(size: Size, color: Rgb8) -> Result<Self> {
        size.require_positive()?;
        let pixels = pixel_len(size)?;
        let len = pixels
            .checked_mul(3)
            .ok_or_else(|| CoreError::invalid_frame("frame byte length overflow"))?;
        let mut data = vec![0_u8; len];
        if color.r == color.g && color.g == color.b {
            data.fill(color.r);
        } else {
            for px in data.chunks_exact_mut(3) {
                px[0] = color.r;
                px[1] = color.g;
                px[2] = color.b;
            }
        }
        Self::from_raw(size, FrameFormat::Rgb8, data)
    }

    /// Solid RGBA fill (stored as [`FrameFormat::Rgba8`]).
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidSize`] or [`CoreError::InvalidFrame`] when
    /// size is not positive or the allocation length overflows `usize`.
    pub fn solid_rgba(size: Size, color: Rgba8) -> Result<Self> {
        size.require_positive()?;
        let pixels = pixel_len(size)?;
        let len = pixels
            .checked_mul(4)
            .ok_or_else(|| CoreError::invalid_frame("frame byte length overflow"))?;
        let mut data = vec![0_u8; len];
        if color.r == color.g && color.g == color.b && color.a == color.r {
            data.fill(color.r);
        } else {
            for px in data.chunks_exact_mut(4) {
                px[0] = color.r;
                px[1] = color.g;
                px[2] = color.b;
                px[3] = color.a;
            }
        }
        Self::from_raw(size, FrameFormat::Rgba8, data)
    }

    /// Frame dimensions.
    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    /// Pixel format.
    #[must_use]
    pub const fn format(&self) -> FrameFormat {
        self.format
    }

    /// Immutable packed pixels.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Mutable packed pixels (copy-on-write if the buffer is shared).
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut Arc::make_mut(&mut self.data)[..]
    }

    /// Consume into raw parts.
    #[must_use]
    pub fn into_raw(self) -> (Size, FrameFormat, Vec<u8>) {
        let data = Arc::try_unwrap(self.data).unwrap_or_else(|a| (*a).clone());
        (self.size, self.format, data)
    }
}

/// Single-channel mask aligned with a frame, values in `0.0..=1.0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    size: Size,
    data: Arc<Vec<f32>>,
}

impl Mask {
    /// Build a mask from per-pixel coverage samples.
    ///
    /// # Errors
    ///
    /// Returns an error when size is invalid or the buffer length mismatches.
    pub fn from_raw(size: Size, data: Vec<f32>) -> Result<Self> {
        size.require_positive()?;
        if data.len() as u64 != size.pixel_count() {
            return Err(CoreError::invalid_frame(format!(
                "expected {} mask samples, got {}",
                size.pixel_count(),
                data.len()
            )));
        }
        Ok(Self {
            size,
            data: Arc::new(data),
        })
    }

    /// Fully opaque mask.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidSize`] or [`CoreError::InvalidFrame`] when
    /// size is not positive or the sample count overflows `usize`.
    pub fn opaque(size: Size) -> Result<Self> {
        size.require_positive()?;
        let pixels = pixel_len(size)?;
        Ok(Self {
            size,
            data: Arc::new(vec![1.0; pixels]),
        })
    }

    /// Fully transparent mask.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidSize`] or [`CoreError::InvalidFrame`] when
    /// size is not positive or the sample count overflows `usize`.
    pub fn transparent(size: Size) -> Result<Self> {
        size.require_positive()?;
        let pixels = pixel_len(size)?;
        Ok(Self {
            size,
            data: Arc::new(vec![0.0; pixels]),
        })
    }

    /// Mask dimensions.
    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    /// Coverage samples, row-major.
    #[must_use]
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Mutable coverage samples (copy-on-write if shared).
    pub fn data_mut(&mut self) -> &mut [f32] {
        &mut Arc::make_mut(&mut self.data)[..]
    }
}

fn pixel_len(size: Size) -> Result<usize> {
    usize::try_from(size.pixel_count())
        .map_err(|_| CoreError::invalid_frame("frame pixel count exceeds usize"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_rgb_length() {
        let frame = Frame::solid_rgb(Size::new(2, 2), Rgb8::RED).unwrap();
        assert_eq!(frame.data().len(), 12);
        assert_eq!(&frame.data()[0..3], &[255, 0, 0]);
    }

    #[test]
    fn rejects_bad_length() {
        let err = Frame::from_raw(Size::new(2, 2), FrameFormat::Rgb8, vec![0; 5]);
        assert!(err.is_err());
    }

    #[test]
    fn clone_shares_until_mutate() {
        let a = Frame::solid_rgb(Size::new(4, 4), Rgb8::BLUE).unwrap();
        let mut b = a.clone();
        assert!(std::ptr::eq(a.data().as_ptr(), b.data().as_ptr()));
        b.data_mut()[0] = 1;
        assert!(!std::ptr::eq(a.data().as_ptr(), b.data().as_ptr()));
    }
}
