# Effect catalog

All video effects implement `VideoEffect::apply`. Audio effects implement `AudioEffect::apply`.

Import via `reelforge::prelude::*` or `reelforge_fx`.

Prefer nesting `apply` calls:

```rust
let a = Crop::new(0, 0, 960, 540).apply(clip)?;
let b = Resize::to(Size::new(640, 360)).apply(a)?;
let c = BlackAndWhite.apply(b)?;
```

See also: [GUIDE.md](GUIDE.md) · [IO.md](IO.md)

## Geometry

| Effect | API | Notes |
|--------|-----|--------|
| Crop | `Crop::new(x, y, w, h)` | Rectangle subregion |
| Resize | `Resize::to(size)` | Default **bilinear** |
| | `Resize::to_nearest(size)` | Fast |
| | `Resize::to_bicubic(size)` / `.bicubic()` | Catmull-Rom quality |
| Rotate | `Rotate::cw90()` / `half()` / `cw270()` | Orthogonal; 90 swaps dims |
| | `Rotate::degrees(theta)` | Free angle; canvas size fixed; black fill |
| Mirror | `MirrorX` / `MirrorY` | Flip |
| Margin | `Margin` | Pad around frame |
| EvenSize | `EvenSize` | Force even dims (yuv420-friendly) |
| Scroll | `Scroll::new(w, h, vx, vy)` | Moving crop window |
| SlideIn / SlideOut | `SlideIn::new(dur, SlideSide::Left)` | Motion transition |

## Time

| Effect | API | Notes |
|--------|-----|--------|
| Subclip | `subclip_video(clip, start, dur)` | Core helper |
| Speed | `Speed::new(factor)` | Timeline scale |
| AccelDecel | `AccelDecel::new(new_dur)` | Ease in/out remapping |
| TimeMirror | `TimeMirror` | Play reversed |
| TimeSymmetrize | `TimeSymmetrize` | Forward then reverse (2x duration) |
| Loop | `Loop::until(dur)` / `Loop::times(n)` | Repeat |
| Freeze | `Freeze::new(t, hold)` | Hold one frame |
| FreezeRegion | `FreezeRegion::new(t, x, y, w, h)` | Freeze a patch |
| SuperSample | `SuperSample::new(d, n)` | Temporal average in +/- d |
| Blink | `Blink::new(on, off)` | Periodic blackouts |

## Color and look

| Effect | API | Notes |
|--------|-----|--------|
| FadeIn / FadeOut | `FadeIn::new(dur)` | Color ramp (black default) |
| CrossFadeIn / Out | mask opacity fades | Good for layering |
| BlackAndWhite | `BlackAndWhite` | BT.601 luma |
| InvertColors | `InvertColors` | |
| MultiplyColor | `MultiplyColor::new(f)` | Brightness scale |
| GammaCorrection | `GammaCorrection::new(gamma)` | |
| LumContrast | `LumContrast::new(lum, contrast)` | |
| Painting | `Painting::new()` | Edge-enhance + ink (`saturation`, `black`) |
| | `Painting::with(sat, black).inky()` | Stronger lines |
| MaskColor | `MaskColor::new(color, thr)` | Chroma key to mask |
| MasksAnd / MasksOr | `MasksAnd::new(other)` | Combine masks |

## Regional blur

| Effect | API | Notes |
|--------|-----|--------|
| HeadBlur | `HeadBlur::fixed(cx, cy, radius)` | Gaussian + soft feather mask |
| | `HeadBlur::auto(r, \|t\| (x,y))` | Tracking path |
| | `HeadBlur::moving(r, intensity, feather, f)` | Full control |
| | `.with_feather(0.35)` | Edge softness |

Default intensity is about `2 * radius / 3`. Quality path is separable Gaussian with smoothstep feather.

## Audio

| Effect | API | Notes |
|--------|-----|--------|
| VolumeGain | `VolumeGain::new(g)` | Uniform gain |
| MultiplyStereoVolume | `MultiplyStereoVolume::new(l, r)` | L/R split |
| AudioFadeIn / Out | `AudioFadeIn::new(dur)` | Linear ramps |
| AudioNormalize | `AudioNormalize::peak()` | Peak to target |
| AudioDelay | `AudioDelay::new(dur)` | Leading silence |

## Subtitles

| API | Format |
|-----|--------|
| `parse_srt` | SubRip |
| `parse_vtt` | WebVTT |
| `parse_ass` | ASS/SSA dialogue |
| `parse_subtitles` | auto-detect from string |
| `parse_subtitles_path` | by extension `.srt` / `.vtt` / `.ass` / `.ssa` |
| `burn_in_layers` | cues to `CompositeLayer`s |

## Composition helpers

- `composite_video(size, layers)`
- `CompositeLayer::new(clip).with_position(...).with_opacity(...).with_layer_index(...)`
- `concatenate_video` / `concatenate_audio`
- Text: `TextClip` / `TextClipOptions`

## Quality tips

1. Deliverables: `Resize::to_bicubic(Size::HD_1080)` then color/title.
2. Tracking blur: `HeadBlur::auto(40.0, |t| (x(t), y(t))).with_feather(0.4)`.
3. Stylized stills/loops: `Painting::new().inky()` after a mild `LumContrast`.
4. Prefer filtergraph for file-only scale/trim when you do not need Rust-side pixel ops.
5. Regression: `psnr_rgb` / `ssim_rgb` on paired frames.
