//! Semantic policy, muted tracks, nested sequences.

use reelforge_core::MediaTime;
use reelforge_project::{
    CaptureProject, Gap, MediaRef, MediaRefId, Metadata, NestedSequence, ProjectId, Retiming,
    SemanticRef, Sequence, SequenceId, SourceRange, TimelineClip, TimelineClipId, TimelineItem,
    TimelineTrack, TimelineTrackId, TrackKind, compile_project,
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

#[test]
fn semantic_policy_emits_adapter_and_redaction() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "policy");
    p.media.push(media("a", "a.mp4"));
    p.semantic.push(SemanticRef::new("subject", "person_a"));
    p.semantic.push(SemanticRef::new("policy", "blur_faces"));
    p.semantic
        .push(SemanticRef::new("query", "who_is_speaking"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(clip("c1", "a", 0.0, 2.0));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
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
    assert_eq!(
        kinds,
        [
            "src",
            "rf.transform.trim",
            "rf.adapter.sightloom",
            "redact",
            "out"
        ]
    );
    let adapter = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.adapter.sightloom" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("adapter");
    assert_eq!(adapter["subjects"][0], "person_a");
    assert_eq!(adapter["policy"][0], "blur_faces");
    assert_eq!(adapter["query"][0], "who_is_speaking");
    assert!(adapter.get("events").is_none());
    out.graph.validate().unwrap();
}

#[test]
fn muted_video_is_skipped() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "mute");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut muted = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    muted.muted = true;
    muted.items.push(clip("c1", "a", 0.0, 2.0));
    let mut live = TimelineTrack::new(TimelineTrackId::new("v1"), TrackKind::Video);
    live.items.push(clip("c2", "a", 0.0, 1.0));
    seq.tracks.push(muted);
    seq.tracks.push(live);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("video track v0 is muted"))
    );
    assert_eq!(
        out.graph
            .nodes
            .iter()
            .filter(|n| matches!(n.body, RenderNodeKind::Source { .. }))
            .count(),
        1
    );
}

#[test]
fn nested_sequence_offsets_child() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "nest");
    p.media.push(media("a", "a.mp4"));
    let mut child = Sequence::new(SequenceId::new("child"), "child");
    let mut ct = TimelineTrack::new(TimelineTrackId::new("cv"), TrackKind::Video);
    ct.items.push(clip("cc", "a", 0.0, 1.0));
    child.tracks.push(ct);
    let mut parent = Sequence::new(SequenceId::new("s"), "main");
    let mut pt = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    pt.items.push(TimelineItem::Gap(Gap {
        duration: MediaTime::from_secs(0.75, 1_000).unwrap(),
    }));
    pt.items.push(TimelineItem::Nested(NestedSequence {
        sequence: SequenceId::new("child"),
        duration: None,
    }));
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
    let start = compose["layers"][0]["start"].as_f64().unwrap();
    assert!((start - 0.75).abs() < 1e-9);
}
