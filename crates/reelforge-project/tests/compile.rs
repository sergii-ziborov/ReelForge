//! `CaptureProject` compile: trim, speed, dissolve, mix, ticks.

use reelforge_core::MediaTime;
use reelforge_project::{
    CAPTURE_PROJECT_VERSION, CaptureProject, Gap, MediaRef, MediaRefId, Metadata, ProjectId,
    Retiming, SemanticRef, Sequence, SequenceId, SourceRange, TimelineClip, TimelineClipId,
    TimelineItem, TimelineTrack, TimelineTrackId, TrackKind, Transition, TransitionKind,
    compile_project,
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
    let start = compose["layers"][0]["start"].as_f64().unwrap();
    assert!((start - 1.5).abs() < 1e-9);
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
    assert!(names.contains(&"rf.transform.fade_in"));
    assert!(names.contains(&"rf.transform.fade_out"));
    assert!(names.contains(&"rf.compose.layers"));
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
