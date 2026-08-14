//! [`AdapterHost`] that materializes a [`crate::MaskPackage`] and resolves silhouettes.

use crate::adapter::{AdapterHost, AdapterOutput, AdapterRequest};
use crate::error::Result;
use crate::mask_package::MaskPackage;
use reelforge_render_graph::{MaskAsset, mask_timeline_from_tracks};
use std::path::Path;

/// Host bound to one on-disk mask package.
#[derive(Debug, Clone)]
pub struct SightloomPackageHost {
    package: MaskPackage,
}

impl SightloomPackageHost {
    /// Open a package directory (`manifest.json` + blobs).
    ///
    /// # Errors
    ///
    /// I/O or JSON.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            package: MaskPackage::open(dir)?,
        })
    }

    /// Package id.
    #[must_use]
    pub fn package_id(&self) -> &str {
        &self.package.manifest.package_id
    }
}

impl AdapterHost for SightloomPackageHost {
    fn materialize(&self, request: &AdapterRequest) -> Result<Option<AdapterOutput>> {
        let wants = request
            .params
            .get("package_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !wants.is_empty() && wants != self.package.manifest.package_id {
            return Ok(None);
        }
        if wants.is_empty()
            && request.params.get("tracks").is_none()
            && request.params.get("query").is_none()
            && request.params.get("package_id").is_none()
        {
            return Ok(None);
        }
        if wants.is_empty() && request.params.get("package_id").is_none() {
            // Fall through to JSON executor; we still resolve External later.
            return Ok(None);
        }
        let value = serde_json::json!({ "tracks": self.package.manifest.tracks });
        let tracks = reelforge_sightloom_adapter::track_timelines_from_value(&value)
            .map_err(|e| crate::error::IoError::message(e.to_string()))?;
        let mut masks = mask_timeline_from_tracks(&tracks);
        rewrite_package_refs(&mut masks, self.package.manifest.package_id.as_str());
        Ok(Some(AdapterOutput {
            masks: if masks.samples.is_empty() {
                None
            } else {
                Some(masks)
            },
            tracks,
            frames: Vec::new(),
        }))
    }

    fn resolve_mask(&self, asset: &MaskAsset) -> Result<Option<MaskAsset>> {
        self.package.resolve_external(asset)
    }
}

fn rewrite_package_refs(masks: &mut reelforge_render_graph::MaskTimeline, package_id: &str) {
    for sample in &mut masks.samples {
        let Some(refer) = sample.asset.as_mut() else {
            continue;
        };
        if let MaskAsset::External { package_id: pid, .. } = &mut refer.asset {
            let looks_like_path = pid.is_empty()
                || pid.contains('/')
                || pid.contains('\\')
                || std::path::Path::new(pid.as_str())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("bin"));
            if looks_like_path {
                *pid = package_id.to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{AdapterContext, AdapterRequest, execute_adapter};
    use reelforge_render_graph::MaskAsset;
    use std::fs;
    use std::sync::Arc;

    #[test]
    fn package_host_resolves_dense() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("masks")).unwrap();
        let mut blob = vec![0_u8; 8 * 8];
        blob[3 * 8 + 3] = 255;
        fs::write(dir.path().join("masks/1.bin"), &blob).unwrap();
        let manifest = serde_json::json!({
            "package_id": "pkg-demo",
            "tracks": [{
                "id": "p1",
                "samples": [{
                    "t": 0.0, "cx": 3.0, "cy": 3.0, "radius": 2.0,
                    "observation": "1",
                    "mask": { "observation": "1", "uri": "masks/1.bin" }
                }]
            }],
            "masks": [{
                "mask_ref": 1,
                "kind": "dense",
                "width": 8, "height": 8,
                "path": "masks/1.bin"
            }]
        });
        fs::write(dir.path().join("manifest.json"), manifest.to_string()).unwrap();
        let host = SightloomPackageHost::open(dir.path()).unwrap();
        let out = execute_adapter(
            &AdapterRequest::new(
                "sightloom",
                serde_json::json!({ "package_id": "pkg-demo" }),
            ),
            &AdapterContext::new().with_host(Arc::new(host)),
        )
        .unwrap();
        let sample = &out.masks.unwrap().samples[0];
        assert!(matches!(
            sample.asset.as_ref().unwrap().asset,
            MaskAsset::Dense { width: 8, .. }
        ));
    }

    #[test]
    fn package_masks_redact_silhouette_not_ellipse() {
        use crate::mask_bridge::apply_region_redaction;
        use reelforge_core::{ColorClip, Duration, Rgb8, Size, Time, VideoClip};
        use reelforge_render_graph::{RedactionStyle, RegionRedaction};

        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("masks")).unwrap();
        let mut blob = vec![0_u8; 32 * 32];
        for y in 8..24 {
            blob[y * 32 + 8] = 255;
        }
        fs::write(dir.path().join("masks/3.bin"), &blob).unwrap();
        let manifest = serde_json::json!({
            "package_id": "pkg-sil",
            "tracks": [{
                "id": "bar",
                "samples": [{
                    "t": 0.0,
                    "left": 0.0, "top": 0.0, "right": 32.0, "bottom": 32.0,
                    "observation": "3",
                    "mask": { "observation": "3", "uri": "masks/3.bin" }
                }]
            }],
            "masks": [{
                "mask_ref": 3, "kind": "dense",
                "width": 32, "height": 32, "path": "masks/3.bin"
            }]
        });
        fs::write(dir.path().join("manifest.json"), manifest.to_string()).unwrap();
        let host = SightloomPackageHost::open(dir.path()).unwrap();
        let out = execute_adapter(
            &AdapterRequest::new("sightloom", serde_json::json!({ "package_id": "pkg-sil" })),
            &AdapterContext::new().with_host(Arc::new(host)),
        )
        .unwrap();
        let clip: std::sync::Arc<dyn VideoClip> = std::sync::Arc::new(ColorClip::new(
            Size::new(32, 32),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let redaction = RegionRedaction {
            masks: out.masks.unwrap(),
            style: RedactionStyle::Solid {
                color: reelforge_core::Rgba8::new(0, 0, 0, 255),
            },
        };
        let frame = apply_region_redaction(clip, &redaction)
            .unwrap()
            .frame_at(Time::ZERO)
            .unwrap();
        let bar = (16 * 32 + 8) * 3;
        let far = (16 * 32 + 24) * 3;
        assert!(frame.data()[bar] < 250, "package silhouette must fill");
        assert_eq!(frame.data()[far], 255, "outside dense mask stays white");
    }
}
