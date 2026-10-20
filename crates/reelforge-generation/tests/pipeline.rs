//! Contract fixtures test scheduling/storage/clock, never neural visual quality.
use reelforge_core::{Time, VideoClip};
use reelforge_generation::*;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
    sync::{Arc, Mutex},
};

fn backend(id: &str, kind: BackendKind) -> Backend {
    Backend {
        id: id.into(),
        fingerprint: "a".repeat(64),
        kind,
        themes: BTreeSet::new(),
        available: true,
        ram_bytes: 1,
        vram_bytes: 0,
        constraints: ModelConstraints {
            max_frames: 81,
            frame_multiple: 4,
            frame_remainder: 1,
        },
        rank: 0,
    }
}
fn shot(id: &str, start: u32, end: u32, theme: &str) -> ShotSpec {
    ShotSpec {
        id: id.into(),
        start,
        end,
        prompt: format!("A changing {theme} scene"),
        theme: theme.into(),
        continuity: json!({"location":"persistent-place","cast":"canonical"}),
        references: vec![],
        conditions: BTreeMap::new(),
        allow_still: false,
    }
}
fn spec() -> GenerationSpec {
    GenerationSpec {
        clock: Clock {
            fps: 24,
            frames: 240,
            width: 16,
            height: 16,
        },
        keyframe_count: 10,
        seed: 42,
        shots: vec![shot("scene", 0, 240, "realism")],
        allow_image_edit_between: false,
    }
}
fn resources() -> Resources {
    Resources {
        ram_bytes: 100,
        vram_bytes: 0,
    }
}
#[derive(Clone, Copy)]
enum Fault {
    None,
    Hold,
    Missing,
    ReplaceAnchor,
    Crash,
}
type Calls = Arc<Mutex<Vec<FrameRequest>>>;
struct Fixture {
    backend: Backend,
    calls: Calls,
    fault: Fault,
}
fn png(index: u32, held: bool) -> Vec<u8> {
    let n = if held { 0 } else { index };
    let [r, g, ..] = n.to_le_bytes();
    let image = image::RgbaImage::from_pixel(16, 16, image::Rgba([r, g, 127, 255]));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
impl Provider for Fixture {
    fn backend(&self) -> &Backend {
        &self.backend
    }
    fn generate(
        &mut self,
        r: &FrameRequest,
        anchors: &BTreeMap<u32, FrameReceipt>,
        store: &FrameStore,
    ) -> Result<BTreeMap<u32, FrameReceipt>> {
        self.calls.lock().unwrap().push(r.clone());
        assert_eq!(anchors.keys().copied().collect::<Vec<_>>(), r.anchors);
        if matches!(self.fault, Fault::Crash) {
            return Err(Error::Provider("interrupted fixture".into()));
        }
        let mut indices = r.indices.clone();
        if r.kind == TaskKind::Between {
            if matches!(self.fault, Fault::Missing) {
                indices.pop();
            }
            if matches!(self.fault, Fault::ReplaceAnchor) {
                indices.push(r.anchors[0]);
            }
        }
        indices
            .into_iter()
            .map(|i| {
                Ok((
                    i,
                    store.ingest_png(&png(i, matches!(self.fault, Fault::Hold)))?,
                ))
            })
            .collect()
    }
    fn recover(
        &mut self,
        _: &FrameRequest,
        _: &BTreeMap<u32, FrameReceipt>,
        _: &FrameStore,
    ) -> Result<Recovery> {
        Ok(Recovery::Unknown)
    }
}
fn providers(fault: Fault) -> (BTreeMap<String, Box<dyn Provider>>, Calls) {
    let calls = Arc::new(Mutex::new(vec![]));
    let mut all: BTreeMap<String, Box<dyn Provider>> = BTreeMap::new();
    for b in [
        backend("keys", BackendKind::Keyframe),
        backend("video", BackendKind::TemporalGeneration),
    ] {
        all.insert(
            b.id.clone(),
            Box::new(Fixture {
                backend: b,
                calls: calls.clone(),
                fault,
            }),
        );
    }
    (all, calls)
}

#[test]
fn ten_sparse_keys_fill_230_frames_cache_and_native_clock() {
    let temp = tempfile::tempdir().unwrap();
    let engine = Engine::open(temp.path()).unwrap();
    let (mut all, calls) = providers(Fault::None);
    let result = engine.run(spec(), &resources(), &mut all).unwrap();
    assert_eq!(
        (
            result.keyframes,
            result.intermediate_frames,
            result.frames.len()
        ),
        (10, 230, 240)
    );
    assert!(result.frame_clock_verified);
    assert!(!result.naturalness_verified);
    let first = calls.lock().unwrap().clone();
    assert_eq!(first.len(), 19);
    let cached = engine.run(spec(), &resources(), &mut all).unwrap();
    assert_eq!(cached.submitted_requests, 0);
    assert_eq!(calls.lock().unwrap().len(), 19);
    assert_eq!(cached.frames, result.frames);
    let mut receipt = result;
    for fps in [24, 30, 48, 60, 120] {
        receipt.clock.fps = fps;
        let clip = GeneratedClip::open(engine.store().clone(), &receipt).unwrap();
        let times = clip.sample_times().unwrap();
        for i in 0..240 {
            let t = times[i as usize];
            assert_eq!(
                clip.frame_at(t).unwrap(),
                clip.frame_at_index(i).unwrap(),
                "fps {fps}, frame {i}"
            );
            let offset = Time::from_secs(t.as_secs() + 0.25 / f64::from(fps));
            assert_eq!(
                clip.frame_at(offset).unwrap(),
                clip.frame_at_index(i).unwrap()
            );
        }
        assert!(
            clip.frame_at(Time::from_secs(clip.duration().as_secs()))
                .is_err()
        );
        assert!(clip.frame_at(Time::from_secs(f64::NAN)).is_err());
    }
}

#[test]
fn different_themes_and_scene_cuts_route_without_crossing() {
    let mut s = spec();
    s.shots = vec![
        shot("sea", 0, 80, "water"),
        shot("city", 80, 160, "realism"),
        shot("drawn", 160, 240, "anime"),
    ];
    let mut backends = vec![backend("keys", BackendKind::Keyframe)];
    for theme in ["water", "realism", "anime"] {
        let mut b = backend(theme, BackendKind::TemporalGeneration);
        b.themes.insert(theme.into());
        backends.push(b);
    }
    let p = compile(s, &backends, &resources()).unwrap();
    let mut indices = BTreeSet::new();
    for r in p.keyframes.iter().chain(&p.between) {
        assert!(
            r.indices
                .iter()
                .chain(&r.anchors)
                .all(|i| *i >= r.shot.start && *i < r.shot.end)
        );
        if r.kind == TaskKind::Between {
            assert_eq!(r.backend.id, r.shot.theme);
        }
        for i in &r.indices {
            assert!(indices.insert(*i));
        }
    }
    assert_eq!(
        indices.into_iter().collect::<Vec<_>>(),
        (0..240).collect::<Vec<_>>()
    );
}

#[test]
fn interpolation_never_satisfies_generation_and_no_memory_means_no_calls() {
    assert!(
        compile(
            spec(),
            &[
                backend("keys", BackendKind::Keyframe),
                backend("rife", BackendKind::Interpolation)
            ],
            &resources()
        )
        .is_err()
    );
    let t = tempfile::tempdir().unwrap();
    let engine = Engine::open(t.path()).unwrap();
    let (mut all, calls) = providers(Fault::None);
    assert!(
        engine
            .run(
                spec(),
                &Resources {
                    ram_bytes: 0,
                    vram_bytes: 0
                },
                &mut all
            )
            .is_err()
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn qwen_edit_is_explicit_and_gets_each_target_frame_with_two_anchors() {
    let mut s = spec();
    s.shots[0]
        .conditions
        .insert(12, json!({"camera":{"zoom":1.2},"pose":{"weight":"right"}}));
    let mut edit = backend("image-edit", BackendKind::ImageEdit);
    edit.constraints = ModelConstraints {
        max_frames: 1,
        frame_multiple: 1,
        frame_remainder: 0,
    };
    let backends = [backend("keys", BackendKind::Keyframe), edit];
    assert!(compile(s.clone(), &backends, &resources()).is_err());
    s.allow_image_edit_between = true;
    let p = compile(s, &backends, &resources()).unwrap();
    assert_eq!(p.between.len(), 230);
    for r in &p.between {
        assert_eq!(r.indices.len(), 1);
        assert_eq!(r.anchors.len(), 2);
        assert_eq!(r.model_frames, 1);
    }
    assert!(
        p.between
            .iter()
            .any(|r| r.shot.conditions.contains_key(&12))
    );
}

#[test]
fn choose_model_that_fits_window_and_padding_never_changes_output_clock() {
    let mut small = backend("small", BackendKind::TemporalGeneration);
    small.constraints.max_frames = 5;
    let big = backend("big", BackendKind::TemporalGeneration);
    let p = compile(
        spec(),
        &[backend("keys", BackendKind::Keyframe), small, big],
        &resources(),
    )
    .unwrap();
    assert!(
        p.between
            .iter()
            .all(|r| r.backend.id == "big" && r.model_frames % 4 == 1)
    );
    assert_eq!(
        p.between.iter().map(|r| r.indices.len()).sum::<usize>(),
        230
    );
}

#[test]
fn editing_scene_memory_invalidates_only_that_scene() {
    let t = tempfile::tempdir().unwrap();
    let engine = Engine::open(t.path()).unwrap();
    let (mut all, calls) = providers(Fault::None);
    let mut s = spec();
    s.shots = vec![
        shot("one", 0, 120, "realism"),
        shot("two", 120, 240, "realism"),
    ];
    engine.run(s.clone(), &resources(), &mut all).unwrap();
    let n = calls.lock().unwrap().len();
    s.shots[1].continuity = json!({"accident":"yesterday","remaining":"broken-glass"});
    let result = engine.run(s, &resources(), &mut all).unwrap();
    let calls = calls.lock().unwrap();
    assert!(result.submitted_requests > 0);
    assert!(calls[n..].iter().all(|r| r.shot.id == "two"));
}

#[test]
fn unknown_submission_is_not_repeated() {
    let t = tempfile::tempdir().unwrap();
    let engine = Engine::open(t.path()).unwrap();
    let (mut all, calls) = providers(Fault::Crash);
    assert!(engine.run(spec(), &resources(), &mut all).is_err());
    assert!(matches!(
        engine.run(spec(), &resources(), &mut all),
        Err(Error::Unknown(_))
    ));
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn bad_outputs_are_rejected_instead_of_holding_or_replacing_anchors() {
    for fault in [Fault::Hold, Fault::Missing, Fault::ReplaceAnchor] {
        let t = tempfile::tempdir().unwrap();
        let engine = Engine::open(t.path()).unwrap();
        let (mut all, _) = providers(fault);
        assert!(engine.run(spec(), &resources(), &mut all).is_err());
    }
}

#[test]
fn source_ownership_and_changed_pixels_are_checked_before_models() {
    let t = tempfile::tempdir().unwrap();
    let engine = Engine::open(t.path()).unwrap();
    let r = engine.store().ingest_png(&png(1, false)).unwrap();
    let mut escaped = r.clone();
    escaped.sha256 = "../escape".into();
    assert!(engine.store().verify(&escaped).is_err());
    let mut wrong = r.clone();
    wrong.pixels_sha256 = "0".repeat(64);
    assert!(engine.store().verify(&wrong).is_err());
    std::fs::write(&r.path, png(2, false)).unwrap();
    let mut s = spec();
    s.shots[0].references = vec![r];
    let (mut all, calls) = providers(Fault::None);
    assert!(engine.run(s, &resources(), &mut all).is_err());
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn malformed_constraints_and_overflowing_shots_do_not_panic() {
    let mut b = backend("broken", BackendKind::TemporalGeneration);
    b.constraints.frame_multiple = 0;
    assert!(
        compile(
            spec(),
            &[backend("keys", BackendKind::Keyframe), b],
            &resources()
        )
        .is_err()
    );
    let mut s = spec();
    s.shots[0].start = u32::MAX;
    s.shots[0].end = u32::MAX;
    assert!(compile(s, &[backend("keys", BackendKind::Keyframe)], &resources()).is_err());
}

#[test]
fn large_scene_graph_is_shared_between_requests() {
    let mut s = spec();
    s.shots[0].continuity = json!({"entities":(0..10_000).map(|i| json!({"id":i,"parent":i/10,"persistent":true})).collect::<Vec<_>>()});
    let plan = compile(
        s,
        &[
            backend("keys", BackendKind::Keyframe),
            backend("video", BackendKind::TemporalGeneration),
        ],
        &resources(),
    )
    .unwrap();
    for request in plan.keyframes.iter().chain(&plan.between) {
        assert!(Arc::ptr_eq(&request.shot, &plan.keyframes[0].shot));
        assert_eq!(
            request.shot.continuity["entities"]
                .as_array()
                .unwrap()
                .len(),
            10_000
        );
    }
}

#[test]
fn indexed_worker_response_decodes_inside_tagged_enum() {
    let response: ProcessResponse = serde_json::from_str(r#"{"status":"completed","frames":{"0":{"path":"/owned/frame.png","sha256":"abc"},"239":{"path":"/owned/last.png","sha256":"def"}}}"#).unwrap();
    let ProcessResponse::Completed { frames } = response else {
        panic!("wrong response")
    };
    assert_eq!(frames.keys().copied().collect::<Vec<_>>(), vec![0, 239]);
    assert!(
        serde_json::from_str::<ProcessResponse>(
            r#"{"status":"completed","frames":{"01":{"path":"/owned/frame.png","sha256":"abc"}}}"#
        )
        .is_err()
    );
}
