//! Criterion benches for `RenderPlan` optimize + `FFmpeg` extraction (no encode).
#![allow(missing_docs)]

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use reelforge_io::{PlanOp, RenderPlan, extract_ffmpeg, optimize_plan};
use std::time::Duration as StdDuration;

fn sample_plan(n_pairs: usize) -> RenderPlan {
    let mut plan = RenderPlan::from_file("bench_input.mp4");
    plan = plan.then(PlanOp::Trim {
        start: 0.5,
        duration: 30.0,
    });
    for i in 0..n_pairs {
        #[allow(clippy::cast_possible_truncation)]
        let i_u = (i % 256) as u32;
        plan = plan
            .then(PlanOp::Identity)
            .then(PlanOp::Crop {
                x: i_u % 8,
                y: i_u % 4,
                w: 1920 - (i_u % 8) * 2,
                h: 1080 - (i_u % 4) * 2,
            })
            .then(PlanOp::Scale {
                w: 1280,
                h: 720,
            })
            .then(PlanOp::Scale {
                w: 640,
                h: 360,
            })
            .then(PlanOp::HFlip)
            .then(PlanOp::HFlip)
            .then(PlanOp::VFlip);
    }
    plan = plan.then(PlanOp::EvenDims);
    plan
}

fn hybrid_plan() -> RenderPlan {
    RenderPlan::from_file("in.mp4")
        .then(PlanOp::Trim {
            start: 1.0,
            duration: 10.0,
        })
        .then(PlanOp::Crop {
            x: 10,
            y: 10,
            w: 1900,
            h: 1060,
        })
        .then(PlanOp::Scale { w: 1280, h: 720 })
        .then(PlanOp::HFlip)
        .then(PlanOp::Custom {
            name: "head_blur".into(),
            params: Some(serde_json::json!({"radius": 32})),
        })
        .then(PlanOp::Scale { w: 640, h: 360 })
        .then(PlanOp::EvenDims)
}

fn bench_render_plan(c: &mut Criterion) {
    let mut g = c.benchmark_group("render_plan");
    g.measurement_time(StdDuration::from_secs(2));
    g.sample_size(50);

    let small = sample_plan(4);
    let large = sample_plan(64);
    let hybrid = hybrid_plan();

    g.bench_function("optimize_small", |b| {
        b.iter(|| {
            let o = optimize_plan(black_box(&small));
            black_box(o.stats.after)
        });
    });

    g.bench_function("optimize_large_64_pairs", |b| {
        b.iter(|| {
            let o = optimize_plan(black_box(&large));
            black_box(o.stats.eliminated())
        });
    });

    g.bench_function("extract_small", |b| {
        b.iter(|| {
            let e = extract_ffmpeg(black_box(&small));
            black_box(e.ffmpeg_op_count)
        });
    });

    g.bench_function("extract_large_64_pairs", |b| {
        b.iter(|| {
            let e = extract_ffmpeg(black_box(&large));
            black_box((e.fully_ffmpeg, e.ffmpeg_op_count))
        });
    });

    g.bench_function("extract_hybrid_custom", |b| {
        b.iter(|| {
            let e = extract_ffmpeg(black_box(&hybrid));
            black_box((e.fully_ffmpeg, e.remainder_op_count))
        });
    });

    g.bench_function("json_roundtrip_small", |b| {
        let text = small.to_json().expect("json");
        b.iter(|| {
            let p = RenderPlan::from_json(black_box(&text)).expect("parse");
            black_box(p.ops.len())
        });
    });

    g.finish();
}

criterion_group!(benches, bench_render_plan);
criterion_main!(benches);
