use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn digest_json<T: Serialize>(value: &T) -> Result<String> {
    struct HashWriter(Sha256);
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(format!("{:x}", writer.0.finalize()))
}
pub(crate) fn is_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|x| x.is_ascii_digit() || (b'a'..=b'f').contains(&x))
}
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Invalid("stored artifact exceeds byte budget".into()));
    }
    Ok(bytes)
}

/// Immutable frame pixels owned independently of a model/agent temporary cache.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameReceipt {
    /// Absolute, content-addressed PNG path.
    pub path: PathBuf,
    /// Exact encoded-byte fingerprint.
    pub sha256: String,
    /// Decoded RGBA fingerprint, to detect holds despite re-encoded metadata.
    pub pixels_sha256: String,
    /// Canvas width.
    pub width: u32,
    /// Canvas height.
    pub height: u32,
}

/// Owned pixel and request storage. Original keyframes are never overwritten.
#[derive(Clone, Debug)]
pub struct FrameStore {
    pub(crate) root: PathBuf,
}
impl FrameStore {
    /// Open/create a storage root; model dependencies are kept elsewhere.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        std::fs::create_dir_all(root.join("frames"))?;
        std::fs::create_dir_all(root.join("requests"))?;
        Ok(Self { root })
    }
    /// Absolute owned storage location.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn decode(bytes: &[u8]) -> Result<image::RgbaImage> {
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(Error::Invalid("frame byte budget".into()));
        }
        let reader =
            image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
        let (w, h) = reader.into_dimensions()?;
        if w == 0 || h == 0 || w > 4096 || h > 4096 {
            return Err(Error::Invalid("frame canvas budget".into()));
        }
        Ok(image::load_from_memory_with_format(bytes, image::ImageFormat::Png)?.to_rgba8())
    }
    /// Preserve a provider's PNG bytes and derive independently checked pixel metadata.
    pub fn ingest_png(&self, bytes: &[u8]) -> Result<FrameReceipt> {
        let rgba = Self::decode(bytes)?;
        let hash = digest(bytes);
        let path = self.root.join("frames").join(format!("{hash}.png"));
        if !path.exists() {
            match Self::write_new(&path, bytes) {
                Ok(()) => {}
                Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        let receipt = FrameReceipt {
            path,
            sha256: hash,
            pixels_sha256: digest(rgba.as_raw()),
            width: rgba.width(),
            height: rgba.height(),
        };
        self.verify(&receipt)?;
        Ok(receipt)
    }
    /// Verify receipt ownership, canvas, encoded bytes and decoded pixels.
    pub fn verify(&self, r: &FrameReceipt) -> Result<()> {
        self.load(r).map(|_| ())
    }
    pub(crate) fn load(&self, r: &FrameReceipt) -> Result<image::RgbaImage> {
        if !is_digest(&r.sha256) || !is_digest(&r.pixels_sha256) {
            return Err(Error::Invalid("invalid frame digest".into()));
        }
        let expected = self.root.join("frames").join(format!("{}.png", r.sha256));
        if r.path != expected || r.path.canonicalize()? != expected {
            return Err(Error::Invalid("frame is not owned by this store".into()));
        }
        let bytes = read_bounded(&r.path, 64 * 1024 * 1024)?;
        if digest(&bytes) != r.sha256 {
            return Err(Error::Invalid("frame fingerprint changed".into()));
        }
        let image = Self::decode(&bytes)?;
        if image.dimensions() != (r.width, r.height) || digest(image.as_raw()) != r.pixels_sha256 {
            return Err(Error::Invalid("frame pixel metadata changed".into()));
        }
        Ok(image)
    }
    pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::Invalid("artifact has no parent".into()))?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
        tmp.write_all(bytes)?;
        tmp.as_file().sync_all()?;
        tmp.persist_noclobber(path)
            .map_err(|e| Error::Io(e.error))?;
        Ok(())
    }
    pub(crate) fn save_new<T: Serialize>(path: &Path, value: &T) -> Result<()> {
        Self::write_new(path, &serde_json::to_vec(value)?)
    }
}
