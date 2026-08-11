//! Pure helpers shared by encode/decode (no process spawn).

use reelforge_core::{Duration, Frame, FrameFormat, Result as CoreResult};

/// Convert any supported frame format to packed RGB24 bytes.
///
/// # Errors
///
/// Returns an error when the RGBA buffer length is invalid.
pub fn frame_to_rgb24(frame: &Frame) -> CoreResult<Vec<u8>> {
    match frame.format() {
        FrameFormat::Rgb8 => Ok(frame.data().to_vec()),
        FrameFormat::Rgba8 => {
            let data = frame.data();
            if !data.len().is_multiple_of(4) {
                return Err(reelforge_core::CoreError::invalid_frame(
                    "rgba buffer length invalid",
                ));
            }
            let mut rgb = Vec::with_capacity(data.len() / 4 * 3);
            for chunk in data.chunks_exact(4) {
                rgb.push(chunk[0]);
                rgb.push(chunk[1]);
                rgb.push(chunk[2]);
            }
            Ok(rgb)
        }
    }
}

/// Number of video frames to emit for `duration` at `fps` (at least 1 if duration > 0).
#[must_use]
pub fn frame_count_for(duration: Duration, fps: f64) -> u64 {
    if duration.as_secs() <= 0.0 || !(fps.is_finite() && fps > 0.0) {
        return 0;
    }
    let n = (duration.as_secs() * fps).round();
    if n < 1.0 {
        1
    } else {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        {
            n as u64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{Rgb8, Size};

    #[test]
    fn rgb_passthrough() {
        let f = Frame::solid_rgb(Size::new(1, 1), Rgb8::RED).unwrap();
        let rgb = frame_to_rgb24(&f).unwrap();
        assert_eq!(rgb, vec![255, 0, 0]);
    }

    #[test]
    fn frame_count_rounds() {
        assert_eq!(frame_count_for(Duration::from_secs(1.0), 24.0), 24);
        assert_eq!(frame_count_for(Duration::ZERO, 24.0), 0);
    }
}
