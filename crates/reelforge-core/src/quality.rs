//! Frame quality metrics (PSNR / SSIM) for regression harnesses.

use crate::error::{CoreError, Result};
use crate::frame::{Frame, FrameFormat};

/// Peak signal-to-noise ratio between two same-size RGB(A) frames (dB).
///
/// Uses peak = 255 over RGB channels only. Identical frames return `f64::INFINITY`.
///
/// # Errors
///
/// Returns an error when sizes/formats differ or frames are empty.
pub fn psnr_rgb(a: &Frame, b: &Frame) -> Result<f64> {
    ensure_compatible(a, b)?;
    let bpp = a.format().bytes_per_pixel();
    let da = a.data();
    let db = b.data();
    let mut sse = 0.0_f64;
    let mut n = 0_u64;
    for (pa, pb) in da.chunks_exact(bpp).zip(db.chunks_exact(bpp)) {
        for c in 0..3.min(bpp) {
            let d = f64::from(pa[c]) - f64::from(pb[c]);
            sse += d * d;
            n += 1;
        }
    }
    if n == 0 {
        return Err(CoreError::invalid_frame("empty frame for PSNR"));
    }
    if sse == 0.0 {
        return Ok(f64::INFINITY);
    }
    #[allow(clippy::cast_precision_loss)]
    let mse = sse / n as f64;
    Ok(10.0 * (255.0_f64 * 255.0 / mse).log10())
}

/// Structural similarity (SSIM) over RGB, mean of per-channel SSIM in `0..=1`.
///
/// Global (single-window) SSIM suitable for unit tests and micro-harnesses —
/// not a multi-scale MS-SSIM implementation.
///
/// # Errors
///
/// Returns an error when sizes/formats differ.
pub fn ssim_rgb(a: &Frame, b: &Frame) -> Result<f64> {
    ensure_compatible(a, b)?;
    let bpp = a.format().bytes_per_pixel();
    let mut sum = 0.0;
    let mut chans = 0_u32;
    for c in 0..3.min(bpp) {
        sum += ssim_channel(a.data(), b.data(), bpp, c);
        chans += 1;
    }
    if chans == 0 {
        return Err(CoreError::invalid_frame("empty frame for SSIM"));
    }
    Ok(sum / f64::from(chans))
}

fn ensure_compatible(a: &Frame, b: &Frame) -> Result<()> {
    if a.size() != b.size() {
        return Err(CoreError::invalid_frame(format!(
            "PSNR/SSIM size mismatch {:?} vs {:?}",
            a.size(),
            b.size()
        )));
    }
    if a.format() != b.format() {
        return Err(CoreError::invalid_frame(format!(
            "PSNR/SSIM format mismatch {:?} vs {:?}",
            a.format(),
            b.format()
        )));
    }
    match a.format() {
        FrameFormat::Rgb8 | FrameFormat::Rgba8 => Ok(()),
    }
}

fn ssim_channel(a: &[u8], b: &[u8], bpp: usize, channel: usize) -> f64 {
    // Constants for 8-bit: L=255
    let k1 = 0.01_f64;
    let k2 = 0.03_f64;
    let l = 255.0_f64;
    let c1 = (k1 * l) * (k1 * l);
    let c2 = (k2 * l) * (k2 * l);

    let mut n = 0_u64;
    let mut sum_a = 0.0_f64;
    let mut sum_b = 0.0_f64;
    for (pa, pb) in a.chunks_exact(bpp).zip(b.chunks_exact(bpp)) {
        sum_a += f64::from(pa[channel]);
        sum_b += f64::from(pb[channel]);
        n += 1;
    }
    if n == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let nf = n as f64;
    let mu_a = sum_a / nf;
    let mu_b = sum_b / nf;

    let mut var_a = 0.0_f64;
    let mut var_b = 0.0_f64;
    let mut cov = 0.0_f64;
    for (pa, pb) in a.chunks_exact(bpp).zip(b.chunks_exact(bpp)) {
        let da = f64::from(pa[channel]) - mu_a;
        let db = f64::from(pb[channel]) - mu_b;
        var_a += da * da;
        var_b += db * db;
        cov += da * db;
    }
    var_a /= nf;
    var_b /= nf;
    cov /= nf;

    let num = (2.0 * mu_a * mu_b + c1) * (2.0 * cov + c2);
    let den = (mu_a * mu_a + mu_b * mu_b + c1) * (var_a + var_b + c2);
    if den == 0.0 { 1.0 } else { num / den }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb8;
    use crate::layout::Size;

    #[test]
    fn identical_perfect() {
        let f = Frame::solid_rgb(Size::new(8, 8), Rgb8::new(40, 80, 120)).unwrap();
        assert!(psnr_rgb(&f, &f).unwrap().is_infinite());
        assert!((ssim_rgb(&f, &f).unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn different_lower() {
        let a = Frame::solid_rgb(Size::new(4, 4), Rgb8::BLACK).unwrap();
        let b = Frame::solid_rgb(Size::new(4, 4), Rgb8::WHITE).unwrap();
        let p = psnr_rgb(&a, &b).unwrap();
        assert!(p.is_finite() && p < 20.0);
        let s = ssim_rgb(&a, &b).unwrap();
        assert!(s < 0.5);
    }
}
