//! On-disk SightLoom-shaped mask package (no vision crate).
//!
//! Layout:
//! ```text
//! package/
//!   manifest.json
//!   masks/7.bin    # raw u8 coverage, width*height
//! ```

use crate::error::{IoError, Result};
use reelforge_render_graph::MaskAsset;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Wire version for [`MaskPackageManifest`].
pub const MASK_PACKAGE_VERSION: u32 = 1;

/// Package root document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaskPackageManifest {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Package id (`MaskAsset::External.package_id`).
    pub package_id: String,
    /// Tracks document (same shape as the `SightLoom` JSON adapter).
    #[serde(default)]
    pub tracks: Vec<reelforge_sightloom_adapter::TrackEntry>,
    /// Pixel blobs.
    #[serde(default)]
    pub masks: Vec<MaskBlobEntry>,
}

fn default_version() -> u32 {
    MASK_PACKAGE_VERSION
}

/// One coverage blob on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaskBlobEntry {
    /// Handle used in [`MaskAsset::External`].
    pub mask_ref: u64,
    /// `dense` or `cropped` (non-zero origin implies cropped).
    #[serde(default)]
    pub kind: String,
    /// Crop left.
    #[serde(default)]
    pub left: u32,
    /// Crop top.
    #[serde(default)]
    pub top: u32,
    /// Coverage width.
    pub width: u32,
    /// Coverage height.
    pub height: u32,
    /// Path relative to the package root.
    pub path: String,
}

/// Loaded package: manifest + blob index.
#[derive(Debug, Clone)]
pub struct MaskPackage {
    /// Absolute package directory.
    pub root: PathBuf,
    /// Manifest.
    pub manifest: MaskPackageManifest,
    index: BTreeMap<u64, MaskBlobEntry>,
}

impl MaskPackage {
    /// Open `dir/manifest.json`.
    ///
    /// # Errors
    ///
    /// I/O or JSON.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let root = dir.as_ref().to_path_buf();
        let path = root.join("manifest.json");
        let text = fs::read_to_string(&path)
            .map_err(|e| IoError::message(format!("mask package read {}: {e}", path.display())))?;
        let manifest: MaskPackageManifest = serde_json::from_str(&text)
            .map_err(|e| IoError::message(format!("mask package parse: {e}")))?;
        let mut index = BTreeMap::new();
        for blob in &manifest.masks {
            index.insert(blob.mask_ref, blob.clone());
        }
        Ok(Self {
            root,
            manifest,
            index,
        })
    }

    /// Lookup a blob by handle.
    #[must_use]
    pub fn blob(&self, mask_ref: u64) -> Option<&MaskBlobEntry> {
        self.index.get(&mask_ref)
    }

    /// Load pixels for `mask_ref`.
    ///
    /// # Errors
    ///
    /// Missing blob, I/O, or size mismatch.
    pub fn load_asset(&self, mask_ref: u64) -> Result<MaskAsset> {
        let entry = self.blob(mask_ref).ok_or_else(|| {
            IoError::message(format!(
                "mask package {}: unknown mask_ref {mask_ref}",
                self.manifest.package_id
            ))
        })?;
        self.load_entry(entry)
    }

    /// Resolve an external handle when it belongs to this package.
    ///
    /// # Errors
    ///
    /// I/O when the blob exists but cannot be read.
    pub fn resolve_external(&self, asset: &MaskAsset) -> Result<Option<MaskAsset>> {
        let MaskAsset::External {
            package_id,
            mask_ref,
        } = asset
        else {
            return Ok(None);
        };
        if !package_id.is_empty()
            && package_id.as_str() != self.manifest.package_id
            && !self.index.values().any(|e| e.path == *package_id)
        {
            return Ok(None);
        }
        if self.index.contains_key(mask_ref) {
            return Ok(Some(self.load_asset(*mask_ref)?));
        }
        if let Some(entry) = self.index.values().find(|e| e.path == *package_id) {
            return Ok(Some(self.load_entry(entry)?));
        }
        Ok(None)
    }

    fn load_entry(&self, entry: &MaskBlobEntry) -> Result<MaskAsset> {
        let path = self.root.join(&entry.path);
        let data = fs::read(&path)
            .map_err(|e| IoError::message(format!("mask blob {}: {e}", path.display())))?;
        let need = (entry.width as usize).saturating_mul(entry.height as usize);
        if data.len() != need {
            return Err(IoError::message(format!(
                "mask blob {} length {}, expected {need}",
                path.display(),
                data.len()
            )));
        }
        let cropped = entry.kind == "cropped" || entry.left != 0 || entry.top != 0;
        if cropped {
            Ok(MaskAsset::Cropped {
                left: entry.left,
                top: entry.top,
                width: entry.width,
                height: entry.height,
                data,
            })
        } else {
            Ok(MaskAsset::Dense {
                width: entry.width,
                height: entry.height,
                data,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_and_load_cropped() {
        let dir = tempfile::tempdir().unwrap();
        let masks = dir.path().join("masks");
        fs::create_dir_all(&masks).unwrap();
        fs::write(masks.join("7.bin"), [255_u8, 0, 0, 255]).unwrap();
        let manifest = serde_json::json!({
            "package_id": "pkg-a",
            "tracks": [],
            "masks": [{
                "mask_ref": 7,
                "kind": "cropped",
                "left": 2, "top": 3,
                "width": 2, "height": 2,
                "path": "masks/7.bin"
            }]
        });
        fs::write(dir.path().join("manifest.json"), manifest.to_string()).unwrap();
        let pkg = MaskPackage::open(dir.path()).unwrap();
        let asset = pkg.load_asset(7).unwrap();
        match asset {
            MaskAsset::Cropped {
                left,
                top,
                width,
                height,
                data,
            } => {
                assert_eq!((left, top, width, height), (2, 3, 2, 2));
                assert_eq!(data, vec![255, 0, 0, 255]);
            }
            other => panic!("{other:?}"),
        }
    }
}
