//! Speech input: microphone capture with cpal on its own thread (the stream is not
//! `Send`) and transcription with whisper-rs, language fixed to Portuguese.

use std::{
    path::Path,
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread,
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::oneshot;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const SAMPLE_RATE: u32 = 16_000;

pub enum RecMsg {
    Start,
    /// Stops and returns 16 kHz mono samples.
    Stop(oneshot::Sender<Vec<f32>>),
}

pub fn has_microphone() -> bool {
    cpal::default_host().default_input_device().is_some()
}

pub fn start_recorder() -> mpsc::Sender<RecMsg> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new().name("mic".into()).spawn(move || recorder(&rx)).expect("spawn mic thread");
    tx
}

fn recorder(rx: &mpsc::Receiver<RecMsg>) {
    while let Ok(msg) = rx.recv() {
        let RecMsg::Start = msg else {
            if let RecMsg::Stop(reply) = msg {
                let _ = reply.send(Vec::new());
            }
            continue;
        };
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let capture = open_stream(buffer.clone());
        if let Err(error) = &capture {
            tracing::warn!(%error, "microphone capture failed");
        }
        // Hold the stream until the operator lets go of the key.
        let reply = loop {
            match rx.recv() {
                Ok(RecMsg::Stop(reply)) => break Some(reply),
                Ok(RecMsg::Start) => {}
                Err(_) => break None,
            }
        };
        let rate = capture.as_ref().map_or(SAMPLE_RATE, |(_, rate)| *rate);
        drop(capture);
        let samples = std::mem::take(&mut *buffer.lock().unwrap_or_else(PoisonError::into_inner));
        match reply {
            Some(reply) => {
                let _ = reply.send(resample(&samples, rate, SAMPLE_RATE));
            }
            None => return,
        }
    }
}

/// Opens the default microphone, mixing every frame down to mono into `buffer`.
fn open_stream(buffer: Arc<Mutex<Vec<f32>>>) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host().default_input_device().ok_or("no microphone")?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = usize::from(supported.channels()).max(1);
    let rate = supported.sample_rate();
    let config = supported.config();
    let on_error = |error| tracing::warn!(%error, "microphone stream error");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _: &_| push_mono(&buffer, data.iter().copied(), channels),
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _: &_| {
                push_mono(&buffer, data.iter().map(|s| f32::from(*s) / f32::from(i16::MAX)), channels);
            },
            on_error,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

fn push_mono(buffer: &Mutex<Vec<f32>>, samples: impl Iterator<Item = f32>, channels: usize) {
    let samples: Vec<f32> = samples.collect();
    let mut buffer = buffer.lock().unwrap_or_else(PoisonError::into_inner);
    buffer.extend(samples.chunks(channels).map(|frame| frame.iter().sum::<f32>() / frame.len() as f32));
}

/// Linear resampling; good enough for speech.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let len = (samples.len() as f64 / ratio) as usize;
    (0..len)
        .map(|i| {
            let position = i as f64 * ratio;
            let index = position as usize;
            let next = samples.get(index + 1).copied().unwrap_or(samples[index]);
            let fraction = (position - index as f64) as f32;
            samples[index] * (1.0 - fraction) + next * fraction
        })
        .collect()
}

pub fn load_model(path: &Path) -> Result<WhisperContext, String> {
    // whisper.cpp logs through `tracing` instead of stderr.
    whisper_rs::install_logging_hooks();
    WhisperContext::new_with_params(path, WhisperContextParameters::default()).map_err(|e| e.to_string())
}

/// CPU heavy: call from `spawn_blocking`.
/// `hint` biases whisper toward words we expect (the pet's name).
pub fn transcribe(
    context: &WhisperContext,
    samples: &[f32],
    language: &str,
    hint: Option<&str>,
) -> Result<String, String> {
    // Whisper needs at least a second of audio.
    let mut padded = samples.to_vec();
    padded.resize(padded.len().max(SAMPLE_RATE as usize + SAMPLE_RATE as usize / 10), 0.0);
    let mut state = context.create_state().map_err(|e| e.to_string())?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(language));
    if let Some(hint) = hint {
        params.set_initial_prompt(hint);
    }
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    let threads = thread::available_parallelism().map_or(4, |n| n.get().min(8));
    params.set_n_threads(threads as _);
    state.full(params, &padded).map_err(|e| e.to_string())?;
    let text: Vec<String> = (0..state.full_n_segments())
        .filter_map(|i| state.get_segment(i))
        .filter_map(|segment| segment.to_str_lossy().ok().map(|s| s.trim().to_owned()))
        .collect();
    Ok(text.join(" ").trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_halves_and_keeps_shape() {
        let samples: Vec<f32> = (0..32_000).map(|i| (i % 2) as f32).collect();
        assert_eq!(resample(&samples, 32_000, 16_000).len(), 16_000);
        assert_eq!(resample(&[0.5; 10], 16_000, 16_000), vec![0.5; 10]);
    }
}
