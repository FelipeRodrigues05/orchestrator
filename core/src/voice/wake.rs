//! Wake word (step 9, on when whisper `tiny` is installed): the microphone stays open, an energy VAD cuts it into
//! utterances, and the voice actor checks each one for the pet's name ("ei KAI").

use std::{
    sync::{Arc, Mutex, PoisonError, mpsc as std_mpsc},
    thread,
    time::Duration,
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

use super::stt::{SAMPLE_RATE, resample};
use crate::model::normalize;

/// 30 ms frames at 16 kHz.
const FRAME: usize = SAMPLE_RATE as usize * 30 / 1000;
/// Speech must last this many frames to open an utterance.
const ONSET_FRAMES: usize = 3;
/// This much quiet closes it.
const HANGOVER_FRAMES: usize = 25;
/// Longest utterance kept (8 s).
const MAX_FRAMES: usize = 8_000 / 30;
/// Shorter bursts of sound (a click, a key, a cough) are not speech: ~0.35 s.
const MIN_SPEECH_FRAMES: usize = 12;

/// Energy voice activity detector over 16 kHz mono frames.
pub struct Vad {
    noise: f32,
    voiced: usize,
    quiet: usize,
    utterance: Vec<f32>,
    active: bool,
    /// Frames above the threshold in the current utterance.
    speech: usize,
}

impl Default for Vad {
    fn default() -> Self {
        Self { noise: 0.005, voiced: 0, quiet: 0, utterance: Vec::new(), active: false, speech: 0 }
    }
}

impl Vad {
    /// Feeds one frame; returns a whole utterance when one just ended.
    pub fn push(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len().max(1) as f32).sqrt();
        let threshold = (self.noise * 3.0).max(0.012);
        let speech = rms > threshold;
        if !self.active && !speech {
            // Track the room's noise floor while nobody talks.
            self.noise = self.noise * 0.95 + rms * 0.05;
        }
        if speech {
            self.voiced += 1;
            self.quiet = 0;
        } else {
            self.quiet += 1;
            if !self.active {
                self.voiced = 0;
            }
        }
        if !self.active && self.voiced >= ONSET_FRAMES {
            self.active = true;
        }
        if self.active {
            self.utterance.extend_from_slice(frame);
            self.speech += usize::from(speech);
            let frames = self.utterance.len() / FRAME;
            if self.quiet >= HANGOVER_FRAMES || frames >= MAX_FRAMES {
                self.active = false;
                self.voiced = 0;
                let utterance = std::mem::take(&mut self.utterance);
                let spoken = std::mem::take(&mut self.speech) >= MIN_SPEECH_FRAMES;
                return spoken.then_some(utterance);
            }
        }
        None
    }
}

/// When `text` calls the pet by `name`, what came after it ("" for just the name).
pub fn match_wake(text: &str, name: &str) -> Option<String> {
    let name = normalize(name);
    let words: Vec<String> =
        normalize(text).split_whitespace().map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_owned()).collect();
    let greeting = ["ei", "oi", "hey", "ola", "e", "o"];
    // Whisper glues a short greeting to the name: "Ei Kai" → "Ficai".
    let glued = |word: &String| {
        let (word, name) = (phonetic(word), phonetic(&name));
        word.len() > name.len() && word.len() <= name.len() + 3 && word.ends_with(&name)
    };
    let position = words.iter().take(3).position(|word| close(word, &name) || glued(word))?;
    if words[..position].iter().any(|w| !greeting.contains(&w.as_str())) {
        return None;
    }
    // Rest of the original words, so the command keeps its accents.
    let original: Vec<&str> = text.split_whitespace().collect();
    let rest = original.get(position + 1..).unwrap_or_default().join(" ");
    Some(rest.trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace()).to_owned())
}

/// Equal by sound, or one edit apart for names of four letters or more: whisper
/// writes "Kai" as "Cai", "Caí" or "Kay".
fn close(word: &str, name: &str) -> bool {
    let (word, name) = (phonetic(word), phonetic(name));
    word == name || (name.chars().count() >= 4 && distance(&word, &name) <= 1)
}

/// Spelling folded to sound: k→c, y→i, w→u, ph→f, doubled letters once.
fn phonetic(word: &str) -> String {
    let folded: String = normalize(word)
        .replace("ph", "f")
        .chars()
        .map(|c| match c {
            'k' => 'c',
            'y' => 'i',
            'w' => 'u',
            other => other,
        })
        .collect();
    let mut out = String::new();
    for c in folded.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current.push((previous[j] + cost).min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

pub enum WakeMsg {
    Stop,
}

/// Opens the microphone and sends each utterance (16 kHz mono) to `utterances`.
pub fn start(utterances: mpsc::UnboundedSender<Vec<f32>>) -> std_mpsc::Sender<WakeMsg> {
    let (tx, rx) = std_mpsc::channel();
    thread::Builder::new()
        .name("wake".into())
        .spawn(move || {
            let pending = Arc::new(Mutex::new(Vec::<f32>::new()));
            let (stream, rate) = match open(pending.clone()) {
                Ok(opened) => opened,
                Err(error) => {
                    tracing::warn!(%error, "wake word: microphone unavailable");
                    return;
                }
            };
            let mut vad = Vad::default();
            let mut carry = Vec::new();
            loop {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(WakeMsg::Stop) | Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                }
                let raw = std::mem::take(&mut *pending.lock().unwrap_or_else(PoisonError::into_inner));
                carry.extend(resample(&raw, rate, SAMPLE_RATE));
                let whole = carry.len() / FRAME * FRAME;
                for frame in carry[..whole].chunks(FRAME) {
                    if let Some(utterance) = vad.push(frame) {
                        let _ = utterances.send(utterance);
                    }
                }
                carry.drain(..whole);
            }
            drop(stream);
        })
        .expect("spawn wake thread");
    tx
}

fn open(buffer: Arc<Mutex<Vec<f32>>>) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host().default_input_device().ok_or("no microphone")?;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let channels = usize::from(supported.channels()).max(1);
    let rate = supported.sample_rate();
    let push = move |samples: Vec<f32>| {
        let mono = samples.chunks(channels).map(|f| f.iter().sum::<f32>() / f.len() as f32);
        buffer.lock().unwrap_or_else(PoisonError::into_inner).extend(mono);
    };
    let on_error = |error| tracing::warn!(%error, "wake stream error");
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            supported.config(),
            move |data: &[f32], _: &_| push(data.to_vec()),
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            supported.config(),
            move |data: &[i16], _: &_| push(data.iter().map(|s| f32::from(*s) / f32::from(i16::MAX)).collect()),
            on_error,
            None,
        ),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frames: usize, amplitude: f32) -> Vec<f32> {
        (0..frames * FRAME).map(|i| amplitude * (i as f32 * 0.3).sin()).collect()
    }

    #[test]
    fn vad_cuts_speech_between_silences() {
        let mut vad = Vad::default();
        let mut audio = tone(40, 0.001);
        audio.extend(tone(30, 0.3));
        audio.extend(tone(40, 0.001));
        let utterances: Vec<Vec<f32>> = audio.chunks(FRAME).filter_map(|frame| vad.push(frame)).collect();
        assert_eq!(utterances.len(), 1);
        let seconds = utterances[0].len() as f32 / SAMPLE_RATE as f32;
        assert!((0.8..2.0).contains(&seconds), "{seconds}s");
    }

    #[test]
    fn short_bursts_are_not_speech() {
        let mut vad = Vad::default();
        let mut audio = tone(40, 0.001);
        // A click: loud, but only a few frames.
        audio.extend(tone(5, 0.4));
        audio.extend(tone(60, 0.001));
        assert!(audio.chunks(FRAME).all(|frame| vad.push(frame).is_none()));
    }

    #[test]
    fn silence_is_never_an_utterance() {
        let mut vad = Vad::default();
        assert!(tone(200, 0.002).chunks(FRAME).all(|frame| vad.push(frame).is_none()));
    }

    #[test]
    fn wake_phrases() {
        assert_eq!(match_wake("Ei, Kai.", "KAI"), Some(String::new()));
        assert_eq!(match_wake("Kai, permitir.", "KAI"), Some("permitir".into()));
        assert_eq!(match_wake("Oi Orbi, resumo", "ORBI"), Some("resumo".into()));
        assert_eq!(match_wake("Orby, agente Tests: rode a suíte", "ORBI"), Some("agente Tests: rode a suíte".into()));
        assert_eq!(match_wake("eu acho que o kai está certo", "KAI"), None);
        assert_eq!(match_wake("café", "KAI"), None);
        assert_eq!(match_wake("Ei, Caí! Resumo.", "KAI"), Some("Resumo".into()));
        assert_eq!(match_wake("Hey Kay", "KAI"), Some(String::new()));
        assert_eq!(match_wake("cair na real", "KAI"), None);
        assert_eq!(match_wake("Ficai resumo.", "KAI"), Some("resumo".into()));
    }
}

#[cfg(test)]
mod speech {
    use std::{io::Write, process::Command};

    use super::*;
    use crate::voice::{data_dir, stt};

    /// Piper says the phrase, the VAD cuts it, whisper hears it, the name matches:
    /// `cargo test -p core wake_hears_a_real_voice -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs Piper, the pt_BR voice and a whisper model"]
    fn wake_hears_a_real_voice() {
        let voice = data_dir().join("voices/pt_BR-faber-medium.onnx");
        let mut piper = Command::new("piper")
            .arg("--model")
            .arg(&voice)
            .arg("--output_raw")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        piper.stdin.take().unwrap().write_all("Ei Kai, resumo.".as_bytes()).unwrap();
        let raw = piper.wait_with_output().unwrap().stdout;
        let samples: Vec<f32> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| f32::from(i16::from_le_bytes(*pair)) / f32::from(i16::MAX))
            .collect();
        let mut audio = vec![0.0; SAMPLE_RATE as usize];
        audio.extend(resample(&samples, 22_050, SAMPLE_RATE));
        audio.extend(vec![0.0; SAMPLE_RATE as usize]);

        let mut vad = Vad::default();
        let utterance = audio.chunks(FRAME).find_map(|frame| vad.push(frame)).expect("the VAD found speech");

        let tiny = data_dir().join("models/ggml-tiny.bin");
        let model = if tiny.is_file() { tiny } else { data_dir().join("models/ggml-small.bin") };
        let context = stt::load_model(&model).unwrap();
        let text =
            stt::transcribe(&context, &utterance, "pt", Some("O assistente se chama Kai."), stt::WAKE_THREADS).unwrap();
        eprintln!("heard: {text:?} with {}", model.display());
        // Piper's synthetic voice garbles some words; the name is what must come through.
        assert!(match_wake(&text, "KAI").is_some(), "the pet's name was not heard in {text:?}");
    }
}
