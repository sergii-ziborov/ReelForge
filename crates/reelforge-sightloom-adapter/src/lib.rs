//! Optional adapter: SightLoom-shaped **JSON** → [`TrackTimeline`].
//!
//! This crate does **not** depend on `SightLoom`. It only understands a
//! document that vision products can export. `ReelForge` consumes tracks;
//! it does not query subjects.

mod convert;
mod document;
mod error;

pub use convert::{DEFAULT_TIMESCALE, document_to_timelines};
pub use document::{MaskEntry, SampleEntry, TRACK_DOC_VERSION, TrackDocument, TrackEntry};
pub use error::{AdapterError, Result};
pub use reelforge_render_graph::{TrackTimeline, mask_timeline_from_tracks};

use std::path::Path;

/// Parse JSON text into track timelines.
///
/// Accepts `{ "tracks": [...] }` or a bare array of tracks.
///
/// # Errors
///
/// JSON or sample-shape failures.
pub fn parse_track_timelines(text: &str) -> Result<Vec<TrackTimeline>> {
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| AdapterError::Json(e.to_string()))?;
    track_timelines_from_value(&value)
}

/// Load a track document from disk.
///
/// # Errors
///
/// I/O, JSON, or sample-shape failures.
pub fn load_track_timelines(path: impl AsRef<Path>) -> Result<Vec<TrackTimeline>> {
    let text =
        std::fs::read_to_string(path.as_ref()).map_err(|e| AdapterError::Io(e.to_string()))?;
    parse_track_timelines(&text)
}

/// Parse a JSON value (`tracks` object or array).
///
/// # Errors
///
/// JSON or sample-shape failures.
pub fn track_timelines_from_value(value: &serde_json::Value) -> Result<Vec<TrackTimeline>> {
    let doc = if value.is_array() {
        let tracks: Vec<TrackEntry> = serde_json::from_value(value.clone())
            .map_err(|e| AdapterError::Json(format!("tracks array: {e}")))?;
        TrackDocument {
            version: TRACK_DOC_VERSION,
            tracks,
        }
    } else {
        serde_json::from_value(value.clone())
            .map_err(|e| AdapterError::Json(format!("tracks object: {e}")))?
    };
    document_to_timelines(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::MediaTime;
    use reelforge_render_graph::{OcclusionState, RegionRedaction};

    #[test]
    fn legacy_tracks_json_still_parses() {
        let json = r#"{
          "version": 1,
          "tracks": [{
            "id": "face_1",
            "kind": "face",
            "samples": [
              {"t": 0.0, "cx": 10.0, "cy": 20.0, "radius": 12.0, "conf": 0.95}
            ]
          }]
        }"#;
        let tracks = parse_track_timelines(json).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].track.as_str(), "face_1");
        assert_eq!(tracks[0].samples.len(), 1);
        let view = tracks[0].to_mask_timeline();
        assert_eq!(view.regions_at(MediaTime::zero(1_000)).len(), 1);
    }

    #[test]
    fn rich_sample_ids_and_occlusion() {
        let json = r#"{
          "tracks": [{
            "id": "tr_1",
            "subject": "person_a",
            "samples": [{
              "t": 0.5,
              "left": 0.0, "top": 0.0, "right": 10.0, "bottom": 10.0,
              "occlusion": "occluded",
              "appearance": "ap_1",
              "observation": "obs_9",
              "mask": { "uri": "masks/obs_9.bin" }
            }]
          }]
        }"#;
        let tracks = parse_track_timelines(json).unwrap();
        let s = &tracks[0].samples[0];
        assert_eq!(s.occlusion, OcclusionState::Occluded);
        assert_eq!(s.appearance.as_ref().unwrap().as_str(), "ap_1");
        assert_eq!(s.observation.as_ref().unwrap().as_str(), "obs_9");
        assert_eq!(
            s.mask.as_ref().unwrap().uri.as_deref(),
            Some("masks/obs_9.bin")
        );
        assert!(tracks[0].to_mask_timeline().regions_at(s.t).is_empty());
    }

    #[test]
    fn redaction_from_parsed_tracks() {
        let json = r#"[{"id":"a","samples":[{"t":0.0,"cx":1.0,"cy":1.0,"radius":4.0}]}]"#;
        let tracks = parse_track_timelines(json).unwrap();
        let r = RegionRedaction::gaussian_tracks(&tracks, 10.0);
        assert_eq!(r.masks.samples.len(), 1);
    }
}
