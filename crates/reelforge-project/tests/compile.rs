//! `CaptureProject` compile: trim, speed, dissolve, mix, ticks.

use reelforge_core::MediaTime;
use reelforge_project::{
    CAPTURE_PROJECT_VERSION, CaptureProject, CropRect, Gap, MediaRef, MediaRefId, Metadata,
    NestedSequence, ProjectId, Retiming, SemanticRef, Sequence, SequenceId, SourceRange,
    TimelineClip, TimelineClipId, TimelineItem, TimelineTrack, TimelineTrackId, TrackKind,
    Transition, TransitionKind, compile_project,
};
use reelforge_render_graph::RenderNodeKind;

fn media(id: &str, uri: &str) -> MediaRef {
    MediaRef {
        id: MediaRefId::new(id),
        uri: uri.into(),
        duration: Some(MediaTime::from_secs(10.0, 1_000).unwrap()),
        role: Some("video".into()),
    }
}

fn clip(id: &str, media_id: &str, start: f64, dur: f64) -> TimelineItem {
    TimelineItem::Clip(TimelineClip {
        id: TimelineClipId::new(id),
        media: MediaRefId::new(media_id),
        source: SourceRange::from_secs(start, dur).unwrap(),
        retiming: Retiming::Identity,
        transition_in: None,
        crop: None,
        scale_to: None,
        metadata: Metadata::default(),
    })
}

fn ops(graph: &reelforge_render_graph::RenderGraph) -> Vec<&str> {
    graph
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            RenderNodeKind::Op { operation, .. } => Some(operation.as_str()),
            _ => None,
        })
        .collect()
}

fn assert_millis(ticks: i64, scale: u64, millis: i64) {
    assert!(scale > 0, "ticks={ticks} scale={scale}");
    assert_eq!(
        ticks * 1_000,
        millis * i64::try_from(scale).unwrap(),
        "ticks={ticks} scale={scale}"
    );
}

#[test]
fn migrate_zero_to_one() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "demo");
    p.version = 0;
    let p = p.migrate().unwrap();
    assert_eq!(p.version, CAPTURE_PROJECT_VERSION);
}

#[test]
fn single_clip_is_source_trim_output() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "cut");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(clip("c1", "a", 1.0, 2.0));
    seq.tracks.push(tr);
    p.sequences.push(seq);

    let out = compile_project(&p).unwrap();
    assert!(out.warnings.is_empty());
    assert_eq!(out.graph.assets.len(), 1);
    let kinds: Vec<_> = out
        .graph
        .nodes
        .iter()
        .map(|n| match &n.body {
            RenderNodeKind::Source { .. } => "src",
            RenderNodeKind::Op { operation, .. } => operation.as_str(),
            RenderNodeKind::Output { .. } => "out",
            RenderNodeKind::Redaction { .. } => "redact",
        })
        .collect();
    assert_eq!(kinds, ["src", "rf.transform.trim", "out"]);
    out.graph.validate().unwrap();
}

#[test]
fn crop_and_scale_compile() {
    use reelforge_project::CropRect;
    let mut p = CaptureProject::new(ProjectId::new("p"), "zoom");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    let TimelineItem::Clip(mut c) = clip("c1", "a", 0.0, 2.0) else {
        panic!("clip");
    };
    c.crop = Some(CropRect {
        x: 80,
        y: 45,
        w: 160,
        h: 90,
    });
    c.scale_to = Some((320, 180));
    tr.items.push(TimelineItem::Clip(c));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let kinds = ops(&out.graph);
    assert!(kinds.contains(&"rf.transform.crop"), "{kinds:?}");
    assert!(kinds.contains(&"rf.transform.even_dims"), "{kinds:?}");
    assert!(kinds.contains(&"rf.transform.scale"), "{kinds:?}");
}

#[test]
fn gap_then_clip_uses_compose_start() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "gap");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(TimelineItem::Gap(Gap {
        duration: MediaTime::from_secs(1.5, 1_000).unwrap(),
    }));
    tr.items.push(clip("c1", "a", 0.0, 2.0));
    seq.tracks.push(tr);
    p.sequences.push(seq);

    let out = compile_project(&p).unwrap();
    let compose = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.compose.layers" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("compose");
    let start = &compose["layers"][0]["start"];
    let ticks = start["ticks"].as_i64().unwrap();
    let scale = start["timescale"].as_u64().unwrap();
    // 1.5s may be stored as 3/2 after the rational cursor reduces 1500/1000.
    assert!(scale > 0, "{start}");
    assert_eq!(
        ticks * 1_000,
        1_500 * i64::try_from(scale).unwrap(),
        "{start}"
    );
}

#[test]
fn json_roundtrip() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "rt");
    p.semantic.push(SemanticRef::new("subject", "person_a"));
    p.media.push(media("a", "a.mp4"));
    let text = p.to_json_pretty().unwrap();
    let q = CaptureProject::from_json(&text).unwrap();
    assert_eq!(q.id.as_str(), "p");
    assert_eq!(q.semantic[0].id, "person_a");
}

#[test]
fn speed_emits_transform() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "spd");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    let TimelineItem::Clip(mut c) = clip("c1", "a", 0.0, 4.0) else {
        panic!("clip");
    };
    c.retiming = Retiming::Speed { factor: 2.0 };
    tr.items.push(TimelineItem::Clip(c));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    assert!(ops(&out.graph).contains(&"rf.transform.speed"));
}

#[test]
fn dissolve_overlaps_and_fades() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "xf");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(clip("c1", "a", 0.0, 2.0));
    let TimelineItem::Clip(mut c2) = clip("c2", "a", 0.0, 2.0) else {
        panic!("clip");
    };
    c2.transition_in = Some(Transition {
        kind: TransitionKind::Dissolve,
        duration: MediaTime::from_secs(0.5, 1_000).unwrap(),
    });
    tr.items.push(TimelineItem::Clip(c2));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let names = ops(&out.graph);
    assert!(names.contains(&"rf.transform.crossfade_in"), "{names:?}");
    assert!(!names.contains(&"rf.transform.fade_in"), "{names:?}");
    assert!(!names.contains(&"rf.transform.fade_out"), "{names:?}");
    assert!(names.contains(&"rf.compose.layers"));
    let compose = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.compose.layers" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("compose");
    // 2s clip minus a 0.5s overlap. The cursor may reduce 1500/1000 to 3/2.
    let start = &compose["layers"][1]["start"];
    assert_millis(
        start["ticks"].as_i64().unwrap(),
        start["timescale"].as_u64().unwrap(),
        1_500,
    );
}

#[test]
fn dissolve_on_a_fresh_track_does_not_fade_the_other() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "xf-tracks");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut lower = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    lower.items.push(clip("c1", "a", 0.0, 2.0));
    let mut upper = TimelineTrack::new(TimelineTrackId::new("v1"), TrackKind::Video);
    let TimelineItem::Clip(mut c2) = clip("c2", "a", 0.0, 2.0) else {
        panic!("clip");
    };
    c2.transition_in = Some(Transition {
        kind: TransitionKind::Dissolve,
        duration: MediaTime::from_secs(0.5, 1_000).unwrap(),
    });
    upper.items.push(TimelineItem::Clip(c2));
    seq.tracks.push(lower);
    seq.tracks.push(upper);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let names = ops(&out.graph);
    assert!(!names.contains(&"rf.transform.fade_out"), "{names:?}");
    assert!(!names.contains(&"rf.transform.fade_in"), "{names:?}");
    assert!(!names.contains(&"rf.transform.crossfade_in"), "{names:?}");
}

#[test]
fn trailing_gap_extends_compose_duration() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "tail");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(clip("c1", "a", 0.0, 2.0));
    tr.items.push(TimelineItem::Gap(Gap {
        duration: MediaTime::from_secs(1.0, 1_000).unwrap(),
    }));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let compose = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.compose.layers" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("a trailing gap must not take the single-clip shortcut");
    let duration = &compose["duration"];
    assert_millis(
        duration["ticks"].as_i64().unwrap(),
        duration["timescale"].as_u64().unwrap(),
        3_000,
    );
    out.graph.validate().unwrap();
}

#[test]
fn audio_track_mixes() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "av");
    p.media.push(media("a", "a.mp4"));
    p.media.push(MediaRef {
        id: MediaRefId::new("m"),
        uri: "m.wav".into(),
        duration: None,
        role: Some("audio".into()),
    });
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut v = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    v.items.push(clip("c1", "a", 0.0, 2.0));
    let mut a = TimelineTrack::new(TimelineTrackId::new("a0"), TrackKind::Audio);
    a.items.push(clip("c2", "m", 0.0, 2.0));
    seq.tracks.push(v);
    seq.tracks.push(a);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let names = ops(&out.graph);
    assert!(names.contains(&"rf.audio.drop"));
    assert!(names.contains(&"rf.audio.mix"));
}

fn project_with_audio_clip(mutate: impl FnOnce(&mut TimelineClip)) -> CaptureProject {
    let mut p = CaptureProject::new(ProjectId::new("p"), "av");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut video = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    video.items.push(clip("pic", "a", 0.0, 2.0));
    let mut audio = TimelineTrack::new(TimelineTrackId::new("a0"), TrackKind::Audio);
    let TimelineItem::Clip(mut sound) = clip("snd", "a", 0.0, 2.0) else {
        panic!("clip");
    };
    mutate(&mut sound);
    audio.items.push(TimelineItem::Clip(sound));
    seq.tracks.push(video);
    seq.tracks.push(audio);
    p.sequences.push(seq);
    p
}

#[test]
fn audio_track_keeps_speed() {
    let out = compile_project(&project_with_audio_clip(|sound| {
        sound.retiming = Retiming::Speed { factor: 2.0 };
    }))
    .unwrap();
    let names = ops(&out.graph);
    assert!(names.contains(&"rf.transform.speed"), "{names:?}");
    assert!(names.contains(&"rf.audio.mix"), "{names:?}");
    assert!(!names.contains(&"rf.transform.freeze"), "{names:?}");
    out.graph.validate().unwrap();
}

#[test]
fn audio_track_refuses_picture_only_retime() {
    let freeze = compile_project(&project_with_audio_clip(|sound| {
        sound.retiming = Retiming::Freeze {
            at: MediaTime::from_secs(0.5, 1_000).unwrap(),
            hold: MediaTime::from_secs(1.0, 1_000).unwrap(),
        };
    }))
    .unwrap_err()
    .to_string();
    assert!(
        freeze.contains("clip snd: freeze is a picture retime"),
        "{freeze}"
    );
    assert!(freeze.contains("audio track"), "{freeze}");

    let looped = compile_project(&project_with_audio_clip(|sound| {
        sound.retiming = Retiming::Loop {
            duration: None,
            times: Some(3),
        };
    }))
    .unwrap_err()
    .to_string();
    assert!(
        looped.contains("clip snd: loop is a picture retime"),
        "{looped}"
    );

    let cropped = compile_project(&project_with_audio_clip(|sound| {
        sound.crop = Some(CropRect {
            x: 0,
            y: 0,
            w: 8,
            h: 8,
        });
    }))
    .unwrap_err()
    .to_string();
    assert!(
        cropped.contains("clip snd: crop is a picture transform"),
        "{cropped}"
    );

    let scaled = compile_project(&project_with_audio_clip(|sound| {
        sound.scale_to = Some((16, 16));
    }))
    .unwrap_err()
    .to_string();
    assert!(
        scaled.contains("clip snd: scale is a picture transform"),
        "{scaled}"
    );
}

#[test]
fn audio_track_refuses_picture_transitions() {
    for (kind, label) in [
        (TransitionKind::Fade, "fade"),
        (TransitionKind::Dissolve, "dissolve"),
        (TransitionKind::Wipe, "wipe"),
    ] {
        let err = compile_project(&project_with_audio_clip(|sound| {
            sound.transition_in = Some(Transition {
                kind,
                duration: MediaTime::from_secs(0.5, 1_000).unwrap(),
            });
        }))
        .unwrap_err()
        .to_string();
        assert!(
            err.contains(&format!(
                "clip snd: {label} is a picture transition; an audio track cannot use it"
            )),
            "{err}"
        );
        assert!(
            !err.contains("slide"),
            "an audio wipe must not compile into a slide: {err}"
        );
    }
}

#[test]
fn freeze_and_loop_emit_transforms() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "time");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    let TimelineItem::Clip(mut freeze) = clip("c1", "a", 0.0, 2.0) else {
        panic!("clip");
    };
    freeze.retiming = Retiming::Freeze {
        at: MediaTime::from_secs(0.5, 1_000).unwrap(),
        hold: MediaTime::from_secs(1.0, 1_000).unwrap(),
    };
    tr.items.push(TimelineItem::Clip(freeze));
    let TimelineItem::Clip(mut lp) = clip("c2", "a", 0.0, 1.0) else {
        panic!("clip");
    };
    lp.retiming = Retiming::Loop {
        duration: None,
        times: Some(3),
    };
    tr.items.push(TimelineItem::Clip(lp));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let names = ops(&out.graph);
    assert!(names.contains(&"rf.transform.freeze"));
    assert!(names.contains(&"rf.transform.loop"));
    let freeze_p = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.transform.freeze" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("freeze");
    assert_eq!(freeze_p["hold"]["ticks"], 1000);
    assert_eq!(freeze_p["at"]["ticks"], 500);
    out.graph.validate().unwrap();
}

#[test]
fn trim_keeps_media_time_ticks() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "ticks");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(clip("c1", "a", 1.0, 2.0));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let params = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.transform.trim" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("trim");
    assert_eq!(params["start"]["ticks"], 1000);
    assert_eq!(params["start"]["timescale"], 1000);
    assert_eq!(params["duration"]["ticks"], 2000);
    assert_eq!(params["duration"]["timescale"], 1000);
}

#[test]
#[allow(clippy::cast_precision_loss)]
fn ntsc_clips_do_not_accumulate_millisecond_error() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "ntsc");
    p.media.push(MediaRef {
        id: MediaRefId::new("a"),
        uri: "a.mp4".into(),
        duration: Some(MediaTime {
            ticks: 120,
            timescale: 1,
        }),
        role: Some("video".into()),
    });
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    let frame = MediaTime {
        ticks: 1001,
        timescale: 30_000,
    };
    for i in 0..1000 {
        tr.items.push(TimelineItem::Clip(TimelineClip {
            id: TimelineClipId::new(format!("c{i}")),
            media: MediaRefId::new("a"),
            source: SourceRange {
                start: MediaTime {
                    ticks: 0,
                    timescale: 30_000,
                },
                duration: frame,
            },
            retiming: Retiming::Identity,
            transition_in: None,
            crop: None,
            scale_to: None,
            metadata: Metadata::default(),
        }));
    }
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let params = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.compose.layers" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("compose");
    let layers = params["layers"].as_array().expect("layers");
    assert_eq!(layers.len(), 1000);
    let start = &layers[999]["start"];
    let ticks = start["ticks"].as_i64().unwrap();
    let scale = start["timescale"].as_u64().unwrap();
    let secs = ticks as f64 / scale as f64;
    let exact = 999.0 * 1001.0 / 30_000.0;
    assert!(
        (secs - exact).abs() < 1e-6,
        "last clip starts at {secs}, expected {exact}"
    );
}

fn ntsc_frame() -> MediaTime {
    MediaTime {
        ticks: 1001,
        timescale: 30_000,
    }
}

fn ntsc_clip(id: &str) -> TimelineItem {
    TimelineItem::Clip(TimelineClip {
        id: TimelineClipId::new(id),
        media: MediaRefId::new("a"),
        source: SourceRange {
            start: MediaTime {
                ticks: 0,
                timescale: 30_000,
            },
            duration: ntsc_frame(),
        },
        retiming: Retiming::Identity,
        transition_in: None,
        crop: None,
        scale_to: None,
        metadata: Metadata::default(),
    })
}

#[allow(clippy::cast_precision_loss)]
fn layer_start_secs(graph: &reelforge_render_graph::RenderGraph, index: usize) -> f64 {
    let compose = graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.compose.layers" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("compose");
    let layers = compose["layers"].as_array().expect("layers");
    let start = &layers[index]["start"];
    let ticks = start["ticks"].as_i64().unwrap();
    let scale = start["timescale"].as_u64().unwrap();
    ticks as f64 / scale as f64
}

#[test]
fn nested_ntsc_does_not_shift_the_next_clip() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "nested-ntsc");
    p.media.push(MediaRef {
        id: MediaRefId::new("a"),
        uri: "a.mp4".into(),
        duration: Some(MediaTime {
            ticks: 120,
            timescale: 1,
        }),
        role: Some("video".into()),
    });
    let mut child = Sequence::new(SequenceId::new("child"), "child");
    let mut ct = TimelineTrack::new(TimelineTrackId::new("cv"), TrackKind::Video);
    for i in 0..1000 {
        ct.items.push(ntsc_clip(&format!("c{i}")));
    }
    child.tracks.push(ct);
    let mut parent = Sequence::new(SequenceId::new("s"), "main");
    let mut pt = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    pt.items.push(TimelineItem::Nested(NestedSequence {
        sequence: SequenceId::new("child"),
        duration: None,
    }));
    pt.items.push(clip("after", "a", 0.0, 1.0));
    parent.tracks.push(pt);
    p.sequences.push(parent);
    p.sequences.push(child);
    let out = compile_project(&p).unwrap();
    let exact = 1_000.0 * 1_001.0 / 30_000.0;
    let secs = layer_start_secs(&out.graph, 1_000);
    assert!(
        (secs - exact).abs() < 1e-6,
        "clip after the nest starts at {secs}, expected {exact}"
    );
}

#[test]
fn nested_grandchild_keeps_the_ntsc_span() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "nested-grandchild");
    p.media.push(media("a", "a.mp4"));
    let mut grand = Sequence::new(SequenceId::new("grand"), "grand");
    let mut gt = TimelineTrack::new(TimelineTrackId::new("gv"), TrackKind::Video);
    gt.items.push(ntsc_clip("frame"));
    grand.tracks.push(gt);
    let mut child = Sequence::new(SequenceId::new("child"), "child");
    let mut ct = TimelineTrack::new(TimelineTrackId::new("cv"), TrackKind::Video);
    ct.items.push(TimelineItem::Nested(NestedSequence {
        sequence: SequenceId::new("grand"),
        duration: None,
    }));
    child.tracks.push(ct);
    let mut parent = Sequence::new(SequenceId::new("s"), "main");
    let mut pt = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    pt.items.push(TimelineItem::Nested(NestedSequence {
        sequence: SequenceId::new("child"),
        duration: None,
    }));
    pt.items.push(clip("after", "a", 0.0, 1.0));
    parent.tracks.push(pt);
    p.sequences.push(parent);
    p.sequences.push(child);
    p.sequences.push(grand);
    let out = compile_project(&p).unwrap();
    let exact = 1_001.0 / 30_000.0;
    let secs = layer_start_secs(&out.graph, 1);
    assert!(
        (secs - exact).abs() < 1e-9,
        "clip after the grandchild starts at {secs}, expected {exact}"
    );
}

#[test]
fn nested_dissolve_shortens_the_following_clip() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "nested-dissolve");
    p.media.push(media("a", "a.mp4"));
    let mut child = Sequence::new(SequenceId::new("child"), "child");
    let mut ct = TimelineTrack::new(TimelineTrackId::new("cv"), TrackKind::Video);
    ct.items.push(clip("c1", "a", 0.0, 2.0));
    let TimelineItem::Clip(mut c2) = clip("c2", "a", 0.0, 2.0) else {
        panic!("clip");
    };
    c2.transition_in = Some(Transition {
        kind: TransitionKind::Dissolve,
        duration: MediaTime::from_secs(0.5, 1_000).unwrap(),
    });
    ct.items.push(TimelineItem::Clip(c2));
    child.tracks.push(ct);
    let mut parent = Sequence::new(SequenceId::new("s"), "main");
    let mut pt = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    pt.items.push(TimelineItem::Nested(NestedSequence {
        sequence: SequenceId::new("child"),
        duration: None,
    }));
    pt.items.push(clip("after", "a", 0.0, 1.0));
    parent.tracks.push(pt);
    p.sequences.push(parent);
    p.sequences.push(child);
    let out = compile_project(&p).unwrap();
    let compose = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.compose.layers" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("compose");
    let start = &compose["layers"][2]["start"];
    assert_millis(
        start["ticks"].as_i64().unwrap(),
        start["timescale"].as_u64().unwrap(),
        3_500,
    );
}
