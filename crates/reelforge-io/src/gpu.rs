//! GPU stage execution (device host + builtin encode / passthrough).
//!
//! `ReelForge` does not ship CUDA/Vulkan kernels. A [`GpuHost`] owns device
//! work (`ExternalSurface`, compute). Builtins: passthrough, and
//! `rf.encode.hw` which selects NVENC/QSV/AMF when the host `ffmpeg` has them.

use crate::error::{IoError, Result};
use reelforge_core::VideoClip;
use serde_json::Value;
use std::sync::Arc;

/// Input to a GPU executor / host.
#[derive(Clone)]
pub struct GpuRequest {
    /// Executor name (`passthrough`, `encode_hw`, …).
    pub name: String,
    /// Optional backend hint (`cuda`, `d3d11`, `nvenc`, …).
    pub backend: Option<String>,
    /// Op params.
    pub params: Value,
    /// Upstream video.
    pub video: Arc<dyn VideoClip>,
}

impl GpuRequest {
    /// Build a request.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        backend: Option<String>,
        params: Value,
        video: Arc<dyn VideoClip>,
    ) -> Self {
        Self {
            name: name.into(),
            backend,
            params,
            video,
        }
    }

    /// Short name (`rf.encode.hw` → `hw`).
    #[must_use]
    pub fn short_name(&self) -> &str {
        self.name.rsplit('.').next().unwrap_or(self.name.as_str())
    }
}

/// Product of a GPU stage (defaults to video passthrough).
#[derive(Clone, Default)]
pub struct GpuOutput {
    /// Replacement video when the device produced one.
    pub video: Option<Arc<dyn VideoClip>>,
    /// Encode codec hint (`h264_nvenc`, …).
    pub video_codec: Option<String>,
}

impl core::fmt::Debug for GpuOutput {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GpuOutput")
            .field("video", &self.video.is_some())
            .field("video_codec", &self.video_codec)
            .finish()
    }
}

/// Host hook for device compute / surface mapping.
pub trait GpuHost: Send + Sync {
    /// Run the request. `Ok(None)` falls through to the registry executor.
    ///
    /// # Errors
    ///
    /// Device / map failures.
    fn execute(&self, request: &GpuRequest) -> Result<Option<GpuOutput>>;
}

/// Named builtin / plugin GPU executor.
pub trait GpuExecutor: Send + Sync {
    /// Registry key.
    fn name(&self) -> &'static str;

    /// Execute the request.
    ///
    /// # Errors
    ///
    /// Missing host for compute-only ops, or device probe failures.
    fn execute(&self, request: &GpuRequest) -> Result<GpuOutput>;
}

/// Host + registry used by the graph runner.
#[derive(Clone, Default)]
pub struct GpuContext {
    /// Optional device host.
    pub host: Option<Arc<dyn GpuHost>>,
    /// Named executors (builtins by default).
    pub registry: crate::GpuRegistry,
}

impl GpuContext {
    /// Builtins only.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install a device host.
    #[must_use]
    pub fn with_host(mut self, host: Arc<dyn GpuHost>) -> Self {
        self.host = Some(host);
        self
    }
}

/// Run the GPU op: host first, then the registered executor.
///
/// # Errors
///
/// Unknown executor or host/executor failures.
pub fn execute_gpu(request: &GpuRequest, ctx: &GpuContext) -> Result<GpuOutput> {
    if let Some(host) = ctx.host.as_deref()
        && let Some(out) = host.execute(request)?
    {
        return Ok(out);
    }
    let key = request.short_name();
    let exec = ctx
        .registry
        .get(key)
        .or_else(|| ctx.registry.get(request.name.as_str()))
        .ok_or_else(|| {
            IoError::message(format!(
                "gpu '{}' has no executor (register GpuHost or GpuRegistry)",
                request.name
            ))
        })?;
    exec.execute(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Duration, Rgb8, Size};

    #[test]
    fn passthrough_keeps_clip() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 8),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let out = execute_gpu(
            &GpuRequest::new("passthrough", None, Value::Null, Arc::clone(&clip)),
            &GpuContext::new(),
        )
        .unwrap();
        assert!(out.video.is_none());
        assert!(out.video_codec.is_none());
    }

    #[test]
    fn unknown_gpu_errors() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::BLUE,
            Duration::from_secs(1.0),
        ));
        let err = execute_gpu(
            &GpuRequest::new("blur", None, Value::Null, clip),
            &GpuContext::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no executor"));
    }
}
