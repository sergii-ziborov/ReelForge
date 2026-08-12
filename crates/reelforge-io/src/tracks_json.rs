//! JSON load/save for [`reelforge_fx::TrackSet`] (`SightLoom` adapter boundary).

use crate::error::{IoError, Result};
use reelforge_fx::{RegionSample, RegionTrack, TrackSet};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Wire format version for track documents.
pub const TRACKS_JSON_VERSION: u32 = 1;

/// JSON document for temporal tracks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracksDocument {
    /// Schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Tracks list.
    #[serde(default)]
    pub tracks: Vec<TrackJson>,
}

fn default_version() -> u32 {
    TRACKS_JSON_VERSION
}

/// One track in JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackJson {
    /// Stable id.
    pub id: String,
    /// Optional kind (`face`, `plate`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Timed samples.
    #[serde(default)]
    pub samples: Vec<SampleJson>,
}

/// One sample — either center+radius or bounding box.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleJson {
    /// Time seconds.
    pub t: f64,
    /// Center X (optional if box used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cx: Option<f32>,
    /// Center Y.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cy: Option<f32>,
    /// Soft radius in pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
    /// Box origin X (`x/y/w/h` form).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    /// Box origin Y.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    /// Box width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub w: Option<f32>,
    /// Box height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub h: Option<f32>,
    /// Box left edge (`left/top/right/bottom` form).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<f32>,
    /// Box top edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<f32>,
    /// Box right edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<f32>,
    /// Box bottom edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<f32>,
    /// Detection confidence when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conf: Option<f32>,
}

impl TracksDocument {
    /// Parse JSON text.
    ///
    /// # Errors
    ///
    /// Returns parse errors.
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).map_err(|e| IoError::message(format!("tracks json: {e}")))
    }

    /// Load from path.
    ///
    /// # Errors
    ///
    /// I/O or parse errors.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .map_err(|e| IoError::message(format!("read tracks: {e}")))?;
        Self::from_json(&text)
    }

    /// Pretty JSON.
    ///
    /// # Errors
    ///
    /// Serde errors.
    pub fn to_json_pretty(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|e| IoError::message(format!("tracks json: {e}")))
    }

    /// Convert to runtime [`TrackSet`].
    ///
    /// # Errors
    ///
    /// Incomplete samples (no center/radius or bbox).
    pub fn to_track_set(&self) -> Result<TrackSet> {
        let mut set = TrackSet::new();
        for tr in &self.tracks {
            let mut track = RegionTrack::new(&tr.id);
            if let Some(k) = &tr.kind {
                track = track.with_kind(k.clone());
            }
            for s in &tr.samples {
                track.push(sample_from_json(s)?);
            }
            set.push(track);
        }
        Ok(set)
    }
}

impl From<&TrackSet> for TracksDocument {
    fn from(set: &TrackSet) -> Self {
        Self {
            version: TRACKS_JSON_VERSION,
            tracks: set
                .tracks
                .iter()
                .map(|tr| TrackJson {
                    id: tr.id.clone(),
                    kind: tr.kind.clone(),
                    samples: tr
                        .samples
                        .iter()
                        .map(|s| SampleJson {
                            t: s.t,
                            cx: Some(s.cx),
                            cy: Some(s.cy),
                            radius: Some(s.radius),
                            x: None,
                            y: None,
                            w: None,
                            h: None,
                            left: None,
                            top: None,
                            right: None,
                            bottom: None,
                            conf: Some(s.conf),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

fn sample_from_json(s: &SampleJson) -> Result<RegionSample> {
    let conf = s.conf.unwrap_or(1.0);
    if let (Some(cx), Some(cy), Some(radius)) = (s.cx, s.cy, s.radius) {
        return Ok(RegionSample {
            t: s.t,
            cx,
            cy,
            radius: radius.max(1.0),
            conf: conf.clamp(0.0, 1.0),
        });
    }
    if let (Some(x), Some(y), Some(w), Some(h)) = (s.x, s.y, s.w, s.h) {
        return Ok(RegionSample::from_xywh(s.t, x, y, w, h, conf));
    }
    if let (Some(left), Some(top), Some(right), Some(bottom)) = (s.left, s.top, s.right, s.bottom) {
        return Ok(RegionSample::from_bbox(s.t, left, top, right, bottom, conf));
    }
    Err(IoError::message(format!(
        "track sample at t={} needs cx/cy/radius or x/y/w/h or left/top/right/bottom",
        s.t
    )))
}

/// Load tracks file into a [`TrackSet`].
///
/// # Errors
///
/// I/O or parse / sample errors.
pub fn load_track_set(path: impl AsRef<Path>) -> Result<TrackSet> {
    TracksDocument::load(path)?.to_track_set()
}

/// Parse tracks JSON text.
///
/// # Errors
///
/// Parse / sample errors.
pub fn parse_track_set(text: &str) -> Result<TrackSet> {
    TracksDocument::from_json(text)?.to_track_set()
}

/// Build [`TrackSet`] from a JSON value (plan custom params).
///
/// Accepts either a full document `{ "tracks": [...] }` or a bare array of tracks.
///
/// # Errors
///
/// Parse errors.
pub fn track_set_from_value(value: &serde_json::Value) -> Result<TrackSet> {
    if value.is_array() {
        let tracks: Vec<TrackJson> = serde_json::from_value(value.clone())
            .map_err(|e| IoError::message(format!("tracks array: {e}")))?;
        return TracksDocument {
            version: TRACKS_JSON_VERSION,
            tracks,
        }
        .to_track_set();
    }
    let doc: TracksDocument = serde_json::from_value(value.clone())
        .map_err(|e| IoError::message(format!("tracks object: {e}")))?;
    doc.to_track_set()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_center_samples() {
        let json = r#"{
          "version": 1,
          "tracks": [{
            "id": "face_1",
            "kind": "face",
            "samples": [
              {"t": 0.0, "cx": 10.0, "cy": 20.0, "radius": 12.0, "conf": 0.95},
              {"t": 1.0, "cx": 30.0, "cy": 25.0, "radius": 14.0}
            ]
          }]
        }"#;
        let set = parse_track_set(json).unwrap();
        assert_eq!(set.len(), 1);
        let (cx, cy, _, conf) = set.tracks[0].region_at(0.5).unwrap();
        assert!((cx - 20.0).abs() < 1e-3);
        assert!((cy - 22.5).abs() < 1e-3);
        assert!(conf > 0.9);
    }

    #[test]
    fn bbox_xywh() {
        let json = r#"{
          "tracks": [{
            "id": "p",
            "samples": [{"t": 0.0, "x": 0.0, "y": 0.0, "w": 40.0, "h": 20.0}]
          }]
        }"#;
        let set = parse_track_set(json).unwrap();
        let (cx, cy, r, _) = set.tracks[0].region_at(0.0).unwrap();
        assert!((cx - 20.0).abs() < 1e-3);
        assert!((cy - 10.0).abs() < 1e-3);
        assert!(r > 1.0);
    }
}
