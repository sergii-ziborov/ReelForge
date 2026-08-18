//! `reelforge graph` — Host encode path (no live `SightLoom`).

use reelforge::{
    GraphOutput, MaskSample, MaskTimeline, MediaAsset, MediaAssetId, MediaTime, NodeId,
    RENDER_GRAPH_VERSION, RegionRedaction, RenderGraph, RenderNode, RenderNodeKind,
    ffmpeg_available,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_reelforge")
}

fn skip_without_ffmpeg() -> bool {
    if ffmpeg_available() {
        false
    } else {
        eprintln!("skipping: ffmpeg not available");
        true
    }
}

fn gen_color_mp4(path: &Path) {
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=white:s=64x64:d=0.4:r=10",
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

fn inline_redaction_graph(input: &Path, output: &Path) -> RenderGraph {
    let mut masks = MaskTimeline::new();
    masks.push(MaskSample::ellipse(
        MediaTime::new(0, 10).unwrap(),
        32.0,
        32.0,
        12.0,
    ));
    RenderGraph {
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
                id: NodeId("blur".into()),
                body: RenderNodeKind::Redaction {
                    redaction: RegionRedaction::gaussian(masks, 8.0),
                },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("out".into()),
                body: RenderNodeKind::Output {
                    name: "main".into(),
                },
                inputs: vec![NodeId("blur".into())],
            },
        ],
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: NodeId("out".into()),
            uri: Some(output.to_string_lossy().into_owned()),
        }],
    }
}

fn package_graph(input: &Path, output: &Path, package_id: &str) -> RenderGraph {
    RenderGraph {
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
                id: NodeId("vision".into()),
                body: RenderNodeKind::Op {
                    operation: reelforge::OperationId::new("rf.adapter.sightloom"),
                    params: serde_json::json!({ "package_id": package_id }),
                },
                inputs: vec![NodeId("src".into())],
            },
            RenderNode {
                id: NodeId("blur".into()),
                body: RenderNodeKind::Redaction {
                    redaction: RegionRedaction {
                        masks: MaskTimeline::new(),
                        style: reelforge::RedactionStyle::Solid {
                            color: reelforge::Rgba8::new(0, 0, 0, 255),
                        },
                    },
                },
                inputs: vec![NodeId("vision".into())],
            },
            RenderNode {
                id: NodeId("out".into()),
                body: RenderNodeKind::Output {
                    name: "main".into(),
                },
                inputs: vec![NodeId("blur".into())],
            },
        ],
        outputs: vec![GraphOutput {
            name: "main".into(),
            node: NodeId("out".into()),
            uri: Some(output.to_string_lossy().into_owned()),
        }],
    }
}

fn write_mask_package(dir: &Path, package_id: &str) {
    std::fs::create_dir_all(dir.join("masks")).unwrap();
    let mut blob = vec![0_u8; 64 * 64];
    for y in 20..44 {
        for x in 20..44 {
            blob[y * 64 + x] = 255;
        }
    }
    std::fs::write(dir.join("masks/1.bin"), &blob).unwrap();
    let manifest = serde_json::json!({
        "package_id": package_id,
        "tracks": [{
            "id": "face",
            "samples": [{
                "t": 0.0, "cx": 32.0, "cy": 32.0, "radius": 16.0,
                "observation": "1",
                "mask": { "observation": "1", "uri": "masks/1.bin" }
            }]
        }],
        "masks": [{
            "mask_ref": 1,
            "kind": "dense",
            "width": 64, "height": 64,
            "path": "masks/1.bin"
        }]
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
}

#[test]
fn graph_explain_prints_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("src.mp4");
    let output = dir.path().join("out.mp4");
    let graph_path = dir.path().join("graph.json");
    let g = inline_redaction_graph(&input, &output);
    std::fs::write(&graph_path, g.to_json_pretty().unwrap()).unwrap();

    let out = Command::new(bin())
        .args(["graph", "--explain", graph_path.to_str().unwrap()])
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "explain failed: {stdout}{stderr}");
    assert!(stdout.contains("execution_stages"), "{stdout}");
    assert!(stdout.contains("output:"), "{stdout}");
}

#[test]
fn graph_run_inline_redaction_writes_mp4() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("src.mp4");
    let output = dir.path().join("out.mp4");
    let graph_path = dir.path().join("graph.json");
    gen_color_mp4(&input);
    let g = inline_redaction_graph(&input, &PathBuf::from("will-be-overridden.mp4"));
    std::fs::write(&graph_path, g.to_json_pretty().unwrap()).unwrap();

    let status = Command::new(bin())
        .args([
            "graph",
            "--run",
            graph_path.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert!(status.success(), "graph --run inline failed");
    assert!(output.is_file(), "output missing");
    assert!(output.metadata().unwrap().len() > 0);
}

#[test]
fn graph_run_mask_package_writes_mp4() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("src.mp4");
    let output = dir.path().join("out.mp4");
    let graph_path = dir.path().join("graph.json");
    let pkg = dir.path().join("pkg");
    gen_color_mp4(&input);
    write_mask_package(&pkg, "pkg-cli");
    let g = package_graph(&input, &output, "pkg-cli");
    std::fs::write(&graph_path, g.to_json_pretty().unwrap()).unwrap();

    let status = Command::new(bin())
        .args([
            "graph",
            "--run",
            graph_path.to_str().unwrap(),
            "--mask-package",
            pkg.to_str().unwrap(),
        ])
        .status()
        .expect("spawn");
    assert!(status.success(), "graph --run --mask-package failed");
    assert!(output.is_file(), "output missing");
    assert!(output.metadata().unwrap().len() > 0);
}

#[test]
fn graph_run_package_id_mismatch_fails() {
    if skip_without_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("src.mp4");
    let output = dir.path().join("out.mp4");
    let graph_path = dir.path().join("graph.json");
    let pkg = dir.path().join("pkg");
    gen_color_mp4(&input);
    write_mask_package(&pkg, "pkg-on-disk");
    let g = package_graph(&input, &output, "pkg-from-graph");
    std::fs::write(&graph_path, g.to_json_pretty().unwrap()).unwrap();

    let out = Command::new(bin())
        .args([
            "graph",
            "--run",
            graph_path.to_str().unwrap(),
            "--mask-package",
            pkg.to_str().unwrap(),
        ])
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "mismatch must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("pkg-on-disk"), "{err}");
    assert!(err.contains("pkg-from-graph"), "{err}");
    assert!(!output.is_file(), "must not write on mismatch");
}
