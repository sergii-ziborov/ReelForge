//! Builtin [`GpuExecutor`]s and the GPU registry.

use crate::gpu::{GpuExecutor, GpuOutput, GpuRequest};
use crate::error::Result;
use crate::realtime::detect_hw_encoders;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Named GPU executors (`passthrough`, `hw`).
#[derive(Clone)]
pub struct GpuRegistry {
    inner: Arc<BTreeMap<String, Arc<dyn GpuExecutor>>>,
}

impl Default for GpuRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl GpuRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BTreeMap::new()),
        }
    }

    /// Passthrough + hardware-encode probe.
    #[must_use]
    pub fn with_builtins() -> Self {
        Self::new()
            .register(Arc::new(GpuPassthroughExecutor))
            .register(Arc::new(HwEncodeExecutor))
    }

    /// Insert or replace by [`GpuExecutor::name`].
    #[must_use]
    pub fn register(self, exec: Arc<dyn GpuExecutor>) -> Self {
        let mut map = (*self.inner).clone();
        map.insert(exec.name().to_string(), exec);
        Self {
            inner: Arc::new(map),
        }
    }

    /// Lookup by short name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn GpuExecutor> {
        self.inner.get(name).map(Arc::as_ref)
    }

    /// Number of executors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Identity GPU stage: pipeline continues, pixels stay on the current clip.
pub struct GpuPassthroughExecutor;

impl GpuExecutor for GpuPassthroughExecutor {
    fn name(&self) -> &'static str {
        "passthrough"
    }

    fn execute(&self, _request: &GpuRequest) -> Result<GpuOutput> {
        Ok(GpuOutput::default())
    }
}

/// Probe host `ffmpeg` for NVENC/QSV/AMF and hint the encode codec.
pub struct HwEncodeExecutor;

impl GpuExecutor for HwEncodeExecutor {
    fn name(&self) -> &'static str {
        "hw"
    }

    fn execute(&self, request: &GpuRequest) -> Result<GpuOutput> {
        if let Some(codec) = request
            .params
            .get("codec")
            .and_then(serde_json::Value::as_str)
        {
            return Ok(GpuOutput {
                video: None,
                video_codec: Some(codec.to_string()),
            });
        }
        if let Some(backend) = request.backend.as_deref() {
            let codec = match backend {
                "nvenc" | "cuda" => Some("h264_nvenc"),
                "qsv" => Some("h264_qsv"),
                "amf" => Some("h264_amf"),
                _ => None,
            };
            if let Some(c) = codec {
                return Ok(GpuOutput {
                    video: None,
                    video_codec: Some(c.into()),
                });
            }
        }
        let hw = detect_hw_encoders()?;
        Ok(GpuOutput {
            video: None,
            video_codec: hw
                .preferred_h264()
                .map(str::to_string)
                .or_else(|| Some("libx264".into())),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_contain_passthrough_and_hw() {
        let r = GpuRegistry::with_builtins();
        assert!(r.get("passthrough").is_some());
        assert!(r.get("hw").is_some());
        assert_eq!(r.len(), 2);
    }
}
