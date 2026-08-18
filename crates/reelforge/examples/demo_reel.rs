//! Designed 0.2 reel: gradient scenes, a readable subject, plate titles, privacy.
//!
//! No lavfi test patterns and no intermediate encode — one `write_video` at the end.
//!
//! ```bash
//! cargo run -p reelforge --example demo_reel --release
//! cargo run -p reelforge --example demo_reel --release -- target/demo/reelforge-0.2.mp4
//! ```
#![allow(clippy::print_stdout, clippy::print_stderr)]

use reelforge::fx::{
    FadeIn, FadeOut, RegionSample, RegionTrack, SlideIn, SlideSide, TrackSet, TrackedPrivacy,
};
use reelforge::io::{ImageClip, WriteVideoOptions, ffmpeg_available, write_video};
use reelforge::prelude::*;
use reelforge::text::{TextClip, TextClipOptions};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const FPS: f64 = 24.0;
const SCENE_A: f64 = 4.5;
const SCENE_B: f64 = 4.0;

/// Face layer origin on the 1080p canvas (privacy track uses the same center).
const FACE_X: i32 = 1320;
const FACE_Y: i32 = 260;
const FACE_W: u32 = 420;
const FACE_H: u32 = 500;
const FACE_CX: f32 = 1530.0;
const FACE_CY: f32 = 470.0;
const FACE_R: f32 = 168.0;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if !ffmpeg_available() {
        return Err("host ffmpeg/ffprobe not on PATH (or REELFORGE_FFMPEG)".into());
    }

    let out = env::args().nth(1).map_or_else(
        || PathBuf::from("target/demo/reelforge-0.2.mp4"),
        PathBuf::from,
    );
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }

    println!("=== ReelForge 0.2 demo reel ===");
    let scene_a = FadeIn::new(Duration::from_secs(0.5)).apply(scene_privacy()?)?;
    let scene_b =
        SlideIn::new(Duration::from_secs(0.5), SlideSide::Right).apply(scene_compile()?)?;
    let reel = FadeOut::new(Duration::from_secs(0.55))
        .apply(concatenate_video(vec![scene_a, scene_b])?)?;

    println!(
        "graph : {}x{}  {:.2}s  → {}",
        reel.size().width,
        reel.size().height,
        reel.duration().as_secs(),
        out.display()
    );
    let t0 = Instant::now();
    write_video(
        reel.as_ref(),
        &WriteVideoOptions::new(out.to_string_lossy(), FPS).with_crf(18),
    )?;
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let bytes = std::fs::metadata(&out).map_or(0, |m| m.len());
    println!("wrote : {bytes} bytes  encode={ms:.0} ms");
    println!("open  : {}", out.canonicalize().unwrap_or(out).display());
    Ok(())
}

fn scene_privacy() -> Result<Arc<dyn VideoClip>, Box<dyn std::error::Error>> {
    let dur = Duration::from_secs(SCENE_A);
    let bg = gradient_clip(
        dur,
        Rgb8::new(10, 18, 34),
        Rgb8::new(18, 78, 92),
        Rgb8::new(28, 48, 80),
    )?;
    let face = Arc::new(Still {
        frame: paint_subject()?,
        mask: Some(paint_subject_mask()?),
        duration: dur,
    });
    let picture = composite_video(
        Size::HD_1080,
        vec![
            CompositeLayer::new(bg),
            CompositeLayer::new(face)
                .with_position(Position::absolute(FACE_X, FACE_Y))
                .with_layer_index(1),
        ],
    )?;
    let mut track = RegionTrack::new("subject").with_kind("face");
    track.push(RegionSample::new(0.0, FACE_CX, FACE_CY, FACE_R));
    track.push(RegionSample::new(
        SCENE_A,
        FACE_CX + 6.0,
        FACE_CY + 4.0,
        FACE_R,
    ));
    let mut tracks = TrackSet::new();
    tracks.push(track);
    let redacted = TrackedPrivacy::gaussian(tracks, 22.0).apply(picture)?;
    titled(
        redacted,
        dur,
        "ReelForge 0.2",
        "stable contracts   ·   tracked privacy",
        96,
        390,
    )
}

fn scene_compile() -> Result<Arc<dyn VideoClip>, Box<dyn std::error::Error>> {
    let dur = Duration::from_secs(SCENE_B);
    let bg = gradient_clip(
        dur,
        Rgb8::new(24, 14, 18),
        Rgb8::new(96, 54, 32),
        Rgb8::new(48, 28, 36),
    )?;
    titled(
        bg,
        dur,
        "Timeline compile",
        "MediaTime ticks   ·   wipe → slides   ·   stage resume",
        200,
        400,
    )
}

fn titled(
    base: Arc<dyn VideoClip>,
    dur: Duration,
    title: &str,
    kicker: &str,
    plate_x: i32,
    plate_y: i32,
) -> Result<Arc<dyn VideoClip>, Box<dyn std::error::Error>> {
    let font = title_font();
    let plate =
        Arc::new(ColorClip::new(Size::new(980, 268), Rgb8::new(8, 10, 16), dur).with_fps(FPS));
    let shadow =
        Arc::new(ColorClip::new(Size::new(980, 268), Rgb8::new(0, 0, 0), dur).with_fps(FPS));
    let head = TextClip::new(
        &TextClipOptions::new(title, 68, dur)
            .with_font_path(font)
            .with_color(Rgba8::new(248, 250, 252, 255))
            .with_padding(12),
    )?;
    let sub = TextClip::new(
        &TextClipOptions::new(kicker, 26, dur)
            .with_font_path(font)
            .with_color(Rgba8::new(186, 198, 210, 255))
            .with_padding(10),
    )?;
    Ok(composite_video(
        Size::HD_1080,
        vec![
            CompositeLayer::new(base),
            CompositeLayer::new(shadow)
                .with_position(Position::absolute(plate_x + 8, plate_y + 10))
                .with_opacity(0.35)
                .with_layer_index(1),
            CompositeLayer::new(plate)
                .with_position(Position::absolute(plate_x, plate_y))
                .with_opacity(0.78)
                .with_layer_index(2),
            CompositeLayer::new(Arc::new(head))
                .with_position(Position::absolute(plate_x + 48, plate_y + 58))
                .with_layer_index(3),
            CompositeLayer::new(Arc::new(sub))
                .with_position(Position::absolute(plate_x + 52, plate_y + 160))
                .with_layer_index(4),
        ],
    )?)
}

fn gradient_clip(
    duration: Duration,
    top: Rgb8,
    bottom: Rgb8,
    orb: Rgb8,
) -> Result<Arc<dyn VideoClip>, Box<dyn std::error::Error>> {
    let frame = paint_gradient(Size::HD_1080, top, bottom, orb)?;
    Ok(Arc::new(
        ImageClip::from_frame(frame, duration)?.with_fps(FPS),
    ))
}

struct Still {
    frame: Frame,
    mask: Option<Mask>,
    duration: Duration,
}

impl VideoClip for Still {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.frame.size()
    }

    fn fps(&self) -> Option<f64> {
        Some(FPS)
    }

    fn frame_at(&self, t: Time) -> Result<Frame, CoreError> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        Ok(self.frame.clone())
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>, CoreError> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        Ok(self.mask.clone())
    }
}

fn title_font() -> &'static str {
    const CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/System/Library/Fonts/Supplemental/Arial.ttf",
    ];
    CANDIDATES
        .iter()
        .copied()
        .find(|p| Path::new(p).is_file())
        .unwrap_or(reelforge::text::BITMAP_FONT)
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn paint_gradient(size: Size, top: Rgb8, bottom: Rgb8, orb: Rgb8) -> Result<Frame, CoreError> {
    let w = size.width as usize;
    let h = size.height as usize;
    let mut data = vec![0_u8; w * h * 3];
    let inv_h = 1.0 / (h.saturating_sub(1).max(1) as f32);
    let inv_w = 1.0 / (w.saturating_sub(1).max(1) as f32);
    let orb_x = w as f32 * 0.72;
    let orb_y = h as f32 * 0.38;
    let orb_r = h as f32 * 0.42;
    for y in 0..h {
        let ty = y as f32 * inv_h;
        let row_r = lerp_u8(top.r, bottom.r, ty);
        let row_g = lerp_u8(top.g, bottom.g, ty);
        let row_b = lerp_u8(top.b, bottom.b, ty);
        for x in 0..w {
            let tx = x as f32 * inv_w;
            let vig = (1.0 - 0.22 * ((tx - 0.5).mul_add(tx - 0.5, (ty - 0.5) * (ty - 0.5)) * 3.2))
                .clamp(0.72, 1.0);
            let dx = x as f32 - orb_x;
            let dy = y as f32 - orb_y;
            let fall = (1.0 - (dx.mul_add(dx, dy * dy).sqrt() / orb_r)).clamp(0.0, 1.0);
            let mix = fall * fall * 0.28;
            let i = (y * w + x) * 3;
            data[i] = lerp_u8(row_r, orb.r, mix).saturating_mul_f32(vig);
            data[i + 1] = lerp_u8(row_g, orb.g, mix).saturating_mul_f32(vig);
            data[i + 2] = lerp_u8(row_b, orb.b, mix).saturating_mul_f32(vig);
        }
    }
    Frame::from_raw(size, FrameFormat::Rgb8, data)
}

#[allow(clippy::too_many_lines)]
fn paint_subject() -> Result<Frame, CoreError> {
    let size = Size::new(FACE_W, FACE_H);
    let mut rgb = vec![0_u8; (FACE_W * FACE_H * 3) as usize];
    // Jacket / shoulders first (behind the head).
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        210.0,
        430.0,
        200.0,
        86.0,
        Rgb8::new(32, 44, 62),
        1.0,
    );
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        210.0,
        188.0,
        168.0,
        168.0,
        Rgb8::new(46, 34, 40),
        1.0,
    ); // hair
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        210.0,
        214.0,
        148.0,
        156.0,
        Rgb8::new(216, 170, 138),
        1.0,
    ); // face
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        62.0,
        224.0,
        26.0,
        34.0,
        Rgb8::new(210, 164, 134),
        1.0,
    );
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        358.0,
        224.0,
        26.0,
        34.0,
        Rgb8::new(210, 164, 134),
        1.0,
    );
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        164.0,
        204.0,
        13.0,
        16.0,
        Rgb8::new(38, 30, 28),
        1.0,
    );
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        256.0,
        204.0,
        13.0,
        16.0,
        Rgb8::new(38, 30, 28),
        1.0,
    );
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        159.0,
        199.0,
        4.0,
        4.0,
        Rgb8::new(240, 236, 230),
        1.0,
    );
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        251.0,
        199.0,
        4.0,
        4.0,
        Rgb8::new(240, 236, 230),
        1.0,
    );
    stamp_ellipse(
        &mut rgb,
        FACE_W,
        FACE_H,
        210.0,
        268.0,
        34.0,
        14.0,
        Rgb8::new(168, 108, 96),
        0.85,
    );
    Frame::from_raw(size, FrameFormat::Rgb8, rgb)
}

fn paint_subject_mask() -> Result<Mask, CoreError> {
    let mut cov = vec![0.0_f32; (FACE_W * FACE_H) as usize];
    stamp_ellipse_mask(&mut cov, FACE_W, FACE_H, 210.0, 430.0, 200.0, 86.0, 0.08);
    stamp_ellipse_mask(&mut cov, FACE_W, FACE_H, 210.0, 200.0, 172.0, 180.0, 0.06);
    Mask::from_raw(Size::new(FACE_W, FACE_H), cov)
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::similar_names
)]
fn stamp_ellipse(
    rgb: &mut [u8],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    color: Rgb8,
    alpha: f32,
) {
    let w = width as usize;
    let x0 = (cx - rx).floor().max(0.0) as usize;
    let y0 = (cy - ry).floor().max(0.0) as usize;
    let x1 = ((cx + rx).ceil() as usize).min(w);
    let y1 = ((cy + ry).ceil() as usize).min(height as usize);
    let inv_rx = 1.0 / rx.max(1.0);
    let inv_ry = 1.0 / ry.max(1.0);
    for y in y0..y1 {
        let ny = (y as f32 - cy) * inv_ry;
        for x in x0..x1 {
            let nx = (x as f32 - cx) * inv_rx;
            let d = nx.mul_add(nx, ny * ny);
            if d > 1.0 {
                continue;
            }
            let edge = (1.0 - d).clamp(0.0, 1.0);
            let a = (edge * 8.0).clamp(0.0, 1.0) * alpha;
            let i = (y * w + x) * 3;
            rgb[i] = lerp_u8(rgb[i], color.r, a);
            rgb[i + 1] = lerp_u8(rgb[i + 1], color.g, a);
            rgb[i + 2] = lerp_u8(rgb[i + 2], color.b, a);
        }
    }
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_arguments,
    clippy::similar_names
)]
fn stamp_ellipse_mask(
    cov: &mut [f32],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    feather: f32,
) {
    let w = width as usize;
    let x0 = (cx - rx).floor().max(0.0) as usize;
    let y0 = (cy - ry).floor().max(0.0) as usize;
    let x1 = ((cx + rx).ceil() as usize).min(w);
    let y1 = ((cy + ry).ceil() as usize).min(height as usize);
    let inv_rx = 1.0 / rx.max(1.0);
    let inv_ry = 1.0 / ry.max(1.0);
    for y in y0..y1 {
        let ny = (y as f32 - cy) * inv_ry;
        for x in x0..x1 {
            let nx = (x as f32 - cx) * inv_rx;
            let d = nx.mul_add(nx, ny * ny).sqrt();
            let a = if d <= 1.0 - feather {
                1.0
            } else if d >= 1.0 {
                0.0
            } else {
                (1.0 - d) / feather
            };
            let i = y * w + x;
            cov[i] = cov[i].max(a.clamp(0.0, 1.0));
        }
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8
    }
}

trait SaturatingMulF32 {
    fn saturating_mul_f32(self, k: f32) -> Self;
}

impl SaturatingMulF32 for u8 {
    fn saturating_mul_f32(self, k: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            (f32::from(self) * k.clamp(0.0, 1.2))
                .round()
                .clamp(0.0, 255.0) as u8
        }
    }
}
