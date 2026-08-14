//! Union-ROI coverage + separable blur (no full-frame × N-region walk).

use crate::tracks::{CoverageMask, RegionAt};
use reelforge_core::Frame;
use std::cell::RefCell;

/// Pixel ROI inclusive-exclusive `[x0, x1) × [y0, y1)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Roi {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl Roi {
    fn w(self) -> usize {
        self.x1.saturating_sub(self.x0)
    }
    fn h(self) -> usize {
        self.y1.saturating_sub(self.y0)
    }
    fn empty(self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

/// Union of region boxes + kernel / feather pad, clipped to the frame.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn union_roi(regions: &[RegionAt], width: usize, height: usize, pad: usize) -> Option<Roi> {
    if regions.is_empty() || width == 0 || height == 0 {
        return None;
    }
    let mut x0 = width;
    let mut y0 = height;
    let mut x1 = 0_usize;
    let mut y1 = 0_usize;
    for r in regions {
        let (rx0, ry0, rx1, ry1) = if let Some(cov) = &r.coverage {
            let (l, t, rgt, btm) = cov.bounds();
            (l as usize, t as usize, rgt as usize, btm as usize)
        } else {
            let rad = (r.radius.ceil() as usize).saturating_add(1);
            let cx = r.cx.round().max(0.0) as usize;
            let cy = r.cy.round().max(0.0) as usize;
            (
                cx.saturating_sub(rad),
                cy.saturating_sub(rad),
                cx.saturating_add(rad),
                cy.saturating_add(rad),
            )
        };
        x0 = x0.min(rx0.saturating_sub(pad));
        y0 = y0.min(ry0.saturating_sub(pad));
        x1 = x1.max(rx1.saturating_add(pad));
        y1 = y1.max(ry1.saturating_add(pad));
    }
    x1 = x1.min(width);
    y1 = y1.min(height);
    let roi = Roi { x0, y0, x1, y1 };
    if roi.empty() { None } else { Some(roi) }
}

/// Stamp ellipses / dense masks into a ROI coverage buffer (`0..=1`).
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(crate) fn stamp_coverage(cov: &mut [f32], roi: Roi, regions: &[RegionAt], feather: f32) {
    let w = roi.w();
    cov.fill(0.0);
    for r in regions {
        if let Some(mask) = &r.coverage {
            stamp_dense(cov, roi, w, mask);
        } else {
            stamp_ellipse(cov, roi, w, r.cx, r.cy, r.radius.max(1.0), feather);
        }
    }
}

#[allow(clippy::many_single_char_names)]
fn stamp_dense(cov: &mut [f32], roi: Roi, stride: usize, mask: &CoverageMask) {
    let (l, t, r, b) = mask.bounds();
    let x0 = (l as usize).max(roi.x0);
    let y0 = (t as usize).max(roi.y0);
    let x1 = (r as usize).min(roi.x1);
    let y1 = (b as usize).min(roi.y1);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y - roi.y0) * stride + (x - roi.x0);
            let v = mask.sample(x, y);
            if v > cov[i] {
                cov[i] = v;
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn stamp_ellipse(
    cov: &mut [f32],
    roi: Roi,
    stride: usize,
    cx: f32,
    cy: f32,
    radius: f32,
    feather: f32,
) {
    let feather_px = (feather * radius).max(0.5);
    let inner = (radius - feather_px).max(0.0);
    let rad = radius.ceil() as usize + 1;
    let x0 = (cx as usize).saturating_sub(rad).max(roi.x0);
    let y0 = (cy as usize).saturating_sub(rad).max(roi.y0);
    let x1 = (cx as usize).saturating_add(rad).min(roi.x1);
    let y1 = (cy as usize).saturating_add(rad).min(roi.y1);
    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let wgt = soft_mask(dist, inner, radius);
            let i = (y - roi.y0) * stride + (x - roi.x0);
            if wgt > cov[i] {
                cov[i] = wgt;
            }
        }
    }
}

fn soft_mask(dist: f32, inner: f32, outer: f32) -> f32 {
    if dist <= inner {
        1.0
    } else if dist >= outer {
        0.0
    } else {
        let t = ((dist - inner) / (outer - inner).max(1e-6)).clamp(0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        1.0 - s
    }
}

struct Scratch {
    src: Vec<u8>,
    tmp: Vec<u8>,
    blurred: Vec<u8>,
    cov: Vec<f32>,
}

impl Scratch {
    fn ensure(&mut self, px: usize, cov: usize) {
        if self.src.len() < px {
            self.src.resize(px, 0);
            self.tmp.resize(px, 0);
            self.blurred.resize(px, 0);
        }
        if self.cov.len() < cov {
            self.cov.resize(cov, 0.0);
        }
    }
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = const {
        RefCell::new(Scratch {
            src: Vec::new(),
            tmp: Vec::new(),
            blurred: Vec::new(),
            cov: Vec::new(),
        })
    };
}

/// Fused gaussian: blur only the union ROI, blend with stamped coverage.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn apply_fused_blur(frame: &mut Frame, regions: &[RegionAt], sigma: f32, feather: f32) {
    let size = frame.size();
    let bpp = frame.format().bytes_per_pixel();
    let fw = size.width as usize;
    let fh = size.height as usize;
    let kernel = gaussian_kernel(sigma.max(0.5));
    let pad = kernel.len() / 2 + 1;
    let Some(roi) = union_roi(regions, fw, fh, pad) else {
        return;
    };
    let rw = roi.w();
    let rh = roi.h();
    let roi_px = rw * rh * bpp;
    let cov_n = rw * rh;

    SCRATCH.with(|cell| {
        let mut s = cell.borrow_mut();
        s.ensure(roi_px, cov_n);
        let Scratch {
            src,
            tmp,
            blurred,
            cov,
        } = &mut *s;
        extract_roi(frame.data(), &mut src[..roi_px], fw, bpp, roi);
        blur_separable(
            &src[..roi_px],
            &mut tmp[..roi_px],
            &mut blurred[..roi_px],
            rw,
            rh,
            bpp,
            &kernel,
        );
        stamp_coverage(&mut cov[..cov_n], roi, regions, feather);
        blend_roi(
            frame.data_mut(),
            &src[..roi_px],
            &blurred[..roi_px],
            &cov[..cov_n],
            fw,
            bpp,
            roi,
        );
    });
}

fn extract_roi(src: &[u8], dst: &mut [u8], frame_w: usize, bpp: usize, roi: Roi) {
    let rw = roi.w();
    for y in 0..roi.h() {
        let si = ((roi.y0 + y) * frame_w + roi.x0) * bpp;
        let di = y * rw * bpp;
        dst[di..di + rw * bpp].copy_from_slice(&src[si..si + rw * bpp]);
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn blend_roi(
    out: &mut [u8],
    src: &[u8],
    blurred: &[u8],
    cov: &[f32],
    frame_w: usize,
    bpp: usize,
    roi: Roi,
) {
    let rw = roi.w();
    for y in 0..roi.h() {
        for x in 0..rw {
            let wgt = cov[y * rw + x];
            if wgt <= 0.0 {
                continue;
            }
            let si = (y * rw + x) * bpp;
            let di = ((roi.y0 + y) * frame_w + roi.x0 + x) * bpp;
            for c in 0..bpp.min(3) {
                let a = f32::from(src[si + c]);
                let b = f32::from(blurred[si + c]);
                out[di + c] = (a * (1.0 - wgt) + b * wgt).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let radius_px = ((sigma * 3.0).ceil().max(1.0) as usize).min(32);
    let mut k = Vec::with_capacity(radius_px * 2 + 1);
    let s2 = 2.0 * sigma * sigma;
    let mut sum = 0.0_f32;
    for i in 0..=radius_px * 2 {
        let offset = i as i32 - radius_px as i32;
        let v = (-(offset * offset) as f32 / s2).exp();
        k.push(v);
        sum += v;
    }
    for v in &mut k {
        *v /= sum;
    }
    k
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn blur_separable(
    src: &[u8],
    tmp: &mut [u8],
    dst: &mut [u8],
    width: usize,
    height: usize,
    bpp: usize,
    kernel: &[f32],
) {
    let r = kernel.len() / 2;
    let w_i = width as isize;
    let h_i = height as isize;
    let r_i = r as isize;
    for y in 0..height {
        for x in 0..width {
            let mut acc = [0.0_f32; 4];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sx = (x as isize + ki as isize - r_i).clamp(0, w_i - 1) as usize;
                let i = (y * width + sx) * bpp;
                for c in 0..bpp.min(4) {
                    acc[c] += f32::from(src[i + c]) * kw;
                }
            }
            let di = (y * width + x) * bpp;
            for c in 0..bpp.min(4) {
                tmp[di + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    for y in 0..height {
        for x in 0..width {
            let mut acc = [0.0_f32; 4];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sy = (y as isize + ki as isize - r_i).clamp(0, h_i - 1) as usize;
                let i = (sy * width + x) * bpp;
                for c in 0..bpp.min(4) {
                    acc[c] += f32::from(tmp[i + c]) * kw;
                }
            }
            let di = (y * width + x) * bpp;
            for c in 0..bpp.min(4) {
                dst[di + c] = acc[c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}
