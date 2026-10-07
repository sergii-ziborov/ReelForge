//! Sample a part hierarchy at the requested time.
//!
//! Placement is `parent · translate(position) · translate(pivot) · rotate · scale · translate(-pivot)`.
//! An anchor is that same local placement applied at a point on another part, not a second parent.
//! The sample clock is the time passed to [`ScenePoseVideo::frame_at`], not a nominal fps grid.
//! Picture and [`VideoClip::mask_at`] coverage use one inverse map. A partial sample keeps the
//! covered color, so a transparent neighbor does not darken the edge. Motion callbacks see the
//! composite clock. The compose executor adds a layer's start to its local keys before that.

use reelforge_core::{
    AlphaMode, CoreError, Duration, Frame, FrameFormat, Mask, Rgb8, Size, Time, VideoClip,
};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Point binding used by [`ScenePartPose::with_anchor`].
#[derive(Debug, Clone, PartialEq)]
pub struct SceneAnchor {
    /// Other part.
    pub target: String,
    /// Point in that part's local pixels.
    pub x: f32,
    /// Point in that part's local pixels.
    pub y: f32,
}

/// One posed part. Motion callbacks see the composite clock.
#[allow(clippy::type_complexity)]
pub struct ScenePartPose {
    /// Stable id. Parents and anchors name this.
    pub id: String,
    /// Picture. Its own clock is the composite clock minus [`Self::start`].
    pub clip: Arc<dyn VideoClip>,
    /// Hierarchy parent. Empty when this part is rooted or anchored.
    pub parent: Option<String>,
    /// Contact point on another part.
    pub anchor: Option<SceneAnchor>,
    /// Local pivot, in source pixels.
    pub pivot_x: f32,
    /// Local pivot, in source pixels.
    pub pivot_y: f32,
    /// Draw order. Higher paints above lower.
    pub order: i32,
    /// When this part becomes active on the composite clock.
    pub start: Time,
    /// Local translation.
    pub x_at: Arc<dyn Fn(Time) -> f32 + Send + Sync>,
    /// Local translation.
    pub y_at: Arc<dyn Fn(Time) -> f32 + Send + Sync>,
    /// Clockwise degrees in a y-down canvas.
    pub rotation_at: Arc<dyn Fn(Time) -> f32 + Send + Sync>,
    /// Uniform scale about the pivot. Must be positive.
    pub scale_at: Arc<dyn Fn(Time) -> f32 + Send + Sync>,
    /// Opacity in `0.0..=1.0`.
    pub opacity_at: Arc<dyn Fn(Time) -> f32 + Send + Sync>,
}

impl ScenePartPose {
    /// Identity placement: origin, no rotation, full opacity, scale 1.
    #[must_use]
    pub fn new(id: impl Into<String>, clip: Arc<dyn VideoClip>) -> Self {
        Self {
            id: id.into(),
            clip,
            parent: None,
            anchor: None,
            pivot_x: 0.0,
            pivot_y: 0.0,
            order: 0,
            start: Time::ZERO,
            x_at: Arc::new(|_: Time| 0.0),
            y_at: Arc::new(|_: Time| 0.0),
            rotation_at: Arc::new(|_: Time| 0.0),
            scale_at: Arc::new(|_: Time| 1.0),
            opacity_at: Arc::new(|_: Time| 1.0),
        }
    }

    /// Set the hierarchy parent.
    #[must_use]
    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    /// Bind this part to a point on `target`.
    #[must_use]
    pub fn with_anchor(mut self, target: impl Into<String>, x: f32, y: f32) -> Self {
        self.anchor = Some(SceneAnchor {
            target: target.into(),
            x,
            y,
        });
        self
    }

    /// Set the local pivot.
    #[must_use]
    pub fn with_pivot(mut self, x: f32, y: f32) -> Self {
        self.pivot_x = x;
        self.pivot_y = y;
        self
    }

    /// Set draw order.
    #[must_use]
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    /// Set the composite start.
    #[must_use]
    pub fn with_start(mut self, start: Time) -> Self {
        self.start = start;
        self
    }

    /// Sample local x.
    #[must_use]
    pub fn with_x_at(mut self, sample: Arc<dyn Fn(Time) -> f32 + Send + Sync>) -> Self {
        self.x_at = sample;
        self
    }

    /// Sample local y.
    #[must_use]
    pub fn with_y_at(mut self, sample: Arc<dyn Fn(Time) -> f32 + Send + Sync>) -> Self {
        self.y_at = sample;
        self
    }

    /// Sample clockwise degrees.
    #[must_use]
    pub fn with_rotation_at(mut self, sample: Arc<dyn Fn(Time) -> f32 + Send + Sync>) -> Self {
        self.rotation_at = sample;
        self
    }

    /// Sample uniform scale.
    #[must_use]
    pub fn with_scale_at(mut self, sample: Arc<dyn Fn(Time) -> f32 + Send + Sync>) -> Self {
        self.scale_at = sample;
        self
    }

    /// Sample opacity.
    #[must_use]
    pub fn with_opacity_at(mut self, sample: Arc<dyn Fn(Time) -> f32 + Send + Sync>) -> Self {
        self.opacity_at = sample;
        self
    }
}

/// Posed parts on one canvas.
pub struct ScenePoseVideo {
    size: Size,
    duration: Duration,
    background: Rgb8,
    parts: Vec<ScenePartPose>,
    fps: Option<f64>,
    sample_times: Option<Vec<Time>>,
}

impl ScenePoseVideo {
    /// Build a pose stack.
    ///
    /// # Errors
    ///
    /// The canvas or duration is unusable, a parent or anchor is missing, a part
    /// has both, or those links form a cycle.
    pub fn new(size: Size, duration: Duration, parts: Vec<ScenePartPose>) -> crate::Result<Self> {
        Self::with_background(size, Rgb8::BLACK, duration, parts)
    }

    /// Build a pose stack on `background`.
    ///
    /// # Errors
    ///
    /// The canvas or duration is unusable, a parent or anchor is missing, a part
    /// has both, or those links form a cycle.
    pub fn with_background(
        size: Size,
        background: Rgb8,
        duration: Duration,
        mut parts: Vec<ScenePartPose>,
    ) -> crate::Result<Self> {
        size.require_positive().map_err(crate::ComposeError::from)?;
        if !duration.is_positive() || !duration.as_secs().is_finite() {
            return Err(crate::ComposeError::Message(
                "scene pose duration must be finite and positive".into(),
            ));
        }
        if parts.is_empty() {
            return Err(crate::ComposeError::Message(
                "scene pose requires at least one part".into(),
            ));
        }
        check_links(&parts)?;
        let mut fps = None;
        for part in &parts {
            if !part.pivot_x.is_finite() || !part.pivot_y.is_finite() {
                return Err(crate::ComposeError::Message(format!(
                    "scene pose part {} pivot is not finite",
                    part.id
                )));
            }
            let start = part.start.as_secs();
            if !start.is_finite() || start < 0.0 {
                return Err(crate::ComposeError::Message(format!(
                    "scene pose part {} start must be finite and >= 0",
                    part.id
                )));
            }
            if let Some(value) = part.clip.fps() {
                fps = Some(fps.map_or(value, |cur: f64| cur.max(value)));
            }
        }
        parts.sort_by_key(|part| part.order);
        Ok(Self {
            size,
            duration,
            background,
            parts,
            fps,
            sample_times: None,
        })
    }

    /// Keep explicit instants for a later persist.
    ///
    /// Times outside `[0, duration)` are dropped. An empty list leaves sampling
    /// on [`VideoClip::fps`].
    #[must_use]
    pub fn with_sample_times(mut self, times: &[Time]) -> Self {
        let kept = keep_times(times, self.duration);
        self.sample_times = if kept.is_empty() { None } else { Some(kept) };
        self
    }

    /// Extend the canvas when `until` is later. A shorter value does not trim.
    #[must_use]
    pub fn hold_until(mut self, until: Duration) -> Self {
        if until.as_secs() > self.duration.as_secs() {
            self.duration = until;
        }
        self
    }
}

impl VideoClip for ScenePoseVideo {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.size
    }

    fn fps(&self) -> Option<f64> {
        self.fps
    }

    fn sample_times(&self) -> Option<Vec<Time>> {
        self.sample_times.clone()
    }

    fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let mut canvas = Frame::solid_rgb(self.size, self.background)?;
        let locals = self
            .parts
            .iter()
            .map(|part| local_matrix(part, t))
            .collect::<reelforge_core::Result<Vec<_>>>()?;
        let worlds = world_matrices(&self.parts, &locals)?;
        for (index, part) in self.parts.iter().enumerate() {
            paint_part(&mut canvas, part, t, worlds[index])?;
        }
        Ok(canvas)
    }
}

fn check_links(parts: &[ScenePartPose]) -> crate::Result<()> {
    let mut ids = BTreeSet::new();
    for part in parts {
        if part.id.is_empty() || !ids.insert(part.id.clone()) {
            return Err(crate::ComposeError::Message(format!(
                "scene pose refuses duplicate pose id {}",
                part.id
            )));
        }
        if part.parent.is_some() && part.anchor.is_some() {
            return Err(crate::ComposeError::Message(format!(
                "scene pose part {} has both a parent and an anchor",
                part.id
            )));
        }
    }
    for part in parts {
        if let Some(parent) = &part.parent {
            if parent == &part.id {
                return Err(crate::ComposeError::Message(format!(
                    "scene pose part {} parents itself",
                    part.id
                )));
            }
            if !ids.contains(parent) {
                return Err(crate::ComposeError::Message(format!(
                    "scene pose part {} parent {parent} does not exist",
                    part.id
                )));
            }
        }
        if let Some(anchor) = &part.anchor {
            if anchor.target == part.id {
                return Err(crate::ComposeError::Message(format!(
                    "scene pose part {} anchors itself",
                    part.id
                )));
            }
            if !ids.contains(&anchor.target) {
                return Err(crate::ComposeError::Message(format!(
                    "scene pose part {} anchor {} does not exist",
                    part.id, anchor.target
                )));
            }
        }
    }
    if let Some(id) = cycle_id(parts) {
        return Err(crate::ComposeError::Message(format!(
            "scene pose part {id} is in a pose cycle"
        )));
    }
    Ok(())
}

fn cycle_id(parts: &[ScenePartPose]) -> Option<String> {
    let mut color = vec![0_u8; parts.len()];
    for start in 0..parts.len() {
        let mut cursor = Some(start);
        while let Some(index) = cursor {
            if color[index] == 2 {
                break;
            }
            if color[index] == 1 {
                return Some(parts[index].id.clone());
            }
            color[index] = 1;
            cursor = dependency(parts, index);
        }
        let mut back = Some(start);
        while let Some(index) = back {
            if color[index] == 2 {
                break;
            }
            color[index] = 2;
            back = dependency(parts, index);
        }
    }
    None
}

fn dependency(parts: &[ScenePartPose], index: usize) -> Option<usize> {
    let name = if let Some(parent) = &parts[index].parent {
        parent.as_str()
    } else {
        parts[index].anchor.as_ref()?.target.as_str()
    };
    parts.iter().position(|part| part.id == name)
}

#[derive(Clone, Copy)]
struct Aff {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Aff {
    const fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    fn translate(x: f32, y: f32) -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        }
    }

    fn scale(factor: f32) -> Self {
        Self {
            a: factor,
            b: 0.0,
            c: 0.0,
            d: factor,
            e: 0.0,
            f: 0.0,
        }
    }

    fn rotate(degrees: f32) -> Self {
        let (a, b, c, d) = rotation_terms(degrees);
        Self {
            a,
            b,
            c,
            d,
            e: 0.0,
            f: 0.0,
        }
    }

    fn mul(self, right: Self) -> Self {
        Self {
            a: self.a * right.a + self.c * right.b,
            b: self.b * right.a + self.d * right.b,
            c: self.a * right.c + self.c * right.d,
            d: self.b * right.c + self.d * right.d,
            e: self.a * right.e + self.c * right.f + self.e,
            f: self.b * right.e + self.d * right.f + self.f,
        }
    }

    fn invert(self) -> Option<Self> {
        let det = self.a * self.d - self.c * self.b;
        if !det.is_finite() || det.abs() < 1.0e-8 {
            return None;
        }
        let inv = 1.0 / det;
        let a = self.d * inv;
        let b = -self.b * inv;
        let c = -self.c * inv;
        let d = self.a * inv;
        Some(Self {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }

    fn apply(self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

fn rotation_terms(degrees: f32) -> (f32, f32, f32, f32) {
    let wrapped = degrees.rem_euclid(360.0);
    if (wrapped - 90.0).abs() <= 1.0e-3 {
        return (0.0, 1.0, -1.0, 0.0);
    }
    if (wrapped - 180.0).abs() <= 1.0e-3 {
        return (-1.0, 0.0, 0.0, -1.0);
    }
    if (wrapped - 270.0).abs() <= 1.0e-3 {
        return (0.0, -1.0, 1.0, 0.0);
    }
    if wrapped <= 1.0e-3 || (wrapped - 360.0).abs() <= 1.0e-3 {
        return (1.0, 0.0, 0.0, 1.0);
    }
    let (sin_t, cos_t) = degrees.to_radians().sin_cos();
    (cos_t, sin_t, -sin_t, cos_t)
}

fn local_matrix(part: &ScenePartPose, t: Time) -> reelforge_core::Result<Aff> {
    let x = (part.x_at)(t);
    let y = (part.y_at)(t);
    let rotation = (part.rotation_at)(t);
    let scale = (part.scale_at)(t);
    if !x.is_finite() || !y.is_finite() || !rotation.is_finite() || !scale.is_finite() {
        return Err(CoreError::invalid_frame(format!(
            "scene pose part {} motion is not finite",
            part.id
        )));
    }
    if scale <= 0.0 {
        return Err(CoreError::invalid_frame(format!(
            "scene pose part {} scale is not positive",
            part.id
        )));
    }
    let placed = Aff::translate(x, y);
    let pivot = Aff::translate(part.pivot_x, part.pivot_y);
    let unpivot = Aff::translate(-part.pivot_x, -part.pivot_y);
    Ok(placed.mul(pivot.mul(Aff::rotate(rotation).mul(Aff::scale(scale).mul(unpivot)))))
}

fn world_matrices(parts: &[ScenePartPose], locals: &[Aff]) -> reelforge_core::Result<Vec<Aff>> {
    let mut color = vec![0_u8; parts.len()];
    let mut worlds = vec![Aff::identity(); parts.len()];
    for start in 0..parts.len() {
        let mut chain = Vec::new();
        let mut cursor = Some(start);
        while let Some(index) = cursor {
            if color[index] == 2 {
                break;
            }
            if color[index] == 1 {
                return Err(CoreError::invalid_frame(format!(
                    "scene pose part {} is in a pose cycle",
                    parts[index].id
                )));
            }
            color[index] = 1;
            chain.push(index);
            cursor = dependency(parts, index);
        }
        for index in chain.into_iter().rev() {
            worlds[index] = match link(parts, index) {
                None => locals[index],
                Some(Link::Parent(parent)) => worlds[parent].mul(locals[index]),
                Some(Link::Anchor {
                    index: target,
                    x,
                    y,
                }) => worlds[target].mul(Aff::translate(x, y)).mul(locals[index]),
            };
            color[index] = 2;
        }
    }
    Ok(worlds)
}

enum Link {
    Parent(usize),
    Anchor { index: usize, x: f32, y: f32 },
}

fn link(parts: &[ScenePartPose], index: usize) -> Option<Link> {
    if let Some(parent) = &parts[index].parent {
        let found = parts.iter().position(|part| &part.id == parent)?;
        return Some(Link::Parent(found));
    }
    let anchor = parts[index].anchor.as_ref()?;
    let found = parts.iter().position(|part| part.id == anchor.target)?;
    Some(Link::Anchor {
        index: found,
        x: anchor.x,
        y: anchor.y,
    })
}

fn paint_part(
    canvas: &mut Frame,
    part: &ScenePartPose,
    t: Time,
    world: Aff,
) -> reelforge_core::Result<()> {
    let local = t.as_secs() - part.start.as_secs();
    if local < 0.0 || local >= part.clip.duration().as_secs() {
        return Ok(());
    }
    let when = Time::from_secs(local);
    let opacity = (part.opacity_at)(t);
    if !opacity.is_finite() {
        return Err(CoreError::invalid_frame(format!(
            "scene pose part {} opacity is not finite",
            part.id
        )));
    }
    let opacity = opacity.clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return Ok(());
    }
    let frame = part.clip.frame_at(when)?;
    let mask = part.clip.mask_at(when)?;
    if let Some(mask) = &mask
        && mask.size() != frame.size()
    {
        return Err(CoreError::invalid_frame(format!(
            "scene pose part {} coverage mask does not match the picture",
            part.id
        )));
    }
    let Some(inverse) = world.invert() else {
        return Err(CoreError::invalid_frame(format!(
            "scene pose part {} transform cannot be inverted",
            part.id
        )));
    };
    stamp(canvas, &frame, mask.as_ref(), world, inverse, opacity)
}

fn stamp(
    canvas: &mut Frame,
    src: &Frame,
    mask: Option<&Mask>,
    world: Aff,
    inverse: Aff,
    opacity: f32,
) -> reelforge_core::Result<()> {
    let premul = src.alpha_mode() == AlphaMode::Premultiplied;
    let src_w = src.size().width;
    let src_h = src.size().height;
    let dst_w = canvas.size().width;
    let dst_h = canvas.size().height;
    let (x0, y0, x1, y1) = paint_span(world, src_w, src_h, dst_w, dst_h);
    let dst_data = canvas.data_mut();
    let row = usize::try_from(dst_w).unwrap_or(usize::MAX);
    for y in y0..y1 {
        for x in x0..x1 {
            let (sx, sy) = inverse.apply(u32_f32(x), u32_f32(y));
            let Some((rgb, coverage)) = sample_covered(src, mask, premul, sx, sy)? else {
                continue;
            };
            let coverage = (coverage * opacity).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let dst_i = (usize::try_from(y).unwrap_or(usize::MAX) * row
                + usize::try_from(x).unwrap_or(usize::MAX))
                * 3;
            if dst_i + 3 > dst_data.len() {
                continue;
            }
            over(&mut dst_data[dst_i..dst_i + 3], rgb, coverage);
        }
    }
    Ok(())
}

fn paint_span(world: Aff, src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (u32, u32, u32, u32) {
    let width = u32_f32(src_w);
    let height = u32_f32(src_h);
    let corners = [
        world.apply(0.0, 0.0),
        world.apply(width, 0.0),
        world.apply(0.0, height),
        world.apply(width, height),
    ];
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for (x, y) in corners {
        if !x.is_finite() || !y.is_finite() {
            return (0, 0, dst_w, dst_h);
        }
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    (
        axis_start(min_x - 1.0, dst_w),
        axis_start(min_y - 1.0, dst_h),
        axis_end(max_x + 1.0, dst_w),
        axis_end(max_y + 1.0, dst_h),
    )
}

fn axis_start(value: f32, limit: u32) -> u32 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    #[allow(clippy::cast_precision_loss)]
    let limit_f = limit as f32;
    let floored = value.floor();
    if floored >= limit_f {
        return limit;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        floored as u32
    }
}

fn axis_end(value: f32, limit: u32) -> u32 {
    if !value.is_finite() {
        return limit;
    }
    #[allow(clippy::cast_precision_loss)]
    let limit_f = limit as f32;
    let ceiled = value.ceil();
    if ceiled <= 0.0 {
        return 0;
    }
    if ceiled >= limit_f {
        return limit;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        ceiled as u32
    }
}

fn sample_covered(
    src: &Frame,
    mask: Option<&Mask>,
    premul: bool,
    sx: f32,
    sy: f32,
) -> reelforge_core::Result<Option<([f32; 3], f32)>> {
    if !sx.is_finite() || !sy.is_finite() {
        return Ok(None);
    }
    let sx = snap_axis(sx);
    let sy = snap_axis(sy);
    let x0 = sx.floor();
    let y0 = sy.floor();
    let fx = sx - x0;
    let fy = sy - y0;
    let mut sum_rgb = [0.0_f32; 3];
    let mut sum_a = 0.0_f32;
    for (ox, wx) in [(0.0_f32, 1.0 - fx), (1.0, fx)] {
        for (oy, wy) in [(0.0_f32, 1.0 - fy), (1.0, fy)] {
            let weight = wx * wy;
            if weight <= 0.0 {
                continue;
            }
            let Some((rgb, alpha)) = tap(src, mask, x0 + ox, y0 + oy, premul)? else {
                continue;
            };
            for channel in 0..3 {
                sum_rgb[channel] += rgb[channel] * weight;
            }
            sum_a += alpha * weight;
        }
    }
    if sum_a <= 0.0 {
        return Ok(None);
    }
    Ok(Some((
        [sum_rgb[0] / sum_a, sum_rgb[1] / sum_a, sum_rgb[2] / sum_a],
        sum_a,
    )))
}

fn tap(
    src: &Frame,
    mask: Option<&Mask>,
    x: f32,
    y: f32,
    premul: bool,
) -> reelforge_core::Result<Option<([f32; 3], f32)>> {
    let Some(ix) = floor_index(x, src.size().width) else {
        return Ok(None);
    };
    let Some(iy) = floor_index(y, src.size().height) else {
        return Ok(None);
    };
    let coverage = coverage_at(mask, ix, iy, src.size().width)?;
    if coverage <= 0.0 {
        return Ok(None);
    }
    let bpp = src.format().bytes_per_pixel();
    let index = (usize::try_from(iy).unwrap_or(usize::MAX)
        * usize::try_from(src.size().width).unwrap_or(usize::MAX)
        + usize::try_from(ix).unwrap_or(usize::MAX))
        * bpp;
    let data = src.data();
    if index + bpp > data.len() {
        return Ok(None);
    }
    let alpha = match src.format() {
        FrameFormat::Rgb8 => 1.0,
        FrameFormat::Rgba8 => f32::from(data[index + 3]) / 255.0,
    };
    if alpha <= 0.0 {
        return Ok(None);
    }
    let mut rgb = [
        f32::from(data[index]),
        f32::from(data[index + 1]),
        f32::from(data[index + 2]),
    ];
    if !premul {
        for channel in &mut rgb {
            *channel *= alpha;
        }
    }
    for channel in &mut rgb {
        *channel *= coverage;
    }
    Ok(Some((rgb, alpha * coverage)))
}

fn coverage_at(mask: Option<&Mask>, x: u32, y: u32, width: u32) -> reelforge_core::Result<f32> {
    let Some(mask) = mask else {
        return Ok(1.0);
    };
    let index = usize::try_from(y).unwrap_or(usize::MAX)
        * usize::try_from(width).unwrap_or(usize::MAX)
        + usize::try_from(x).unwrap_or(usize::MAX);
    let Some(sample) = mask.data().get(index).copied() else {
        return Err(CoreError::invalid_frame(
            "scene pose coverage is outside the picture",
        ));
    };
    if !sample.is_finite() {
        return Err(CoreError::invalid_frame(
            "scene pose coverage is not finite",
        ));
    }
    Ok(sample.clamp(0.0, 1.0))
}

fn snap_axis(value: f32) -> f32 {
    let nearest = value.round();
    if (value - nearest).abs() <= 2.0e-3 {
        nearest
    } else {
        value
    }
}

fn floor_index(value: f32, limit: u32) -> Option<u32> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let limit_f = limit as f32;
    if value >= limit_f {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(value as u32)
}

fn u32_f32(value: u32) -> f32 {
    #[allow(clippy::cast_precision_loss)]
    {
        value as f32
    }
}

fn over(dst: &mut [u8], rgb: [f32; 3], coverage: f32) {
    let inv = 1.0 - coverage;
    for channel in 0..3 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let mixed = (rgb[channel] * coverage + f32::from(dst[channel]) * inv)
            .round()
            .clamp(0.0, 255.0) as u8;
        dst[channel] = mixed;
    }
}

fn keep_times(times: &[Time], duration: Duration) -> Vec<Time> {
    let mut kept: Vec<Time> = times
        .iter()
        .copied()
        .filter(|time| {
            let secs = time.as_secs();
            secs.is_finite() && secs >= 0.0 && secs < duration.as_secs()
        })
        .collect();
    kept.sort_by(|left, right| left.as_secs().total_cmp(&right.as_secs()));
    kept.dedup_by(|left, right| {
        left.as_secs().total_cmp(&right.as_secs()) == std::cmp::Ordering::Equal
    });
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, FrameFormat, Rgb8};

    fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 3] {
        let width = usize::try_from(frame.size().width).unwrap();
        let index = (usize::try_from(y).unwrap() * width + usize::try_from(x).unwrap()) * 3;
        let data = frame.data();
        [data[index], data[index + 1], data[index + 2]]
    }

    struct Pair {
        left: Rgb8,
        right: Rgb8,
    }

    impl VideoClip for Pair {
        fn duration(&self) -> Duration {
            Duration::from_secs(2.0)
        }

        fn size(&self) -> Size {
            Size::new(2, 1)
        }

        fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
            if t.as_secs() < 0.0 || t.as_secs() >= 2.0 {
                return Err(CoreError::TimeOutOfRange {
                    time: t,
                    range: (Time::ZERO, Time::from_secs(2.0)),
                });
            }
            let mut frame = Frame::zeros(Size::new(2, 1), FrameFormat::Rgb8)?;
            let data = frame.data_mut();
            data[0] = self.left.r;
            data[1] = self.left.g;
            data[2] = self.left.b;
            data[3] = self.right.r;
            data[4] = self.right.g;
            data[5] = self.right.b;
            Ok(frame)
        }
    }

    #[test]
    fn identity_places_the_source_pixel_at_the_translation() {
        let clip = Arc::new(ColorClip::new(
            Size::new(1, 1),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let part = ScenePartPose::new("dot", clip)
            .with_x_at(Arc::new(|_: Time| 1.0))
            .with_y_at(Arc::new(|_: Time| 2.0));
        let pose =
            ScenePoseVideo::new(Size::new(4, 4), Duration::from_secs(1.0), vec![part]).unwrap();
        let frame = pose.frame_at(Time::ZERO).unwrap();
        assert_eq!(pixel(&frame, 1, 2), [255, 255, 255]);
        assert_eq!(pixel(&frame, 0, 0), [0, 0, 0]);
        assert_eq!(pose.frame_at(Time::ZERO).unwrap().data(), frame.data());
    }

    #[test]
    fn anchor_keeps_contact_when_the_target_moves() {
        let stem = ScenePartPose::new(
            "stem",
            Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::RED,
                Duration::from_secs(2.0),
            )),
        )
        .with_x_at(Arc::new(
            |time: Time| if time.as_secs() < 0.5 { 2.0 } else { 4.0 },
        ))
        .with_y_at(Arc::new(|_: Time| 1.0));
        let petal = ScenePartPose::new(
            "petal",
            Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::BLUE,
                Duration::from_secs(2.0),
            )),
        )
        .with_anchor("stem", 1.0, 0.0)
        .with_order(1);
        let pose =
            ScenePoseVideo::new(Size::new(8, 4), Duration::from_secs(2.0), vec![stem, petal])
                .unwrap()
                .with_sample_times(&[Time::ZERO, Time::from_secs(1.0)]);
        let early = pose.frame_at(Time::ZERO).unwrap();
        let later = pose.frame_at(Time::from_secs(1.0)).unwrap();
        assert_eq!(pixel(&early, 2, 1), [255, 0, 0]);
        assert_eq!(pixel(&early, 3, 1), [0, 0, 255]);
        assert_eq!(pixel(&later, 4, 1), [255, 0, 0]);
        assert_eq!(pixel(&later, 5, 1), [0, 0, 255]);
        assert_eq!(
            pose.frame_at(Time::from_secs(1.0)).unwrap().data(),
            later.data()
        );
        assert_eq!(pose.sample_times().map(|times| times.len()), Some(2));
    }

    #[test]
    fn quarter_turn_swings_the_off_pivot_pixel() {
        let bar = ScenePartPose::new(
            "bar",
            Arc::new(Pair {
                left: Rgb8::RED,
                right: Rgb8::GREEN,
            }),
        )
        .with_x_at(Arc::new(|_: Time| 1.0))
        .with_y_at(Arc::new(|_: Time| 1.0))
        .with_rotation_at(Arc::new(|_: Time| 90.0));
        let pose =
            ScenePoseVideo::new(Size::new(4, 4), Duration::from_secs(1.0), vec![bar]).unwrap();
        let frame = pose.frame_at(Time::ZERO).unwrap();
        assert_eq!(pixel(&frame, 1, 1), [255, 0, 0]);
        assert_eq!(pixel(&frame, 1, 2), [0, 255, 0]);
    }

    #[test]
    fn parent_and_anchor_together_are_refused() {
        let stem = ScenePartPose::new(
            "stem",
            Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::RED,
                Duration::from_secs(1.0),
            )),
        );
        let petal = ScenePartPose::new(
            "petal",
            Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::BLUE,
                Duration::from_secs(1.0),
            )),
        )
        .with_parent("stem")
        .with_anchor("stem", 1.0, 0.0);
        let Err(err) =
            ScenePoseVideo::new(Size::new(4, 4), Duration::from_secs(1.0), vec![stem, petal])
        else {
            panic!("parent and anchor were accepted");
        };
        assert!(err.to_string().contains("both a parent and an anchor"));
    }

    #[test]
    fn a_parent_cycle_is_refused() {
        let left = ScenePartPose::new(
            "left",
            Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::RED,
                Duration::from_secs(1.0),
            )),
        )
        .with_parent("right");
        let right = ScenePartPose::new(
            "right",
            Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::BLUE,
                Duration::from_secs(1.0),
            )),
        )
        .with_parent("left");
        let Err(err) =
            ScenePoseVideo::new(Size::new(2, 2), Duration::from_secs(1.0), vec![left, right])
        else {
            panic!("a parent cycle was accepted");
        };
        assert!(err.to_string().contains("pose cycle"));
    }

    struct CoveredBar;

    impl VideoClip for CoveredBar {
        fn duration(&self) -> Duration {
            Duration::from_secs(1.0)
        }

        fn size(&self) -> Size {
            Size::new(2, 1)
        }

        fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
            if !(0.0..1.0).contains(&t.as_secs()) {
                return Err(CoreError::TimeOutOfRange {
                    time: t,
                    range: (Time::ZERO, Time::from_secs(1.0)),
                });
            }
            let mut frame = Frame::zeros(Size::new(2, 1), FrameFormat::Rgb8)?;
            let data = frame.data_mut();
            data[0] = 255;
            data[4] = 255;
            Ok(frame)
        }

        fn mask_at(&self, t: Time) -> reelforge_core::Result<Option<Mask>> {
            if !(0.0..1.0).contains(&t.as_secs()) {
                return Err(CoreError::TimeOutOfRange {
                    time: t,
                    range: (Time::ZERO, Time::from_secs(1.0)),
                });
            }
            Ok(Some(Mask::from_raw(Size::new(2, 1), vec![1.0, 0.25])?))
        }
    }

    struct SoftEdge;

    impl VideoClip for SoftEdge {
        fn duration(&self) -> Duration {
            Duration::from_secs(1.0)
        }

        fn size(&self) -> Size {
            Size::new(2, 1)
        }

        fn frame_at(&self, _: Time) -> reelforge_core::Result<Frame> {
            let mut frame = Frame::zeros(Size::new(2, 1), FrameFormat::Rgb8)?;
            let data = frame.data_mut();
            data[0] = 255;
            data[1] = 255;
            data[2] = 255;
            Ok(frame)
        }

        fn mask_at(&self, _: Time) -> reelforge_core::Result<Option<Mask>> {
            Ok(Some(Mask::from_raw(Size::new(2, 1), vec![1.0, 0.0])?))
        }
    }

    struct QuarterDot;

    impl VideoClip for QuarterDot {
        fn duration(&self) -> Duration {
            Duration::from_secs(1.0)
        }

        fn size(&self) -> Size {
            Size::new(1, 1)
        }

        fn frame_at(&self, _: Time) -> reelforge_core::Result<Frame> {
            Frame::solid_rgb(Size::new(1, 1), Rgb8::WHITE)
        }

        fn mask_at(&self, _: Time) -> reelforge_core::Result<Option<Mask>> {
            Ok(Some(Mask::from_raw(Size::new(1, 1), vec![0.25])?))
        }
    }

    struct MismatchedMask;

    impl VideoClip for MismatchedMask {
        fn duration(&self) -> Duration {
            Duration::from_secs(1.0)
        }

        fn size(&self) -> Size {
            Size::new(1, 1)
        }

        fn frame_at(&self, _: Time) -> reelforge_core::Result<Frame> {
            Frame::solid_rgb(Size::new(1, 1), Rgb8::WHITE)
        }

        fn mask_at(&self, _: Time) -> reelforge_core::Result<Option<Mask>> {
            Ok(Some(Mask::from_raw(Size::new(2, 1), vec![1.0, 1.0])?))
        }
    }

    #[test]
    fn coverage_rotates_with_the_picture() {
        let bar = ScenePartPose::new("bar", Arc::new(CoveredBar))
            .with_x_at(Arc::new(|_: Time| 1.0))
            .with_y_at(Arc::new(|_: Time| 1.0))
            .with_rotation_at(Arc::new(|_: Time| 90.0));
        let pose =
            ScenePoseVideo::new(Size::new(4, 4), Duration::from_secs(1.0), vec![bar]).unwrap();
        let frame = pose.frame_at(Time::ZERO).unwrap();
        assert_eq!(pixel(&frame, 1, 1), [255, 0, 0]);
        assert_eq!(pixel(&frame, 1, 2), [0, 64, 0]);
        assert_eq!(pose.frame_at(Time::ZERO).unwrap().data(), frame.data());
    }

    #[test]
    fn half_pixel_edge_keeps_the_covered_color() {
        let part =
            ScenePartPose::new("edge", Arc::new(SoftEdge)).with_x_at(Arc::new(|_: Time| 0.5));
        let on_black =
            ScenePoseVideo::new(Size::new(3, 1), Duration::from_secs(1.0), vec![part]).unwrap();
        let dark = on_black.frame_at(Time::ZERO).unwrap();
        assert_eq!(pixel(&dark, 0, 0), [128, 128, 128]);
        assert_eq!(pixel(&dark, 1, 0), [128, 128, 128]);
        assert_eq!(pixel(&dark, 2, 0), [0, 0, 0]);
        assert_eq!(on_black.frame_at(Time::ZERO).unwrap().data(), dark.data());

        let part =
            ScenePartPose::new("edge", Arc::new(SoftEdge)).with_x_at(Arc::new(|_: Time| 0.5));
        let on_white = ScenePoseVideo::with_background(
            Size::new(3, 1),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
            vec![part],
        )
        .unwrap();
        let light = on_white.frame_at(Time::ZERO).unwrap();
        assert_eq!(light.data(), &[255_u8; 9]);
    }

    #[test]
    fn scaled_coverage_stays_with_the_pixel() {
        let part =
            ScenePartPose::new("dot", Arc::new(QuarterDot)).with_scale_at(Arc::new(|_: Time| 2.0));
        let pose =
            ScenePoseVideo::new(Size::new(2, 2), Duration::from_secs(1.0), vec![part]).unwrap();
        let frame = pose.frame_at(Time::ZERO).unwrap();
        assert_eq!(pixel(&frame, 0, 0), [64, 64, 64]);
        assert_eq!(pixel(&frame, 1, 0), [32, 32, 32]);
        assert_eq!(pose.frame_at(Time::ZERO).unwrap().data(), frame.data());
    }

    #[test]
    fn a_coverage_mask_must_match_the_picture() {
        let part = ScenePartPose::new("dot", Arc::new(MismatchedMask));
        let pose =
            ScenePoseVideo::new(Size::new(2, 2), Duration::from_secs(1.0), vec![part]).unwrap();
        let Err(err) = pose.frame_at(Time::ZERO) else {
            panic!("a mismatched coverage mask was painted");
        };
        assert!(err.to_string().contains("does not match the picture"));
    }
}
