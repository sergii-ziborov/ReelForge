//! Spatial layout: frame size and placement anchors.

use crate::error::{CoreError, Result};

/// Integer pixel size of a raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Size {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Size {
    /// 720p HD — 1280×720.
    pub const HD_720: Self = Self::new(1280, 720);
    /// 1080p Full HD — 1920×1080.
    pub const HD_1080: Self = Self::new(1920, 1080);
    /// 1440p QHD — 2560×1440.
    pub const QHD: Self = Self::new(2560, 1440);
    /// 4K UHD — 3840×2160.
    pub const UHD_4K: Self = Self::new(3840, 2160);
    /// DCI 4K cinema — 4096×2160.
    pub const DCI_4K: Self = Self::new(4096, 2160);
    /// 8K UHD — 7680×4320.
    pub const UHD_8K: Self = Self::new(7680, 4320);

    /// Construct a size.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Whether both dimensions are non-zero.
    #[must_use]
    pub const fn is_positive(self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// Approximate bytes for a packed RGB8 frame (`width * height * 3`).
    #[must_use]
    pub const fn rgb8_byte_len(self) -> Option<u64> {
        self.pixel_count().checked_mul(3)
    }

    /// Validate that both dimensions are non-zero.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidSize`] when either dimension is zero.
    pub fn require_positive(self) -> Result<Self> {
        if self.is_positive() {
            Ok(self)
        } else {
            Err(CoreError::InvalidSize(self))
        }
    }

    /// Total pixel count as `u64`.
    #[must_use]
    pub const fn pixel_count(self) -> u64 {
        self.width as u64 * self.height as u64
    }

    /// Whether both dimensions are even (common codec constraint).
    #[must_use]
    pub const fn is_even(self) -> bool {
        self.width.is_multiple_of(2) && self.height.is_multiple_of(2)
    }
}

/// Named anchor used when placing a clip inside a larger canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Anchor {
    /// Top-left corner of the canvas.
    #[default]
    TopLeft,
    /// Top-center edge.
    Top,
    /// Top-right corner.
    TopRight,
    /// Middle of the left edge.
    Left,
    /// Geometric center.
    Center,
    /// Middle of the right edge.
    Right,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-center edge.
    Bottom,
    /// Bottom-right corner.
    BottomRight,
}

/// Placement of a clip's origin on a parent canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Position {
    /// Absolute pixel offset of the top-left corner of the child.
    Absolute {
        /// Horizontal offset in pixels.
        x: i32,
        /// Vertical offset in pixels.
        y: i32,
    },
    /// Place using a named anchor within the parent, plus optional pixel nudge.
    Anchored {
        /// Anchor on the parent.
        anchor: Anchor,
        /// Extra horizontal offset applied after anchoring.
        offset_x: i32,
        /// Extra vertical offset applied after anchoring.
        offset_y: i32,
    },
}

impl Position {
    /// Top-left at `(x, y)`.
    #[must_use]
    pub const fn absolute(x: i32, y: i32) -> Self {
        Self::Absolute { x, y }
    }

    /// Centered on the parent with no extra offset.
    #[must_use]
    pub const fn center() -> Self {
        Self::Anchored {
            anchor: Anchor::Center,
            offset_x: 0,
            offset_y: 0,
        }
    }

    /// Anchored placement with optional nudge.
    #[must_use]
    pub const fn anchored(anchor: Anchor, offset_x: i32, offset_y: i32) -> Self {
        Self::Anchored {
            anchor,
            offset_x,
            offset_y,
        }
    }

    /// Resolve to an absolute top-left pixel for a child of `child` on a
    /// parent of `parent`.
    #[must_use]
    pub fn resolve(self, parent: Size, child: Size) -> (i32, i32) {
        match self {
            Self::Absolute { x, y } => (x, y),
            Self::Anchored {
                anchor,
                offset_x,
                offset_y,
            } => {
                let (ax, ay) = anchor_origin(anchor, parent, child);
                (ax + offset_x, ay + offset_y)
            }
        }
    }
}

impl Default for Position {
    fn default() -> Self {
        Self::absolute(0, 0)
    }
}

fn anchor_origin(anchor: Anchor, parent: Size, child: Size) -> (i32, i32) {
    let pw = parent.width.cast_signed();
    let ph = parent.height.cast_signed();
    let cw = child.width.cast_signed();
    let ch = child.height.cast_signed();
    let cx = (pw - cw) / 2;
    let cy = (ph - ch) / 2;
    match anchor {
        Anchor::TopLeft => (0, 0),
        Anchor::Top => (cx, 0),
        Anchor::TopRight => (pw - cw, 0),
        Anchor::Left => (0, cy),
        Anchor::Center => (cx, cy),
        Anchor::Right => (pw - cw, cy),
        Anchor::BottomLeft => (0, ph - ch),
        Anchor::Bottom => (cx, ph - ch),
        Anchor::BottomRight => (pw - cw, ph - ch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_resolve() {
        let parent = Size::HD_1080;
        let child = Size::new(200, 100);
        let (x, y) = Position::center().resolve(parent, child);
        assert_eq!((x, y), (860, 490));
    }

    #[test]
    fn uhd_presets() {
        assert_eq!(Size::UHD_4K, Size::new(3840, 2160));
        assert_eq!(Size::UHD_8K, Size::new(7680, 4320));
        assert!(Size::UHD_4K.is_even());
        assert!(Size::UHD_8K.is_even());
        assert_eq!(Size::UHD_4K.rgb8_byte_len(), Some(3840 * 2160 * 3));
        assert_eq!(Size::UHD_8K.rgb8_byte_len(), Some(7680 * 4320 * 3));
    }
}
