//! Editable scene document: links survive a reload, and one part does not edit another.

use reelforge_core::MediaTime;
use reelforge_project::{
    GroupId, MotionKey, MotionProperty, PartAnchor, PartId, SceneCanvas, SceneDocument, SceneGroup,
    SceneMedia, ScenePart, ScenePoint, SourceLink, compile_scene,
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

fn canvas() -> SceneCanvas {
    SceneCanvas {
        width: 8,
        height: 4,
        duration: MediaTime::from_secs(2.0, 1_000).unwrap(),
    }
}

fn photo() -> Vec<SceneMedia> {
    vec![SceneMedia {
        id: reelforge_project::MediaRefId::new("photo"),
        uri: "mem://photo".into(),
    }]
}

fn key(secs: i64, value: f64) -> MotionKey {
    MotionKey {
        t: MediaTime::new(secs, 1).unwrap(),
        value,
    }
}

#[test]
fn invalid_links_do_not_reach_the_compiler() {
    let mut scene = two_parts();
    scene.parts[0].parent = Some(PartId::new("petal"));
    scene.parts[1].parent = Some(PartId::new("stem"));
    let err = scene.validate().unwrap_err();
    assert!(err.to_string().contains("parent cycle"));
    let err = compile_scene(&scene, &photo(), canvas()).unwrap_err();
    assert!(err.to_string().contains("parent cycle"));

    let mut both = two_parts();
    both.parts[1].parent = Some(PartId::new("stem"));
    both.parts[1].anchor = Some(PartAnchor {
        target: PartId::new("stem"),
        point: ScenePoint::new(1.0, 0.0),
    });
    assert!(
        both.validate()
            .unwrap_err()
            .to_string()
            .contains("both a parent and an anchor")
    );

    let mut group = two_parts();
    group.parts[1].group = Some(GroupId::new("flower"));
    assert!(
        group
            .validate()
            .unwrap_err()
            .to_string()
            .contains("missing part")
    );

    let mut unsorted = two_parts();
    unsorted.parts[1].source.mask = None;
    unsorted
        .add_motion_key(
            &PartId::new("petal"),
            MotionProperty::PositionX,
            key(1, 2.0),
        )
        .unwrap();
    unsorted
        .add_motion_key(
            &PartId::new("petal"),
            MotionProperty::PositionX,
            key(0, 1.0),
        )
        .unwrap();
    assert!(
        unsorted
            .validate()
            .unwrap_err()
            .to_string()
            .contains("not strictly increasing")
    );

    let mut mixed = two_parts();
    mixed.parts[0].parent = Some(PartId::new("petal"));
    mixed.parts[1].anchor = Some(PartAnchor {
        target: PartId::new("stem"),
        point: ScenePoint::new(0.0, 0.0),
    });
    assert!(
        mixed
            .validate()
            .unwrap_err()
            .to_string()
            .contains("pose cycle")
    );
}

#[test]
fn compile_scene_keeps_parent_pivot_and_rotation() {
    let mut scene = two_parts();
    scene.parts[1].source.mask = None;
    scene.parts[1].parent = Some(PartId::new("stem"));
    scene.parts[1].pivot = ScenePoint::new(1.0, 2.0);
    scene.parts[1].order = 1;
    scene
        .add_motion_key(
            &PartId::new("petal"),
            MotionProperty::Rotation,
            key(0, 15.0),
        )
        .unwrap();
    scene
        .add_motion_key(&PartId::new("stem"), MotionProperty::PositionX, key(0, 4.0))
        .unwrap();
    scene
        .add_motion_key(&PartId::new("stem"), MotionProperty::PositionX, key(1, 8.0))
        .unwrap();
    scene.set_hidden(&PartId::new("stem"), false).unwrap();

    let compiled = compile_scene(&scene, &photo(), canvas()).unwrap();
    let loaded = SceneDocument::from_json(&scene.to_json().unwrap()).unwrap();
    let again = compile_scene(&loaded, &photo(), canvas()).unwrap();
    assert_eq!(compiled.resolved, again.resolved);
    assert_eq!(compiled.graph, again.graph);
    let text = serde_json::to_string(&compiled.graph).unwrap();
    assert!(text.contains("\"parent\":\"stem\""), "{text}");
    assert!(text.contains("\"rotation\""), "{text}");
    assert!(text.contains("keyframes"), "{text}");
    assert!(text.contains("1.0"), "{text}");
    assert_eq!(compiled.resolved.parts.len(), 2);
    assert!((compiled.resolved.parts[1].pivot.x - 1.0).abs() < 1e-9);

    scene.parts[1].source.mask = Some("petal-mask".into());
    let err = compile_scene(&scene, &photo(), canvas()).unwrap_err();
    assert!(err.to_string().contains("coverage is not compiled"));
}

#[test]
fn compile_scene_keeps_an_anchor_and_drops_hidden_parts() {
    let mut scene = two_parts();
    scene.parts[1].source.mask = None;
    scene
        .bind_anchor(
            &PartId::new("petal"),
            PartAnchor {
                target: PartId::new("stem"),
                point: ScenePoint::new(1.0, 0.0),
            },
        )
        .unwrap();
    scene.set_hidden(&PartId::new("stem"), true).unwrap();
    let err = compile_scene(&scene, &photo(), canvas()).unwrap_err();
    assert!(err.to_string().contains("not a visible part"));

    scene.set_hidden(&PartId::new("stem"), false).unwrap();
    scene.set_hidden(&PartId::new("petal"), true).unwrap();
    let compiled = compile_scene(&scene, &photo(), canvas()).unwrap();
    assert_eq!(compiled.resolved.parts.len(), 1);
    assert_eq!(compiled.resolved.parts[0].id.as_str(), "stem");
    let text = serde_json::to_string(&compiled.graph).unwrap();
    assert!(!text.contains("petal"), "{text}");
}
