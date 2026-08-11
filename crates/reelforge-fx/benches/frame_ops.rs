//! Criterion benches for common frame-graph operations.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use reelforge_compose::{CompositeLayer, CompositeVideo};
use reelforge_core::{ColorClip, Duration, Position, Rgb8, Size, Time, VideoClip, VideoEffect};
use reelforge_fx::{BlackAndWhite, Crop, FadeIn, Resize};
use std::sync::Arc;
use std::time::Duration as StdDuration;

fn sample_clip(size: Size, secs: f64) -> Arc<dyn VideoClip> {
    Arc::new(
        ColorClip::new(size, Rgb8::new(40, 80, 160), Duration::from_secs(secs)).with_fps(24.0),
    )
}

fn bench_frame_graph(c: &mut Criterion) {
    let mut g = c.benchmark_group("frame_graph");
    g.measurement_time(StdDuration::from_secs(3));
    g.sample_size(30);

    let base = sample_clip(Size::new(1280, 720), 5.0);
    let cropped = Crop::new(100, 50, 960, 540).apply(base).unwrap();
    let resized = Resize::to(Size::new(640, 360)).apply(cropped).unwrap();
    let faded = FadeIn::new(Duration::from_secs(0.5)).apply(resized).unwrap();
    let gray = BlackAndWhite.apply(faded).unwrap();

    g.bench_function("720p_chain_frame_at", |b| {
        b.iter(|| {
            let f = gray.frame_at(Time::from_secs(0.25)).unwrap();
            black_box(f.data().len())
        });
    });

    let layer_a = CompositeLayer::new(sample_clip(Size::new(640, 360), 2.0));
    let layer_b = CompositeLayer::new(sample_clip(Size::new(200, 100), 2.0))
        .with_position(Position::center())
        .with_layer_index(1)
        .with_opacity(0.7);
    let comp = CompositeVideo::new(Size::new(640, 360), vec![layer_a, layer_b]).unwrap();

    g.bench_function("composite_two_layers_frame_at", |b| {
        b.iter(|| {
            let f = comp.frame_at(Time::from_secs(0.1)).unwrap();
            black_box(f.data().len())
        });
    });

    g.finish();
}

fn bench_solid_frames(c: &mut Criterion) {
    let mut g = c.benchmark_group("solid_frames");
    g.measurement_time(StdDuration::from_secs(2));
    g.sample_size(40);

    let clip = sample_clip(Size::new(1920, 1080), 1.0);
    g.bench_function("1080p_color_frame", |b| {
        b.iter(|| {
            let f = clip.frame_at(Time::ZERO).unwrap();
            black_box(f.data()[0])
        });
    });

    g.finish();
}

criterion_group!(benches, bench_frame_graph, bench_solid_frames);
criterion_main!(benches);
