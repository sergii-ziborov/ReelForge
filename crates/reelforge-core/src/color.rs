//! Pixel color values used by solid fills and simple draws.

/// 8-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb8 {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl Rgb8 {
    /// Pure black.
    pub const BLACK: Self = Self::new(0, 0, 0);
    /// Pure white.
    pub const WHITE: Self = Self::new(255, 255, 255);
    /// Pure red.
    pub const RED: Self = Self::new(255, 0, 0);
    /// Pure green.
    pub const GREEN: Self = Self::new(0, 255, 0);
    /// Pure blue.
    pub const BLUE: Self = Self::new(0, 0, 255);

    /// Construct an opaque RGB triple.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Lift to RGBA with full opacity.
    #[must_use]
    pub const fn with_alpha(self, a: u8) -> Rgba8 {
        Rgba8 {
            r: self.r,
            g: self.g,
            b: self.b,
            a,
        }
    }
}

/// 8-bit RGBA color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba8 {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel (`255` = opaque).
    pub a: u8,
}

impl Rgba8 {
    /// Fully transparent black.
    pub const TRANSPARENT: Self = Self::new(0, 0, 0, 0);
    /// Opaque black.
    pub const BLACK: Self = Self::new(0, 0, 0, 255);
    /// Opaque white.
    pub const WHITE: Self = Self::new(255, 255, 255, 255);

    /// Construct an RGBA quadruple.
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Drop alpha, keeping RGB.
    #[must_use]
    pub const fn rgb(self) -> Rgb8 {
        Rgb8 {
            r: self.r,
            g: self.g,
            b: self.b,
        }
    }
}

impl Default for Rgba8 {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl From<Rgb8> for Rgba8 {
    fn from(value: Rgb8) -> Self {
        value.with_alpha(255)
    }
}
