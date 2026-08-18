//! On-disk SightLoom-shaped mask package (no vision crate).
//!
//! Layout:
//! ```text
//! package/
//!   manifest.json
//!   masks/7.bin    # raw u8 coverage, width*height
//! ```
//!
//! Opening a package from an agent-supplied path is a security boundary:
//! blob paths are confined to the package root, dimensions and decoded
//! bytes are capped, and optional SHA-256 hashes are verified when present.

use crate::error::{IoError, Result};
use reelforge_render_graph::{MaskAsset, MaskDecodeLimits};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Wire version for [`MaskPackageManifest`].
pub const MASK_PACKAGE_VERSION: u32 = 1;

/// Default maximum number of mask blobs in one package.
pub const MASK_PACKAGE_MAX_BLOBS: usize = 4096;

/// Package-level open / load policy.
///
/// Construct with [`MaskPackageLimits::new`]. The type is `#[non_exhaustive]`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MaskPackageLimits {
    /// Pixel / RLE / polygon decode caps.
    pub decode: MaskDecodeLimits,
    /// Max entries in `manifest.masks`.
    pub max_blobs: usize,
    /// Require every blob to declare a matching `content_hash`.
    pub require_content_hash: bool,
    /// Require the manifest `package_hash` and verify it.
    pub require_package_hash: bool,
    /// If set, `manifest.source_hash` must match (after normalizing).
    pub expected_source_hash: Option<String>,
}

impl Default for MaskPackageLimits {
    fn default() -> Self {
        Self::new()
    }
}

impl MaskPackageLimits {
    /// Stock policy: decode caps on, hashes verified when present.
    #[must_use]
    pub fn new() -> Self {
        Self {
            decode: MaskDecodeLimits::new(),
            max_blobs: MASK_PACKAGE_MAX_BLOBS,
            require_content_hash: false,
            require_package_hash: false,
            expected_source_hash: None,
        }
    }

    /// Fail closed: hashes must be present and match.
    #[must_use]
    pub fn require_hashes(mut self) -> Self {
        self.require_content_hash = true;
        self.require_package_hash = true;
        self
    }

    /// Expect a source-image SHA-256 (hex, optional `sha256:` prefix).
    #[must_use]
    pub fn with_expected_source_hash(mut self, hash: impl Into<String>) -> Self {
        self.expected_source_hash = Some(hash.into());
        self
    }

    /// Override decode caps.
    #[must_use]
    pub fn with_decode(mut self, decode: MaskDecodeLimits) -> Self {
        self.decode = decode;
        self
    }
}

/// Package root document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// SHA-256 of id + source metadata + blob content hashes (excludes itself).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_hash: Option<String>,
    /// Source frame width the masks were generated from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_width: Option<u32>,
    /// Source frame height the masks were generated from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_height: Option<u32>,
    /// SHA-256 of the source image / key frame (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
}

fn default_version() -> u32 {
    MASK_PACKAGE_VERSION
}

impl MaskPackageManifest {
    /// Empty v1 package.
    #[must_use]
    pub fn new(package_id: impl Into<String>) -> Self {
        Self {
            version: MASK_PACKAGE_VERSION,
            package_id: package_id.into(),
            tracks: Vec::new(),
            masks: Vec::new(),
            package_hash: None,
            source_width: None,
            source_height: None,
            source_hash: None,
        }
    }
}

/// One coverage blob on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
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
    /// SHA-256 of the blob bytes (hex, optional `sha256:` prefix).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
}

impl MaskBlobEntry {
    /// Dense / cropped blob descriptor (no hash).
    #[must_use]
    pub fn new(mask_ref: u64, width: u32, height: u32, path: impl Into<String>) -> Self {
        Self {
            mask_ref,
            kind: String::new(),
            left: 0,
            top: 0,
            width,
            height,
            path: path.into(),
            content_hash: None,
        }
    }
}

/// Loaded package: manifest + blob index.
#[derive(Debug, Clone)]
pub struct MaskPackage {
    /// Canonical package directory.
    pub root: PathBuf,
    /// Manifest.
    pub manifest: MaskPackageManifest,
    /// Limits used for this open.
    pub limits: MaskPackageLimits,
    index: BTreeMap<u64, MaskBlobEntry>,
}

impl MaskPackage {
    /// Open `dir/manifest.json` with [`MaskPackageLimits::new`].
    ///
    /// # Errors
    ///
    /// I/O, JSON, version, path escape, or limit violations.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(dir, MaskPackageLimits::new())
    }

    /// Open with an explicit policy.
    ///
    /// # Errors
    ///
    /// Same as [`MaskPackage::open`].
    pub fn open_with(dir: impl AsRef<Path>, limits: MaskPackageLimits) -> Result<Self> {
        let root = canonicalize_dir(dir.as_ref())?;
        let path = root.join("manifest.json");
        let text = fs::read_to_string(&path)
            .map_err(|e| IoError::message(format!("mask package read {}: {e}", path.display())))?;
        let mut manifest: MaskPackageManifest = serde_json::from_str(&text)
            .map_err(|e| IoError::message(format!("mask package parse: {e}")))?;
        if manifest.version == 0 {
            manifest.version = MASK_PACKAGE_VERSION;
        }
        if manifest.version > MASK_PACKAGE_VERSION {
            return Err(IoError::message(format!(
                "mask package version {} is newer than {MASK_PACKAGE_VERSION}",
                manifest.version
            )));
        }
        if manifest.package_id.trim().is_empty() {
            return Err(IoError::message("mask package: empty package_id"));
        }
        if manifest.masks.len() > limits.max_blobs {
            return Err(IoError::message(format!(
                "mask package: {} blobs exceeds {}",
                manifest.masks.len(),
                limits.max_blobs
            )));
        }
        if let Some(expected) = limits.expected_source_hash.as_deref() {
            let Some(got) = manifest.source_hash.as_deref() else {
                return Err(IoError::message(
                    "mask package: source_hash required by policy",
                ));
            };
            if !hashes_eq(got, expected) {
                return Err(IoError::message("mask package: source_hash mismatch"));
            }
        }
        let mut index = BTreeMap::new();
        for blob in &manifest.masks {
            validate_blob_meta(blob, &manifest, &limits)?;
            confine_path(&root, &blob.path)?;
            if index.insert(blob.mask_ref, blob.clone()).is_some() {
                return Err(IoError::message(format!(
                    "mask package: duplicate mask_ref {}",
                    blob.mask_ref
                )));
            }
        }
        let pkg = Self {
            root,
            manifest,
            limits,
            index,
        };
        pkg.verify_hashes()?;
        Ok(pkg)
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
    /// Missing blob, I/O, hash mismatch, or size mismatch.
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

    /// SHA-256 of `bytes` as lowercase hex.
    #[must_use]
    pub fn hash_bytes(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }

    /// Recompute the package hash from blob files on disk.
    ///
    /// # Errors
    ///
    /// I/O while reading blobs.
    pub fn compute_package_hash(&self) -> Result<String> {
        compute_package_hash(&self.root, &self.manifest)
    }

    fn verify_hashes(&self) -> Result<()> {
        if self.limits.require_package_hash && self.manifest.package_hash.is_none() {
            return Err(IoError::message(
                "mask package: package_hash required by policy",
            ));
        }
        for blob in &self.manifest.masks {
            if self.limits.require_content_hash && blob.content_hash.is_none() {
                return Err(IoError::message(format!(
                    "mask blob {}: content_hash required by policy",
                    blob.mask_ref
                )));
            }
            if let Some(declared) = blob.content_hash.as_deref() {
                let path = confine_path(&self.root, &blob.path)?;
                let bytes = fs::read(&path)
                    .map_err(|e| IoError::message(format!("mask blob {}: {e}", path.display())))?;
                if !hashes_eq(declared, &sha256_hex(&bytes)) {
                    return Err(IoError::message(format!(
                        "mask blob {}: content_hash mismatch",
                        path.display()
                    )));
                }
            }
        }
        if let Some(declared) = self.manifest.package_hash.as_deref() {
            let got = compute_package_hash(&self.root, &self.manifest)?;
            if !hashes_eq(declared, &got) {
                return Err(IoError::message("mask package: package_hash mismatch"));
            }
        }
        Ok(())
    }

    fn load_entry(&self, entry: &MaskBlobEntry) -> Result<MaskAsset> {
        let path = confine_path(&self.root, &entry.path)?;
        let meta = fs::metadata(&path)
            .map_err(|e| IoError::message(format!("mask blob {}: {e}", path.display())))?;
        let need = pixel_need(entry.width, entry.height, &self.limits.decode)?;
        if meta.len() != need as u64 {
            return Err(IoError::message(format!(
                "mask blob {} length {}, expected {need}",
                path.display(),
                meta.len()
            )));
        }
        let data = fs::read(&path)
            .map_err(|e| IoError::message(format!("mask blob {}: {e}", path.display())))?;
        if data.len() != need {
            return Err(IoError::message(format!(
                "mask blob {} length {}, expected {need}",
                path.display(),
                data.len()
            )));
        }
        let digest = sha256_hex(&data);
        if let Some(declared) = entry.content_hash.as_deref() {
            if !hashes_eq(declared, &digest) {
                return Err(IoError::message(format!(
                    "mask blob {}: content_hash mismatch",
                    path.display()
                )));
            }
        } else if self.limits.require_content_hash {
            return Err(IoError::message(format!(
                "mask blob {}: content_hash required by policy",
                path.display()
            )));
        }
        let cropped = entry.kind == "cropped" || entry.left != 0 || entry.top != 0;
        let asset = if cropped {
            MaskAsset::Cropped {
                left: entry.left,
                top: entry.top,
                width: entry.width,
                height: entry.height,
                data,
            }
        } else {
            MaskAsset::Dense {
                width: entry.width,
                height: entry.height,
                data,
            }
        };
        asset
            .try_to_coverage_with(&self.limits.decode)
            .map_err(|e| IoError::message(e.to_string()))?;
        Ok(asset)
    }
}

fn validate_blob_meta(
    entry: &MaskBlobEntry,
    manifest: &MaskPackageManifest,
    limits: &MaskPackageLimits,
) -> Result<()> {
    pixel_need(entry.width, entry.height, &limits.decode)?;
    let right = entry
        .left
        .checked_add(entry.width)
        .ok_or_else(|| IoError::message("mask blob: left+width overflows"))?;
    let bottom = entry
        .top
        .checked_add(entry.height)
        .ok_or_else(|| IoError::message("mask blob: top+height overflows"))?;
    if let (Some(sw), Some(sh)) = (manifest.source_width, manifest.source_height)
        && (right > sw || bottom > sh)
    {
        return Err(IoError::message(format!(
            "mask blob {} crop {right}x{bottom} exceeds source {sw}x{sh}",
            entry.mask_ref
        )));
    }
    Ok(())
}

fn pixel_need(width: u32, height: u32, limits: &MaskDecodeLimits) -> Result<usize> {
    if width == 0 || height == 0 {
        return Err(IoError::message("mask blob: width and height must be > 0"));
    }
    if width > limits.max_width || height > limits.max_height {
        return Err(IoError::message(format!(
            "mask blob: {width}x{height} exceeds {}x{}",
            limits.max_width, limits.max_height
        )));
    }
    let n = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| IoError::message("mask blob: dimension overflow"))?;
    if n > limits.max_decoded_bytes {
        return Err(IoError::message(format!(
            "mask blob: {n} bytes exceeds {}",
            limits.max_decoded_bytes
        )));
    }
    Ok(n)
}

fn canonicalize_dir(dir: &Path) -> Result<PathBuf> {
    let meta = fs::metadata(dir)
        .map_err(|e| IoError::message(format!("mask package {}: {e}", dir.display())))?;
    if !meta.is_dir() {
        return Err(IoError::message(format!(
            "mask package {}: not a directory",
            dir.display()
        )));
    }
    fs::canonicalize(dir)
        .map_err(|e| IoError::message(format!("mask package canonicalize {}: {e}", dir.display())))
}

/// Resolve `relative` under `root`. Rejects `..`, absolute paths, and escapes.
fn confine_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty() || relative.contains('\0') {
        return Err(IoError::message("mask blob path is empty or contains NUL"));
    }
    let declared = Path::new(relative);
    if declared.is_absolute() {
        return Err(IoError::message(format!(
            "mask blob path `{relative}` must be relative to the package"
        )));
    }
    for component in declared.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(IoError::message(format!(
                    "mask blob path `{relative}` escapes the package"
                )));
            }
        }
    }
    let joined = root.join(declared);
    let canon = fs::canonicalize(&joined).map_err(|e| {
        IoError::message(format!("mask blob canonicalize {}: {e}", joined.display()))
    })?;
    if !canon.starts_with(root) {
        return Err(IoError::message(format!(
            "mask blob path `{relative}` escapes {}",
            root.display()
        )));
    }
    Ok(canon)
}

fn compute_package_hash(root: &Path, manifest: &MaskPackageManifest) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"reelforge-mask-package-v1\n");
    hasher.update(manifest.package_id.as_bytes());
    hasher.update(b"\n");
    write_opt_u32(&mut hasher, manifest.source_width);
    write_opt_u32(&mut hasher, manifest.source_height);
    hasher.update(normalize_hash(manifest.source_hash.as_deref()).as_bytes());
    hasher.update(b"\n");
    let mut blobs = manifest.masks.clone();
    blobs.sort_by_key(|b| b.mask_ref);
    for blob in &blobs {
        let path = confine_path(root, &blob.path)?;
        let bytes = fs::read(&path)
            .map_err(|e| IoError::message(format!("mask hash {}: {e}", path.display())))?;
        let content = sha256_hex(&bytes);
        hasher.update(blob.mask_ref.to_le_bytes());
        hasher.update(blob.kind.as_bytes());
        hasher.update(b"\0");
        hasher.update(blob.left.to_le_bytes());
        hasher.update(blob.top.to_le_bytes());
        hasher.update(blob.width.to_le_bytes());
        hasher.update(blob.height.to_le_bytes());
        hasher.update(blob.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex_of(&hasher.finalize()))
}

fn write_opt_u32(hasher: &mut Sha256, value: Option<u32>) {
    match value {
        Some(v) => {
            hasher.update([1_u8]);
            hasher.update(v.to_le_bytes());
        }
        None => hasher.update([0_u8]),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_of(&Sha256::digest(bytes))
}

fn hex_of(digest: &[u8]) -> String {
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn normalize_hash(value: Option<&str>) -> String {
    value
        .map(|v| {
            v.trim()
                .strip_prefix("sha256:")
                .unwrap_or(v)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

fn hashes_eq(a: &str, b: &str) -> bool {
    normalize_hash(Some(a)) == normalize_hash(Some(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pkg(dir: &Path, manifest: &serde_json::Value) {
        fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    }

    #[test]
    fn open_and_load_cropped() {
        let dir = tempfile::tempdir().unwrap();
        let masks = dir.path().join("masks");
        fs::create_dir_all(&masks).unwrap();
        fs::write(masks.join("7.bin"), [255_u8, 0, 0, 255]).unwrap();
        let digest = sha256_hex(&[255, 0, 0, 255]);
        let manifest = serde_json::json!({
            "package_id": "pkg-a",
            "source_width": 8,
            "source_height": 8,
            "tracks": [],
            "masks": [{
                "mask_ref": 7,
                "kind": "cropped",
                "left": 2, "top": 3,
                "width": 2, "height": 2,
                "path": "masks/7.bin",
                "content_hash": digest
            }]
        });
        write_pkg(dir.path(), &manifest);
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

    #[test]
    fn rejects_parent_dir_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().parent().unwrap().join("escape.bin");
        fs::write(&outside, [1_u8, 2, 3, 4]).unwrap();
        fs::create_dir_all(dir.path().join("masks")).unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-x",
                "masks": [{
                    "mask_ref": 1,
                    "width": 2, "height": 2,
                    "path": "../escape.bin"
                }]
            }),
        );
        let err = MaskPackage::open(dir.path()).unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");
        let _ = fs::remove_file(outside);
    }

    #[test]
    fn rejects_absolute_blob_path() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join("masks");
        fs::create_dir_all(&abs).unwrap();
        fs::write(abs.join("1.bin"), [1_u8, 2, 3, 4]).unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-abs",
                "masks": [{
                    "mask_ref": 1,
                    "width": 2, "height": 2,
                    "path": abs.join("1.bin").to_string_lossy()
                }]
            }),
        );
        let err = MaskPackage::open(dir.path()).unwrap_err();
        assert!(err.to_string().contains("relative"), "{err}");
    }

    #[test]
    fn rejects_future_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "version": 99,
                "package_id": "pkg-v",
                "masks": []
            }),
        );
        let err = MaskPackage::open(dir.path()).unwrap_err();
        assert!(err.to_string().contains("version 99"), "{err}");
    }

    #[test]
    fn rejects_oversized_blob_claim() {
        let dir = tempfile::tempdir().unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-big",
                "masks": [{
                    "mask_ref": 1,
                    "width": 20000, "height": 20000,
                    "path": "masks/1.bin"
                }]
            }),
        );
        let err = MaskPackage::open(dir.path()).unwrap_err();
        assert!(err.to_string().contains("exceeds"), "{err}");
    }

    #[test]
    fn rejects_crop_outside_source() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("masks")).unwrap();
        fs::write(dir.path().join("masks/1.bin"), [1_u8, 2, 3, 4]).unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-src",
                "source_width": 4,
                "source_height": 4,
                "masks": [{
                    "mask_ref": 1,
                    "kind": "cropped",
                    "left": 3, "top": 3,
                    "width": 2, "height": 2,
                    "path": "masks/1.bin"
                }]
            }),
        );
        let err = MaskPackage::open(dir.path()).unwrap_err();
        assert!(err.to_string().contains("exceeds source"), "{err}");
    }

    #[test]
    fn content_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("masks")).unwrap();
        fs::write(dir.path().join("masks/1.bin"), [9_u8, 9, 9, 9]).unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-h",
                "masks": [{
                    "mask_ref": 1,
                    "width": 2, "height": 2,
                    "path": "masks/1.bin",
                    "content_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }]
            }),
        );
        let err = MaskPackage::open(dir.path()).unwrap_err();
        assert!(err.to_string().contains("content_hash"), "{err}");
    }

    #[test]
    fn package_hash_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("masks")).unwrap();
        fs::write(dir.path().join("masks/1.bin"), [1_u8, 2, 3, 4]).unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-ph",
                "source_width": 2,
                "source_height": 2,
                "masks": [{
                    "mask_ref": 1,
                    "width": 2, "height": 2,
                    "path": "masks/1.bin"
                }]
            }),
        );
        let tmp = MaskPackage::open(dir.path()).unwrap();
        let hash = tmp.compute_package_hash().unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-ph",
                "package_hash": hash,
                "source_width": 2,
                "source_height": 2,
                "masks": [{
                    "mask_ref": 1,
                    "width": 2, "height": 2,
                    "path": "masks/1.bin"
                }]
            }),
        );
        let mut check = MaskPackageLimits::new();
        check.require_package_hash = true;
        let pkg = MaskPackage::open_with(dir.path(), check.clone()).unwrap();
        assert!(hashes_eq(
            pkg.manifest.package_hash.as_deref().unwrap(),
            &hash
        ));
        fs::write(dir.path().join("masks/1.bin"), [0_u8, 0, 0, 0]).unwrap();
        let err = MaskPackage::open_with(dir.path(), check).unwrap_err();
        assert!(err.to_string().contains("package_hash"), "{err}");
    }

    #[test]
    fn expected_source_hash_must_match() {
        let dir = tempfile::tempdir().unwrap();
        write_pkg(
            dir.path(),
            &serde_json::json!({
                "package_id": "pkg-sh",
                "source_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "masks": []
            }),
        );
        let err = MaskPackage::open_with(
            dir.path(),
            MaskPackageLimits::new().with_expected_source_hash(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        )
        .unwrap_err();
        assert!(err.to_string().contains("source_hash"), "{err}");
        MaskPackage::open_with(
            dir.path(),
            MaskPackageLimits::new().with_expected_source_hash(
                "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            ),
        )
        .unwrap();
    }
}
