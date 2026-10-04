//! End-to-end `RenderGraph` runner (optional host `FFmpeg`).

use reelforge_core::{Duration, Frame, FrameFormat, MediaTime, Size, Time, VideoClip};
use reelforge_io::{
    GraphRunOptions, ImageClip, OpenVideoOptions, WriteControl, WriteVideoOptions,
    explain_render_graph, ffmpeg_available, materialize_graph, open_video, run_render_graph,
    run_render_graph_with, run_render_graph_with_manifest, write_video,
};
use reelforge_render_graph::{
    GraphOutput, MaskSample, MaskTimeline, MediaAsset, MediaAssetId, NodeId, OperationId,
    RENDER_GRAPH_VERSION, RegionRedaction, RenderGraph, RenderNode, RenderNodeKind,
};
use std::path::{Path, PathBuf};
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

fn write_split_mp4(path: &Path) {
    let mut data = vec![0_u8; 64 * 48 * 3];
    for y in 0..48 {
        for x in 0..64 {
            let i = (y * 64 + x) * 3;
            if x < 32 {
                data[i] = 255;
            } else {
                data[i + 2] = 255;
            }
        }
    }
    let frame = Frame::from_raw(Size::new(64, 48), FrameFormat::Rgb8, data).expect("frame");
    let clip = ImageClip::from_frame(frame, Duration::from_secs(0.4))
        .expect("clip")
        .with_fps(10.0);
    write_video(
        &clip,
        &WriteVideoOptions::new(path.to_string_lossy(), 10.0).with_crf(16),
    )
    .expect("write split");
}

fn rgb_of(path: &Path, x: u32, y: u32) -> [u8; 3] {
    let opened = open_video(&OpenVideoOptions::new(path.to_string_lossy())).expect("open");
    let frame = opened.frame_at(Time::from_secs(0.05)).expect("frame");
    let i = (y as usize * opened.size().width as usize + x as usize) * 3;
    let data = frame.data();
    [data[i], data[i + 1], data[i + 2]]
}

fn assert_red(px: [u8; 3]) {
    assert!(px[0] > 180 && px[1] < 80 && px[2] < 80, "red {px:?}");
}

fn assert_blue(px: [u8; 3]) {
    assert!(px[2] > 180 && px[0] < 80 && px[1] < 80, "blue {px:?}");
}

fn assert_yellow(px: [u8; 3]) {
    assert!(px[0] > 180 && px[1] > 180 && px[2] < 80, "yellow {px:?}");
}

fn assert_cyan(px: [u8; 3]) {
    assert!(px[0] < 80 && px[1] > 180 && px[2] > 180, "cyan {px:?}");
}

fn file_uri(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn source_and_out(src: &str, out: &str, middle: Vec<RenderNode>, tail: &str) -> RenderGraph {
    let mut nodes = vec![RenderNode {
        id: NodeId("src".into()),
        body: RenderNodeKind::Source {
            asset: MediaAssetId("a".into()),
        },
        inputs: vec![],
    }];
    nodes.extend(middle);
    nodes.push(RenderNode {
        id: NodeId("out".into()),
        body: RenderNodeKind::Output {
            name: "main".into(),
        },
        inputs: vec![NodeId(tail.into())],
    });
    RenderGraph {
        version: RENDER_GRAPH_VERSION,
        assets: vec![MediaAsset {
            id: MediaAssetId("a".into()),
            uri: src.into(),
            duration: None,
            role: Some("video".into()),
        }],
        nodes,
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: NodeId("out".into()),
            uri: Some(out.into()),
        }],
    }
}

fn line_graph(src: &Path, out: &Path) -> RenderGraph {
    source_and_out(
        &file_uri(src),
        &file_uri(out),
        vec![
            RenderNode {
                id: NodeId("h".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.transform.hflip"),
                    params: serde_json::json!({}),
                },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("inv".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.color.invert"),
                    params: serde_json::json!({}),
                },
                inputs: vec![NodeId("h".into())],
            },
        ],
        "inv",
    )
}

fn diamond_graph(src: &Path, out: &Path) -> RenderGraph {
    source_and_out(
        &file_uri(src),
        &file_uri(out),
        vec![
            RenderNode {
                id: NodeId("h".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.transform.hflip"),
                    params: serde_json::json!({}),
                },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("v".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.transform.vflip"),
                    params: serde_json::json!({}),
                },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("mix".into()),
                body: RenderNodeKind::Op {
                    operation: OperationId::new("rf.compose.layers"),
                    params: serde_json::json!({
                        "w": 128,
                        "h": 48,
                        "layers": [{ "x": 0 }, { "x": 64 }]
                    }),
                },
                inputs: vec![NodeId("v".into()), NodeId("h".into())],
            },
        ],
        "mix",
    )
}

fn repeated_graph(src: &Path, out: &Path) -> RenderGraph {
    source_and_out(
        &file_uri(src),
        &file_uri(out),
        vec![RenderNode {
            id: NodeId("mix".into()),
            body: RenderNodeKind::Op {
                operation: OperationId::new("rf.compose.layers"),
                params: serde_json::json!({
                    "w": 128,
                    "h": 48,
                    "layers": [{ "x": 0 }, { "x": 64 }]
                }),
            },
            inputs: vec![NodeId("src".into()), NodeId("src".into())],
        }],
        "mix",
    )
}

fn run_both(src: &Path, fast: &Path, slow: &Path, build: fn(&Path, &Path) -> RenderGraph) {
    let mut video_only = GraphRunOptions::new().video_only();
    video_only.crf = Some(16);
    video_only.fps = Some(10.0);
    let mut baseline = GraphRunOptions::new();
    baseline.crf = Some(16);
    baseline.fps = Some(10.0);
    run_render_graph_with(&build(src, fast), &WriteControl::default(), &video_only)
        .expect("video-only path");
    run_render_graph_with(&build(src, slow), &WriteControl::default(), &baseline)
        .expect("baseline path");
}

#[test]
fn fastpath_on_and_off_keep_the_same_picture() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src.mp4");
    write_split_mp4(&src);

    let line_fast = dir.path().join("line-fast.mp4");
    let line_slow = dir.path().join("line-slow.mp4");
    run_both(&src, &line_fast, &line_slow, line_graph);
    assert_yellow(rgb_of(&line_fast, 8, 24));
    assert_yellow(rgb_of(&line_slow, 8, 24));
    assert_cyan(rgb_of(&line_fast, 56, 24));
    assert_cyan(rgb_of(&line_slow, 56, 24));

    let dia_fast = dir.path().join("dia-fast.mp4");
    let dia_slow = dir.path().join("dia-slow.mp4");
    run_both(&src, &dia_fast, &dia_slow, diamond_graph);
    assert_red(rgb_of(&dia_fast, 8, 24));
    assert_blue(rgb_of(&dia_fast, 72, 24));
    assert_red(rgb_of(&dia_slow, 8, 24));
    assert_blue(rgb_of(&dia_slow, 72, 24));

    let rep_fast = dir.path().join("rep-fast.mp4");
    let rep_slow = dir.path().join("rep-slow.mp4");
    run_both(&src, &rep_fast, &rep_slow, repeated_graph);
    assert_red(rgb_of(&rep_fast, 8, 24));
    assert_blue(rgb_of(&rep_fast, 96, 24));
    assert_red(rgb_of(&rep_slow, 8, 24));
    assert_blue(rgb_of(&rep_slow, 96, 24));
}
