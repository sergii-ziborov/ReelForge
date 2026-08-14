//! External / hardware surface handle (no pixels in process).

use crate::layout::Size;
use crate::surface::PixelFormat;

/// Device backend that owns an [`ExternalSurface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ExternalBackend {
    /// Unspecified host / adapter.
    #[default]
    Unknown,
    /// CUDA / NVDEC / NVENC.
    Cuda,
    /// Direct3D 11 (`d3d11va` / DXGI).
    D3d11,
    /// VA-API.
    Vaapi,
    /// Vulkan video.
    Vulkan,
    /// Metal / `VideoToolbox`.
    Metal,
}

/// Opaque GPU / hardware surface. Pixels stay on the device.
///
/// `ReelForge` does not dereference [`Self::handle`]. A GPU executor or
/// capture adapter maps it. [`crate::VideoSurface::to_rgb_frame`] fails
/// until the host resolves the handle to CPU planes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ExternalSurface {
    /// Device family.
    pub backend: ExternalBackend,
    /// Adapter-defined handle (texture id, `CUdeviceptr`, …).
    pub handle: u64,
    /// Pixel layout of the device resource.
    pub format: PixelFormat,
    /// Frame size.
    pub size: Size,
    /// Optional device / context tag.
    pub device_tag: Option<String>,
}

impl ExternalSurface {
    /// Construct a handle (no pixel validation).
    #[must_use]
    pub fn new(backend: ExternalBackend, handle: u64, format: PixelFormat, size: Size) -> Self {
        Self {
            backend,
            handle,
            format,
            size,
            device_tag: None,
        }
    }

    /// Attach a device / context tag.
    #[must_use]
    pub fn with_device_tag(mut self, tag: impl Into<String>) -> Self {
        self.device_tag = Some(tag.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_is_opaque() {
        let e = ExternalSurface::new(
            ExternalBackend::Cuda,
            0xDEAD_BEEF,
            PixelFormat::Nv12,
            Size::new(64, 64),
        )
        .with_device_tag("gpu0");
        assert_eq!(e.handle, 0xDEAD_BEEF);
        assert_eq!(e.device_tag.as_deref(), Some("gpu0"));
    }
}
