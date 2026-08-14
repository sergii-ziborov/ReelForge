//! Adapter executors: `SightLoom` materialize → masks, without querying subjects.
//!
//! `ReelForge` never talks to a vision crate. A host implements [`AdapterHost`]
//! to call `SightLoom`. Builtin [`crate::AdapterRegistry`] runs JSON / package
//! executors and can resolve [`MaskAsset::External`] into pixels.

use crate::error::{IoError, Result};
use reelforge_core::VideoClip;
use reelforge_render_graph::{MaskAsset, MaskFrame, MaskTimeline, TrackTimeline};
use serde_json::Value;
use std::sync::Arc;

/// Input to an adapter executor / host.
#[derive(Clone)]
pub struct AdapterRequest {
    /// Adapter name (`sightloom`, `rf.adapter.sightloom`, …).
    pub adapter: String,
    /// Query / tracks / package params.
    pub params: Value,
    /// Upstream video (host may sample frames; JSON path ignores it).
    pub video: Option<Arc<dyn VideoClip>>,
}

impl AdapterRequest {
    /// Params-only request (tests / JSON).
    #[must_use]
    pub fn new(adapter: impl Into<String>, params: Value) -> Self {
        Self {
            adapter: adapter.into(),
            params,
            video: None,
        }
    }

    /// Attach the upstream clip.
    #[must_use]
    pub fn with_video(mut self, video: Arc<dyn VideoClip>) -> Self {
        self.video = Some(video);
        self
    }

    /// Short name (`rf.adapter.sightloom` → `sightloom`).
    #[must_use]
    pub fn short_name(&self) -> &str {
        self.adapter
            .rsplit('.')
            .next()
            .unwrap_or(self.adapter.as_str())
    }
}

/// Product of an adapter stage (video stays a passthrough).
#[derive(Debug, Clone, Default)]
pub struct AdapterOutput {
    /// Materialized ROI / silhouette view.
    pub masks: Option<MaskTimeline>,
    /// Identity tracks when the document carried them.
    pub tracks: Vec<TrackTimeline>,
    /// Optional pixel frames (merged onto [`Self::masks`] by subject + time).
    pub frames: Vec<MaskFrame>,
}

/// Host hook for vision materialization (`SightLoom` / Capture / tests).
pub trait AdapterHost: Send + Sync {
    /// Resolve the request. `Ok(None)` falls through to the registered executor.
    ///
    /// # Errors
    ///
    /// Host / package / query failures.
    fn materialize(&self, request: &AdapterRequest) -> Result<Option<AdapterOutput>>;

    /// Map an [`MaskAsset::External`] handle to CPU pixels when the host can.
    ///
    /// # Errors
    ///
    /// Package / I/O failures (missing handle is `Ok(None)`).
    fn resolve_mask(&self, asset: &MaskAsset) -> Result<Option<MaskAsset>> {
        let _ = asset;
        Ok(None)
    }
}

/// Named builtin / plugin adapter executor.
pub trait AdapterExecutor: Send + Sync {
    /// Registry key (`sightloom`).
    fn name(&self) -> &'static str;

    /// Materialize tracks / masks from the request.
    ///
    /// # Errors
    ///
    /// Unknown shape, missing host for query/package, or JSON errors.
    fn materialize(&self, request: &AdapterRequest) -> Result<AdapterOutput>;

    /// Optional pixel resolve for [`MaskAsset::External`].
    ///
    /// # Errors
    ///
    /// Executor I/O failures.
    fn resolve_mask(&self, asset: &MaskAsset) -> Result<Option<MaskAsset>> {
        let _ = asset;
        Ok(None)
    }
}

/// Host + registry used by the graph runner.
#[derive(Clone, Default)]
pub struct AdapterContext {
    /// Optional `SightLoom` / test host.
    pub host: Option<Arc<dyn AdapterHost>>,
    /// Named executors (builtins by default).
    pub registry: crate::AdapterRegistry,
}

impl AdapterContext {
    /// Builtins only.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Install a host.
    #[must_use]
    pub fn with_host(mut self, host: Arc<dyn AdapterHost>) -> Self {
        self.host = Some(host);
        self
    }
}

/// Run the adapter: host first, then the registered executor, then resolve assets.
///
/// # Errors
///
/// Unknown adapter, unusable params, or host/executor failures.
pub fn execute_adapter(request: &AdapterRequest, ctx: &AdapterContext) -> Result<AdapterOutput> {
    if let Some(host) = ctx.host.as_deref()
        && let Some(out) = host.materialize(request)?
    {
        return finish_output(out, request, ctx);
    }
    let exec = ctx
        .registry
        .get(request.short_name())
        .or_else(|| ctx.registry.get(request.adapter.as_str()))
        .ok_or_else(|| {
            IoError::message(format!(
                "adapter '{}' has no executor (register AdapterHost or AdapterRegistry)",
                request.adapter
            ))
        })?;
    let out = exec.materialize(request)?;
    finish_output(out, request, ctx)
}

fn finish_output(
    mut out: AdapterOutput,
    request: &AdapterRequest,
    ctx: &AdapterContext,
) -> Result<AdapterOutput> {
    if out.masks.is_none() && !out.tracks.is_empty() {
        let masks = reelforge_render_graph::mask_timeline_from_tracks(&out.tracks);
        if !masks.samples.is_empty() {
            out.masks = Some(masks);
        }
    }
    if let Some(masks) = out.masks.as_mut() {
        attach_frames(masks, &out.frames);
        resolve_timeline(masks, request, ctx)?;
    }
    Ok(out)
}

fn attach_frames(masks: &mut MaskTimeline, frames: &[MaskFrame]) {
    for sample in &mut masks.samples {
        if sample.asset.is_some() {
            continue;
        }
        let Some(hit) = frames
            .iter()
            .find(|frame| frame.subject == sample.subject_id() && frame.time == sample.t)
        else {
            continue;
        };
        sample.asset = Some(hit.mask.clone());
    }
}

fn resolve_timeline(
    masks: &mut MaskTimeline,
    request: &AdapterRequest,
    ctx: &AdapterContext,
) -> Result<()> {
    let exec = ctx
        .registry
        .get(request.short_name())
        .or_else(|| ctx.registry.get(request.adapter.as_str()));
    for sample in &mut masks.samples {
        let Some(refer) = sample.asset.as_ref() else {
            continue;
        };
        if !matches!(refer.asset, MaskAsset::External { .. }) {
            continue;
        }
        let mut resolved = None;
        if let Some(host) = ctx.host.as_deref() {
            resolved = host.resolve_mask(&refer.asset)?;
        }
        if resolved.is_none()
            && let Some(exec) = exec
        {
            resolved = exec.resolve_mask(&refer.asset)?;
        }
        if let Some(asset) = resolved {
            sample.asset = Some(reelforge_render_graph::MaskAssetRef::inline(asset));
        }
    }
    Ok(())
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
        let out = execute_adapter(
            &AdapterRequest::new("sightloom", params),
            &AdapterContext::new(),
        )
        .unwrap();
        assert_eq!(out.tracks.len(), 1);
        assert_eq!(out.masks.unwrap().samples.len(), 1);
    }

    #[test]
    fn query_without_host_errors() {
        let err = execute_adapter(
            &AdapterRequest::new("sightloom", serde_json::json!({ "query": "person in red" })),
            &AdapterContext::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("AdapterHost") || err.to_string().contains("query"));
    }

    #[test]
    fn unknown_adapter_errors() {
        let err = execute_adapter(
            &AdapterRequest::new("gpu-seg", serde_json::json!({})),
            &AdapterContext::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("no executor"));
    }

    struct QueryHost;

    impl AdapterHost for QueryHost {
        fn materialize(&self, request: &AdapterRequest) -> Result<Option<AdapterOutput>> {
            use reelforge_core::MediaTime;
            use reelforge_render_graph::{MaskAssetRef, MaskSample, SubjectId};
            if request.params.get("query").is_none() {
                return Ok(None);
            }
            if request.video.is_none() {
                return Err(IoError::message("query host needs video"));
            }
            let mut masks = MaskTimeline::new();
            masks.push(
                MaskSample::ellipse_subject(
                    SubjectId::new("p"),
                    MediaTime::new(0, 30).unwrap(),
                    8.0,
                    8.0,
                    4.0,
                )
                .with_asset(MaskAssetRef::external("pkg", 7)),
            );
            Ok(Some(AdapterOutput {
                masks: Some(masks),
                tracks: Vec::new(),
                frames: Vec::new(),
            }))
        }

        fn resolve_mask(&self, asset: &MaskAsset) -> Result<Option<MaskAsset>> {
            match asset {
                MaskAsset::External { mask_ref: 7, .. } => {
                    let mut data = vec![0_u8; 16 * 16];
                    data[8 * 16 + 8] = 255;
                    Ok(Some(MaskAsset::Dense {
                        width: 16,
                        height: 16,
                        data,
                    }))
                }
                _ => Ok(None),
            }
        }
    }

    #[test]
    fn host_query_gets_video_and_resolves_dense() {
        use reelforge_core::{ColorClip, Duration, Rgb8, Size};
        use std::sync::Arc;
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(16, 16),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let out = execute_adapter(
            &AdapterRequest::new("sightloom", serde_json::json!({ "query": "person" }))
                .with_video(clip),
            &AdapterContext::new().with_host(Arc::new(QueryHost)),
        )
        .unwrap();
        let sample = &out.masks.unwrap().samples[0];
        assert!(matches!(
            sample.asset.as_ref().unwrap().asset,
            MaskAsset::Dense { width: 16, .. }
        ));
    }
}
