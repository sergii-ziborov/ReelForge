//! End-to-end `RenderGraph` runner (optional host `FFmpeg`).

use reelforge_core::MediaTime;
use reelforge_io::{
    GraphRunOptions, WriteControl, explain_render_graph, ffmpeg_available, materialize_graph,
    run_render_graph, run_render_graph_with_manifest,
};
use reelforge_render_graph::{
    GraphOutput, MaskSample, MaskTimeline, MediaAsset, MediaAssetId, NodeId, OperationId,
    RENDER_GRAPH_VERSION, RegionRedaction, RenderGraph, RenderNode, RenderNodeKind,
};
use std::path::PathBuf;
use std::process::Command;

fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available() {
        false
    } else {
        eprintln!("skipping: ffmpeg/ffprobe not available");
        true
    }
}

fn gen_color_mp4(path: &PathBuf) {
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=white:s=64x64:d=1:r=10",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-crf",
            "28",
        ])
        .arg(path)
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "ffmpeg color source failed: {status}");
}

#[test]
fn explain_and_materialize_seed_free_requires_file() {
    let g = RenderGraph {
        version: RENDER_GRAPH_VERSION,
        assets: vec![MediaAsset {
            id: MediaAssetId("a".into()),
            uri: "definitely-missing-file.mp4".into(),
            duration: None,
            role: None,
        }],
        nodes: vec![
            RenderNode {
                id: NodeId("src".into()),
                body: RenderNodeKind::Source {
                    asset: MediaAssetId("a".into()),
                },
                inputs: vec![],
            },
            RenderNode {
                id: NodeId("out".into()),
                body: RenderNodeKind::Output {
                    name: "main".into(),
                },
                inputs: vec![NodeId("src".into())],
            },
        ],
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: NodeId("out".into()),
            uri: Some("out.mp4".into()),
        }],
    };
    let text = explain_render_graph(&g).expect("explain");
    assert!(text.contains("execution_stages"));
    assert!(materialize_graph(&g).is_err());
}

#[test]
fn run_graph_trim_redaction_encode() {
    if skip_without_ffmpeg() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let input = dir.path().join("src.mp4");
    let output = dir.path().join("out.mp4");
    gen_color_mp4(&input);

    let mut masks = MaskTimeline::new();
    masks.push(MaskSample::ellipse(
        MediaTime::new(0, 10).unwrap(),
        32.0,
        32.0,
        12.0,
    ));

    let g = RenderGraph {
        version: RENDER_GRAPH_VERSION,
        assets: vec![MediaAsset {
            id: MediaAssetId("a".into()),
            uri: input.to_string_lossy().into_owned(),
            duration: None,
            role: Some("video".into()),
        }],
        nodes: vec![
            RenderNode {
                id: NodeId("src".into()),
                body: RenderNodeKind::Source {
                    asset: MediaAssetId("a".into()),
                },
                inputs: vec![],
            },
            RenderNode {
                id: NodeId("trim".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.transform.trim"),
                    params: serde_json::json!({ "start": 0.0, "duration": 0.5 }),
                },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("flip".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.transform.hflip"),
                    params: serde_json::json!({}),
                },
                inputs: vec![NodeId("trim".into())],
            },
            RenderNode {
                id: NodeId("blur".into()),
                body: RenderNodeKind::Redaction {
                    redaction: RegionRedaction::gaussian(masks, 8.0),
                },
                inputs: vec![NodeId("flip".into())],
            },
            RenderNode {
                id: NodeId("enc".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.encode.h264"),
                    params: serde_json::json!({ "crf": 30 }),
                },
                inputs: vec![NodeId("blur".into())],
            },
            RenderNode {
                id: NodeId("out".into()),
                body: RenderNodeKind::Output {
                    name: "main".into(),
                },
                inputs: vec![NodeId("enc".into())],
            },
        ],
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: NodeId("out".into()),
            uri: Some(output.to_string_lossy().into_owned()),
        }],
    };

    let man =
        run_render_graph_with_manifest(&g, &WriteControl::default(), &GraphRunOptions::default())
            .expect("run_render_graph_with_manifest");
    assert!(output.is_file(), "output missing");
    assert!(output.metadata().unwrap().len() > 0);
    assert_eq!(man.outputs.len(), 1);
    assert_eq!(
        man.outputs[0].uri.as_deref(),
        Some(output.to_string_lossy().as_ref())
    );
    assert!(
        man.outputs[0].file_fingerprint.is_some(),
        "expected sealed file fingerprint"
    );
    assert!(man.run_fingerprint.is_some());
}

#[test]
fn run_graph_pixelate_style() {
    if skip_without_ffmpeg() {
        return;
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let input = dir.path().join("src.mp4");
    let output = dir.path().join("pix.mp4");
    gen_color_mp4(&input);

    let mut masks = MaskTimeline::new();
    masks.push(MaskSample::ellipse(
        MediaTime::new(0, 10).unwrap(),
        32.0,
        32.0,
        16.0,
    ));
    let redaction = RegionRedaction {
        masks,
        style: reelforge_render_graph::RedactionStyle::Pixelate { block_size: 8 },
    };

    let g = RenderGraph {
        version: RENDER_GRAPH_VERSION,
        assets: vec![MediaAsset {
            id: MediaAssetId("a".into()),
            uri: input.to_string_lossy().into_owned(),
            duration: None,
            role: None,
        }],
        nodes: vec![
            RenderNode {
                id: NodeId("src".into()),
                body: RenderNodeKind::Source {
                    asset: MediaAssetId("a".into()),
                },
                inputs: vec![],
            },
            RenderNode {
                id: NodeId("pix".into()),
                body: RenderNodeKind::Redaction { redaction },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("out".into()),
                body: RenderNodeKind::Output {
                    name: "main".into(),
                },
                inputs: vec![NodeId("pix".into())],
            },
        ],
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: NodeId("out".into()),
            uri: Some(output.to_string_lossy().into_owned()),
        }],
    };

    run_render_graph(&g).expect("pixelate graph");
    assert!(output.is_file());
}
