//! Stage / full-run cache hooks (engine side; Capture owns policy).

use crate::error::{IoError, Result};
use reelforge_render_graph::{
    CompiledOp, ExecutionPlan, RenderGraph, StageCacheKey, fingerprint_graph_run,
    fingerprint_stage, fingerprint_stage_key,
};
use sha2::{Digest, Sha256};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Directory-backed stage output cache.
///
/// Keys are hex fingerprints; values are files under `root` with a free extension
/// (typically `.mp4` for intermediate or final artifacts).
#[derive(Debug, Clone)]
pub struct StageCache {
    root: PathBuf,
}

impl StageCache {
    /// Create (and ensure) a cache directory.
    ///
    /// # Errors
    ///
    /// Cannot create the directory.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)
            .map_err(|e| IoError::message(format!("stage cache mkdir {}: {e}", root.display())))?;
        Ok(Self { root })
    }

    /// Cache root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path for a fingerprint + extension (e.g. `mp4` without dot).
    ///
    /// The filename is a hex digest of the key, so `|` and `:` in the logical
    /// key stay off the filesystem.
    #[must_use]
    pub fn path_for(&self, fingerprint: &str, ext: &str) -> PathBuf {
        let ext = ext.trim_start_matches('.');
        let name = sha256_hex(fingerprint.as_bytes());
        self.root.join(format!("{name}.{ext}"))
    }

    /// Return a cache file only when its bytes match the stored SHA-256.
    #[must_use]
    pub fn hit(&self, fingerprint: &str, ext: &str) -> Option<PathBuf> {
        let p = self.path_for(fingerprint, ext);
        validated_file(&p)
    }

    /// Copy `src` into the cache slot for `fingerprint`.
    ///
    /// The bytes land in a partial file, then replace the slot together with a
    /// SHA-256 sidecar. A reader never accepts a half-written artifact.
    ///
    /// # Errors
    ///
    /// I/O failures.
    pub fn store_copy(
        &self,
        fingerprint: &str,
        ext: &str,
        src: impl AsRef<Path>,
    ) -> Result<PathBuf> {
        let dest = self.path_for(fingerprint, ext);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| IoError::message(format!("stage cache parent: {e}")))?;
        }
        let ext = ext.trim_start_matches('.');
        let safe = sha256_hex(fingerprint.as_bytes());
        let tmp = self.root.join(format!(".{safe}.{ext}.partial"));
        let tmp_side = self.root.join(format!(".{safe}.{ext}.sha256.partial"));
        let stored = (|| {
            fs::copy(src.as_ref(), &tmp)
                .map_err(|e| IoError::message(format!("stage cache store: {e}")))?;
            let digest = hash_file(&tmp).ok_or_else(|| {
                IoError::message("stage cache store: could not hash the partial artifact")
            })?;
            fs::write(&tmp_side, format!("{digest}\n"))
                .map_err(|e| IoError::message(format!("stage cache sidecar: {e}")))?;
            replace_file(&tmp, &dest)?;
            replace_file(&tmp_side, &sidecar(&dest))?;
            Ok(dest)
        })();
        if stored.is_err() {
            let _ = fs::remove_file(&tmp);
            let _ = fs::remove_file(&tmp_side);
        }
        stored
    }

    /// Full-run fingerprint (graph + plan).
    ///
    /// # Errors
    ///
    /// Serde / graph errors.
    pub fn run_fingerprint(graph: &RenderGraph, plan: &ExecutionPlan) -> Result<String> {
        fingerprint_graph_run(graph, plan).map_err(|e| IoError::message(e.to_string()))
    }

    /// Stage-local fingerprint helper (legacy: backend + node ids only).
    #[must_use]
    pub fn stage_key(backend: &str, node_ids: &[impl AsRef<str>]) -> String {
        fingerprint_stage(backend, node_ids)
    }

    /// Strong stage key: input hash + compiled ops (id/version/params) + backend + `FFmpeg`.
    #[must_use]
    pub fn stage_key_full(
        backend: &str,
        node_ids: &[String],
        input_fingerprint: &str,
        compiled: &[CompiledOp],
        ffmpeg_version: &str,
        host_tag: &str,
    ) -> String {
        fingerprint_stage_key(&StageCacheKey {
            backend,
            node_ids,
            input_fingerprint,
            compiled,
            ffmpeg_version,
            host_tag,
        })
    }

    /// Fingerprint an intermediate `FFmpeg` filter stage.
    ///
    /// Includes source URI, source bytes, filtergraph, node ids, and host
    /// `FFmpeg` version so a tool upgrade or a replaced file does not reuse a
    /// stale intermediate.
    #[must_use]
    pub fn ffmpeg_prefix_key(source_uri: &str, vf: &str, node_ids: &[impl AsRef<str>]) -> String {
        let mut h = DefaultHasher::new();
        "ffmpeg_prefix_v3".hash(&mut h);
        source_uri.hash(&mut h);
        source_digest(source_uri).hash(&mut h);
        vf.hash(&mut h);
        for id in node_ids {
            id.as_ref().hash(&mut h);
        }
        probe_ffmpeg_version_cached().hash(&mut h);
        format!("{:016x}", h.finish())
    }

    /// Restore a cached intermediate into `dest` when present.
    ///
    /// # Errors
    ///
    /// Copy failures.
    pub fn restore_to(&self, fingerprint: &str, ext: &str, dest: impl AsRef<Path>) -> Result<bool> {
        let Some(src) = self.hit(fingerprint, ext) else {
            return Ok(false);
        };
        if let Some(parent) = dest.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|e| IoError::message(format!("stage restore mkdir: {e}")))?;
        }
        fs::copy(src, dest.as_ref())
            .map_err(|e| IoError::message(format!("stage restore copy: {e}")))?;
        Ok(true)
    }
}

/// Best-effort host `FFmpeg` version (first line), cached for the process.
#[must_use]
pub fn probe_ffmpeg_version_cached() -> &'static str {
    static VER: OnceLock<String> = OnceLock::new();
    VER.get_or_init(probe_ffmpeg_version).as_str()
}

/// SHA-256 of a source file, or `absent` when the URI is not a readable file.
#[must_use]
pub(crate) fn source_digest(uri: &str) -> String {
    hash_file(Path::new(uri)).unwrap_or_else(|| "absent".into())
}

/// SHA-256 over asset ids, URIs, and source file bytes.
#[must_use]
pub(crate) fn source_set_fingerprint<'a>(
    assets: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> String {
    let mut hasher = Sha256::new();
    for (id, uri) in assets {
        hasher.update(len_le(id));
        hasher.update(id.as_bytes());
        hasher.update(len_le(uri));
        hasher.update(uri.as_bytes());
        let digest = source_digest(uri);
        hasher.update(len_le(&digest));
        hasher.update(digest.as_bytes());
    }
    hex_encode(&hasher.finalize())
}

fn len_le(text: &str) -> [u8; 8] {
    u64::try_from(text.len()).unwrap_or(u64::MAX).to_le_bytes()
}

fn validated_file(path: &Path) -> Option<PathBuf> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    let expected = fs::read_to_string(sidecar(path)).ok()?;
    let actual = hash_file(path)?;
    if expected.trim() != actual {
        return None;
    }
    Some(path.to_path_buf())
}

fn sidecar(artifact: &Path) -> PathBuf {
    PathBuf::from(format!("{}.sha256", artifact.display()))
}

fn hash_file(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0_u8; 8192];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex_encode(&hasher.finalize()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(&Sha256::digest(bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn replace_file(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        fs::remove_file(to).map_err(|e| IoError::message(format!("stage cache replace: {e}")))?;
    }
    fs::rename(from, to).map_err(|e| IoError::message(format!("stage cache rename: {e}")))
}

fn probe_ffmpeg_version() -> String {
    let bin = std::env::var("REELFORGE_FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    let output = std::process::Command::new(&bin).arg("-version").output();
    match output {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            text.lines()
                .next()
                .unwrap_or("ffmpeg-unknown")
                .trim()
                .to_string()
        }
        _ => "ffmpeg-missing".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_hit() {
        let dir = tempfile::tempdir().unwrap();
        let cache = StageCache::open(dir.path()).unwrap();
        let src = dir.path().join("blob.bin");
        fs::write(&src, b"hello").unwrap();
        let dest = cache.store_copy("abc123", "bin", &src).unwrap();
        assert!(dest.is_file());
        assert_eq!(cache.hit("abc123", "bin").as_deref(), Some(dest.as_path()));
        assert!(cache.hit("missing", "bin").is_none());
    }

    #[test]
    fn ffmpeg_prefix_key_stable() {
        let a = StageCache::ffmpeg_prefix_key("in.mp4", "hflip", &["a", "b"]);
        let b = StageCache::ffmpeg_prefix_key("in.mp4", "hflip", &["a", "b"]);
        assert_eq!(a, b);
        let c = StageCache::ffmpeg_prefix_key("in.mp4", "vflip", &["a", "b"]);
        assert_ne!(a, c);
    }

    #[test]
    fn prefix_key_tracks_source_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.mp4");
        fs::write(&src, b"aaaa").unwrap();
        let uri = src.to_string_lossy();
        let first = StageCache::ffmpeg_prefix_key(&uri, "hflip", &["a"]);
        let again = StageCache::ffmpeg_prefix_key(&uri, "hflip", &["a"]);
        assert_eq!(first, again);
        fs::write(&src, b"bbbb").unwrap();
        let changed = StageCache::ffmpeg_prefix_key(&uri, "hflip", &["a"]);
        assert_ne!(first, changed);
    }

    #[test]
    fn corrupt_artifact_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = StageCache::open(dir.path()).unwrap();
        let src = dir.path().join("blob.bin");
        fs::write(&src, b"hello").unwrap();
        let dest = cache.store_copy("abc123", "bin", &src).unwrap();
        fs::write(&dest, b"tampered").unwrap();
        assert!(cache.hit("abc123", "bin").is_none());
    }
}
