//! ASCII control room: a single character-grid surface drawn by `render`, laid out by
//! `room`. GTK only paints, forwards input and bridges the runtime channels.

mod float;
mod grid;
mod palette;
mod pet;
mod popup;
mod render;
mod room;
mod tray;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use adw::prelude::*;
use gtk::{gdk, glib};
use orch_core::orchestrator::{Command, Event};

use popup::Notifier;
use render::{Painter, Viewport};
use room::{Key, Room};

/// Sends whatever the room queued to the runtime.
type Flush = Rc<dyn Fn()>;
type Shared<T> = Rc<RefCell<T>>;

pub fn build_window(
    app: &adw::Application,
    commands: async_channel::Sender<Command>,
    events: async_channel::Receiver<Event>,
) {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    install_styles();

    let room = Rc::new(RefCell::new(Room::new()));
    let viewport = Rc::new(Cell::new(Viewport::default()));
    let screen = new_screen(&room, &viewport);
    // Unbounded channel: `try_send` never blocks GTK.
    let flush: Flush = Rc::new({
        let room = room.clone();
        move || {
            let (queued, browse) = {
                let mut room = room.borrow_mut();
                (room.drain(), room.take_browse())
            };
            for command in queued {
                if commands.try_send(command).is_err() {
                    tracing::warn!("runtime is not accepting commands");
                }
            }
            if browse {
                choose_project_folder(commands.clone());
            }
        }
    });

    let uses_client_side_decorations = !is_hyprland();
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Orchestrator")
        .decorated(uses_client_side_decorations)
        .default_width(1280)
        .default_height(820)
        .build();

    let popup = Rc::new(Notifier::new(app, {
        let room = room.clone();
        let flush = flush.clone();
        let screen = screen.clone();
        move |request, answer| {
            room.borrow_mut().answer(request, answer);
            flush();
            screen.queue_draw();
        }
    }));

    window.add_controller(key_controller(&room, &screen, &flush));
    screen.add_controller(click_controller(&room, &viewport, &flush));
    for controller in wheel_controllers(&room, &viewport) {
        screen.add_controller(controller);
    }
    start_ticking(&room, &screen, &flush);

    // The wake word listens for this pet's name.
    room.borrow_mut().send_pet_name();
    // Editors installed as desktop apps (JetBrains, Zed…), on top of the PATH scan in core.
    let editors = desktop_editors();
    if !editors.is_empty() {
        room.borrow_mut().send(Command::AddEditors(editors));
    }
    flush();

    let tray = tray::install(app, &window, room.clone());
    bridge_events(events, &room, &screen, &window, popup, tray.clone(), flush);

    set_content(&window, &screen, uses_client_side_decorations);
    window.present();
    screen.grab_focus();
    room.borrow_mut().set_can_float(float::supported());
    float::install(app, room.clone(), &window);
    // With a tray the room only hides. Without one, closing it quits: the floating
    // pet is a window of the app too and would keep an invisible process alive.
    window.connect_close_request({
        let app = app.clone();
        move |window| {
            if tray.is_some() {
                window.set_visible(false);
                return glib::Propagation::Stop;
            }
            app.quit();
            glib::Propagation::Proceed
        }
    });
}

/// The system's folder chooser; the folder picked opens as a project.
fn choose_project_folder(commands: async_channel::Sender<Command>) {
    let dialog = gtk::FileDialog::builder()
        .title("Open project")
        .modal(true)
        .build();
    dialog.select_folder(
        None::<&gtk::Window>,
        gtk::gio::Cancellable::NONE,
        move |result| {
            let Some(root) = result.ok().and_then(|folder| folder.path()) else {
                return;
            };
            if commands.try_send(Command::OpenProject { root }).is_err() {
                tracing::warn!("runtime is not accepting commands");
            }
        },
    );
}

/// The drawing surface: the room composes a grid at every frame, `render` paints it.
fn new_screen(room: &Shared<Room>, viewport: &Rc<Cell<Viewport>>) -> gtk::DrawingArea {
    let screen = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .focusable(true)
        .css_classes(["control-room"])
        .build();
    let painter = Painter::new(&screen);
    screen.set_draw_func({
        let room = room.clone();
        let viewport = viewport.clone();
        move |area, cr, width, height| {
            let (cols, rows, view) = painter.fit(width, height);
            viewport.set(view);
            let grid = room.borrow_mut().compose(cols, rows);
            painter.paint(area, cr, &grid, view, (width, height));
        }
    });
    screen
}

/// Keys go to the room; holding Ctrl+Space talks.
fn key_controller(
    room: &Shared<Room>,
    screen: &gtk::DrawingArea,
    flush: &Flush,
) -> gtk::EventControllerKey {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let room = room.clone();
        let screen = screen.clone();
        let flush = flush.clone();
        move |_, key, _, modifiers| {
            if is_paste(key, modifiers) {
                paste(&room, &screen, &flush);
                return glib::Propagation::Stop;
            }
            {
                let mut room = room.borrow_mut();
                if key == gdk::Key::space && modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
                    room.voice_press();
                } else if let Some(key) = map_key(key, modifiers, room.is_typing()) {
                    room.key(key);
                } else {
                    return glib::Propagation::Proceed;
                }
            }
            flush();
            screen.queue_draw();
            glib::Propagation::Stop
        }
    });
    keys.connect_key_released({
        let room = room.clone();
        let screen = screen.clone();
        let flush = flush.clone();
        move |_, key, _, _| {
            if matches!(
                key,
                gdk::Key::space | gdk::Key::Control_L | gdk::Key::Control_R
            ) {
                room.borrow_mut().voice_release();
                flush();
                screen.queue_draw();
            }
        }
    });
    keys
}

/// Ctrl+V, Ctrl+Shift+V or Shift+Insert.
fn is_paste(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    matches!(key, gdk::Key::v | gdk::Key::V) && ctrl || key == gdk::Key::Insert && shift
}

/// The clipboard's text goes where the operator is typing.
fn paste(room: &Shared<Room>, screen: &gtk::DrawingArea, flush: &Flush) {
    let clipboard = screen.clipboard();
    let (room, screen, flush) = (room.clone(), screen.clone(), flush.clone());
    glib::spawn_future_local(async move {
        match clipboard.read_text_future().await {
            Ok(Some(text)) => {
                room.borrow_mut().paste(&text);
                flush();
                screen.queue_draw();
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "could not read the clipboard"),
        }
    });
}

/// Clicks land on the grid cell under the pointer.
fn click_controller(
    room: &Shared<Room>,
    viewport: &Rc<Cell<Viewport>>,
    flush: &Flush,
) -> gtk::GestureClick {
    let click = gtk::GestureClick::new();
    click.connect_pressed({
        let room = room.clone();
        let viewport = viewport.clone();
        let flush = flush.clone();
        move |gesture, _, x, y| {
            if let Some(widget) = gesture.widget() {
                widget.grab_focus();
                if let Some((col, row)) = viewport.get().cell_at(x, y) {
                    room.borrow_mut().click(col, row);
                }
                flush();
                widget.queue_draw();
            }
        }
    });
    click
}

/// The mouse wheel scrolls the log or console under the pointer; a motion
/// controller remembers where the pointer is.
fn wheel_controllers(
    room: &Shared<Room>,
    viewport: &Rc<Cell<Viewport>>,
) -> [gtk::EventController; 2] {
    let pointer = Rc::new(Cell::new((0.0, 0.0)));
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let pointer = pointer.clone();
        move |_, x, y| pointer.set((x, y))
    });
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    wheel.connect_scroll({
        let room = room.clone();
        let viewport = viewport.clone();
        move |controller, _, dy| {
            let (x, y) = pointer.get();
            if let Some((col, row)) = viewport.get().cell_at(x, y) {
                // Three rows a notch; wheel up (negative dy) reads older lines.
                let rows = (-dy * 3.0).round() as isize;
                room.borrow_mut().scroll(col, row, rows);
                if let Some(widget) = controller.widget() {
                    widget.queue_draw();
                }
            }
            glib::Propagation::Stop
        }
    });
    [motion.upcast(), wheel.upcast()]
}

/// Animations, pings and blinking run on the room's tick.
fn start_ticking(room: &Shared<Room>, screen: &gtk::DrawingArea, flush: &Flush) {
    glib::timeout_add_local(room::TICK, {
        let room = room.clone();
        let screen = screen.downgrade();
        let flush = flush.clone();
        move || {
            let Some(screen) = screen.upgrade() else {
                return glib::ControlFlow::Break;
            };
            room.borrow_mut().tick();
            flush();
            screen.queue_draw();
            glib::ControlFlow::Continue
        }
    });
}

/// Runtime events reach the room, the system notifications and the tray.
fn bridge_events(
    events: async_channel::Receiver<Event>,
    room: &Shared<Room>,
    screen: &gtk::DrawingArea,
    window: &adw::ApplicationWindow,
    popup: Rc<Notifier>,
    tray: Option<Rc<tray::TrayIcon>>,
    flush: Flush,
) {
    let room = room.clone();
    let screen = screen.clone();
    let window = window.clone();
    glib::spawn_future_local(async move {
        while let Ok(event) = events.recv().await {
            match event {
                Event::State(state) => {
                    popup.sync(&state, window.is_active());
                    if let Some(tray) = &tray {
                        tray.sync(&state);
                    }
                    room.borrow_mut().set_state(*state);
                }
                Event::Pong { .. } => room.borrow_mut().on_pong(),
                Event::Notice { text, level } => room.borrow_mut().notice(&text, level),
            }
            flush();
            screen.queue_draw();
        }
        room.borrow_mut().disconnect();
        screen.queue_draw();
    });
}

/// A header bar where the compositor draws no decorations; the bare grid otherwise.
fn set_content(
    window: &adw::ApplicationWindow,
    screen: &gtk::DrawingArea,
    client_side_decorations: bool,
) {
    if !client_side_decorations {
        window.set_content(Some(screen));
        return;
    }
    let view = adw::ToolbarView::new();
    let header = adw::HeaderBar::builder()
        .title_widget(&gtk::Label::new(Some("ORCHESTRATOR // CONTROL ROOM")))
        .css_classes(["flat"])
        .build();
    view.add_top_bar(&header);
    view.set_content(Some(screen));
    window.set_content(Some(&view));
}

/// Known editors among the `.desktop` apps.
fn desktop_editors() -> Vec<orch_core::model::Editor> {
    let known: [(&str, &str, bool); 10] = [
        ("code", "code", false),
        ("com.microsoft.VSCode", "code", false),
        ("visual-studio-code", "code", false),
        ("dev.zed.Zed", "zed", false),
        ("zed", "zed", false),
        ("cursor", "cursor", false),
        ("sublime_text", "subl", false),
        ("nvim", "nvim", true),
        ("helix", "helix", true),
        ("Helix", "helix", true),
    ];
    gtk::gio::AppInfo::all()
        .into_iter()
        .filter_map(|info| {
            let id = info.id()?.trim_end_matches(".desktop").to_owned();
            let (name, terminal) = match id
                .strip_prefix("jetbrains-")
                .filter(|p| !p.starts_with("toolbox"))
            {
                Some(product) => (
                    product.split('-').next().unwrap_or(product).to_owned(),
                    false,
                ),
                None => known
                    .iter()
                    .find(|(known, _, _)| *known == id)
                    .map(|(_, name, t)| ((*name).to_owned(), *t))?,
            };
            let command = info.executable().to_string_lossy().into_owned();
            (!command.is_empty()).then_some(orch_core::model::Editor {
                name,
                command,
                terminal,
            })
        })
        .collect()
}

fn map_key(key: gdk::Key, modifiers: gdk::ModifierType, typing: bool) -> Option<Key> {
    let mapped = match key {
        gdk::Key::Return | gdk::Key::KP_Enter
            if modifiers.contains(gdk::ModifierType::SHIFT_MASK) =>
        {
            Key::NewLine
        }
        gdk::Key::Return | gdk::Key::KP_Enter => Key::Enter,
        gdk::Key::Left | gdk::Key::KP_Left => Key::Left,
        gdk::Key::Right | gdk::Key::KP_Right => Key::Right,
        gdk::Key::Home | gdk::Key::KP_Home => Key::Home,
        gdk::Key::End | gdk::Key::KP_End => Key::End,
        gdk::Key::Delete | gdk::Key::KP_Delete => Key::Delete,
        gdk::Key::Page_Up | gdk::Key::KP_Page_Up => Key::Page {
            up: true,
            log: modifiers.contains(gdk::ModifierType::SHIFT_MASK),
        },
        gdk::Key::Page_Down | gdk::Key::KP_Page_Down => Key::Page {
            up: false,
            log: modifiers.contains(gdk::ModifierType::SHIFT_MASK),
        },
        gdk::Key::Escape => Key::Escape,
        gdk::Key::BackSpace => Key::Backspace,
        gdk::Key::Up => Key::Up,
        gdk::Key::Down => Key::Down,
        gdk::Key::F1 => Key::Function(1),
        gdk::Key::F2 => Key::Function(2),
        gdk::Key::F3 => Key::Function(3),
        gdk::Key::F4 => Key::Function(4),
        gdk::Key::F5 => Key::Function(5),
        gdk::Key::F6 => Key::Function(6),
        // Alt+1…9: open projects.
        key if modifiers.contains(gdk::ModifierType::ALT_MASK)
            && key.to_unicode().is_some_and(|c| ('1'..='9').contains(&c)) =>
        {
            Key::Project(key.to_unicode().and_then(|c| c.to_digit(10)).unwrap_or(1) as u8)
        }
        // Palette: Ctrl+P (VS Code), Ctrl+Shift+P; Ctrl+J/K move in lists (LazyVim).
        gdk::Key::p | gdk::Key::P if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => {
            Key::Palette
        }
        gdk::Key::j if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => Key::Down,
        gdk::Key::k if modifiers.contains(gdk::ModifierType::CONTROL_MASK) => Key::Up,
        _ => {
            // Leave Ctrl/Alt shortcuts to GTK unless the command line is open.
            let chord = gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK;
            if modifiers.intersects(chord) && !typing {
                return None;
            }
            return key
                .to_unicode()
                .filter(|ch| !ch.is_control())
                .map(Key::Char);
        }
    };
    Some(mapped)
}

fn install_styles() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        "window, .control-room { background: #000000; }
         window.pet-float { background: transparent; }
         headerbar { background: #000000; color: #e6e6e6; font-family: monospace;
                     font-weight: 700; box-shadow: none; border-bottom: 1px solid #444444; }",
    );
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn is_hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        || std::env::var_os("XDG_CURRENT_DESKTOP").is_some_and(|desktop| {
            desktop
                .to_string_lossy()
                .split(':')
                .any(|name| name.eq_ignore_ascii_case("hyprland"))
        })
}
