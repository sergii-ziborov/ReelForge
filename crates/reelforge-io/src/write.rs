//! Write video clips to container files via `FFmpeg`.

use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, encode_rawvideo_h264, frame_count_for};
use crate::options::WriteVideoOptions;
use reelforge_core::{Duration, Size, Time, VideoClip};
use std::path::Path;

/// Encode `clip` to a video file using the `FFmpeg` CLI.
///
/// Video-only write path; audio mux is separate.
/// Default codec is `libx264` with `yuv420p` (even frame dimensions required).
///
/// # Errors
///
/// Returns tool, timing, size, or process errors.
pub fn write_video(clip: &dyn VideoClip, options: &WriteVideoOptions) -> Result<()> {
    if !(options.fps.is_finite() && options.fps > 0.0) {
        return Err(IoError::message(format!("invalid fps {}", options.fps)));
    }

    let duration = options.duration.unwrap_or_else(|| clip.duration());
    if !duration.is_positive() {
        return Err(IoError::message("write duration must be > 0"));
    }

    let size = options.size.unwrap_or_else(|| clip.size());
    let size = ensure_even_size(size)?;
    if options.size.is_none() && clip.size() != size {
        // Odd source sizes are cropped to even for yuv420p encoders.
        if clip.size().width < size.width || clip.size().height < size.height {
            return Err(IoError::message(
                "cannot expand frame size during write; resize the clip first",
            ));
        }
    }

    let tools = FfmpegTools::discover()?;
    let path = Path::new(&options.path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| IoError::message(format!("create output dir: {e}")))?;
    }

    let video_codec = options.video_codec.as_deref().unwrap_or("libx264");
    let pixel_format = options.pixel_format.as_deref().unwrap_or("yuv420p");

    let n = frame_count_for(duration, options.fps);
    if n == 0 {
        return Err(IoError::message("no frames to write"));
    }

    let fps = options.fps;
    let frames = (0..n).map(move |i| {
        #[allow(clippy::cast_precision_loss)]
        let t = Time::from_secs(i as f64 / fps);
        let t = if t.as_secs() >= duration.as_secs() {
            Time::from_secs((duration.as_secs() - 1.0 / fps).max(0.0))
        } else {
            t
        };
        let mut frame = clip.frame_at(t)?;
        if frame.size() != size {
            frame = crop_top_left(&frame, size).map_err(IoError::from)?;
        }
        Ok(frame)
    });

    encode_rawvideo_h264(
        &tools,
        path,
        size,
        fps,
        video_codec,
        options.crf,
        pixel_format,
        frames,
    )
}

fn ensure_even_size(size: Size) -> Result<Size> {
    size.require_positive().map_err(IoError::from)?;
    let w = size.width - (size.width % 2);
    let h = size.height - (size.height % 2);
    if w == 0 || h == 0 {
        return Err(IoError::message(format!(
            "size {size:?} collapses to zero when forced even"
        )));
    }
    Ok(Size::new(w, h))
}

fn crop_top_left(
    frame: &reelforge_core::Frame,
    size: Size,
) -> reelforge_core::Result<reelforge_core::Frame> {
    let src = frame.size();
    if size.width > src.width || size.height > src.height {
        return Err(reelforge_core::CoreError::invalid_frame(
            "crop size exceeds source",
        ));
    }
    let bpp = frame.format().bytes_per_pixel();
    let mut out = Vec::with_capacity(
        usize::try_from(size.pixel_count())
            .map_err(|_| reelforge_core::CoreError::invalid_frame("overflow"))?
            * bpp,
    );
    let row_src = src.width as usize * bpp;
    let row_dst = size.width as usize * bpp;
    let data = frame.data();
    for y in 0..size.height as usize {
        let start = y * row_src;
        out.extend_from_slice(&data[start..start + row_dst]);
    }
    reelforge_core::Frame::from_raw(size, frame.format(), out)
}

/// Suggested duration helper for callers.
#[must_use]
pub fn write_duration(clip: &dyn VideoClip, options: &WriteVideoOptions) -> Duration {
    options.duration.unwrap_or_else(|| clip.duration())
}
