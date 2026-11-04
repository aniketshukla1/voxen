//! Mac audio in and out: the microphone feeds the listener, the speaker plays the voices.

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// What's left to play, shared with the audio thread. Clones share one speaker.
// ponytail: a mutex inside the audio callback; switch to a lock-free ring buffer if playback glitches.
#[derive(Clone)]
pub struct Speaker {
    queue: Arc<Mutex<VecDeque<f32>>>,
    rate: u32,
}

impl Speaker {
    /// Queues mono samples recorded at `from_rate` Hz.
    pub fn play(&self, samples: &[i16], from_rate: u32) {
        let resampled = resample(samples, from_rate, self.rate);
        if let Ok(mut queue) = self.queue.lock() {
            queue.extend(resampled);
        }
    }

    /// Goes quiet at once.
    pub fn stop(&self) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.clear();
        }
    }

    pub fn idle(&self) -> bool {
        self.queue.lock().map(|q| q.is_empty()).unwrap_or(true)
    }
}

/// Opens the default output device. Sound plays for as long as the stream is kept alive.
pub fn speaker() -> Result<(cpal::Stream, Speaker)> {
    let device = cpal::default_host()
        .default_output_device()
        .context("no speaker or headphones found")?;
    let supported = device.default_output_config()?;
    let config: StreamConfig = supported.config();
    let speaker = Speaker {
        queue: Arc::default(),
        rate: config.sample_rate,
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => output::<f32>(&device, config, speaker.clone())?,
        SampleFormat::I16 => output::<i16>(&device, config, speaker.clone())?,
        other => bail!("speaker sample format {other} isn't supported yet"),
    };
    stream.play()?;
    Ok((stream, speaker))
}

fn output<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: StreamConfig,
    speaker: Speaker,
) -> Result<cpal::Stream> {
    let channels = usize::from(config.channels);
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut queue = speaker.queue.lock().ok();
            for frame in data.chunks_mut(channels) {
                let value = queue.as_mut().and_then(|q| q.pop_front()).unwrap_or(0.0);
                frame.fill(T::from_sample(value));
            }
        },
        |e| eprintln!("(speaker: {e})"),
        None,
    )?)
}

/// Opens the default microphone and sends mono 16-bit frames to `tx`. Returns the stream,
/// which must be kept alive to keep listening, and its sample rate.
pub fn mic(tx: mpsc::UnboundedSender<Vec<i16>>) -> Result<(cpal::Stream, u32)> {
    let device = cpal::default_host()
        .default_input_device()
        .context("no microphone found")?;
    let supported = device.default_input_config()?;
    let config: StreamConfig = supported.config();
    let rate = config.sample_rate;
    let stream = match supported.sample_format() {
        SampleFormat::F32 => input::<f32>(&device, config, tx)?,
        SampleFormat::I16 => input::<i16>(&device, config, tx)?,
        other => bail!("microphone sample format {other} isn't supported yet"),
    };
    stream.play()?;
    Ok((stream, rate))
}

fn input<T: SizedSample>(
    device: &cpal::Device,
    config: StreamConfig,
    tx: mpsc::UnboundedSender<Vec<i16>>,
) -> Result<cpal::Stream>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mono = data
                .chunks(channels)
                .map(|frame| {
                    let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                    i16::from_sample(sum / channels as f32)
                })
                .collect();
            tx.send(mono).ok();
        },
        |e| eprintln!("(microphone: {e})"),
        None,
    )?)
}

/// Linear resampling from `from` Hz to `to` Hz, converting to floats in -1.0..1.0.
// ponytail: linear interpolation per chunk; use a proper resampler if voices sound harsh.
pub fn resample(samples: &[i16], from: u32, to: u32) -> Vec<f32> {
    let Some(&last) = samples.last() else {
        return Vec::new();
    };
    let len = (samples.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..len)
        .map(|i| {
            let pos = i as f64 * step;
            let j = pos as usize;
            let a = f32::from(samples[j]);
            let b = f32::from(samples.get(j + 1).copied().unwrap_or(last));
            (a + (b - a) * (pos - j as f64) as f32) / 32768.0
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_up_down_and_level() {
        let scale = |v: &[f32]| v.iter().map(|s| (s * 32768.0).round()).collect::<Vec<_>>();
        assert_eq!(
            scale(&resample(&[0, 1000], 24_000, 48_000)),
            vec![0.0, 500.0, 1000.0, 1000.0]
        );
        assert_eq!(
            scale(&resample(&[0, 100, 200, 300], 48_000, 24_000)),
            vec![0.0, 200.0]
        );
        assert_eq!(resample(&[16384], 48_000, 48_000), vec![0.5]);
        assert!(resample(&[], 24_000, 48_000).is_empty());
    }
}
