//! The voice actor's state and one method per message: speech out, push-to-talk,
//! the wake word, and transcripts back to the orchestrator.

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, oneshot};
use whisper_rs::WhisperContext;

use super::{
    Msg, Parts, Priority, VoiceEvent, VoiceHandle, find_wake_model, phrases::Phrase, stt, transcribe_later, tts, wake,
};
use crate::model::{Language, VoiceStatus};

/// After the pet hears just its name, the command may follow for this long.
const ARMED_FOR: Duration = Duration::from_secs(8);

type Whisper = Arc<OnceLock<Result<WhisperContext, String>>>;

pub fn spawn(events: mpsc::UnboundedSender<VoiceEvent>) -> VoiceHandle {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let handle = VoiceHandle { tx: tx.clone() };
    let mut actor = VoiceActor::new(tx, events);
    actor.publish();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            actor.handle(msg);
            actor.publish();
        }
    });
    handle
}

struct VoiceActor {
    tx: mpsc::UnboundedSender<Msg>,
    events: mpsc::UnboundedSender<VoiceEvent>,
    language: Language,
    voice_choice: Option<String>,
    speech: Option<std::sync::mpsc::Sender<tts::TtsMsg>>,
    recorder: Option<std::sync::mpsc::Sender<stt::RecMsg>>,
    model: Option<PathBuf>,
    whisper: Whisper,
    status: VoiceStatus,
    /// The pet's name and the listening thread, while the wake word is on.
    wake: Option<(String, std::sync::mpsc::Sender<wake::WakeMsg>)>,
    /// The small model the wake word checks utterances with.
    wake_model: Whisper,
    /// Until when the pet waits for a command after hearing its name.
    armed_until: Option<Instant>,
}

impl VoiceActor {
    fn new(tx: mpsc::UnboundedSender<Msg>, events: mpsc::UnboundedSender<VoiceEvent>) -> Self {
        let language = Language::default();
        let Parts { speech, recorder, model, whisper, status } = Parts::detect(&tx, language, None);
        Self {
            tx,
            events,
            language,
            voice_choice: None,
            speech,
            recorder,
            model,
            whisper,
            status,
            wake: None,
            wake_model: Arc::new(OnceLock::new()),
            armed_until: None,
        }
    }

    fn publish(&self) {
        let _ = self.events.send(VoiceEvent::Status(self.status.clone()));
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Speak(priority, phrase) => self.speak(priority, &phrase),
            Msg::SetLanguage(language) => {
                self.language = language;
                self.reload();
            }
            Msg::SetVoice(choice) => {
                self.voice_choice = choice;
                self.reload();
            }
            Msg::Reload => self.reload(),
            Msg::Press => self.press(),
            Msg::Release => self.release(),
            Msg::Speaking(on) => self.status.speaking = on,
            Msg::Wake(name) => self.set_wake(name),
            Msg::Utterance(samples) => self.utterance(samples),
            Msg::WakeCheck(result) => self.wake_check(result),
            Msg::Transcribed(result) => self.transcribed(result),
        }
    }

    fn speak(&self, priority: Priority, phrase: &Phrase) {
        tracing::debug!(?priority, "speak");
        if let Some(speech) = &self.speech {
            let _ = speech.send(tts::TtsMsg::Speak(priority, phrase.render(self.language)));
        }
    }

    /// Looks for models and voices again; dropping the old senders ends the old threads.
    fn reload(&mut self) {
        let fresh = Parts::detect(&self.tx, self.language, self.voice_choice.as_deref());
        (self.speech, self.recorder, self.model, self.whisper) =
            (fresh.speech, fresh.recorder, fresh.model, fresh.whisper);
        self.status = VoiceStatus { wake: self.status.wake, ..fresh.status };
    }

    /// Push-to-talk pressed: start recording, silence the speech.
    fn press(&mut self) {
        if self.status.listening {
            return;
        }
        if let Some(speech) = &self.speech {
            let _ = speech.send(tts::TtsMsg::Mute(true));
        }
        if let Some(recorder) = &self.recorder {
            let _ = recorder.send(stt::RecMsg::Start);
        }
        self.status.listening = true;
    }

    /// Push-to-talk released: transcribe what was recorded.
    fn release(&mut self) {
        if !self.status.listening {
            return;
        }
        self.status.listening = false;
        if let Some(speech) = &self.speech {
            let _ = speech.send(tts::TtsMsg::Mute(false));
        }
        let (Some(recorder), Some(model)) = (&self.recorder, self.model.clone()) else { return };
        self.status.transcribing = true;
        let (reply, samples) = oneshot::channel();
        let _ = recorder.send(stt::RecMsg::Stop(reply));
        let (whisper, language, tx) = (self.whisper.clone(), self.language, self.tx.clone());
        tokio::spawn(async move {
            let samples = samples.await.unwrap_or_default();
            transcribe_later(model, whisper, language, None, samples, move |result| {
                let _ = tx.send(Msg::Transcribed(result));
            });
        });
    }

    /// Listens for the pet's name, or stops listening with `None`.
    fn set_wake(&mut self, name: Option<String>) {
        if let Some((_, listener)) = self.wake.take() {
            let _ = listener.send(wake::WakeMsg::Stop);
        }
        if let (Some(name), true) = (name, find_wake_model().is_some()) {
            let tx = self.tx.clone();
            let (utterances, mut incoming) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                while let Some(samples) = incoming.recv().await {
                    let _ = tx.send(Msg::Utterance(samples));
                }
            });
            self.wake = Some((name, wake::start(utterances)));
        }
        self.status.wake = self.wake.is_some();
        self.status.armed = false;
        self.armed_until = None;
    }

    /// An utterance from the open microphone: the command when the pet is armed,
    /// otherwise a quick check for its name.
    fn utterance(&mut self, samples: Vec<f32>) {
        // Never react to our own speech or while push-to-talk is in use.
        let busy = self.status.listening || self.status.transcribing || self.status.speaking;
        if busy || self.wake.is_none() {
            return;
        }
        let tx = self.tx.clone();
        if self.armed_until.is_some_and(|until| Instant::now() < until) {
            self.armed_until = None;
            self.status.armed = false;
            let Some(model) = self.model.clone() else { return };
            self.status.transcribing = true;
            transcribe_later(model, self.whisper.clone(), self.language, None, samples, move |result| {
                let _ = tx.send(Msg::Transcribed(result));
            });
        } else if let Some(path) = find_wake_model() {
            let hint = self.wake.as_ref().map(|(name, _)| format!("O assistente se chama {name}."));
            transcribe_later(path, self.wake_model.clone(), self.language, hint, samples, move |result| {
                let _ = tx.send(Msg::WakeCheck(result));
            });
        }
    }

    fn wake_check(&mut self, result: Result<String, String>) {
        let (Some((name, _)), Ok(text)) = (&self.wake, result) else { return };
        match wake::match_wake(&text, name) {
            // Just the name: wait for the command.
            Some(rest) if rest.is_empty() => {
                self.armed_until = Some(Instant::now() + ARMED_FOR);
                self.status.armed = true;
            }
            // "KAI, permitir": the command came with the name.
            Some(rest) => self.heard(rest),
            None => {}
        }
    }

    fn transcribed(&mut self, result: Result<String, String>) {
        self.status.transcribing = false;
        match result {
            Ok(text) if !text.is_empty() => self.heard(text),
            Ok(_) => self.status.heard = Some(String::new()),
            Err(error) => {
                tracing::warn!(%error, "transcription failed");
                self.status.detail = Some(format!("STT error: {error}"));
            }
        }
    }

    fn heard(&mut self, text: String) {
        self.status.heard = Some(text.clone());
        let _ = self.events.send(VoiceEvent::Heard(text));
    }
}
