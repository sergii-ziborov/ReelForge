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
        crop: None,
        scale_to: None,
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
    let start = &compose["layers"][0]["start"];
    assert_eq!(start["ticks"], 750);
    assert_eq!(start["timescale"], 1000);
}

#[test]
fn wipe_compiles_to_opposing_slides() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "wipe");
    p.media.push(media("a", "a.mp4"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(clip("c1", "a", 0.0, 2.0));
    let TimelineItem::Clip(mut c2) = clip("c2", "a", 0.0, 2.0) else {
        panic!("clip");
    };
    c2.transition_in = Some(reelforge_project::Transition {
        kind: reelforge_project::TransitionKind::Wipe,
        duration: MediaTime::from_secs(0.5, 1_000).unwrap(),
    });
    tr.items.push(TimelineItem::Clip(c2));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let names: Vec<_> = out
        .graph
        .nodes
        .iter()
        .filter_map(|n| match &n.body {
            RenderNodeKind::Op { operation, .. } => Some(operation.as_str()),
            _ => None,
        })
        .collect();
    assert!(names.contains(&"rf.transform.slide_in"));
    assert!(names.contains(&"rf.transform.slide_out"));
    assert!(
        out.warnings.iter().any(|w| w.contains("opposing slides")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn subtitle_track_emits_burn() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "subs");
    p.media.push(media("a", "a.mp4"));
    p.media.push(MediaRef {
        id: MediaRefId::new("s"),
        uri: "talk.srt".into(),
        duration: None,
        role: Some("subtitle".into()),
    });
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut v = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    v.items.push(clip("c1", "a", 0.0, 2.0));
    let mut st = TimelineTrack::new(TimelineTrackId::new("s0"), TrackKind::Subtitle);
    st.items.push(clip("sc", "s", 0.0, 2.0));
    seq.tracks.push(v);
    seq.tracks.push(st);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let burn = out
        .graph
        .nodes
        .iter()
        .find_map(|n| match &n.body {
            RenderNodeKind::Op { operation, params }
                if operation.as_str() == "rf.subtitle.burn" =>
            {
                Some(params)
            }
            _ => None,
        })
        .expect("subtitle burn");
    assert_eq!(burn["cues"][0]["uri"], "talk.srt");
    assert_eq!(burn["cues"][0]["start"]["ticks"], 0);
    assert!(
        !out.warnings.iter().any(|w| w.contains("not compiled")),
        "{:?}",
        out.warnings
    );
}

#[test]
fn query_only_semantic_skips_empty_redaction() {
    let mut p = CaptureProject::new(ProjectId::new("p"), "query");
    p.media.push(media("a", "a.mp4"));
    p.semantic
        .push(SemanticRef::new("query", "who_is_speaking"));
    p.semantic.push(SemanticRef::new("event", "knock"));
    let mut seq = Sequence::new(SequenceId::new("s"), "main");
    let mut tr = TimelineTrack::new(TimelineTrackId::new("v0"), TrackKind::Video);
    tr.items.push(clip("c1", "a", 0.0, 2.0));
    seq.tracks.push(tr);
    p.sequences.push(seq);
    let out = compile_project(&p).unwrap();
    let has_redact = out
        .graph
        .nodes
        .iter()
        .any(|n| matches!(n.body, RenderNodeKind::Redaction { .. }));
    assert!(
        !has_redact,
        "query/event-only must not attach empty redaction"
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
    assert_eq!(adapter["refs"][0]["kind"], "query");
    assert_eq!(adapter["query"][0], "who_is_speaking");
}
