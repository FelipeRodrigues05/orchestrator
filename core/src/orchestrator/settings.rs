//! Settings the operator changes from the room: API keys, the pet's model, voice,
//! language, wake word, sandbox, permission timeout, the email address and CLI
//! updates. Each one is applied right away and saved in SQLite.

use std::time::Duration;

use super::{Internal, Orchestrator};
use crate::{
    model::{Language, LogLevel, Provider, Secret},
    runner::update,
    secrets, voice,
};

impl Orchestrator {
    pub(super) fn set_api_key(&mut self, service: String, key: Secret) {
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            let (text, level) = match secrets::store(&service, &key.0).await {
                Ok(_) => (format!("{service} key saved in the Secret Service"), LogLevel::Info),
                Err(error) => (format!("could not save the {service} key: {error}"), LogLevel::Error),
            };
            let _ = inbox.send(Internal::Notice(text, level));
        });
        // The refresh races the write; a second one catches up.
        self.refresh_providers();
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = inbox.send(Internal::Notice(String::new(), LogLevel::Info));
        });
    }

    pub(super) fn set_host_provider(&mut self, label: String) {
        let usable = self
            .find_provider(Some(&label))
            .is_some_and(|p| matches!(p, Provider::Api { .. } | Provider::Local { .. }));
        if !usable {
            self.notice(LogLevel::Warn, format!("the pet needs an API or local model, not {label}"));
            return;
        }
        if let Some(store) = &self.store {
            store.set_setting("host_provider", &label);
        }
        self.host_choice = Some(label);
        self.apply_host_provider();
        let current = self.state.host.provider.clone().unwrap_or_default();
        self.notice(LogLevel::Info, format!("the pet now thinks with {current}"));
    }

    pub(super) fn set_email_url(&mut self, url: &str) {
        if !["http://", "https://", "mailto:"].iter().any(|scheme| url.starts_with(scheme)) {
            self.notice(LogLevel::Warn, "use an http(s) or mailto address");
            return;
        }
        if let Some(store) = &self.store {
            store.set_setting("email_url", url);
        }
        self.notice(LogLevel::Info, format!("email: {url}"));
    }

    /// The UI tells the pet's name at start; the wake word listens for it unless off.
    pub(super) async fn pet_name(&mut self, name: String) {
        let off = match &self.store {
            Some(store) => store.setting("wake").await.as_deref() == Some("off"),
            None => false,
        };
        if let (false, Some(voice)) = (off, &self.voice) {
            voice.set_wake(Some(name));
        }
    }

    pub(super) fn set_language(&mut self, text: &str) {
        let Some(language) = Language::parse(text) else {
            self.notice(LogLevel::Warn, "languages: pt, en");
            return;
        };
        if let Some(voice) = &self.voice {
            voice.set_language(language);
        }
        if let Some(store) = &self.store {
            store.set_setting("language", language.code());
        }
        self.state.voice.language = language;
        self.notice(LogLevel::Info, format!("language: {}", language.name()));
    }

    /// Switches to an installed Piper voice, or downloads it first.
    pub(super) fn set_voice(&mut self, id: &str) {
        let id = id.trim().trim_end_matches(".onnx").to_owned();
        if voice::installed_voices().contains(&id) {
            if let Some(voice) = &self.voice {
                voice.set_voice(Some(id.clone()));
            }
            if let Some(store) = &self.store {
                store.set_setting("voice", &id);
            }
            self.notice(LogLevel::Info, format!("voice: {id}"));
            return;
        }
        let Some(downloads) = voice::install::voice(&id) else {
            self.notice(LogLevel::Warn, format!("{id} is not a Piper voice id (like en_US-lessac-medium)"));
            return;
        };
        self.notice(LogLevel::Info, format!("downloading the voice {id}…"));
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            for download in &downloads {
                if let Err(error) = voice::install::fetch(download, |_| {}).await {
                    let _ = inbox.send(Internal::VoiceDownloaded { voice: id, result: Err(error) });
                    return;
                }
            }
            let _ = inbox.send(Internal::VoiceDownloaded { voice: id, result: Ok(()) });
        });
    }

    pub(super) fn set_wake(&mut self, name: Option<String>) {
        let Some(voice) = &self.voice else { return };
        if name.is_some() && !self.state.voice.stt {
            self.notice(LogLevel::Warn, "the wake word needs speech-to-text: :voice install");
            return;
        }
        let text = match &name {
            Some(name) => format!("say \"ei {name}\" to talk without a key"),
            None => "wake word off".into(),
        };
        if let Some(store) = &self.store {
            store.set_setting("wake", if name.is_some() { "on" } else { "off" });
        }
        voice.set_wake(name);
        self.notice(LogLevel::Info, text);
    }

    /// Downloads the whisper models and the default Piper voice, with progress.
    pub(super) fn install_voice(&mut self) {
        self.notice(LogLevel::Info, "downloading the voice models (~550 MB)…");
        let inbox = self.inbox.clone();
        tokio::spawn(async move {
            for download in voice::install::downloads() {
                let name = download.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let progress = inbox.clone();
                let label = name.clone();
                let result = voice::install::fetch(&download, move |percent| {
                    let _ = progress.send(Internal::Notice(format!("{label}: {percent}%"), LogLevel::Info));
                })
                .await;
                if let Err(error) = result {
                    let _ = inbox.send(Internal::VoiceInstalled(Err(format!("{name}: {error}"))));
                    return;
                }
            }
            let _ = inbox.send(Internal::VoiceInstalled(Ok(())));
        });
    }

    pub(super) fn set_sandbox(&mut self, on: bool) {
        if on && !crate::sandbox::available() {
            self.notice(LogLevel::Warn, "bubblewrap (bwrap) is not installed");
            return;
        }
        self.state.sandbox = on;
        if let Some(store) = &self.store {
            store.set_setting("sandbox", if on { "on" } else { "off" });
        }
        let text = if on { "sandbox on: agents can only write their worktree" } else { "sandbox off" };
        self.notice(LogLevel::Info, format!("{text} (applies to agents started from now on)"));
    }

    pub(super) fn set_permission_timeout(&mut self, minutes: u64) {
        let minutes = minutes.clamp(1, 24 * 60);
        self.broker.set_timeout(Duration::from_secs(minutes * 60));
        if let Some(store) = &self.store {
            store.set_setting("permission_timeout_minutes", &minutes.to_string());
        }
        self.notice(LogLevel::Info, format!("permission requests now wait {minutes} min"));
    }

    /// Updates every agent CLI that has a known updater, once each.
    pub(super) fn update_clis(&mut self) {
        let mut plans: Vec<update::Plan> = Vec::new();
        for option in &self.state.providers {
            if let Some(plan) = update::plan(&option.provider)
                && plan.argv.is_some()
                && !plans.iter().any(|p| p.label == plan.label)
            {
                plans.push(plan);
            }
        }
        if plans.is_empty() {
            self.notice(LogLevel::Info, "no agent CLI with a known updater");
            return;
        }
        let labels: Vec<_> = plans.iter().map(|p| p.label.clone()).collect();
        self.notice(LogLevel::Info, format!("updating {}…", labels.join(", ")));
        self.update(None, plans);
    }
}
