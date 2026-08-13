//! Image planes and row strides for [`crate::VideoSurface`].

use crate::error::{CoreError, Result};
use crate::layout::Size;
use crate::surface::PixelFormat;
use std::sync::Arc;

/// One image plane (packed RGB = a single plane).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfacePlane {
    data: Arc<Vec<u8>>,
    stride: usize,
    width: u32,
    height: u32,
}

impl SurfacePlane {
    /// Build a plane; `data.len()` must be at least `stride * height`.
    ///
    /// # Errors
    ///
    /// Zero size, stride too small, or buffer too short.
    pub fn new(width: u32, height: u32, stride: usize, data: Vec<u8>) -> Result<Self> {
        if width == 0 || height == 0 {
            return Err(CoreError::invalid_frame("plane width/height must be > 0"));
        }
        let min_stride = usize::try_from(width).unwrap_or(usize::MAX);
        if stride < min_stride {
            return Err(CoreError::invalid_frame(format!(
                "plane stride {stride} < width {width}"
            )));
        }
        let rows = usize::try_from(height).unwrap_or(usize::MAX);
        let need = stride.saturating_mul(rows);
        if data.len() < need {
            return Err(CoreError::invalid_frame(format!(
                "plane buffer {} < stride*height {need}",
                data.len()
            )));
        }
        Ok(Self {
            data: Arc::new(data),
            stride,
            width,
            height,
        })
    }

    /// Width in samples.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in samples.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Bytes per row.
    #[must_use]
    pub const fn stride(&self) -> usize {
        self.stride
    }

    /// Backing bytes (may include row padding).
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Copy `row_bytes` from each row, dropping stride padding.
    ///
    /// # Errors
    ///
    /// `row_bytes` is zero, larger than the stride, or the buffer is short.
    pub fn compact(&self, row_bytes: usize) -> Result<Vec<u8>> {
        if row_bytes == 0 || row_bytes > self.stride {
            return Err(CoreError::invalid_frame(format!(
                "compact row_bytes {row_bytes} vs stride {}",
                self.stride
            )));
        }
        let rows = usize::try_from(self.height).unwrap_or(usize::MAX);
        let need = self.stride.saturating_mul(rows);
        if self.data.len() < need {
            return Err(CoreError::invalid_frame(format!(
                "plane buffer {} < stride*height {need}",
                self.data.len()
            )));
        }
        if row_bytes == self.stride {
            return Ok(self.data[..need].to_vec());
        }
        let mut out = Vec::with_capacity(row_bytes.saturating_mul(rows));
        for y in 0..rows {
            let start = y.saturating_mul(self.stride);
            out.extend_from_slice(&self.data[start..start + row_bytes]);
        }
        Ok(out)
    }
}

impl PixelFormat {
    /// Number of planes for this format.
    #[must_use]
    pub const fn plane_count(self) -> usize {
        match self {
            Self::Rgb8 | Self::Rgba8 | Self::Bgra8 => 1,
            Self::Nv12 => 2,
            Self::Yuv420p => 3,
        }
    }

    /// `(width, height, min_stride)` of `plane` for a frame of `frame`.
    #[must_use]
    pub fn plane_geometry(self, frame: Size, plane: usize) -> Option<(u32, u32, usize)> {
        if plane >= self.plane_count() {
            return None;
        }
        let w = frame.width;
        let h = frame.height;
        match (self, plane) {
            (Self::Rgb8, 0) => Some((w, h, packed_stride(w, 3)?)),
            (Self::Rgba8 | Self::Bgra8, 0) => Some((w, h, packed_stride(w, 4)?)),
            (Self::Yuv420p | Self::Nv12, 0) => Some((w, h, packed_stride(w, 1)?)),
            (Self::Yuv420p, 1 | 2) => {
                let cw = w.div_ceil(2);
                let ch = h.div_ceil(2);
                Some((cw, ch, packed_stride(cw, 1)?))
            }
            (Self::Nv12, 1) => {
                let ch = h.div_ceil(2);
                Some((w, ch, packed_stride(w, 1)?))
            }
            _ => None,
        }
    }
}

fn packed_stride(width: u32, bpp: u32) -> Option<usize> {
    usize::try_from(width.checked_mul(bpp)?).ok()
}

/// Check `planes` match `format` + `frame` (count and minimum geometry).
///
/// # Errors
///
/// Wrong plane count or subsampled size.
pub fn validate_planes(format: PixelFormat, frame: Size, planes: &[SurfacePlane]) -> Result<()> {
    if planes.len() != format.plane_count() {
        return Err(CoreError::invalid_frame(format!(
            "{format:?} expects {} planes, got {}",
            format.plane_count(),
            planes.len()
        )));
    }
    for (i, plane) in planes.iter().enumerate() {
        let Some((w, h, min_stride)) = format.plane_geometry(frame, i) else {
            return Err(CoreError::invalid_frame(format!(
                "{format:?} has no geometry for plane {i}"
            )));
        };
        if plane.width != w || plane.height != h {
            return Err(CoreError::invalid_frame(format!(
                "{format:?} plane {i} size {}x{}, expected {w}x{h}",
                plane.width, plane.height
            )));
        }
        if plane.stride < min_stride {
            return Err(CoreError::invalid_frame(format!(
                "{format:?} plane {i} stride {} < {min_stride}",
                plane.stride
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_rgb_one_plane() {
        assert_eq!(PixelFormat::Rgb8.plane_count(), 1);
        let (w, h, stride) = PixelFormat::Rgb8
            .plane_geometry(Size::new(4, 2), 0)
            .unwrap();
        assert_eq!((w, h, stride), (4, 2, 12));
    }

    #[test]
    fn yuv420p_three_planes() {
        let fmt = PixelFormat::Yuv420p;
        assert_eq!(fmt.plane_count(), 3);
        let frame = Size::new(8, 4);
        assert_eq!(fmt.plane_geometry(frame, 0), Some((8, 4, 8)));
        assert_eq!(fmt.plane_geometry(frame, 1), Some((4, 2, 4)));
        assert_eq!(fmt.plane_geometry(frame, 2), Some((4, 2, 4)));
    }

    #[test]
    fn nv12_two_planes() {
        let frame = Size::new(8, 4);
        assert_eq!(PixelFormat::Nv12.plane_geometry(frame, 0), Some((8, 4, 8)));
        assert_eq!(PixelFormat::Nv12.plane_geometry(frame, 1), Some((8, 2, 8)));
    }

    #[test]
    fn rejects_short_buffer() {
        assert!(SurfacePlane::new(4, 2, 4, vec![0; 4]).is_err());
    }

    #[test]
    fn compact_drops_row_padding() {
        let mut padded = vec![0_u8; 8 * 2];
        padded[0..3].copy_from_slice(&[1, 2, 3]);
        padded[8..11].copy_from_slice(&[4, 5, 6]);
        let plane = SurfacePlane::new(3, 2, 8, padded).unwrap();
        assert_eq!(plane.compact(3).unwrap(), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn yuv420p_from_planes() {
        use crate::media_time::MediaTime;
        use crate::surface::{ColorInfo, MemoryLocation, StreamTimeBase, VideoSurface};

        let y = SurfacePlane::new(8, 4, 8, vec![0; 32]).unwrap();
        let u = SurfacePlane::new(4, 2, 4, vec![128; 8]).unwrap();
        let v = SurfacePlane::new(4, 2, 4, vec![128; 8]).unwrap();
        let s = VideoSurface::from_planes(
            PixelFormat::Yuv420p,
            Size::new(8, 4),
            vec![y, u, v],
            MediaTime::zero(1_000),
            None,
            ColorInfo::default(),
            StreamTimeBase::HZ_1K,
        )
        .unwrap();
        assert_eq!(s.planes().len(), 3);
        assert_eq!(s.location(), MemoryLocation::CpuPlanar);
        assert_eq!(s.stride(), 8);
        assert!(s.to_frame().is_err());
    }

    #[test]
    fn rejects_wrong_plane_count() {
        let y = SurfacePlane::new(8, 4, 8, vec![0; 32]).unwrap();
        assert!(validate_planes(PixelFormat::Yuv420p, Size::new(8, 4), &[y]).is_err());
    }

    #[test]
    fn padded_rgb_roundtrips_to_frame() {
        use crate::frame::FrameFormat;
        use crate::media_time::MediaTime;
        use crate::surface::{ColorInfo, ColorRange, StreamTimeBase, VideoSurface};

        let mut padded = vec![0_u8; 16 * 2];
        padded[0..3].copy_from_slice(&[255, 0, 0]);
        padded[16..19].copy_from_slice(&[0, 255, 0]);
        let plane = SurfacePlane::new(4, 2, 16, padded).unwrap();
        let s = VideoSurface::from_planes(
            PixelFormat::Rgb8,
            Size::new(4, 2),
            vec![plane],
            MediaTime::zero(1_000),
            None,
            ColorInfo {
                range: ColorRange::Full,
                ..ColorInfo::default()
            },
            StreamTimeBase::HZ_1K,
        )
        .unwrap();
        assert_eq!(s.stride(), 16);
        let frame = s.to_frame().unwrap();
        assert_eq!(frame.format(), FrameFormat::Rgb8);
        assert_eq!(&frame.data()[0..3], &[255, 0, 0]);
        assert_eq!(&frame.data()[12..15], &[0, 255, 0]);
        assert_eq!(frame.data().len(), 4 * 2 * 3);
    }
}
