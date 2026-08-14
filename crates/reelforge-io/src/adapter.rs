//! Adapter stage execution (`SightLoom` materialize → [`MaskTimeline`]).
//!
//! `ReelForge` does **not** query subjects. A host implements [`AdapterHost`]
//! to talk to `SightLoom`; the default path only accepts already-exported
//! tracks / masks JSON.

use crate::error::{IoError, Result};
use reelforge_render_graph::{MaskTimeline, TrackTimeline, mask_timeline_from_tracks};
use serde_json::Value;

/// Product of an adapter stage (video stays a passthrough).
#[derive(Debug, Clone, Default)]
pub struct AdapterOutput {
    /// Materialized ROI / silhouette view.
    pub masks: Option<MaskTimeline>,
    /// Identity tracks when the document carried them.
    pub tracks: Vec<TrackTimeline>,
}

/// Host hook for vision materialization (`SightLoom`, tests, Capture).
pub trait AdapterHost: Send + Sync {
    /// Resolve `adapter` + params. Return `Ok(None)` to fall through to JSON.
    ///
    /// # Errors
    ///
    /// Host / package / query failures.
    fn materialize(&self, adapter: &str, params: &Value) -> Result<Option<AdapterOutput>>;
}

/// Execute a scheduled adapter (`sightloom` / `rf.adapter.sightloom`).
///
/// # Errors
///
/// Unknown adapter without a host, or unusable params.
pub fn execute_adapter(
    name: &str,
    params: &Value,
    host: Option<&dyn AdapterHost>,
) -> Result<AdapterOutput> {
    if let Some(host) = host
        && let Some(out) = host.materialize(name, params)?
    {
        return Ok(out);
    }
    let short = name.rsplit('.').next().unwrap_or(name);
    match short {
        "sightloom" => materialize_sightloom_json(params),
        other => Err(IoError::message(format!(
            "adapter '{other}' has no host (register AdapterHost or pass tracks/masks JSON)"
        ))),
    }
}

fn materialize_sightloom_json(params: &Value) -> Result<AdapterOutput> {
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
        });
    }
    if let Some(masks) = params.get("masks") {
        let timeline: MaskTimeline = serde_json::from_value(masks.clone())
            .map_err(|e| IoError::message(format!("adapter masks: {e}")))?;
        return Ok(AdapterOutput {
            masks: Some(timeline),
            tracks: Vec::new(),
        });
    }
    if params.get("package_id").is_some() {
        return Err(IoError::message(
            "adapter 'sightloom' package_id needs an AdapterHost to resolve masks",
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
    fn json_tracks_materialize() {
        let params = serde_json::json!({
            "tracks": [{
                "id": "face_1",
                "samples": [{"t": 0.0, "cx": 10.0, "cy": 10.0, "radius": 5.0}]
            }]
        });
        let out = execute_adapter("sightloom", &params, None).unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.masks.unwrap().samples.len(), 1);
    }

    #[test]
    fn query_without_host_errors() {
        let err = execute_adapter(
            "sightloom",
            &serde_json::json!({ "query": "person in red" }),
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("AdapterHost"));
    }

    #[test]
    fn unknown_adapter_errors() {
        let err = execute_adapter("gpu-seg", &serde_json::json!({}), None).unwrap_err();
        assert!(err.to_string().contains("no host"));
    }
}
