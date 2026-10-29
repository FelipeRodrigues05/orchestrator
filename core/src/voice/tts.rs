//! Speech output: Piper as a subprocess, played with rodio on its own thread.
//! A priority queue makes permission requests jump ahead (SPEC "Saída").

use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    io::{Read, Write},
    num::NonZero,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use super::Priority;

pub enum TtsMsg {
    Speak(Priority, String),
    /// Silences speech while the operator talks; stops the current phrase.
    Mute(bool),
}

pub struct Voice {
    pub piper: PathBuf,
    pub model: PathBuf,
}

/// Starts the speech thread. `speaking` reports when a phrase starts and ends.
pub fn start(voice: Voice, speaking: impl Fn(bool) + Send + 'static) -> mpsc::Sender<TtsMsg> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new().name("tts".into()).spawn(move || run(&voice, &rx, speaking)).expect("spawn tts thread");
    tx
}

fn run(voice: &Voice, rx: &mpsc::Receiver<TtsMsg>, speaking: impl Fn(bool)) {
    let sink = match rodio::DeviceSinkBuilder::open_default_sink() {
        Ok(sink) => sink,
        Err(error) => {
            tracing::warn!(%error, "no audio output; speech disabled");
            return;
        }
    };
    let rate = sample_rate(&voice.model);
    let mut queue = BinaryHeap::new();
    let mut seq = 0_u64;
    let mut muted = false;

    let mut take = |msg: TtsMsg, queue: &mut BinaryHeap<_>, muted: &mut bool| match msg {
        TtsMsg::Speak(priority, text) => {
            seq += 1;
            queue.push((priority, Reverse(seq), text));
        }
        TtsMsg::Mute(on) => *muted = on,
    };

    loop {
        if queue.is_empty() || muted {
            match rx.recv() {
                Ok(msg) => take(msg, &mut queue, &mut muted),
                Err(_) => return,
            }
            continue;
        }
        while let Ok(msg) = rx.try_recv() {
            take(msg, &mut queue, &mut muted);
        }
        if muted {
            continue;
        }
        let Some((_, _, text)) = queue.pop() else {
            continue;
        };
        let samples = match synthesize(voice, &text) {
            Ok(samples) => samples,
            Err(error) => {
                tracing::warn!(%error, "piper failed");
                continue;
            }
        };
        speaking(true);
        let player = rodio::Player::connect_new(sink.mixer());
        player.append(rodio::buffer::SamplesBuffer::new(NonZero::<u16>::MIN, rate, samples));
        while !player.empty() {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(TtsMsg::Mute(true)) => {
                    muted = true;
                    player.stop();
                    break;
                }
                Ok(msg) => take(msg, &mut queue, &mut muted),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        speaking(false);
    }
}

/// `echo text | piper --model voice.onnx --output_raw` → mono 16-bit PCM.
fn synthesize(voice: &Voice, text: &str) -> std::io::Result<Vec<f32>> {
    let mut child = Command::new(&voice.piper)
        .arg("--model")
        .arg(&voice.model)
        .arg("--output_raw")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let missing = |pipe| std::io::Error::other(format!("piper has no {pipe}"));
    child.stdin.take().ok_or_else(|| missing("stdin"))?.write_all(text.as_bytes())?;
    let mut raw = Vec::new();
    child.stdout.take().ok_or_else(|| missing("stdout"))?.read_to_end(&mut raw)?;
    child.wait()?;
    Ok(raw.as_chunks::<2>().0.iter().map(|pair| f32::from(i16::from_le_bytes(*pair)) / f32::from(i16::MAX)).collect())
}

/// Piper's usual rate, for voices whose `.onnx.json` does not say.
const DEFAULT_RATE: NonZero<u32> = NonZero::new(22_050).unwrap();

/// Piper voices ship a `.onnx.json` with the sample rate; 22 050 Hz otherwise.
fn sample_rate(model: &Path) -> NonZero<u32> {
    let mut config = model.as_os_str().to_owned();
    config.push(".json");
    std::fs::read_to_string(config)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json["audio"]["sample_rate"].as_u64())
        .and_then(|rate| NonZero::new(rate as u32))
        .unwrap_or(DEFAULT_RATE)
}
