//! Voice actor (ADR 0011): push-to-talk capture + whisper transcription in, Piper
//! speech out. Everything else talks to it through `VoiceHandle`.

mod actor;
pub mod commands;
pub mod install;
pub mod phrases;
pub mod shortcut;
pub mod stt;
pub mod tts;
pub mod wake;

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use tokio::sync::mpsc;
use whisper_rs::WhisperContext;

use crate::{
    discovery::which,
    model::{Language, VoiceStatus},
};
pub use actor::spawn;
use phrases::Phrase;

/// Speech queue order: permission > error > agent done > the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Other,
    Done,
    Error,
    Permission,
}

#[derive(Debug, Clone)]
pub enum VoiceEvent {
    Status(VoiceStatus),
    /// A transcript, already trimmed; empty transcripts are dropped.
    Heard(String),
}

enum Msg {
    Speak(Priority, Phrase),
    SetLanguage(Language),
    /// `None`: the default voice of the language.
    SetVoice(Option<String>),
    Press,
    Release,
    Speaking(bool),
    Transcribed(Result<String, String>),
    /// Look for models and voices again (after `:voice install`).
    Reload,
    /// Listen for the pet's name; `None` turns the wake word off.
    Wake(Option<String>),
    /// An utterance cut by the wake word's VAD.
    Utterance(Vec<f32>),
    /// The quick transcript of an utterance, checked for the pet's name.
    WakeCheck(Result<String, String>),
}

#[derive(Clone)]
pub struct VoiceHandle {
    tx: mpsc::UnboundedSender<Msg>,
}

impl VoiceHandle {
    /// Says text that is already in the operator's language.
    pub fn speak(&self, priority: Priority, text: impl Into<String>) {
        let _ = self.tx.send(Msg::Speak(priority, Phrase::Text(text.into())));
    }

    /// Says a phrase in the operator's language.
    pub fn announce(&self, priority: Priority, phrase: Phrase) {
        let _ = self.tx.send(Msg::Speak(priority, phrase));
    }

    pub fn set_language(&self, language: Language) {
        let _ = self.tx.send(Msg::SetLanguage(language));
    }

    pub fn set_voice(&self, voice: Option<String>) {
        let _ = self.tx.send(Msg::SetVoice(voice));
    }

    /// Push-to-talk pressed: start listening, silence the speech.
    pub fn press(&self) {
        let _ = self.tx.send(Msg::Press);
    }

    pub fn release(&self) {
        let _ = self.tx.send(Msg::Release);
    }

    pub fn reload(&self) {
        let _ = self.tx.send(Msg::Reload);
    }

    pub fn set_wake(&self, name: Option<String>) {
        let _ = self.tx.send(Msg::Wake(name));
    }
}

/// Where downloaded models and voices live: `~/.local/share/orchestrator`.
pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("orchestrator")
}

/// The small model the wake word checks utterances with; the main one otherwise.
fn find_wake_model() -> Option<PathBuf> {
    let tiny = data_dir().join("models/ggml-tiny.bin");
    tiny.is_file().then_some(tiny).or_else(find_whisper_model)
}

/// Transcribes on a blocking thread with a lazily loaded model.
fn transcribe_later(
    model: PathBuf,
    context: Arc<OnceLock<Result<WhisperContext, String>>>,
    language: Language,
    hint: Option<String>,
    samples: Vec<f32>,
    done: impl FnOnce(Result<String, String>) + Send + 'static,
) {
    tokio::spawn(async move {
        let result = tokio::task::spawn_blocking(move || match context.get_or_init(|| stt::load_model(&model)) {
            Ok(context) => stt::transcribe(context, &samples, language.code(), hint.as_deref()),
            Err(error) => Err(error.clone()),
        })
        .await
        .unwrap_or_else(|error| Err(error.to_string()));
        done(result);
    });
}

fn find_whisper_model() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORCH_WHISPER_MODEL") {
        return Some(PathBuf::from(path));
    }
    let dir = data_dir().join("models");
    // `medium` is too slow on a CPU; with a GPU it is the better choice.
    let order: &[&str] = if cfg!(any(feature = "gpu-vulkan", feature = "gpu-cuda")) {
        &["medium", "large-v3-turbo", "small", "base", "tiny"]
    } else {
        &["small", "base", "medium", "large-v3-turbo", "tiny"]
    };
    order.iter().map(|size| dir.join(format!("ggml-{size}.bin"))).find(|path| path.is_file())
}

/// Piper voices in the data folder, by file stem.
pub fn installed_voices() -> Vec<String> {
    let mut voices: Vec<String> = std::fs::read_dir(data_dir().join("voices"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "onnx"))
        .filter_map(|path| path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
        .collect();
    voices.sort();
    voices
}

/// The operator's voice if installed, else the language's default, else any
/// installed voice of that language.
fn find_piper_voice(language: Language, choice: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORCH_PIPER_VOICE") {
        return Some(PathBuf::from(path));
    }
    let dir = data_dir().join("voices");
    let file = |stem: &str| Some(dir.join(format!("{stem}.onnx"))).filter(|path| path.is_file());
    choice
        .and_then(file)
        .or_else(|| file(language.default_voice()))
        .or_else(|| installed_voices().iter().find(|v| v.starts_with(language.voice_prefix())).and_then(|v| file(v)))
}

/// What this machine offers for speech right now; rebuilt on `reload`.
struct Parts {
    speech: Option<std::sync::mpsc::Sender<tts::TtsMsg>>,
    recorder: Option<std::sync::mpsc::Sender<stt::RecMsg>>,
    model: Option<PathBuf>,
    whisper: Arc<OnceLock<Result<WhisperContext, String>>>,
    status: VoiceStatus,
}

impl Parts {
    fn detect(tx: &mpsc::UnboundedSender<Msg>, language: Language, choice: Option<&str>) -> Self {
        let piper = which("piper").or_else(|| which("piper-tts"));
        let voice = find_piper_voice(language, choice);
        let model = find_whisper_model();
        let microphone = stt::has_microphone();

        let mut missing = Vec::new();
        if model.is_none() {
            missing.push("STT: no whisper model (:voice install)".to_owned());
        } else if !microphone {
            missing.push("STT: no microphone".to_owned());
        }
        if piper.is_none() {
            missing.push("TTS: install piper (AUR piper-tts-bin)".to_owned());
        } else if voice.is_none() {
            missing.push(format!("TTS: no {} voice (:voice {})", language.code(), language.default_voice()));
        }

        let voice_name = voice.as_ref().and_then(|v| v.file_stem()).map(|s| s.to_string_lossy().into_owned());
        let speech = match (piper, voice) {
            (Some(piper), Some(model)) => {
                let tx = tx.clone();
                Some(tts::start(tts::Voice { piper, model }, move |on| {
                    let _ = tx.send(Msg::Speaking(on));
                }))
            }
            _ => None,
        };
        let recorder = (model.is_some() && microphone).then(stt::start_recorder);
        let status = VoiceStatus {
            stt: recorder.is_some(),
            tts: speech.is_some(),
            detail: (!missing.is_empty()).then(|| missing.join(" · ")),
            language,
            voice: voice_name,
            voices: installed_voices(),
            ..VoiceStatus::default()
        };
        Self { speech, recorder, model, whisper: Arc::new(OnceLock::new()), status }
    }
}
