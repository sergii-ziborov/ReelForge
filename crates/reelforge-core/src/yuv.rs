//! Packed `FFmpeg` `rawvideo` layout: split a tight buffer into [`SurfacePlane`]s.

use crate::error::{CoreError, Result};
use crate::layout::Size;
use crate::plane::{SurfacePlane, validate_planes};
use crate::surface::PixelFormat;

impl PixelFormat {
    /// Map an `ffprobe` `pix_fmt` to the rawvideo format we will request.
    ///
    /// Native `nv12` stays NV12. Other YUV (4:2:2 / 4:4:4 / 10-bit / `yuvj*`)
    /// is requested as 8-bit [`PixelFormat::Yuv420p`]. RGB-family stays packed.
    #[must_use]
    pub fn from_ffmpeg_pix_fmt(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        match n.as_str() {
            "nv12" => Self::Nv12,
            "rgba" | "rgb0" => Self::Rgba8,
            "bgra" | "bgr0" => Self::Bgra8,
            n if is_yuv_family(n) => Self::Yuv420p,
            _ => Self::Rgb8,
        }
    }

    /// `ffmpeg -pix_fmt` name for a tightly packed `rawvideo` frame.
    #[must_use]
    pub const fn ffmpeg_raw_name(self) -> &'static str {
        match self {
            Self::Rgb8 => "rgb24",
            Self::Rgba8 => "rgba",
            Self::Bgra8 => "bgra",
            Self::Yuv420p => "yuv420p",
            Self::Nv12 => "nv12",
        }
    }

    /// Tight packed size of one frame (`stride == min_stride` on every plane).
    #[must_use]
    pub fn packed_frame_bytes(self, frame: Size) -> Option<usize> {
        let mut total = 0_usize;
        for i in 0..self.plane_count() {
            let (_, h, stride) = self.plane_geometry(frame, i)?;
            let rows = usize::try_from(h).ok()?;
            total = total.checked_add(stride.checked_mul(rows)?)?;
        }
        Some(total)
    }
}

fn is_yuv_family(name: &str) -> bool {
    name.starts_with("yuv")
        || name.starts_with("nv")
        || name.starts_with("p0")
        || name.contains("yuv")
}

/// Split a tightly packed `rawvideo` buffer into planes.
///
/// # Errors
///
/// Length mismatch or invalid plane geometry.
pub fn split_packed_planes(
    format: PixelFormat,
    frame: Size,
    data: &[u8],
) -> Result<Vec<SurfacePlane>> {
    let need = format
        .packed_frame_bytes(frame)
        .ok_or_else(|| CoreError::invalid_frame("packed frame size overflow"))?;
    if data.len() != need {
        return Err(CoreError::invalid_frame(format!(
            "{format:?} packed size {need} for {frame:?}, got {}",
            data.len()
        )));
    }
    let mut offset = 0;
    let mut planes = Vec::with_capacity(format.plane_count());
    for i in 0..format.plane_count() {
        let (w, h, stride) = format.plane_geometry(frame, i).ok_or_else(|| {
            CoreError::invalid_frame(format!("{format:?} has no geometry for plane {i}"))
        })?;
        let rows = usize::try_from(h).unwrap_or(usize::MAX);
        let len = stride.saturating_mul(rows);
        let slice = data[offset..offset + len].to_vec();
        planes.push(SurfacePlane::new(w, h, stride, slice)?);
        offset += len;
    }
    validate_planes(format, frame, &planes)?;
    Ok(planes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_sizes() {
        let frame = Size::new(8, 4);
        assert_eq!(PixelFormat::Yuv420p.packed_frame_bytes(frame), Some(48));
        assert_eq!(PixelFormat::Nv12.packed_frame_bytes(frame), Some(48));
        assert_eq!(PixelFormat::Rgb8.packed_frame_bytes(frame), Some(96));
    }

    #[test]
    fn maps_ffmpeg_names() {
        assert_eq!(
            PixelFormat::from_ffmpeg_pix_fmt("yuv420p"),
            PixelFormat::Yuv420p
        );
        assert_eq!(
            PixelFormat::from_ffmpeg_pix_fmt("yuvj420p"),
            PixelFormat::Yuv420p
        );
        assert_eq!(
            PixelFormat::from_ffmpeg_pix_fmt("yuv422p10le"),
            PixelFormat::Yuv420p
        );
        assert_eq!(PixelFormat::from_ffmpeg_pix_fmt("nv12"), PixelFormat::Nv12);
        assert_eq!(PixelFormat::from_ffmpeg_pix_fmt("rgb24"), PixelFormat::Rgb8);
        assert_eq!(PixelFormat::Yuv420p.ffmpeg_raw_name(), "yuv420p");
        assert_eq!(PixelFormat::Nv12.ffmpeg_raw_name(), "nv12");
    }

    #[test]
    fn split_yuv420p() {
        let frame = Size::new(8, 4);
        let mut buf = vec![16_u8; 32];
        buf.extend(vec![80_u8; 8]);
        buf.extend(vec![200_u8; 8]);
        let planes = split_packed_planes(PixelFormat::Yuv420p, frame, &buf).unwrap();
        assert_eq!(planes.len(), 3);
        assert_eq!(planes[0].data()[0], 16);
        assert_eq!(planes[1].data()[0], 80);
        assert_eq!(planes[2].data()[0], 200);
        assert_eq!((planes[1].width(), planes[1].height()), (4, 2));
    }

    #[test]
    fn split_rejects_wrong_len() {
        assert!(split_packed_planes(PixelFormat::Yuv420p, Size::new(8, 4), &[0; 10]).is_err());
    }
}
