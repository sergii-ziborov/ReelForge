//! Locate `ffmpeg` and `ffprobe` binaries.

use crate::error::{IoError, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Resolved paths to the CLI tools.
#[derive(Debug, Clone)]
pub struct FfmpegTools {
    /// `ffmpeg` executable.
    pub ffmpeg: PathBuf,
    /// `ffprobe` executable.
    pub ffprobe: PathBuf,
}

static TOOLS: OnceLock<std::result::Result<FfmpegTools, String>> = OnceLock::new();

impl FfmpegTools {
    /// Discover tools from the environment and `PATH`.
    ///
    /// Search order:
    /// 1. `REELFORGE_FFMPEG` / `REELFORGE_FFPROBE`
    /// 2. `ffmpeg` / `ffprobe` on `PATH`
    ///
    /// # Errors
    ///
    /// Returns [`IoError::ToolsNotFound`] when either binary is missing.
    pub fn discover() -> Result<Self> {
        match TOOLS.get_or_init(discover_inner) {
            Ok(tools) => Ok(tools.clone()),
            Err(msg) => Err(IoError::ToolsNotFound(msg.clone())),
        }
    }
}

/// Whether both tools resolve successfully.
#[must_use]
pub fn ffmpeg_available() -> bool {
    FfmpegTools::discover().is_ok()
}

fn discover_inner() -> std::result::Result<FfmpegTools, String> {
    let ffmpeg = resolve_tool("REELFORGE_FFMPEG", "ffmpeg")?;
    let ffprobe = resolve_tool("REELFORGE_FFPROBE", "ffprobe")?;
    // Sanity: binaries run and print a version line.
    ensure_runs(&ffmpeg, "ffmpeg")?;
    ensure_runs(&ffprobe, "ffprobe")?;
    Ok(FfmpegTools { ffmpeg, ffprobe })
}

fn resolve_tool(env_key: &str, name: &str) -> std::result::Result<PathBuf, String> {
    if let Ok(path) = std::env::var(env_key) {
        let p = PathBuf::from(path);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("{env_key}={} is not a file", p.display()));
    }
    which(name)
        .ok_or_else(|| format!("`{name}` not found on PATH (set {env_key} or install FFmpeg)"))
}

fn which(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        let with_exe = dir.join(format!("{name}.exe"));
        if with_exe.is_file() {
            return Some(with_exe);
        }
    }
    None
}

fn ensure_runs(bin: &Path, label: &str) -> std::result::Result<(), String> {
    let output = Command::new(bin)
        .arg("-version")
        .output()
        .map_err(|e| format!("failed to spawn {label} at {}: {e}", bin.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{label} at {} exited with {}",
            bin.display(),
            output.status
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_is_deterministic() {
        // Either both succeed or both fail; no panic.
        let a = FfmpegTools::discover().ok();
        let b = FfmpegTools::discover().ok();
        assert_eq!(a.is_some(), b.is_some());
        assert_eq!(ffmpeg_available(), a.is_some());
    }
}
