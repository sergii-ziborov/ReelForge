//! Nonzero PCM regressions for block reads and bus composition.

use reelforge_compose::{ConcatAudio, MixAudio, MixTrack};
use reelforge_core::{
    AudioBuffer, AudioClip, AudioFormat, AudioMixMode, AudioTimeline, CoreError, Duration,
    MediaTime, SampleLayout, Time,
};
use std::sync::Arc;

struct StrictPcm {
    format: AudioFormat,
    samples: Vec<f32>,
}

impl AudioClip for StrictPcm {
    fn duration(&self) -> Duration {
        self.format
            .duration_of_frames((self.samples.len() / usize::from(self.format.channels())) as u64)
    }

    fn format(&self) -> AudioFormat {
        self.format
    }

    fn samples_at(&self, t: Time, frames: usize) -> reelforge_core::Result<AudioBuffer> {
        self.samples_at_media(MediaTime::from_time(t, self.format.sample_rate)?, frames)
    }

    fn samples_at_media(&self, t: MediaTime, frames: usize) -> reelforge_core::Result<AudioBuffer> {
        let first = usize::try_from(t.frame_index(f64::from(self.format.sample_rate))).unwrap();
        let ch = usize::from(self.format.channels());
        let end = (first + frames) * ch;
        if t.ticks < 0 || end > self.samples.len() {
            return Err(CoreError::invalid_audio(
                "read exceeds strict source interval",
            ));
        }
        AudioBuffer::from_interleaved(self.format, self.samples[first * ch..end].to_vec())
    }
}

fn pcm(samples: &[f32]) -> Arc<dyn AudioClip> {
    Arc::new(StrictPcm {
        format: AudioFormat::new(48_000, SampleLayout::Mono).unwrap(),
        samples: samples.to_vec(),
    })
}

fn read_blocks(clip: &dyn AudioClip, frames: usize, block_size: usize) -> Vec<f32> {
    let mut samples = Vec::new();
    let mut first = 0;
    while first < frames {
        let count = block_size.min(frames - first);
        let t = MediaTime::new(i64::try_from(first).unwrap(), clip.format().sample_rate).unwrap();
        samples.extend_from_slice(clip.samples_at_media(t, count).unwrap().samples());
        first += count;
    }
    samples
}

#[test]
fn concat_crosses_multiple_strict_source_boundaries_in_one_pull() {
    let clip = ConcatAudio::new(vec![
        pcm(&[0.1, 0.2, 0.3]),
        pcm(&[0.4, 0.5]),
        pcm(&[0.6, 0.7, 0.8, 0.9]),
    ])
    .unwrap();
    let t = MediaTime::new(2, 48_000).unwrap();
    assert_eq!(
        clip.samples_at_media(t, 7).unwrap().samples(),
        &[0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]
    );
    for block in [1, 7, 128, 4096] {
        assert_eq!(
            read_blocks(&clip, 9, block),
            vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]
        );
    }
    assert_eq!(
        clip.samples_at_media(MediaTime::new(8, 48_000).unwrap(), 3)
            .unwrap()
            .samples(),
        &[0.9, 0.0, 0.0]
    );
}

#[test]
fn mix_bounds_reads_to_the_active_source_interval() {
    let clip = MixAudio::new(vec![
        MixTrack::new(pcm(&[0.1, 0.2, 0.3])).with_start(Time::from_secs(2.0 / 48_000.0)),
    ])
    .unwrap();
    assert_eq!(
        read_blocks(&clip, 7, 7),
        vec![0.0, 0.0, 0.1, 0.2, 0.3, 0.0, 0.0]
    );
    assert_eq!(read_blocks(&clip, 7, 1), read_blocks(&clip, 7, 7));
}

#[test]
fn mix_rejects_nonfinite_gain_start_and_duration() {
    for gain in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(
            MixAudio::new(vec![MixTrack {
                clip: pcm(&[0.1]),
                gain,
                start: Time::ZERO
            }])
            .is_err()
        );
    }
    for start in [f64::NAN, f64::INFINITY] {
        assert!(
            MixAudio::new(vec![
                MixTrack::new(pcm(&[0.1])).with_start(Time::from_secs(start))
            ])
            .is_err()
        );
    }
}

#[test]
fn buffer_rejects_invalid_format_and_nonfinite_samples() {
    let bad = AudioFormat {
        sample_rate: 48_000,
        layout: SampleLayout::Discrete(0),
    };
    assert!(AudioBuffer::from_interleaved(bad, vec![]).is_err());
    assert!(AudioBuffer::silence(bad, 2).is_err());
    for sample in [f32::NAN, f32::INFINITY] {
        assert!(AudioBuffer::from_interleaved(AudioFormat::STEREO_48K, vec![sample, 0.0]).is_err());
    }
}

#[test]
fn linear_nested_buses_preserve_headroom_and_cancellation() {
    let inner: Arc<dyn AudioClip> = Arc::new(
        MixAudio::new_with_mode(
            vec![MixTrack::new(pcm(&[0.8])), MixTrack::new(pcm(&[0.8]))],
            AudioMixMode::Linear,
        )
        .unwrap(),
    );
    assert_eq!(read_blocks(inner.as_ref(), 1, 1), vec![1.6]);
    let nested = MixAudio::new_with_mode(
        vec![MixTrack::new(inner), MixTrack::new(pcm(&[-0.8]))],
        AudioMixMode::Linear,
    )
    .unwrap();
    let flat = MixAudio::new_with_mode(
        vec![
            MixTrack::new(pcm(&[0.8])),
            MixTrack::new(pcm(&[0.8])),
            MixTrack::new(pcm(&[-0.8])),
        ],
        AudioMixMode::Linear,
    )
    .unwrap();
    assert_eq!(read_blocks(&nested, 1, 1), read_blocks(&flat, 1, 1));
    assert_eq!(read_blocks(&nested, 1, 1), vec![0.8]);
    let legacy =
        MixAudio::new(vec![MixTrack::new(pcm(&[0.8])), MixTrack::new(pcm(&[0.8]))]).unwrap();
    assert_eq!(read_blocks(&legacy, 1, 1), vec![1.0]);
}

#[test]
fn stereo_offsets_preroll_and_seek_do_not_change_block_reads() {
    let format = AudioFormat::STEREO_48K;
    let signal: Arc<dyn AudioClip> = Arc::new(StrictPcm {
        format,
        samples: vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4],
    });
    let clip = MixAudio::new_with_mode(
        vec![MixTrack::new(signal).with_start(Time::from_secs(-1.0 / 48_000.0))],
        AudioMixMode::Linear,
    )
    .unwrap();
    let expected = vec![0.2, -0.2, 0.3, -0.3, 0.4, -0.4];
    for block in [1, 7, 128, 4096] {
        assert_eq!(read_blocks(&clip, 3, block), expected);
    }
    let tail = clip
        .samples_at_media(MediaTime::new(2, 48_000).unwrap(), 2)
        .unwrap();
    assert_eq!(tail.samples(), &[0.4, -0.4, 0.0, 0.0]);
    assert_eq!(read_blocks(&clip, 3, 3), expected);
}

#[test]
fn rational_video_boundaries_do_not_accumulate_audio_rounding() {
    let clock = AudioTimeline::from_format(AudioFormat::STEREO_48K).unwrap();
    let mut frames = 0;
    for n in 0..1000 {
        let (first, last) = clock.range(
            MediaTime::new(n * 1001, 30_000).unwrap(),
            MediaTime::new((n + 1) * 1001, 30_000).unwrap(),
        );
        frames += last - first;
    }
    assert_eq!(frames, 1_601_600);
}

#[test]
fn missing_pcm_inside_a_source_is_refused_instead_of_silenced() {
    struct ShortSource;
    impl AudioClip for ShortSource {
        fn duration(&self) -> Duration {
            Duration::from_secs(1.0)
        }
        fn format(&self) -> AudioFormat {
            AudioFormat::STEREO_48K
        }
        fn samples_at(&self, _: Time, frames: usize) -> reelforge_core::Result<AudioBuffer> {
            AudioBuffer::silence(self.format(), frames.saturating_sub(1))
        }
    }
    let source: Arc<dyn AudioClip> = Arc::new(ShortSource);
    let concat = ConcatAudio::new(vec![Arc::clone(&source)]).unwrap();
    let mix = MixAudio::new(vec![MixTrack::new(source)]).unwrap();
    assert!(concat.samples_at(Time::ZERO, 7).is_err());
    assert!(mix.samples_at(Time::ZERO, 7).is_err());
}

#[test]
fn stereo_concat_keeps_channel_order_across_a_boundary() {
    let source = |samples: &[f32]| -> Arc<dyn AudioClip> {
        Arc::new(StrictPcm {
            format: AudioFormat::STEREO_48K,
            samples: samples.to_vec(),
        })
    };
    let concat =
        ConcatAudio::new(vec![source(&[0.1, -0.1, 0.2, -0.2]), source(&[0.3, -0.3])]).unwrap();
    assert_eq!(
        read_blocks(&concat, 3, 4096),
        vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3]
    );
}
