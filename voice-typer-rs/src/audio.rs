//! Microphone capture via cpal. Push-to-talk style: start() then stop() returns f32 mono 16kHz.
//!
//! Streaming tap: if a Sender is attached via `set_chunk_tap()`, each cpal callback
//! also pushes its mono-downmixed chunk through that channel at the device's
//! NATIVE sample rate. Used by the WebSocket streaming path to forward audio
//! to Deepgram in real time.
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, Stream, StreamConfig};

pub const TARGET_SR: u32 = 16_000;

pub struct Recorder {
    samples: Arc<Mutex<Vec<f32>>>,
    chunk_tap: Arc<Mutex<Option<Sender<Vec<f32>>>>>,
    stream: Option<Stream>,
    src_sr: u32,
    src_channels: u16,
    device_name: String,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            chunk_tap: Arc::new(Mutex::new(None)),
            stream: None,
            src_sr: 0,
            src_channels: 0,
            device_name: String::new(),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.stream.is_some()
    }

    /// Attach an mpsc sender that receives each cpal callback's mono samples
    /// at the device's native sample rate (call `source_sample_rate()` to get it).
    /// Pass None to detach.
    pub fn set_chunk_tap(&self, tap: Option<Sender<Vec<f32>>>) {
        *self.chunk_tap.lock().unwrap() = tap;
    }

    /// Native sample rate of the active stream (0 if not recording).
    pub fn source_sample_rate(&self) -> u32 {
        self.src_sr
    }

    /// Pick an input device. `name_hint` empty = default. Otherwise first device whose
    /// name contains the substring (case-insensitive).
    fn pick_device(name_hint: &str) -> Result<Device> {
        let host = cpal::default_host();
        if name_hint.is_empty() {
            return host
                .default_input_device()
                .ok_or_else(|| anyhow!("no default input device"));
        }
        let needle = name_hint.to_lowercase();
        for dev in host.input_devices()? {
            let name = dev.name().unwrap_or_default().to_lowercase();
            if name.contains(&needle) {
                return Ok(dev);
            }
        }
        // Fallback to default if hint not matched
        host.default_input_device()
            .ok_or_else(|| anyhow!("no input device matched and no default"))
    }

    pub fn list_input_devices() -> Vec<String> {
        let host = cpal::default_host();
        let mut v = Vec::new();
        if let Ok(it) = host.input_devices() {
            for d in it {
                if let Ok(n) = d.name() {
                    v.push(n);
                }
            }
        }
        v
    }

    pub fn start(&mut self, device_name: &str) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let device = Self::pick_device(device_name)?;
        self.device_name = device.name().unwrap_or_default();
        let cfg = device
            .default_input_config()
            .context("no default input config")?;
        self.src_sr = cfg.sample_rate().0;
        self.src_channels = cfg.channels();
        let sample_format = cfg.sample_format();
        let stream_cfg: StreamConfig = cfg.into();

        // Reset buffer
        {
            let mut buf = self.samples.lock().unwrap();
            buf.clear();
            buf.reserve(self.src_sr as usize * 8); // ~8s headroom per channel
        }

        let samples = Arc::clone(&self.samples);
        let tap = Arc::clone(&self.chunk_tap);
        let channels = self.src_channels as usize;
        let err_fn = |e| log::warn!("audio stream error: {e}");

        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _| push_mono(&samples, &tap, data, channels, |x| x),
                err_fn,
                None,
            )?,
            SampleFormat::I16 => device.build_input_stream(
                &stream_cfg,
                move |data: &[i16], _| {
                    push_mono(&samples, &tap, data, channels, |x| x as f32 / i16::MAX as f32)
                },
                err_fn,
                None,
            )?,
            SampleFormat::U16 => device.build_input_stream(
                &stream_cfg,
                move |data: &[u16], _| {
                    push_mono(&samples, &tap, data, channels, |x| {
                        (x as f32 - 32768.0) / 32768.0
                    })
                },
                err_fn,
                None,
            )?,
            other => return Err(anyhow!("unsupported sample format: {other:?}")),
        };
        stream.play()?;
        log::info!(
            "recording: {} @ {} Hz, {} ch",
            self.device_name,
            self.src_sr,
            self.src_channels
        );
        self.stream = Some(stream);
        Ok(())
    }

    /// Stop and return f32 mono 16kHz audio (resampled).
    pub fn stop(&mut self) -> Vec<f32> {
        if let Some(s) = self.stream.take() {
            drop(s);
        }
        let raw = std::mem::take(&mut *self.samples.lock().unwrap());
        if raw.is_empty() || self.src_sr == 0 {
            return raw;
        }
        if self.src_sr == TARGET_SR {
            return raw;
        }
        resample_linear(&raw, self.src_sr, TARGET_SR)
    }

    /// Sample rate of the captured audio. We always return 16-kHz (resampled).
    pub fn output_sample_rate(&self) -> u32 {
        TARGET_SR
    }
}

fn push_mono<T: Copy, F: Fn(T) -> f32>(
    out: &Arc<Mutex<Vec<f32>>>,
    tap: &Arc<Mutex<Option<Sender<Vec<f32>>>>>,
    interleaved: &[T],
    channels: usize,
    cvt: F,
) {
    // Build mono chunk first (used by both the buffer and the tap).
    let mono: Vec<f32> = if channels <= 1 {
        interleaved.iter().copied().map(&cvt).collect()
    } else {
        interleaved
            .chunks_exact(channels)
            .map(|frame| {
                let sum: f32 = frame.iter().copied().map(&cvt).sum();
                sum / channels as f32
            })
            .collect()
    };

    // Persist into the recording buffer for the batch path.
    out.lock().unwrap().extend(mono.iter().copied());

    // Forward to the streaming tap, if attached. Non-blocking — drop on full.
    if let Some(tx) = tap.lock().unwrap().as_ref() {
        let _ = tx.send(mono);
    }
}

/// Linear resampler (good enough for speech going into Whisper).
fn resample_linear(input: &[f32], from_sr: u32, to_sr: u32) -> Vec<f32> {
    if input.is_empty() || from_sr == to_sr {
        return input.to_vec();
    }
    let ratio = from_sr as f64 / to_sr as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let idx = pos.floor() as usize;
        let frac = (pos - idx as f64) as f32;
        let a = input[idx];
        let b = if idx + 1 < input.len() {
            input[idx + 1]
        } else {
            a
        };
        out.push(a + (b - a) * frac);
    }
    out
}
