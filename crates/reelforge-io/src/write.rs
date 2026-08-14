//! Write video (and optional audio) clips to container files via `FFmpeg`.

use crate::control::{WriteControl, WriteProgress, WriteStage};
use crate::encode_native::encode_sampled_rawvideo;
use crate::error::{IoError, Result};
use crate::ffmpeg::{FfmpegTools, encode_rawvideo_gif, frame_count_for, mux_video_audio};
use crate::options::{WriteGifOptions, WriteVideoOptions};
use crate::pipeline::encode_sampled_h264;
use reelforge_core::{
    AudioClip, Duration, MemoryLocation, PixelFormat, Size, Time, VideoClip, surface_to_rawvideo,
};
use std::io::Write;
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
    write_video_with(clip, options, &WriteControl::default())
}

/// Like [`write_video`] with explicit progress / cancel / pipeline controls.
///
/// # Errors
///
/// Returns tool, cancel, timing, size, or process errors.
pub fn write_video_with(
    clip: &dyn VideoClip,
    options: &WriteVideoOptions,
    control: &WriteControl,
) -> Result<()> {
    write_video_inner(clip, options, None, control)
}

/// Encode video and mux `audio` into the same container.
///
/// Audio is **streamed** to a temp PCM file in chunks (no full-timeline buffer).
/// Video is encoded first, then muxed.
///
/// # Errors
///
/// Returns tool, timing, size, sample, cancel, or process errors.
pub fn write_av(
    video: &dyn VideoClip,
    audio: &dyn AudioClip,
    options: &WriteVideoOptions,
) -> Result<()> {
    write_av_with(video, audio, options, &WriteControl::default())
}

/// Like [`write_av`] with explicit progress / cancel / pipeline controls.
///
/// # Errors
///
/// Returns tool, cancel, timing, size, sample, or process errors.
pub fn write_av_with(
    video: &dyn VideoClip,
    audio: &dyn AudioClip,
    options: &WriteVideoOptions,
    control: &WriteControl,
) -> Result<()> {
    write_video_inner(video, options, Some(audio), control)
}

/// Encode `clip` to an animated GIF via host `ffmpeg` (palettegen/paletteuse).
///
/// # Errors
///
/// Returns tool, timing, size, or process errors.
pub fn write_gif(clip: &dyn VideoClip, options: &WriteGifOptions) -> Result<()> {
    write_gif_with(clip, options, &WriteControl::default())
}

/// Like [`write_gif`] with cancel / progress (video stage only).
///
/// # Errors
///
/// Returns tool, cancel, timing, size, or process errors.
pub fn write_gif_with(
    clip: &dyn VideoClip,
    options: &WriteGifOptions,
    control: &WriteControl,
) -> Result<()> {
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
    ensure_parent_dir(Path::new(&options.path))?;

    let n = frame_count_for(duration, options.fps);
    if n == 0 {
        return Err(IoError::message("no frames to write"));
    }
    let fps = options.fps;
    let frames = (0..n).map(move |i| {
        control.check_cancel()?;
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
        control.report(WriteProgress::new(WriteStage::Video, i + 1, n));
        Ok(frame)
    });
    encode_rawvideo_gif(&tools, Path::new(&options.path), size, fps, frames)?;
    control.report(WriteProgress::new(WriteStage::Done, n, n));
    Ok(())
}

fn write_video_inner(
    clip: &dyn VideoClip,
    options: &WriteVideoOptions,
    audio: Option<&dyn AudioClip>,
    control: &WriteControl,
) -> Result<()> {
    let prepared = prepare_write(clip, options)?;
    let tools = FfmpegTools::discover()?;
    let path = Path::new(&options.path);
    ensure_parent_dir(path)?;

    let video_codec = options.video_codec.as_deref().unwrap_or("libx264");
    let pixel_format = options.pixel_format.as_deref().unwrap_or("yuv420p");
    let audio_codec = options.audio_codec.as_deref().unwrap_or("aac");
    let PreparedWrite {
        duration,
        size,
        frame_count: n,
        fps,
    } = prepared;

    let native = options
        .prefer_native_encode
        .then(|| probe_native_format(clip, size))
        .flatten();

    if let Some(in_fmt) = native {
        return write_native(
            clip,
            options,
            audio,
            control,
            &tools,
            path,
            &prepared,
            video_codec,
            pixel_format,
            audio_codec,
            in_fmt,
        );
    }

    let sample = |i: u64| -> Result<reelforge_core::Frame> {
        sample_write_frame(clip, i, fps, duration, size)
    };

    if let Some(audio) = audio {
        write_av_temps(
            &tools,
            path,
            size,
            fps,
            video_codec,
            pixel_format,
            audio_codec,
            options,
            n,
            &sample,
            audio,
            duration,
            control,
        )
    } else {
        encode_sampled_h264(
            &tools,
            path,
            size,
            fps,
            video_codec,
            options.crf,
            pixel_format,
            &options.extra_ffmpeg_args,
            n,
            &sample,
            control,
        )?;
        control.report(WriteProgress::new(WriteStage::Done, n, n));
        Ok(())
    }
}

struct PreparedWrite {
    duration: Duration,
    size: Size,
    frame_count: u64,
    fps: f64,
}

fn prepare_write(clip: &dyn VideoClip, options: &WriteVideoOptions) -> Result<PreparedWrite> {
    if !(options.fps.is_finite() && options.fps > 0.0) {
        return Err(IoError::message(format!("invalid fps {}", options.fps)));
    }
    let duration = options.duration.unwrap_or_else(|| clip.duration());
    if !duration.is_positive() {
        return Err(IoError::message("write duration must be > 0"));
    }
    let size = options.size.unwrap_or_else(|| clip.size());
    let size = ensure_even_size(size)?;
    if options.size.is_none()
        && clip.size() != size
        && (clip.size().width < size.width || clip.size().height < size.height)
    {
        return Err(IoError::message(
            "cannot expand frame size during write; resize the clip first",
        ));
    }
    let frame_count = frame_count_for(duration, options.fps);
    if frame_count == 0 {
        return Err(IoError::message("no frames to write"));
    }
    Ok(PreparedWrite {
        duration,
        size,
        frame_count,
        fps: options.fps,
    })
}

fn probe_native_format(clip: &dyn VideoClip, out_size: Size) -> Option<PixelFormat> {
    if clip.size() != out_size {
        return None;
    }
    let surface = clip.surface_at(Time::ZERO).ok()?;
    if surface.size() != out_size {
        return None;
    }
    if surface.location() == MemoryLocation::External || surface.external().is_some() {
        return None;
    }
    surface.format().is_yuv().then_some(surface.format())
}

fn sample_write_surface(
    clip: &dyn VideoClip,
    i: u64,
    fps: f64,
    duration: Duration,
    size: Size,
    expected: PixelFormat,
) -> Result<Vec<u8>> {
    let t = sample_time(i, fps, duration);
    let surface = clip.surface_at(t).map_err(IoError::from)?;
    if surface.size() != size {
        return Err(IoError::message(format!(
            "native surface size {:?} != output {size:?}",
            surface.size()
        )));
    }
    if surface.format() != expected {
        return Err(IoError::message(format!(
            "native surface {:?} drifted to {:?}",
            expected,
            surface.format()
        )));
    }
    surface_to_rawvideo(&surface).map_err(IoError::from)
}

fn sample_time(i: u64, fps: f64, duration: Duration) -> Time {
    #[allow(clippy::cast_precision_loss)]
    let t = Time::from_secs(i as f64 / fps);
    if t.as_secs() >= duration.as_secs() {
        Time::from_secs((duration.as_secs() - 1.0 / fps).max(0.0))
    } else {
        t
    }
}

#[allow(clippy::too_many_arguments)]
fn write_native(
    clip: &dyn VideoClip,
    options: &WriteVideoOptions,
    audio: Option<&dyn AudioClip>,
    control: &WriteControl,
    tools: &FfmpegTools,
    path: &Path,
    prepared: &PreparedWrite,
    video_codec: &str,
    pixel_format: &str,
    audio_codec: &str,
    in_fmt: PixelFormat,
) -> Result<()> {
    let PreparedWrite {
        duration,
        size,
        frame_count: n,
        fps,
    } = *prepared;
    let sample =
        |i: u64| -> Result<Vec<u8>> { sample_write_surface(clip, i, fps, duration, size, in_fmt) };
    if let Some(audio) = audio {
        let video_tmp = temp_sibling(path, "rf-vid", "mp4");
        let audio_tmp = temp_sibling(path, "rf-aud", "pcm");
        let video_result = encode_sampled_rawvideo(
            tools,
            &video_tmp,
            size,
            fps,
            video_codec,
            options.crf,
            in_fmt,
            pixel_format,
            &options.extra_ffmpeg_args,
            n,
            &sample,
            control,
        );
        let result = match video_result {
            Ok(()) => {
                let fmt = render_audio_pcm_streaming(audio, duration, &audio_tmp, control)?;
                control.report(WriteProgress::new(WriteStage::Mux, 0, 1));
                control.check_cancel()?;
                let r = mux_video_audio(
                    tools,
                    &video_tmp,
                    &audio_tmp,
                    path,
                    audio_codec,
                    fmt.sample_rate,
                    fmt.channels(),
                );
                if r.is_ok() {
                    control.report(WriteProgress::new(WriteStage::Mux, 1, 1));
                }
                r
            }
            Err(e) => Err(e),
        };
        let _ = std::fs::remove_file(&video_tmp);
        let _ = std::fs::remove_file(&audio_tmp);
        result?;
        control.report(WriteProgress::new(WriteStage::Done, n, n));
        return Ok(());
    }
    encode_sampled_rawvideo(
        tools,
        path,
        size,
        fps,
        video_codec,
        options.crf,
        in_fmt,
        pixel_format,
        &options.extra_ffmpeg_args,
        n,
        &sample,
        control,
    )?;
    control.report(WriteProgress::new(WriteStage::Done, n, n));
    Ok(())
}

fn sample_write_frame(
    clip: &dyn VideoClip,
    i: u64,
    fps: f64,
    duration: Duration,
    size: Size,
) -> Result<reelforge_core::Frame> {
    #[allow(clippy::cast_precision_loss)]
    let t = Time::from_secs(i as f64 / fps);
    let t = if t.as_secs() >= duration.as_secs() {
        Time::from_secs((duration.as_secs() - 1.0 / fps).max(0.0))
    } else {
        t
    };
    let mut frame = clip.frame_at(t).map_err(IoError::from)?;
    if frame.size() != size {
        frame = crop_top_left(&frame, size).map_err(IoError::from)?;
    }
    Ok(frame)
}

#[allow(clippy::too_many_arguments)]
fn write_av_temps(
    tools: &FfmpegTools,
    path: &Path,
    size: Size,
    fps: f64,
    video_codec: &str,
    pixel_format: &str,
    audio_codec: &str,
    options: &WriteVideoOptions,
    n: u64,
    sample: &(dyn Fn(u64) -> Result<reelforge_core::Frame> + Sync),
    audio: &dyn AudioClip,
    duration: Duration,
    control: &WriteControl,
) -> Result<()> {
    let video_tmp = temp_sibling(path, "rf-vid", "mp4");
    let audio_tmp = temp_sibling(path, "rf-aud", "pcm");
    let video_result = encode_sampled_h264(
        tools,
        &video_tmp,
        size,
        fps,
        video_codec,
        options.crf,
        pixel_format,
        &options.extra_ffmpeg_args,
        n,
        sample,
        control,
    );
    let result = match video_result {
        Ok(()) => {
            let fmt = render_audio_pcm_streaming(audio, duration, &audio_tmp, control)?;
            control.report(WriteProgress::new(WriteStage::Mux, 0, 1));
            control.check_cancel()?;
            let r = mux_video_audio(
                tools,
                &video_tmp,
                &audio_tmp,
                path,
                audio_codec,
                fmt.sample_rate,
                fmt.channels(),
            );
            if r.is_ok() {
                control.report(WriteProgress::new(WriteStage::Mux, 1, 1));
                control.report(WriteProgress::new(WriteStage::Done, 1, 1));
            }
            r
        }
        Err(e) => Err(e),
    };
    let _ = std::fs::remove_file(&video_tmp);
    let _ = std::fs::remove_file(&audio_tmp);
    result
}

/// Stream PCM to `path` in chunks (peak memory ≈ one chunk, not full duration).
fn render_audio_pcm_streaming(
    audio: &dyn AudioClip,
    duration: Duration,
    path: &Path,
    control: &WriteControl,
) -> Result<reelforge_core::AudioFormat> {
    /// ~1 s at 48 kHz per pull.
    const CHUNK: usize = 48_000;

    let format = audio.format();
    let total = format.frames_for_duration(duration);
    let total = usize::try_from(total).map_err(|_| IoError::message("audio length overflow"))?;
    let total_u64 = u64::try_from(total).unwrap_or(u64::MAX);

    let mut file =
        std::fs::File::create(path).map_err(|e| IoError::message(format!("create pcm: {e}")))?;
    let mut scratch = Vec::with_capacity(
        CHUNK
            .saturating_mul(4)
            .saturating_mul(format.channels() as usize),
    );
    let mut written = 0_usize;
    let mut chunk_idx = 0_u64;
    #[allow(clippy::cast_possible_truncation)]
    let chunk_total = total.div_ceil(CHUNK) as u64;

    while written < total {
        control.check_cancel()?;
        let n = (total - written).min(CHUNK);
        #[allow(clippy::cast_precision_loss)]
        let t = Time::from_secs(written as f64 / f64::from(format.sample_rate));
        let t = if t.as_secs() >= duration.as_secs() && duration.as_secs() > 0.0 {
            Time::from_secs((duration.as_secs() - 1.0 / f64::from(format.sample_rate)).max(0.0))
        } else {
            t
        };
        let buf = audio.samples_at(t, n).map_err(IoError::from)?;
        scratch.clear();
        for s in buf.samples() {
            scratch.extend_from_slice(&s.to_le_bytes());
        }
        file.write_all(&scratch)
            .map_err(|e| IoError::message(format!("write pcm: {e}")))?;
        written += n;
        chunk_idx += 1;
        control.report(WriteProgress::new(
            WriteStage::Audio,
            chunk_idx.min(chunk_total.max(1)),
            chunk_total.max(1),
        ));
        let _ = total_u64;
    }
    file.flush()
        .map_err(|e| IoError::message(format!("flush pcm: {e}")))?;
    Ok(format)
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| IoError::message(format!("create output dir: {e}")))?;
    }
    Ok(())
}

fn temp_sibling(path: &Path, tag: &str, ext: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("reelforge");
    // Extension matters: ffmpeg needs a known video container for the temp mux input.
    let name = format!(".{stem}.{tag}.{}.{ext}", std::process::id());
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
    fn silence_streams_pcm_bytes() {
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
        let fmt = render_audio_pcm_streaming(
            &audio,
            Duration::from_secs(0.05),
            &path,
            &WriteControl::default(),
        )
        .unwrap();
        assert_eq!(fmt.sample_rate, 8_000);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.is_empty());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn streaming_audio_respects_cancel() {
        let audio = SilenceClip::new(
            AudioFormat {
                sample_rate: 48_000,
                layout: SampleLayout::Stereo,
            },
            Duration::from_secs(2.0),
        );
        let token = crate::control::CancelToken::new();
        token.cancel();
        let control = WriteControl::new().with_cancel(token);
        let dir = std::env::temp_dir().join(format!("rf-pcm-c-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("a.pcm");
        let err = render_audio_pcm_streaming(&audio, Duration::from_secs(2.0), &path, &control);
        assert!(matches!(err, Err(IoError::Cancelled)));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
