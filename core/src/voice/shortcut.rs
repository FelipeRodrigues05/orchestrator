//! Global push-to-talk through the `GlobalShortcuts` portal (KDE, GNOME 48+,
//! Hyprland). Without the portal the in-window Ctrl+Space still works.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures::StreamExt;

use super::VoiceHandle;

const PUSH_TO_TALK: &str = "push-to-talk";

pub fn spawn(voice: VoiceHandle) {
    tokio::spawn(async move {
        if let Err(error) = run(voice).await {
            tracing::info!(%error, "global push-to-talk unavailable; Ctrl+Space works in the window");
        }
    });
}

async fn run(voice: VoiceHandle) -> ashpd::Result<()> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session(Default::default()).await?;
    let shortcut = NewShortcut::new(PUSH_TO_TALK, "Orchestrator: push to talk").preferred_trigger("CTRL+space");
    portal.bind_shortcuts(&session, &[shortcut], None, Default::default()).await?.response()?;
    tracing::info!("global push-to-talk bound");

    let mut activated = portal.receive_activated().await?;
    let mut deactivated = portal.receive_deactivated().await?;
    loop {
        tokio::select! {
            Some(event) = activated.next() => {
                if event.shortcut_id() == PUSH_TO_TALK {
                    voice.press();
                }
            }
            Some(event) = deactivated.next() => {
                if event.shortcut_id() == PUSH_TO_TALK {
                    voice.release();
                }
            }
            else => break,
        }
    }
    // Keep the session alive as long as the streams are.
    drop(session);
    Ok(())
}
