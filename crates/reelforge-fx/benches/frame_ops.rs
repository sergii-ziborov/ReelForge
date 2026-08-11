//! Criterion benches for common frame-graph operations (720p / 4K / 8K).
//!
//! UHD graphs allocate large RGB buffers; they run in a separate group so
//! 720p timings are not inflated by memory pressure.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use reelforge_compose::{CompositeLayer, CompositeVideo};
use reelforge_core::{ColorClip, Duration, Position, Rgb8, Size, Time, VideoClip, VideoEffect};
use reelforge_fx::{BlackAndWhite, Crop, FadeIn, Resize, ResizeFilter};
use std::sync::Arc;
use std::time::Duration as StdDuration;

fn sample_clip(size: Size, secs: f64) -> Arc<dyn VideoClip> {
    Arc::new(
        ColorClip::new(size, Rgb8::new(40, 80, 160), Duration::from_secs(secs)).with_fps(24.0),
    )
}

fn chain(
    base: Arc<dyn VideoClip>,
    crop: (u32, u32, u32, u32),
    target: Size,
    filter: ResizeFilter,
) -> Arc<dyn VideoClip> {
    let cropped = Crop::new(crop.0, crop.1, crop.2, crop.3).apply(base).unwrap();
    let resized = Resize::to(target).with_filter(filter).apply(cropped).unwrap();
    let faded = FadeIn::new(Duration::from_secs(0.5)).apply(resized).unwrap();
    BlackAndWhite.apply(faded).unwrap()
}

fn bench_frame_graph_hd(c: &mut Criterion) {
    let mut g = c.benchmark_group("frame_graph_hd");
    g.measurement_time(StdDuration::from_secs(3));
    g.sample_size(30);

    let chain_720 = chain(
        sample_clip(Size::HD_720, 5.0),
        (100, 50, 960, 540),
        Size::new(640, 360),
        ResizeFilter::Nearest,
    );
    g.bench_function("720p_chain_frame_at", |b| {
        b.iter(|| {
            let f = chain_720.frame_at(Time::from_secs(0.25)).unwrap();
            black_box(f.data().len())
        });
    });

    let chain_720_bi = chain(
        sample_clip(Size::HD_720, 5.0),
        (100, 50, 960, 540),
        Size::new(640, 360),
        ResizeFilter::Bilinear,
    );
    g.bench_function("720p_chain_bilinear_frame_at", |b| {
        b.iter(|| {
            let f = chain_720_bi.frame_at(Time::from_secs(0.25)).unwrap();
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

fn bench_frame_graph_uhd(c: &mut Criterion) {
    let mut g = c.benchmark_group("frame_graph_uhd");
    g.measurement_time(StdDuration::from_secs(4));
    g.sample_size(20);

    // 4K crop → 1080p resize → fade → B&W (nearest speed path)
    {
        let chain_4k = chain(
            sample_clip(Size::UHD_4K, 5.0),
            (120, 60, 3200, 1800),
            Size::HD_1080,
            ResizeFilter::Nearest,
        );
        g.bench_function("4k_chain_to_1080_frame_at", |b| {
            b.iter(|| {
                let f = chain_4k.frame_at(Time::from_secs(0.25)).unwrap();
                black_box(f.data().len())
            });
        });
    }

    {
        let chain_4k_bi = chain(
            sample_clip(Size::UHD_4K, 5.0),
            (120, 60, 3200, 1800),
            Size::HD_1080,
            ResizeFilter::Bilinear,
        );
        g.bench_function("4k_chain_to_1080_bilinear_frame_at", |b| {
            b.iter(|| {
                let f = chain_4k_bi.frame_at(Time::from_secs(0.25)).unwrap();
                black_box(f.data().len())
            });
        });
    }

    // Near-full 4K path
    {
        let chain_4k_full = chain(
            sample_clip(Size::UHD_4K, 5.0),
            (40, 20, 3760, 2120),
            Size::new(3200, 1800),
            ResizeFilter::Nearest,
        );
        g.bench_function("4k_chain_near_full_frame_at", |b| {
            b.iter(|| {
                let f = chain_4k_full.frame_at(Time::from_secs(0.25)).unwrap();
                black_box(f.data().len())
            });
        });
    }

    // 8K → 1080p downscale chain
    {
        let chain_8k = chain(
            sample_clip(Size::UHD_8K, 5.0),
            (200, 100, 6400, 3600),
            Size::HD_1080,
            ResizeFilter::Nearest,
        );
        g.bench_function("8k_chain_to_1080_frame_at", |b| {
            b.iter(|| {
                let f = chain_8k.frame_at(Time::from_secs(0.25)).unwrap();
                black_box(f.data().len())
            });
        });
    }

    g.finish();
}

fn bench_solid_frames(c: &mut Criterion) {
    let mut g = c.benchmark_group("solid_frames");
    g.measurement_time(StdDuration::from_secs(2));
    g.sample_size(40);

    for (name, size) in [
        ("1080p_color_frame", Size::HD_1080),
        ("4k_color_frame", Size::UHD_4K),
        ("8k_color_frame", Size::UHD_8K),
    ] {
        let clip = sample_clip(size, 1.0);
        g.bench_function(name, |b| {
            b.iter(|| {
                let f = clip.frame_at(Time::ZERO).unwrap();
                black_box(f.data()[0])
            });
        });
        // Drop large solid before next size.
        drop(clip);
    }

    g.finish();
}

criterion_group!(
    benches,
    bench_frame_graph_hd,
    bench_frame_graph_uhd,
    bench_solid_frames
);
criterion_main!(benches);
