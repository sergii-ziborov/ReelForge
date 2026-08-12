//! Stage / full-run cache hooks (engine side; Capture owns policy).

use crate::error::{IoError, Result};
use reelforge_render_graph::{
    ExecutionPlan, RenderGraph, fingerprint_graph_run, fingerprint_stage,
};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

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
    #[must_use]
    pub fn path_for(&self, fingerprint: &str, ext: &str) -> PathBuf {
        let ext = ext.trim_start_matches('.');
        self.root.join(format!("{fingerprint}.{ext}"))
    }

    /// Return existing cache file when present and non-empty.
    #[must_use]
    pub fn hit(&self, fingerprint: &str, ext: &str) -> Option<PathBuf> {
        let p = self.path_for(fingerprint, ext);
        match fs::metadata(&p) {
            Ok(m) if m.is_file() && m.len() > 0 => Some(p),
            _ => None,
        }
    }

    /// Copy `src` into the cache slot for `fingerprint`.
    ///
    /// # Errors
    ///
    /// I/O failures.
    pub fn store_copy(&self, fingerprint: &str, ext: &str, src: impl AsRef<Path>) -> Result<PathBuf> {
        let dest = self.path_for(fingerprint, ext);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| IoError::message(format!("stage cache parent: {e}")))?;
        }
        fs::copy(src.as_ref(), &dest)
            .map_err(|e| IoError::message(format!("stage cache store: {e}")))?;
        Ok(dest)
    }

    /// Full-run fingerprint (graph + plan).
    ///
    /// # Errors
    ///
    /// Serde / graph errors.
    pub fn run_fingerprint(graph: &RenderGraph, plan: &ExecutionPlan) -> Result<String> {
        fingerprint_graph_run(graph, plan).map_err(|e| IoError::message(e.to_string()))
    }

    /// Stage-local fingerprint helper.
    #[must_use]
    pub fn stage_key(backend: &str, node_ids: &[impl AsRef<str>]) -> String {
        fingerprint_stage(backend, node_ids)
    }

    /// Fingerprint an intermediate `FFmpeg` filter stage (source URI + vf + node ids).
    #[must_use]
    pub fn ffmpeg_prefix_key(source_uri: &str, vf: &str, node_ids: &[impl AsRef<str>]) -> String {
        let mut h = DefaultHasher::new();
        "ffmpeg_prefix".hash(&mut h);
        source_uri.hash(&mut h);
        vf.hash(&mut h);
        for id in node_ids {
            id.as_ref().hash(&mut h);
        }
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
}
