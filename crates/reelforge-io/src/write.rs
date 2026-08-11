//! Write video (and optional audio) clips to container files via `FFmpeg`.

use crate::error::{IoError, Result};
use crate::ffmpeg::{
    FfmpegTools, encode_rawvideo_gif, encode_rawvideo_h264, frame_count_for, mux_video_audio,
};
use crate::options::{WriteGifOptions, WriteVideoOptions};
use reelforge_core::{AudioClip, Duration, Size, Time, VideoClip};
use std::path::{Path, PathBuf};

/// Encode `clip` to a video file using the `FFmpeg` CLI (video only).
///
/// Default codec is `libx264` with `yuv420p` (even frame dimensions required).
/// Prefer [`write_av`] when an audio track should be muxed.
///
/// # Errors
///
/// Returns tool, timing, size, or process errors.
pub fn write_video(clip: &dyn VideoClip, options: &WriteVideoOptions) -> Result<()> {
    write_video_inner(clip, options, None)
}

/// Encode video and mux `audio` into the same container.
///
/// Renders the full audio duration (clipped to the written video duration) as
/// PCM, encodes video, then remuxes with the configured audio codec (default `aac`).
///
/// # Errors
///
/// Returns tool, timing, size, sample, or process errors.
pub fn write_av(
    video: &dyn VideoClip,
    audio: &dyn AudioClip,
    options: &WriteVideoOptions,
) -> Result<()> {
    write_video_inner(video, options, Some(audio))
}

/// Encode `clip` to an animated GIF via host `ffmpeg` (palettegen/paletteuse).
///
/// # Errors
///
/// Returns tool, timing, size, or process errors.
pub fn write_gif(clip: &dyn VideoClip, options: &WriteGifOptions) -> Result<()> {
    if !(options.fps.is_finite() && options.fps > 0.0) {
        return Err(IoError::message(format!("invalid fps {}", options.fps)));
    }
    let duration = options.duration.unwrap_or_else(|| clip.duration());
    if !duration.is_positive() {
        return Err(IoError::message("gif duration must be > 0"));
    }
    let size = options.size.unwrap_or_else(|| clip.size());
    size.require_positive().map_err(IoError::from)?;
    if options.size.is_none() {
        // keep clip size
    } else if clip.size().width < size.width || clip.size().height < size.height {
        return Err(IoError::message(
            "cannot expand frame size during gif write; resize the clip first",
        ));
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
    encode_rawvideo_gif(&tools, path, size, fps, frames)
}

fn write_video_inner(
    clip: &dyn VideoClip,
    options: &WriteVideoOptions,
    audio: Option<&dyn AudioClip>,
) -> Result<()> {
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
    let audio_codec = options.audio_codec.as_deref().unwrap_or("aac");

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

    if let Some(audio) = audio {
        // Video → temp, audio → temp PCM, then mux.
        let video_tmp = temp_sibling(path, "rf-vid");
        let audio_tmp = temp_sibling(path, "rf-aud");
        let video_result = encode_rawvideo_h264(
            &tools,
            &video_tmp,
            size,
            fps,
            video_codec,
            options.crf,
            pixel_format,
            frames,
        );
        let audio_result = render_audio_pcm(audio, duration, &audio_tmp);
        let mux_result = match (video_result, audio_result) {
            (Ok(()), Ok(fmt)) => mux_video_audio(
                &tools,
                &video_tmp,
                &audio_tmp,
                path,
                audio_codec,
                fmt.sample_rate,
                fmt.channels(),
            ),
            (Err(e), _) | (_, Err(e)) => Err(e),
        };
        let _ = std::fs::remove_file(&video_tmp);
        let _ = std::fs::remove_file(&audio_tmp);
        mux_result
    } else {
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
}

fn render_audio_pcm(
    audio: &dyn AudioClip,
    duration: Duration,
    path: &Path,
) -> Result<reelforge_core::AudioFormat> {
    /// ~1 s at 48 kHz per pull.
    const CHUNK: usize = 48_000;

    let format = audio.format();
    let total = format.frames_for_duration(duration);
    let total = usize::try_from(total).map_err(|_| IoError::message("audio length overflow"))?;
    // Pull in chunks to avoid one giant samples_at when implementations allocate.
    let mut all = Vec::with_capacity(total.saturating_mul(format.channels() as usize));
    let mut written = 0_usize;
    while written < total {
        let n = (total - written).min(CHUNK);
        #[allow(clippy::cast_precision_loss)]
        let t = Time::from_secs(written as f64 / f64::from(format.sample_rate));
        // samples_at may error at exact end; clamp time into range.
        let t = if t.as_secs() >= duration.as_secs() && duration.as_secs() > 0.0 {
            Time::from_secs((duration.as_secs() - 1.0 / f64::from(format.sample_rate)).max(0.0))
        } else {
            t
        };
        let buf = audio.samples_at(t, n).map_err(IoError::from)?;
        all.extend_from_slice(buf.samples());
        written += n;
    }

    let mut bytes = Vec::with_capacity(all.len() * 4);
    for s in all {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, bytes).map_err(|e| IoError::message(format!("write pcm: {e}")))?;
    Ok(format)
}

fn temp_sibling(path: &Path, tag: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("reelforge");
    let name = format!(".{stem}.{tag}.{}.tmp", std::process::id());
    parent.join(name)
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

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{AudioFormat, ColorClip, Rgb8, SampleLayout, SilenceClip};

    #[test]
    fn even_size_floors() {
        let s = ensure_even_size(Size::new(641, 361)).unwrap();
        assert_eq!(s, Size::new(640, 360));
    }

    #[test]
    fn write_duration_uses_clip() {
        let clip = ColorClip::new(Size::new(4, 4), Rgb8::BLACK, Duration::from_secs(2.0));
        let opts = WriteVideoOptions::new("x.mp4", 24.0);
        assert!((write_duration(&clip, &opts).as_secs() - 2.0).abs() < 1e-9);
    }

    #[test]
    fn silence_renders_pcm_bytes() {
        let audio = SilenceClip::new(
            AudioFormat {
                sample_rate: 8_000,
                layout: SampleLayout::Mono,
            },
            Duration::from_secs(0.05),
        );
        let dir = std::env::temp_dir().join(format!("rf-pcm-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("a.pcm");
        let fmt = render_audio_pcm(&audio, Duration::from_secs(0.05), &path).unwrap();
        assert_eq!(fmt.sample_rate, 8_000);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
