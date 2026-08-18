//! Capture → ReelForge wire contract.
//!
//! The JSON is a copy of
//! `ReelForge-Capture/crates/reelforge-capture-schema/tests/golden/capture_project_v1.json`.
//! When Capture re-blesses that file, copy it here and keep this test green.

use reelforge_project::{CAPTURE_PROJECT_VERSION, CaptureProject, TimelineItem, compile_project};
use reelforge_render_graph::RenderNodeKind;
use std::path::PathBuf;

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/capture_project_v1.json")
}

#[test]
fn capture_golden_parses() {
    let text = std::fs::read_to_string(golden_path()).expect("golden document");
    let project = CaptureProject::from_json(&text).expect("Capture golden must parse");
    assert_eq!(project.version, CAPTURE_PROJECT_VERSION);
    assert_eq!(project.id.as_str(), "ses_golden");
    let seq = project.active().expect("active sequence");
    assert_eq!(seq.id.as_str(), "main");
    assert_eq!(seq.tracks.len(), 2);
    assert_eq!(seq.tracks[0].kind, reelforge_project::TrackKind::Video);
    assert_eq!(seq.tracks[1].id.as_str(), "a_system");

    let TimelineItem::Clip(zoom) = &seq.tracks[0].items[0] else {
        panic!("first item is the zoomed clip");
    };
    let crop = zoom.crop.expect("crop from Capture click-zoom");
    assert_eq!((crop.x, crop.y, crop.w, crop.h), (80, 45, 160, 90));
    assert_eq!(zoom.scale_to, Some((320, 180)));
    assert!(matches!(seq.tracks[0].items[2], TimelineItem::Gap(_)));
}

#[test]
fn capture_golden_compiles() {
    let text = std::fs::read_to_string(golden_path()).expect("golden document");
    let project = CaptureProject::from_json(&text).unwrap();
    let out = compile_project(&project).expect("golden must compile");
    let ops: Vec<&str> = out
        .graph
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            RenderNodeKind::Op { operation, .. } => Some(operation.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        ops.iter().any(|o| *o == "rf.transform.crop"),
        "zoom crop missing: {ops:?}"
    );
    assert!(
        ops.iter().any(|o| *o == "rf.transform.scale"),
        "zoom scale missing: {ops:?}"
    );
    assert!(
        ops.iter().any(|o| *o == "rf.transform.speed"),
        "speed clip missing: {ops:?}"
    );
    out.graph.validate().unwrap();
}

#[test]
fn capture_golden_bytes_are_the_checked_in_file() {
    // Guard against an empty / truncated copy during a sloppy sync.
    let text = std::fs::read_to_string(golden_path()).expect("golden document");
    assert!(text.contains("\"id\": \"ses_golden\""));
    assert!(text.contains("\"scale_to\""));
    let parsed = CaptureProject::from_json(&text).unwrap();
    let again = CaptureProject::from_json(&parsed.to_json_pretty().unwrap()).unwrap();
    assert_eq!(parsed, again);
}
