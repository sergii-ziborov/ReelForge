//! Pixel-accurate mask payloads (dense / cropped / RLE / polygon / external).
//!
//! [`crate::MaskTimeline`] remains the timed ROI view. Attach a [`MaskAsset`]
//! when the vision adapter already has a silhouette — do not collapse it to
//! an ellipse.

use crate::error::{GraphError, Result};
use crate::ids::SubjectId;
use reelforge_core::MediaTime;
use serde::{Deserialize, Serialize};

/// Subject key on a materialized mask frame (same as [`SubjectId`]).
pub type SubjectKey = SubjectId;

/// Soft / hard decode caps for mask payloads (inline JSON or package blobs).
///
/// Defaults fit 8K coverage (`8192×8192` / 64 MiB). Construct with
/// [`MaskDecodeLimits::new`] — the type is `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MaskDecodeLimits {
    /// Max coverage width.
    pub max_width: u32,
    /// Max coverage height.
    pub max_height: u32,
    /// Max decoded `width * height` bytes.
    pub max_decoded_bytes: usize,
    /// Max RLE run count.
    pub max_rle_runs: usize,
    /// Max polygon vertices.
    pub max_polygon_points: usize,
    /// When true, RLE runs must cover every pixel exactly.
    pub require_rle_complete: bool,
}

impl Default for MaskDecodeLimits {
    fn default() -> Self {
        Self::new()
    }
}

impl MaskDecodeLimits {
    /// Stock limits: 8K box, 64 MiB, 1M RLE runs, 4096 polygon points.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_width: 8192,
            max_height: 8192,
            max_decoded_bytes: 64 * 1024 * 1024,
            max_rle_runs: 1_048_576,
            max_polygon_points: 4096,
            require_rle_complete: true,
        }
    }

    /// Override the decoded-byte ceiling.
    #[must_use]
    pub const fn with_max_decoded_bytes(mut self, bytes: usize) -> Self {
        self.max_decoded_bytes = bytes;
        self
    }

    /// Override the pixel box.
    #[must_use]
    pub const fn with_max_dimensions(mut self, width: u32, height: u32) -> Self {
        self.max_width = width;
        self.max_height = height;
        self
    }
}

/// Handle or inline payload consumed by redaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaskAsset {
    /// Full-frame coverage, row-major `width * height` bytes (`0..=255`).
    Dense {
        /// Frame width.
        width: u32,
        /// Frame height.
        height: u32,
        /// Coverage bytes.
        data: Vec<u8>,
    },
    /// Tight crop of coverage (`data` is `width * height`).
    Cropped {
        /// Left origin in the source frame.
        left: u32,
        /// Top origin in the source frame.
        top: u32,
        /// Crop width.
        width: u32,
        /// Crop height.
        height: u32,
        /// Coverage bytes.
        data: Vec<u8>,
    },
    /// Run-length coverage (`count, value`) in scan order.
    Rle {
        /// Frame width.
        width: u32,
        /// Frame height.
        height: u32,
        /// Packed runs.
        runs: Vec<(u32, u8)>,
    },
    /// Closed polygon in frame pixels.
    Polygon {
        /// Vertices `(x, y)`.
        points: Vec<(f32, f32)>,
    },
    /// Mask stays in an adapter package (host resolves to pixels).
    External {
        /// Package / document id.
        package_id: String,
        /// Adapter-defined mask handle.
        mask_ref: u64,
    },
}

/// Inline asset or a store handle wrapping [`MaskAsset`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskAssetRef {
    /// Optional host store id (0 = inline only).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub id: u64,
    /// Payload.
    pub asset: MaskAsset,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// One timed, identity-tagged mask at a media instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaskFrame {
    /// Sample time.
    pub time: MediaTime,
    /// Subject key (same as [`SubjectId`]).
    pub subject: SubjectKey,
    /// Mask payload.
    pub mask: MaskAssetRef,
    /// Detector confidence `0..=1`.
    pub confidence: f32,
}

impl MaskAssetRef {
    /// Inline asset.
    #[must_use]
    pub fn inline(asset: MaskAsset) -> Self {
        Self { id: 0, asset }
    }

    /// External package handle.
    #[must_use]
    pub fn external(package_id: impl Into<String>, mask_ref: u64) -> Self {
        Self {
            id: mask_ref,
            asset: MaskAsset::External {
                package_id: package_id.into(),
                mask_ref,
            },
        }
    }
}

impl MaskFrame {
    /// Construct a frame.
    #[must_use]
    pub fn new(time: MediaTime, subject: SubjectId, mask: MaskAssetRef, confidence: f32) -> Self {
        Self {
            time,
            subject,
            mask,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

impl MaskAsset {
    /// Axis-aligned box covering the silhouette, if known without a host.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn bbox(&self) -> Option<(f32, f32, f32, f32)> {
        match self {
            Self::Dense { width, height, .. } | Self::Rle { width, height, .. } => {
                Some((0.0, 0.0, *width as f32, *height as f32))
            }
            Self::Cropped {
                left,
                top,
                width,
                height,
                ..
            } => {
                let l = *left as f32;
                let t = *top as f32;
                Some((l, t, l + *width as f32, t + *height as f32))
            }
            Self::Polygon { points } if !points.is_empty() => {
                let mut l = f32::MAX;
                let mut t = f32::MAX;
                let mut r = f32::MIN;
                let mut b = f32::MIN;
                for &(x, y) in points {
                    l = l.min(x);
                    t = t.min(y);
                    r = r.max(x);
                    b = b.max(y);
                }
                Some((l, t, r, b))
            }
            Self::Polygon { .. } | Self::External { .. } => None,
        }
    }

    /// Decode to cropped coverage (`0..=255`). `External` and invalid payloads
    /// return `None` (use [`MaskAsset::try_to_coverage`] for the reason).
    #[must_use]
    pub fn to_coverage(&self) -> Option<DecodedCoverage> {
        self.try_to_coverage().ok().flatten()
    }

    /// Decode with [`MaskDecodeLimits::new`].
    ///
    /// `External` is `Ok(None)` — the host must resolve it first. Oversized,
    /// incomplete, or malformed payloads are errors (do not allocate).
    ///
    /// # Errors
    ///
    /// Dimension / byte / RLE / polygon limit violations, or length mismatch.
    pub fn try_to_coverage(&self) -> Result<Option<DecodedCoverage>> {
        self.try_to_coverage_with(&MaskDecodeLimits::new())
    }

    /// Decode with explicit limits.
    ///
    /// # Errors
    ///
    /// Same as [`MaskAsset::try_to_coverage`].
    pub fn try_to_coverage_with(
        &self,
        limits: &MaskDecodeLimits,
    ) -> Result<Option<DecodedCoverage>> {
        match self {
            Self::Dense {
                width,
                height,
                data,
            } => decode_dense(0, 0, *width, *height, data, limits).map(Some),
            Self::Cropped {
                left,
                top,
                width,
                height,
                data,
            } => decode_dense(*left, *top, *width, *height, data, limits).map(Some),
            Self::Rle {
                width,
                height,
                runs,
            } => decode_rle(*width, *height, runs, limits).map(Some),
            Self::Polygon { points } => rasterize_polygon(points, limits).map(Some),
            Self::External { .. } => Ok(None),
        }
    }
}

/// Decoded axis-aligned coverage used by the privacy pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedCoverage {
    /// Left origin.
    pub left: u32,
    /// Top origin.
    pub top: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// `width * height` coverage bytes.
    pub data: Vec<u8>,
}

fn pixel_count(width: u32, height: u32, limits: &MaskDecodeLimits) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(GraphError::message("mask: width and height must be > 0"));
    }
    if width > limits.max_width || height > limits.max_height {
        return Err(GraphError::message(format!(
            "mask: {width}x{height} exceeds {}x{}",
            limits.max_width, limits.max_height
        )));
    }
    let n = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| GraphError::message(format!("mask: {width}x{height} overflows usize")))?;
    if n > limits.max_decoded_bytes {
        return Err(GraphError::message(format!(
            "mask: {n} decoded bytes exceeds {}",
            limits.max_decoded_bytes
        )));
    }
    Ok(n)
}

fn decode_dense(
    left: u32,
    top: u32,
    width: u32,
    height: u32,
    data: &[u8],
    limits: &MaskDecodeLimits,
) -> Result<DecodedCoverage> {
    let n = pixel_count(width, height, limits)?;
    if data.len() != n {
        return Err(GraphError::message(format!(
            "mask: payload {} bytes, expected {n}",
            data.len()
        )));
    }
    if left.checked_add(width).is_none() || top.checked_add(height).is_none() {
        return Err(GraphError::message(
            "mask: crop origin + size overflows u32",
        ));
    }
    Ok(DecodedCoverage {
        left,
        top,
        width,
        height,
        data: data.to_vec(),
    })
}

fn decode_rle(
    width: u32,
    height: u32,
    runs: &[(u32, u8)],
    limits: &MaskDecodeLimits,
) -> Result<DecodedCoverage> {
    if runs.len() > limits.max_rle_runs {
        return Err(GraphError::message(format!(
            "mask: {} RLE runs exceeds {}",
            runs.len(),
            limits.max_rle_runs
        )));
    }
    let n = pixel_count(width, height, limits)?;
    let mut covered = 0_u64;
    for &(count, _) in runs {
        covered = covered
            .checked_add(u64::from(count))
            .ok_or_else(|| GraphError::message("mask: RLE run length overflows"))?;
    }
    let need = n as u64;
    if limits.require_rle_complete && covered != need {
        return Err(GraphError::message(format!(
            "mask: RLE covers {covered} pixels, expected {need}"
        )));
    }
    if covered > need {
        return Err(GraphError::message(format!(
            "mask: RLE covers {covered} pixels, expected at most {need}"
        )));
    }
    let mut data = vec![0_u8; n];
    let mut i = 0_usize;
    for &(count, value) in runs {
        let end = i + count as usize;
        data[i..end].fill(value);
        i = end;
    }
    Ok(DecodedCoverage {
        left: 0,
        top: 0,
        width,
        height,
        data,
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn rasterize_polygon(points: &[(f32, f32)], limits: &MaskDecodeLimits) -> Result<DecodedCoverage> {
    if points.len() < 3 {
        return Err(GraphError::message("mask: polygon needs at least 3 points"));
    }
    if points.len() > limits.max_polygon_points {
        return Err(GraphError::message(format!(
            "mask: {} polygon points exceeds {}",
            points.len(),
            limits.max_polygon_points
        )));
    }
    if points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return Err(GraphError::message("mask: polygon has non-finite vertex"));
    }
    let (bbox_l, bbox_t, bbox_r, bbox_b) = MaskAsset::Polygon {
        points: points.to_vec(),
    }
    .bbox()
    .ok_or_else(|| GraphError::message("mask: polygon bbox is empty"))?;
    if ![bbox_l, bbox_t, bbox_r, bbox_b]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(GraphError::message("mask: polygon bbox is non-finite"));
    }
    let left = bbox_l.floor().max(0.0) as u32;
    let top = bbox_t.floor().max(0.0) as u32;
    let width_f = (bbox_r.ceil() - left as f32).max(1.0);
    let height_f = (bbox_b.ceil() - top as f32).max(1.0);
    if width_f > f64::from(limits.max_width) as f32
        || height_f > f64::from(limits.max_height) as f32
    {
        return Err(GraphError::message(format!(
            "mask: polygon bbox {width_f}x{height_f} exceeds {}x{}",
            limits.max_width, limits.max_height
        )));
    }
    let width = width_f as u32;
    let height = height_f as u32;
    let pixels = pixel_count(width, height, limits)?;
    let mut data = vec![0_u8; pixels];
    for y in 0..height {
        for x in 0..width {
            let px = left as f32 + x as f32 + 0.5;
            let py = top as f32 + y as f32 + 0.5;
            if point_in_polygon(px, py, points) {
                data[(y * width + x) as usize] = 255;
            }
        }
    }
    Ok(DecodedCoverage {
        left,
        top,
        width,
        height,
        data,
    })
}

fn point_in_polygon(x: f32, y: f32, pts: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        let intersect =
            ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi + f32::EPSILON) + xi);
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_roundtrip_bar() {
        let asset = MaskAsset::Rle {
            width: 4,
            height: 1,
            runs: vec![(1, 0), (2, 255), (1, 0)],
        };
        let cov = asset.to_coverage().unwrap();
        assert_eq!(cov.data, vec![0, 255, 255, 0]);
    }

    #[test]
    fn polygon_fills_square() {
        let asset = MaskAsset::Polygon {
            points: vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)],
        };
        let cov = asset.to_coverage().unwrap();
        assert!(cov.data.contains(&255));
    }

    #[test]
    fn external_has_no_pixels() {
        let a = MaskAsset::External {
            package_id: "pkg".into(),
            mask_ref: 7,
        };
        assert!(a.to_coverage().is_none());
        assert!(a.try_to_coverage().unwrap().is_none());
        assert!(a.bbox().is_none());
    }

    #[test]
    fn dense_rejects_length_mismatch() {
        let a = MaskAsset::Dense {
            width: 2,
            height: 2,
            data: vec![1, 2, 3],
        };
        assert!(a.to_coverage().is_none());
        assert!(a.try_to_coverage().is_err());
    }

    #[test]
    fn rle_rejects_incomplete() {
        let a = MaskAsset::Rle {
            width: 4,
            height: 1,
            runs: vec![(2, 255)],
        };
        let err = a.try_to_coverage().unwrap_err();
        assert!(err.to_string().contains("RLE covers 2"));
    }

    #[test]
    fn rle_rejects_too_many_runs() {
        let limits = MaskDecodeLimits::new();
        let runs = vec![(1, 1); limits.max_rle_runs + 1];
        let a = MaskAsset::Rle {
            width: 8,
            height: 8,
            runs,
        };
        assert!(
            a.try_to_coverage()
                .unwrap_err()
                .to_string()
                .contains("RLE runs")
        );
    }

    #[test]
    fn huge_dimensions_do_not_allocate() {
        let a = MaskAsset::Dense {
            width: u32::MAX,
            height: u32::MAX,
            data: vec![],
        };
        let err = a.try_to_coverage().unwrap_err();
        assert!(err.to_string().contains("exceeds"));
    }

    #[test]
    fn polygon_rejects_nan_and_too_many_points() {
        let nan = MaskAsset::Polygon {
            points: vec![(0.0, 0.0), (1.0, 0.0), (f32::NAN, 1.0)],
        };
        assert!(
            nan.try_to_coverage()
                .unwrap_err()
                .to_string()
                .contains("non-finite")
        );
        let many = MaskAsset::Polygon {
            points: vec![(0.0, 0.0); 5000],
        };
        assert!(
            many.try_to_coverage()
                .unwrap_err()
                .to_string()
                .contains("polygon points")
        );
    }
}
