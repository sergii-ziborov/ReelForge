//! Pixel-accurate mask payloads (dense / cropped / RLE / polygon / external).
//!
//! [`crate::MaskTimeline`] remains the timed ROI view. Attach a [`MaskAsset`]
//! when the vision adapter already has a silhouette — do not collapse it to
//! an ellipse.

use crate::ids::SubjectId;
use reelforge_core::MediaTime;

/// Subject key on a materialized mask frame (same as [`SubjectId`]).
pub type SubjectKey = SubjectId;
use serde::{Deserialize, Serialize};

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
    pub fn new(
        time: MediaTime,
        subject: SubjectId,
        mask: MaskAssetRef,
        confidence: f32,
    ) -> Self {
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

    /// Decode to cropped coverage (`0..=255`). `External` returns `None`.
    #[must_use]
    pub fn to_coverage(&self) -> Option<DecodedCoverage> {
        match self {
            Self::Dense {
                width,
                height,
                data,
            } => Some(DecodedCoverage {
                left: 0,
                top: 0,
                width: *width,
                height: *height,
                data: data.clone(),
            }),
            Self::Cropped {
                left,
                top,
                width,
                height,
                data,
            } => Some(DecodedCoverage {
                left: *left,
                top: *top,
                width: *width,
                height: *height,
                data: data.clone(),
            }),
            Self::Rle {
                width,
                height,
                runs,
            } => decode_rle(*width, *height, runs),
            Self::Polygon { points } => rasterize_polygon(points),
            Self::External { .. } => None,
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

fn decode_rle(width: u32, height: u32, runs: &[(u32, u8)]) -> Option<DecodedCoverage> {
    let n = (width as usize).checked_mul(height as usize)?;
    let mut data = vec![0_u8; n];
    let mut i = 0_usize;
    for &(count, value) in runs {
        let end = i.saturating_add(count as usize).min(n);
        data[i..end].fill(value);
        i = end;
        if i >= n {
            break;
        }
    }
    Some(DecodedCoverage {
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
fn rasterize_polygon(points: &[(f32, f32)]) -> Option<DecodedCoverage> {
    if points.len() < 3 {
        return None;
    }
    let (l, t, r, b) = MaskAsset::Polygon {
        points: points.to_vec(),
    }
    .bbox()?;
    let left = l.floor().max(0.0) as u32;
    let top = t.floor().max(0.0) as u32;
    let width = ((r.ceil() - left as f32).max(1.0) as u32).min(4096);
    let height = ((b.ceil() - top as f32).max(1.0) as u32).min(4096);
    let mut data = vec![0_u8; width as usize * height as usize];
    for y in 0..height {
        for x in 0..width {
            let px = left as f32 + x as f32 + 0.5;
            let py = top as f32 + y as f32 + 0.5;
            if point_in_polygon(px, py, points) {
                data[(y * width + x) as usize] = 255;
            }
        }
    }
    Some(DecodedCoverage {
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
        let intersect = ((yi > y) != (yj > y))
            && (x < (xj - xi) * (y - yi) / (yj - yi + f32::EPSILON) + xi);
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
        assert!(a.bbox().is_none());
    }
}
