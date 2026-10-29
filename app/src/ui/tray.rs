//! Tray icon (StatusNotifierItem): shows or hides the room, toggles the wake word
//! and quits. With the tray up, closing the room hides it instead of quitting.
//! The SNI service lives on ksni's own thread; clicks come back to GTK as `Action`s.
//! It registers under the connection's unique name: the quickshell host keeps items
//! registered under `org.kde.StatusNotifierItem-<pid>-1` after the app exits.

use std::{cell::RefCell, rc::Rc};

use adw::prelude::*;
use gtk::glib;
use ksni::{
    MenuItem, Status, ToolTip,
    blocking::{Handle, TrayMethods},
    menu::{CheckmarkItem, StandardItem},
};
use orch_core::model::{AgentState, AppState};

use super::room::Room;

// Sent as pixels: tray hosts with their own icon index (quickshell) do not find
// icons installed after they started, and a checkout has none installed at all.
const ICON: &[u8] = include_bytes!("../../data/dev.feliperodrigues.Orchestrator.svg");
const ATTENTION_ICON: &[u8] =
    include_bytes!("../../data/dev.feliperodrigues.Orchestrator-attention.svg");

enum Action {
    Toggle(Option<String>),
    Show(Option<String>),
    Wake(bool),
    Quit,
}

/// What the tray shows, refreshed from each state snapshot.
#[derive(Default, Clone, PartialEq, Eq)]
struct Summary {
    running: usize,
    waiting: usize,
    wake: bool,
    project: Option<String>,
}

impl Summary {
    fn of(state: &AppState) -> Self {
        let count = |wanted: AgentState| state.agents.iter().filter(|a| a.state == wanted).count();
        Self {
            running: count(AgentState::Running),
            waiting: state.permissions.len(),
            wake: state.voice.wake,
            project: (!state.projects.is_empty()).then(|| {
                state
                    .projects
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        }
    }
}

struct Tray {
    summary: Summary,
    icon: Vec<ksni::Icon>,
    attention: Vec<ksni::Icon>,
    actions: async_channel::Sender<Action>,
}

impl Tray {
    fn send(&self, action: Action) {
        let _ = self.actions.send_blocking(action);
    }
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "orchestrator".into()
    }

    fn title(&self) -> String {
        "Orchestrator".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }

    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.attention.clone()
    }

    fn status(&self) -> Status {
        if self.summary.waiting > 0 {
            Status::NeedsAttention
        } else {
            Status::Active
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let Summary {
            running,
            waiting,
            project,
            ..
        } = &self.summary;
        let mut lines = vec![project.clone().unwrap_or_else(|| "no project open".into())];
        lines.push(format!("{running} running"));
        if *waiting > 0 {
            lines.push(format!("{waiting} waiting for permission"));
        }
        ToolTip {
            icon_pixmap: self.icon.clone(),
            title: "Orchestrator".into(),
            description: lines.join("\n"),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Action::Toggle(ksni::current_xdg_activation_token()));
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let waiting = self.summary.waiting;
        let label = if waiting > 0 {
            format!("Open control room ({waiting} waiting)")
        } else {
            "Open control room".into()
        };
        vec![
            StandardItem {
                label,
                activate: Box::new(|tray: &mut Self| {
                    tray.send(Action::Show(ksni::current_xdg_activation_token()));
                }),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Listen for the pet's name".into(),
                checked: self.summary.wake,
                activate: Box::new(|tray: &mut Self| tray.send(Action::Wake(!tray.summary.wake))),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|tray: &mut Self| tray.send(Action::Quit)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

pub struct TrayIcon {
    handle: Handle<Tray>,
    summary: RefCell<Summary>,
}

impl TrayIcon {
    /// Feeds a new snapshot; the tray is only touched when what it shows changed.
    pub fn sync(&self, state: &AppState) {
        let summary = Summary::of(state);
        if *self.summary.borrow() == summary {
            return;
        }
        self.summary.replace(summary.clone());
        self.handle.update(move |tray| tray.summary = summary);
    }
}

/// Puts the icon in the tray; `None` when no tray host is running.
pub fn install(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    room: Rc<RefCell<Room>>,
) -> Option<Rc<TrayIcon>> {
    let (actions, received) = async_channel::unbounded();
    let handle = match (Tray {
        summary: Summary::default(),
        icon: pixmaps(ICON),
        attention: pixmaps(ATTENTION_ICON),
        actions,
    })
    .disable_dbus_name(true)
    .spawn()
    {
        Ok(handle) => handle,
        Err(error) => {
            tracing::info!(%error, "no tray");
            return None;
        }
    };
    glib::spawn_future_local({
        let app = app.clone();
        let window = window.clone();
        async move {
            while let Ok(action) = received.recv().await {
                match action {
                    Action::Toggle(_) if window.is_visible() && window.is_active() => {
                        window.set_visible(false);
                    }
                    Action::Toggle(token) | Action::Show(token) => {
                        if let Some(token) = token {
                            window.set_startup_id(&token);
                        }
                        window.present();
                    }
                    Action::Wake(on) => room.borrow_mut().set_wake(on),
                    Action::Quit => app.quit(),
                }
            }
        }
    });
    Some(Rc::new(TrayIcon {
        handle,
        summary: RefCell::new(Summary::default()),
    }))
}

/// The SVG rasterised at the sizes bars ask for, as ARGB32 in network byte order.
fn pixmaps(svg: &'static [u8]) -> Vec<ksni::Icon> {
    [22, 32, 48, 64]
        .into_iter()
        .filter_map(|size| {
            let stream = gtk::gio::MemoryInputStream::from_bytes(&glib::Bytes::from_static(svg));
            let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream_at_scale(
                &stream,
                size,
                size,
                true,
                gtk::gio::Cancellable::NONE,
            )
            .inspect_err(|error| tracing::warn!(%error, "tray icon"))
            .ok()?;
            let pixbuf = if pixbuf.has_alpha() {
                pixbuf
            } else {
                pixbuf.add_alpha(false, 0, 0, 0).ok()?
            };
            let (width, height, stride) =
                (pixbuf.width(), pixbuf.height(), pixbuf.rowstride() as usize);
            let bytes = pixbuf.read_pixel_bytes();
            let mut data = Vec::with_capacity((width * height * 4) as usize);
            for row in bytes.chunks(stride).take(height as usize) {
                for rgba in row[..width as usize * 4].as_chunks::<4>().0 {
                    data.extend_from_slice(&[rgba[3], rgba[0], rgba[1], rgba[2]]);
                }
            }
            Some(ksni::Icon {
                width,
                height,
                data,
            })
        })
        .collect()
}
