//! Mac audio in and out: the microphone feeds the listener, the speaker plays the voices.
//! Each device runs on its own thread, because audio streams can't move between threads
//! on every platform; the handles returned here can.

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc as sync_mpsc};
use tokio::sync::mpsc;

/// How loud the audio is right now, from 0 (silence) to 1, shared with the audio thread.
#[derive(Clone, Default)]
pub struct Level(Arc<AtomicU32>);

impl Level {
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }

    /// The audio it measured has stopped: only this handle is left.
    pub fn gone(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }
}

/// The loudness of `count` samples whose squares add up to `squares`: decibels from -50
/// to -10 mapped onto 0 to 1, so talking lands around the middle.
fn level_of(squares: f32, count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let decibels = 10.0 * (squares / count as f32).max(1e-10).log10();
    ((decibels + 50.0) / 40.0).clamp(0.0, 1.0)
}

/// What's left to play, shared with the audio thread. Clones share one speaker.
// ponytail: a mutex inside the audio callback; switch to a lock-free ring buffer if playback glitches.
#[derive(Clone)]
pub struct Speaker {
    queue: Arc<Mutex<VecDeque<f32>>>,
    rate: u32,
    level: Level,
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

    /// How loud what's playing is right now, 0 to 1.
    pub fn level(&self) -> f32 {
        self.level.get()
    }
}

/// Opens the default output device for the rest of the program's life.
pub fn speaker() -> Result<Speaker> {
    let (ready_tx, ready_rx) = sync_mpsc::channel();
    std::thread::spawn(move || match open_speaker() {
        Ok((_stream, speaker)) => {
            ready_tx.send(Ok(speaker)).ok();
            loop {
                std::thread::park();
            }
        }
        Err(e) => {
            ready_tx.send(Err(e)).ok();
        }
    });
    ready_rx.recv().context("the speaker thread stopped")?
}

fn open_speaker() -> Result<(cpal::Stream, Speaker)> {
    let device = cpal::default_host()
        .default_output_device()
        .context("no speaker or headphones found")?;
    let supported = device.default_output_config()?;
    let config: StreamConfig = supported.config();
    let speaker = Speaker {
        queue: Arc::default(),
        rate: config.sample_rate,
        level: Level::default(),
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
            let mut squares = 0.0;
            for frame in data.chunks_mut(channels) {
                let value = queue.as_mut().and_then(|q| q.pop_front()).unwrap_or(0.0);
                squares += value * value;
                frame.fill(T::from_sample(value));
            }
            speaker.level.set(level_of(squares, data.len() / channels.max(1)));
        },
        |e| eprintln!("(speaker: {e})"),
        None,
    )?)
}

/// The open microphone. It closes when this is dropped.
pub struct Mic {
    /// Samples per second of the frames it sends.
    pub rate: u32,
    /// How loud you are right now.
    pub level: Level,
    _close: sync_mpsc::Sender<()>,
}

/// Opens the default microphone and sends mono 16-bit frames to `tx` until the `Mic` is dropped.
pub fn mic(tx: mpsc::UnboundedSender<Vec<i16>>) -> Result<Mic> {
    let (ready_tx, ready_rx) = sync_mpsc::channel();
    let (close_tx, close_rx) = sync_mpsc::channel::<()>();
    let level = Level::default();
    let meter = level.clone();
    std::thread::spawn(move || match open_mic(tx, meter) {
        Ok((_stream, rate)) => {
            ready_tx.send(Ok(rate)).ok();
            close_rx.recv().ok(); // returns once the Mic is dropped
        }
        Err(e) => {
            ready_tx.send(Err(e)).ok();
        }
    });
    let rate = ready_rx.recv().context("the microphone thread stopped")??;
    Ok(Mic {
        rate,
        level,
        _close: close_tx,
    })
}

fn open_mic(tx: mpsc::UnboundedSender<Vec<i16>>, level: Level) -> Result<(cpal::Stream, u32)> {
    let device = cpal::default_host()
        .default_input_device()
        .context("no microphone found")?;
    let supported = device.default_input_config()?;
    let config: StreamConfig = supported.config();
    let rate = config.sample_rate;
    let stream = match supported.sample_format() {
        SampleFormat::F32 => input::<f32>(&device, config, tx, level)?,
        SampleFormat::I16 => input::<i16>(&device, config, tx, level)?,
        other => bail!("microphone sample format {other} isn't supported yet"),
    };
    stream.play()?;
    Ok((stream, rate))
}

fn input<T: SizedSample>(
    device: &cpal::Device,
    config: StreamConfig,
    tx: mpsc::UnboundedSender<Vec<i16>>,
    level: Level,
) -> Result<cpal::Stream>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut squares = 0.0;
            let mono: Vec<i16> = data
                .chunks(channels)
                .map(|frame| {
                    let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                    let value = sum / channels as f32;
                    squares += value * value;
                    i16::from_sample(value)
                })
                .collect();
            level.set(level_of(squares, mono.len()));
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
    fn loudness_puts_silence_at_zero_and_talking_in_the_middle() {
        let level = |amplitude: f32| level_of(amplitude * amplitude * 100.0, 100);
        assert_eq!(level_of(0.0, 0), 0.0);
        assert_eq!(level(0.0), 0.0);
        assert!((0.3..0.8).contains(&level(0.05)), "{}", level(0.05));
        assert_eq!(level(1.0), 1.0);
        let meter = Level::default();
        meter.set(0.4);
        assert_eq!(meter.get(), 0.4);
        assert!(meter.gone());
    }

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
