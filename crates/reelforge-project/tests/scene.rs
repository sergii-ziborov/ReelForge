//! Editable scene document: links survive a reload, and one part does not edit another.

use reelforge_core::MediaTime;
use reelforge_project::{
    GroupId, MotionKey, MotionProperty, PartAnchor, PartId, SceneDocument, SceneGroup, ScenePart,
    ScenePoint, SourceLink,
};

fn two_parts() -> SceneDocument {
    let mut scene = SceneDocument::new("scene", "flower");
    scene
        .parts
        .push(ScenePart::new("stem", SourceLink::media("photo")));
    scene.parts.push(ScenePart::new(
        "petal",
        SourceLink::media("photo").with_mask("petal-mask"),
    ));
    scene.groups.push(SceneGroup::new("flower", "Flower"));
    scene
}

#[test]
fn scene_round_trip_keeps_links() {
    let mut scene = two_parts();
    scene
        .assign_group(&PartId::new("petal"), &GroupId::new("flower"))
        .unwrap();
    scene
        .bind_anchor(
            &PartId::new("petal"),
            PartAnchor {
                target: PartId::new("stem"),
                point: ScenePoint::new(0.5, 1.0),
            },
        )
        .unwrap();
    scene
        .add_motion_key(
            &PartId::new("petal"),
            MotionProperty::Rotation,
            MotionKey {
                t: MediaTime::from_secs(0.5, 1_000).unwrap(),
                value: 15.0,
            },
        )
        .unwrap();
    let loaded = SceneDocument::from_json(&scene.to_json().unwrap()).unwrap();
    assert_eq!(loaded, scene);
    assert_eq!(loaded.part(&PartId::new("stem")).unwrap().source.mask, None);
    assert_eq!(
        loaded
            .part(&PartId::new("petal"))
            .unwrap()
            .source
            .mask
            .as_deref(),
        Some("petal-mask")
    );
}

#[test]
fn replace_hide_and_motion_stay_on_one_part() {
    let mut scene = two_parts();
    let stem = PartId::new("stem");
    let petal = PartId::new("petal");
    scene
        .replace_source(&petal, SourceLink::media("petal-photo"))
        .unwrap();
    scene.set_hidden(&petal, true).unwrap();
    scene
        .add_motion_key(
            &petal,
            MotionProperty::PositionX,
            MotionKey {
                t: MediaTime::new(0, 1_000).unwrap(),
                value: 4.0,
            },
        )
        .unwrap();

    let stem_part = scene.part(&stem).unwrap();
    assert!(stem_part.replacement.is_none());
    assert!(!stem_part.hidden);
    assert_eq!(stem_part.effective_source().media.as_str(), "photo");
    let petal_part = scene.part(&petal).unwrap();
    assert_eq!(petal_part.effective_source().media.as_str(), "petal-photo");
    assert!(petal_part.hidden);
    assert_eq!(petal_part.source.media.as_str(), "photo");
    assert_eq!(scene.motion.channels.len(), 1);
    assert_eq!(scene.motion.channels[0].part, petal);
}

#[test]
fn missing_part_does_not_change_the_scene() {
    let scene = two_parts();
    let mut edited = scene.clone();
    let err = edited
        .replace_source(&PartId::new("missing"), SourceLink::media("other"))
        .unwrap_err();
    assert!(err.to_string().contains("unknown part"));
    assert_eq!(edited, scene);
}

#[test]
fn anchor_requires_another_part() {
    let mut scene = two_parts();
    let before = scene.clone();
    let err = scene
        .bind_anchor(
            &PartId::new("stem"),
            PartAnchor {
                target: PartId::new("stem"),
                point: ScenePoint::new(0.0, 0.0),
            },
        )
        .unwrap_err();
    assert!(err.to_string().contains("cannot anchor"));
    assert_eq!(scene, before);

    let err = scene
        .bind_anchor(
            &PartId::new("petal"),
            PartAnchor {
                target: PartId::new("missing"),
                point: ScenePoint::new(0.0, 0.0),
            },
        )
        .unwrap_err();
    assert!(err.to_string().contains("unknown part"));
    assert!(scene.part(&PartId::new("petal")).unwrap().anchor.is_none());
}

#[test]
fn version_zero_migrates_and_a_newer_version_is_rejected() {
    let old = r#"{"id":"scene","name":"flower"}"#;
    let loaded = SceneDocument::from_json(old).unwrap();
    assert_eq!(loaded.version, 1);
    assert!(loaded.parts.is_empty());

    let future = r#"{"version":2,"id":"scene","name":"flower"}"#;
    let err = SceneDocument::from_json(future).unwrap_err();
    assert!(err.to_string().contains("version 2"));
}
