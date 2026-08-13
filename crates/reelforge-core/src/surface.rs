//! Timed video surface — P1 media object (not just packed RGB).
//!
//! `Frame` stays the pixel buffer used by effects. [`VideoSurface`] adds
//! format, planes/strides, PTS, frame duration, color tags, and memory location.

use crate::alpha::AlphaMode;
use crate::error::{CoreError, Result};
use crate::frame::{Frame, FrameFormat};
use crate::layout::Size;
use crate::media_time::MediaTime;
use crate::plane::{SurfacePlane, validate_planes};

/// Pixel layout of a [`VideoSurface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum PixelFormat {
    /// Packed 8-bit RGB, 3 bytes/pixel.
    #[default]
    Rgb8,
    /// Packed 8-bit RGBA, 4 bytes/pixel.
    Rgba8,
    /// Packed 8-bit BGRA (file decode when the source is BGRA).
    Bgra8,
    /// Planar 4:2:0 8-bit (file `surface_at` for typical YUV sources).
    Yuv420p,
    /// Semi-planar NV12 (file `surface_at` when `ffprobe` reports `nv12`).
    Nv12,
}

impl PixelFormat {
    /// Bytes per packed pixel, when the format is packed.
    #[must_use]
    pub const fn packed_bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Rgb8 => Some(3),
            Self::Rgba8 | Self::Bgra8 => Some(4),
            Self::Yuv420p | Self::Nv12 => None,
        }
    }

    /// Whether this is a packed RGB-family format the current clip graph can sample.
    #[must_use]
    pub const fn is_packed_rgb(self) -> bool {
        matches!(self, Self::Rgb8 | Self::Rgba8)
    }
}

impl From<FrameFormat> for PixelFormat {
    fn from(value: FrameFormat) -> Self {
        match value {
            FrameFormat::Rgb8 => Self::Rgb8,
            FrameFormat::Rgba8 => Self::Rgba8,
        }
    }
}

impl TryFrom<PixelFormat> for FrameFormat {
    type Error = CoreError;

    fn try_from(value: PixelFormat) -> Result<Self> {
        match value {
            PixelFormat::Rgb8 => Ok(Self::Rgb8),
            PixelFormat::Rgba8 => Ok(Self::Rgba8),
            other => Err(CoreError::invalid_frame(format!(
                "pixel format {other:?} is not a clip-graph FrameFormat"
            ))),
        }
    }
}

/// Where pixel memory lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum MemoryLocation {
    /// Host packed buffer (today's `Frame` backing).
    #[default]
    CpuPacked,
    /// Host planar planes (`Yuv420p` / `Nv12` via [`VideoSurface::from_planes`]).
    CpuPlanar,
    /// Device / external surface (future GPU).
    External,
}

/// Sample range / color tags (HDR fields come later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ColorRange {
    /// Unspecified / unknown.
    #[default]
    Unspecified,
    /// Full range (0–255 for 8-bit).
    Full,
    /// Limited / studio range (16–235).
    Limited,
}

/// Color space matrix (`ffprobe` `color_space`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ColorSpace {
    /// Unknown.
    #[default]
    Unspecified,
    /// RGB / identity.
    Rgb,
    /// BT.601 / SMPTE 170M / BT.470.
    Bt601,
    /// BT.709.
    Bt709,
    /// BT.2020 non-constant luminance.
    Bt2020,
}

/// Primaries (`ffprobe` `color_primaries`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ColorPrimaries {
    /// Unknown.
    #[default]
    Unspecified,
    /// BT.709.
    Bt709,
    /// BT.601 / SMPTE 170M.
    Bt601,
    /// BT.2020.
    Bt2020,
}

/// Transfer characteristic (`ffprobe` `color_transfer`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ColorTransfer {
    /// Unknown.
    #[default]
    Unspecified,
    /// BT.709 / BT.601 gamma.
    Bt709,
    /// Linear.
    Linear,
    /// PQ (SMPTE ST 2084).
    Smpte2084,
    /// HLG (ARIB STD-B67).
    Hlg,
}

/// Stream `time_base` as `num/den` (`FFmpeg` style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StreamTimeBase {
    /// Numerator (usually `1`).
    pub num: u32,
    /// Denominator (ticks per second).
    pub den: u32,
}

impl StreamTimeBase {
    /// 90 kHz media clock (`1/90000`).
    pub const HZ_90K: Self = Self {
        num: 1,
        den: 90_000,
    };

    /// 1 kHz clock.
    pub const HZ_1K: Self = Self { num: 1, den: 1_000 };

    /// Construct when both parts are non-zero.
    #[must_use]
    pub const fn new(num: u32, den: u32) -> Option<Self> {
        if num == 0 || den == 0 {
            None
        } else {
            Some(Self { num, den })
        }
    }

    /// Ticks per second (`den`).
    #[must_use]
    pub const fn timescale(self) -> u32 {
        if self.den == 0 { 1 } else { self.den }
    }
}

impl Default for StreamTimeBase {
    fn default() -> Self {
        Self::HZ_1K
    }
}

/// Color metadata attached to a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ColorInfo {
    /// Sample range.
    #[cfg_attr(feature = "serde", serde(default))]
    pub range: ColorRange,
    /// YUV/RGB matrix.
    #[cfg_attr(feature = "serde", serde(default))]
    pub space: ColorSpace,
    /// Primaries.
    #[cfg_attr(feature = "serde", serde(default))]
    pub primaries: ColorPrimaries,
    /// Transfer / OETF.
    #[cfg_attr(feature = "serde", serde(default))]
    pub transfer: ColorTransfer,
}

/// Timed, located pixel surface.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSurface {
    format: PixelFormat,
    size: Size,
    timestamp: MediaTime,
    duration: Option<MediaTime>,
    time_base: StreamTimeBase,
    location: MemoryLocation,
    color: ColorInfo,
    alpha: AlphaMode,
    planes: Vec<SurfacePlane>,
}

impl VideoSurface {
    /// Packed RGB/RGBA surface from an existing [`Frame`].
    ///
    /// Uses full-range tags and `1/{timestamp.timescale}` as the stream clock.
    #[must_use]
    pub fn from_frame(frame: Frame, timestamp: MediaTime, duration: Option<MediaTime>) -> Self {
        let tb = StreamTimeBase {
            num: 1,
            den: timestamp.timescale.max(1),
        };
        Self::from_frame_with(
            frame,
            timestamp,
            duration,
            ColorInfo {
                range: ColorRange::Full,
                ..ColorInfo::default()
            },
            tb,
        )
    }

    /// Packed surface with explicit color + stream time base (file sources).
    #[must_use]
    pub fn from_frame_with(
        frame: Frame,
        timestamp: MediaTime,
        duration: Option<MediaTime>,
        color: ColorInfo,
        time_base: StreamTimeBase,
    ) -> Self {
        let format = PixelFormat::from(frame.format());
        let size = frame.size();
        let alpha = frame.alpha_mode();
        let bpp = format.packed_bytes_per_pixel().unwrap_or(3);
        let stride = usize::try_from(size.width).unwrap_or(0).saturating_mul(bpp);
        let (_, _, data) = frame.into_raw();
        let plane = SurfacePlane::new(size.width, size.height, stride, data)
            .expect("Frame dimensions produce a valid packed plane");
        Self {
            format,
            size,
            timestamp,
            duration,
            time_base,
            location: MemoryLocation::CpuPacked,
            color,
            alpha,
            planes: vec![plane],
        }
    }

    /// Build a surface from explicit planes (packed RGB or reserved planar).
    ///
    /// File `surface_at` uses this after a native YUV/`nv12` decode. Clip-graph
    /// `from_frame` still emits packed RGB. Does **not** convert pixels.
    ///
    /// # Errors
    ///
    /// Wrong plane count, subsampled size, or stride below the format minimum.
    pub fn from_planes(
        format: PixelFormat,
        size: Size,
        planes: Vec<SurfacePlane>,
        timestamp: MediaTime,
        duration: Option<MediaTime>,
        color: ColorInfo,
        time_base: StreamTimeBase,
    ) -> Result<Self> {
        validate_planes(format, size, &planes)?;
        let location = if format.packed_bytes_per_pixel().is_some() && planes.len() == 1 {
            MemoryLocation::CpuPacked
        } else {
            MemoryLocation::CpuPlanar
        };
        Ok(Self {
            format,
            size,
            timestamp,
            duration,
            time_base,
            location,
            color,
            alpha: AlphaMode::for_pixel_format(format),
            planes,
        })
    }

    /// Presentation timestamp.
    #[must_use]
    pub const fn timestamp(&self) -> MediaTime {
        self.timestamp
    }

    /// Duration of this sample, when known (PTS delta or 1/fps).
    #[must_use]
    pub const fn duration(&self) -> Option<MediaTime> {
        self.duration
    }

    /// Pixel format.
    #[must_use]
    pub const fn format(&self) -> PixelFormat {
        self.format
    }

    /// Dimensions.
    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    /// Bytes per row of plane 0 (packed RGB, or luma for planar).
    #[must_use]
    pub fn stride(&self) -> usize {
        self.planes.first().map_or(0, SurfacePlane::stride)
    }

    /// All image planes (one for packed RGB).
    #[must_use]
    pub fn planes(&self) -> &[SurfacePlane] {
        &self.planes
    }

    /// Plane `index`, if present.
    #[must_use]
    pub fn plane(&self, index: usize) -> Option<&SurfacePlane> {
        self.planes.get(index)
    }

    /// Memory location.
    #[must_use]
    pub const fn location(&self) -> MemoryLocation {
        self.location
    }

    /// Color tags.
    #[must_use]
    pub const fn color(&self) -> ColorInfo {
        self.color
    }

    /// Color-alpha tag (`Opaque` for YUV / RGB; `Straight` for RGBA by default).
    #[must_use]
    pub const fn alpha_mode(&self) -> AlphaMode {
        self.alpha
    }

    /// Stream time base from the source (or inferred from PTS timescale).
    #[must_use]
    pub const fn time_base(&self) -> StreamTimeBase {
        self.time_base
    }

    /// Packed bytes of plane 0 (may include row padding).
    #[must_use]
    pub fn data(&self) -> &[u8] {
        self.planes.first().map_or(&[], SurfacePlane::data)
    }

    /// Convert back to a clip-graph [`Frame`] (packed RGB/RGBA only).
    ///
    /// Drops per-row stride padding so [`Frame`] stays tightly packed.
    ///
    /// # Errors
    ///
    /// Not a packed RGB-family format, or buffer length mismatch.
    pub fn to_frame(&self) -> Result<Frame> {
        let fmt = FrameFormat::try_from(self.format)?;
        let plane = self
            .planes
            .first()
            .ok_or_else(|| CoreError::invalid_frame("surface has no plane 0"))?;
        let bpp = self
            .format
            .packed_bytes_per_pixel()
            .ok_or_else(|| CoreError::invalid_frame("packed Frame needs a packed pixel format"))?;
        let row = usize::try_from(self.size.width)
            .unwrap_or(0)
            .saturating_mul(bpp);
        let data = plane.compact(row)?;
        Frame::from_raw(self.size, fmt, data)?.with_alpha_mode(self.alpha)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb8;

    #[test]
    fn from_frame_sets_stride_and_pts() {
        let frame = Frame::solid_rgb(Size::new(4, 2), Rgb8::RED).unwrap();
        let ts = MediaTime::new(3000, 90_000).unwrap();
        let dur = MediaTime::new(3000, 90_000).unwrap();
        let s = VideoSurface::from_frame(frame, ts, Some(dur));
        assert_eq!(s.timestamp(), ts);
        assert_eq!(s.duration(), Some(dur));
        assert_eq!(s.stride(), 4 * 3);
        assert_eq!(s.planes().len(), 1);
        assert_eq!(s.format(), PixelFormat::Rgb8);
        assert_eq!(s.location(), MemoryLocation::CpuPacked);
        assert_eq!(s.alpha_mode(), crate::alpha::AlphaMode::Opaque);
        assert_eq!(s.color().range, ColorRange::Full);
        assert_eq!(s.time_base().den, 90_000);
        let back = s.to_frame().unwrap();
        assert_eq!(back.size(), Size::new(4, 2));
        assert_eq!(&back.data()[0..3], &[255, 0, 0]);
    }

    #[test]
    fn from_frame_with_keeps_probe_color_and_time_base() {
        let frame = Frame::solid_rgb(Size::new(2, 2), Rgb8::BLUE).unwrap();
        let color = ColorInfo {
            range: ColorRange::Limited,
            space: ColorSpace::Bt709,
            primaries: ColorPrimaries::Bt709,
            transfer: ColorTransfer::Smpte2084,
        };
        let tb = StreamTimeBase::HZ_90K;
        let s = VideoSurface::from_frame_with(
            frame,
            MediaTime::new(0, 90_000).unwrap(),
            None,
            color,
            tb,
        );
        assert_eq!(s.color(), color);
        assert_eq!(s.time_base(), tb);
    }

    #[test]
    fn planar_is_not_a_frame() {
        let err = FrameFormat::try_from(PixelFormat::Nv12).unwrap_err();
        assert!(err.to_string().contains("Nv12"));
    }
}
