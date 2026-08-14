//! Builtin [`AdapterExecutor`]s and the adapter registry.

use crate::adapter::{AdapterExecutor, AdapterOutput, AdapterRequest};
use crate::error::{IoError, Result};
use reelforge_render_graph::{MaskTimeline, mask_timeline_from_tracks};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Named adapter executors (`sightloom` by default).
#[derive(Clone)]
pub struct AdapterRegistry {
    inner: Arc<BTreeMap<String, Arc<dyn AdapterExecutor>>>,
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl AdapterRegistry {
    /// Empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(BTreeMap::new()),
        }
    }

    /// `sightloom` JSON / package executor.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r = r.register(Arc::new(SightloomJsonExecutor));
        r
    }

    /// Insert or replace by [`AdapterExecutor::name`].
    #[must_use]
    pub fn register(self, exec: Arc<dyn AdapterExecutor>) -> Self {
        let mut map = (*self.inner).clone();
        map.insert(exec.name().to_string(), exec);
        Self {
            inner: Arc::new(map),
        }
    }

    /// Lookup by short name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn AdapterExecutor> {
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

/// Default `SightLoom` adapter: exported tracks / masks JSON only.
pub struct SightloomJsonExecutor;

impl AdapterExecutor for SightloomJsonExecutor {
    fn name(&self) -> &'static str {
        "sightloom"
    }

    fn materialize(&self, request: &AdapterRequest) -> Result<AdapterOutput> {
        materialize_sightloom_json(&request.params)
    }
}

fn materialize_sightloom_json(params: &serde_json::Value) -> Result<AdapterOutput> {
    if params.get("tracks").is_some() || params.get("document").is_some() || params.is_array() {
        let value = params.get("document").unwrap_or(params);
        let tracks = reelforge_sightloom_adapter::track_timelines_from_value(value)
            .map_err(|e| IoError::message(e.to_string()))?;
        let masks = mask_timeline_from_tracks(&tracks);
        return Ok(AdapterOutput {
            masks: if masks.samples.is_empty() {
                None
            } else {
                Some(masks)
            },
            tracks,
            frames: Vec::new(),
        });
    }
    if let Some(masks) = params.get("masks") {
        let timeline: MaskTimeline = serde_json::from_value(masks.clone())
            .map_err(|e| IoError::message(format!("adapter masks: {e}")))?;
        return Ok(AdapterOutput {
            masks: Some(timeline),
            tracks: Vec::new(),
            frames: Vec::new(),
        });
    }
    if params.get("package_id").is_some() {
        return Err(IoError::message(
            "adapter 'sightloom' package_id needs an AdapterHost to resolve masks",
        ));
    }
    if params.get("query").is_some() {
        return Err(IoError::message(
            "adapter 'sightloom' query needs an AdapterHost (ReelForge does not query subjects)",
        ));
    }
    Err(IoError::message(
        "adapter 'sightloom' needs tracks, masks, or an AdapterHost that can resolve the query",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_contain_sightloom() {
        let r = AdapterRegistry::with_builtins();
        assert!(r.get("sightloom").is_some());
        assert_eq!(r.len(), 1);
    }
}
