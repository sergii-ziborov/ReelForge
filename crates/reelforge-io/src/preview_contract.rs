//! Preview contract for Capture / editor (engine hooks, not cache policy).

use crate::error::{IoError, Result};
use crate::graph_run::{GraphRunOptions, materialize_graph_with_seeds};
use crate::preview::proxy_size;
use reelforge_core::{Frame, MediaTime, Size, VideoClip};
use reelforge_fx::resize_bilinear;
use reelforge_render_graph::{MediaAssetId, RenderGraph};
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::sync::Arc;

/// How expensive a preview sample may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PreviewQuality {
    /// Small box, fast scrub (`320×180` default).
    Draft,
    /// Editor proxy box (`640×360` default).
    #[default]
    Proxy,
    /// Native clip size (no downscale).
    Full,
}

/// Persistent preview / proxy spec (Capture stores policy; we only interpret it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewSpec {
    /// Quality band.
    pub quality: PreviewQuality,
    /// Max width (ignored when [`PreviewQuality::Full`]).
    pub max_width: u32,
    /// Max height (ignored when [`PreviewQuality::Full`]).
    pub max_height: u32,
}

impl PreviewSpec {
    /// Draft / proxy / full with stock boxes.
    #[must_use]
    pub const fn from_quality(quality: PreviewQuality) -> Self {
        match quality {
            PreviewQuality::Draft => Self {
                quality,
                max_width: 320,
                max_height: 180,
            },
            PreviewQuality::Proxy => Self {
                quality,
                max_width: 640,
                max_height: 360,
            },
            PreviewQuality::Full => Self {
                quality,
                max_width: 0,
                max_height: 0,
            },
        }
    }

    /// Output size for a source frame.
    #[must_use]
    pub fn output_size(self, src: Size) -> Size {
        match self.quality {
            PreviewQuality::Full => src,
            PreviewQuality::Draft | PreviewQuality::Proxy => {
                let w = if self.max_width == 0 {
                    640
                } else {
                    self.max_width
                };
                let h = if self.max_height == 0 {
                    360
                } else {
                    self.max_height
                };
                proxy_size(src, w, h)
            }
        }
    }
}

/// One preview sample request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewRequest {
    /// Media time to sample.
    pub time: MediaTime,
    /// Quality / box.
    pub spec: PreviewSpec,
}

impl PreviewRequest {
    /// Sample at `time` with a stock quality box.
    #[must_use]
    pub const fn at(time: MediaTime, quality: PreviewQuality) -> Self {
        Self {
            time,
            spec: PreviewSpec::from_quality(quality),
        }
    }
}

/// RGB frame ready for a viewer / thumbnail cache.
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    /// Requested media time.
    pub time: MediaTime,
    /// Output size (may be smaller than the source).
    pub size: Size,
    /// Packed RGB.
    pub frame: Frame,
}

/// Sample one preview frame from a clip (resize, no encode).
///
/// # Errors
///
/// Time out of range, resize, or sample failures.
pub fn preview_clip(clip: &dyn VideoClip, request: PreviewRequest) -> Result<PreviewFrame> {
    let target = request.spec.output_size(clip.size());
    let sampled = clip
        .frame_at(request.time.to_time())
        .map_err(IoError::from)?;
    let frame = if target == sampled.size() {
        sampled
    } else {
        resize_bilinear(&sampled, target).map_err(IoError::from)?
    };
    Ok(PreviewFrame {
        time: request.time,
        size: frame.size(),
        frame,
    })
}

/// Materialize a graph (optional seeds) and sample a preview frame.
///
/// # Errors
///
/// Graph, materialize, or sample failures.
pub fn preview_graph<S: BuildHasher>(
    graph: &RenderGraph,
    request: PreviewRequest,
    options: &GraphRunOptions,
    seeds: &HashMap<MediaAssetId, Arc<dyn VideoClip>, S>,
) -> Result<PreviewFrame> {
    let (clip, _) = materialize_graph_with_seeds(graph, &options.registry, seeds)?;
    preview_clip(clip.as_ref(), request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Duration, Rgb8};

    #[test]
    fn draft_is_smaller_than_source() {
        let clip = ColorClip::new(Size::new(1920, 1080), Rgb8::RED, Duration::from_secs(1.0));
        let prev = preview_clip(
            &clip,
            PreviewRequest::at(MediaTime::zero(1_000), PreviewQuality::Draft),
        )
        .unwrap();
        assert!(prev.size.width <= 320 && prev.size.height <= 180);
        assert_eq!(&prev.frame.data()[0..3], &[255, 0, 0]);
    }

    #[test]
    fn full_keeps_native_size() {
        let clip = ColorClip::new(Size::new(64, 48), Rgb8::BLUE, Duration::from_secs(1.0));
        let prev = preview_clip(
            &clip,
            PreviewRequest::at(MediaTime::zero(30), PreviewQuality::Full),
        )
        .unwrap();
        assert_eq!(prev.size, Size::new(64, 48));
    }

    #[test]
    fn preview_graph_with_seed() {
        use reelforge_render_graph::{
            GraphOutput, MediaAsset, NodeId, RENDER_GRAPH_VERSION, RenderNode, RenderNodeKind,
        };
        let g = RenderGraph {
            version: RENDER_GRAPH_VERSION,
            assets: vec![MediaAsset {
                id: MediaAssetId("a".into()),
                uri: "seed://color".into(),
                duration: None,
                role: Some("video".into()),
            }],
            nodes: vec![
                RenderNode {
                    id: NodeId("src".into()),
                    body: RenderNodeKind::Source {
                        asset: MediaAssetId("a".into()),
                    },
                    inputs: vec![],
                },
                RenderNode {
                    id: NodeId("out".into()),
                    body: RenderNodeKind::Output {
                        name: "main".into(),
                    },
                    inputs: vec![NodeId("src".into())],
                },
            ],
            outputs: vec![GraphOutput {
                name: "main".into(),
                node: NodeId("out".into()),
                uri: None,
            }],
        };
        let seed: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(128, 72),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let mut seeds = HashMap::new();
        seeds.insert(MediaAssetId("a".into()), seed);
        let prev = preview_graph(
            &g,
            PreviewRequest::at(MediaTime::zero(1_000), PreviewQuality::Draft),
            &GraphRunOptions::default(),
            &seeds,
        )
        .unwrap();
        assert!(prev.size.width <= 320);
        assert_eq!(prev.frame.size(), prev.size);
    }
}
