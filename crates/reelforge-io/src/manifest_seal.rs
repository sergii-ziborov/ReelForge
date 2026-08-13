//! Seal a planned [`ArtifactManifest`] with on-disk file fingerprints.

use crate::error::{IoError, Result};
use reelforge_render_graph::ArtifactManifest;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::Path;

/// Hash file contents with the same engine hasher used for planned fingerprints.
///
/// # Errors
///
/// Cannot open or read the file.
pub fn fingerprint_file(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let mut file = File::open(path)
        .map_err(|e| IoError::message(format!("fingerprint open {}: {e}", path.display())))?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buf = [0_u8; 8192];
    let mut total = 0_u64;
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| IoError::message(format!("fingerprint read {}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        buf[..n].hash(&mut hasher);
    }
    total.hash(&mut hasher);
    Ok(format!("{:016x}", hasher.finish()))
}

/// Fill [`ArtifactRef::file_fingerprint`] for artifacts whose `uri` is an existing file.
///
/// Planned [`ArtifactRef::fingerprint`] is left unchanged.
///
/// # Errors
///
/// I/O while hashing a path that exists.
pub fn seal_manifest_on_disk(manifest: &mut ArtifactManifest) -> Result<()> {
    for art in manifest.outputs.iter_mut().chain(
        manifest
            .stages
            .iter_mut()
            .flat_map(|s| s.artifacts.iter_mut()),
    ) {
        let Some(uri) = art.uri.as_deref() else {
            continue;
        };
        let path = Path::new(uri);
        if !path.is_file() {
            continue;
        }
        art.file_fingerprint = Some(fingerprint_file(path)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn file_fingerprint_stable_and_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.bin");
        std::fs::File::create(&a)
            .unwrap()
            .write_all(b"hello")
            .unwrap();
        std::fs::File::create(&b)
            .unwrap()
            .write_all(b"hello")
            .unwrap();
        let fa = fingerprint_file(&a).unwrap();
        let fb = fingerprint_file(&b).unwrap();
        assert_eq!(fa, fb, "same bytes → same fingerprint");
        assert_eq!(fa, fingerprint_file(&a).unwrap());
        std::fs::File::create(&a)
            .unwrap()
            .write_all(b"hello!")
            .unwrap();
        assert_ne!(fa, fingerprint_file(&a).unwrap());
    }
}
