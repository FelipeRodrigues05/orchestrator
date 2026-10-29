mod ui;

use adw::prelude::*;
use gtk::glib;
use orch_core::runtime::Runtime;
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "dev.feliperodrigues.Orchestrator";

fn main() -> glib::ExitCode {
    enter_own_scope();
    init_logging();

    extend_path();

    // Tokio lives on its own thread; this one belongs to GTK.
    let runtime = match Runtime::start() {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "failed to start the runtime");
            return glib::ExitCode::FAILURE;
        }
    };

    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_startup(|_| {
        // Installed by scripts/install.sh; from a checkout the icon is read from app/data.
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display)
                .add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/data"));
        }
        gtk::Window::set_default_icon_name(APP_ID);
    });

    let commands = runtime.commands.clone();
    let events = runtime.events.clone();
    app.connect_activate(move |app| {
        // A single window: there is a single event receiver.
        // The room, not the floating pet (also an app window, often hidden).
        if let Some(window) = app
            .windows()
            .into_iter()
            .find(glib::prelude::ObjectExt::is::<adw::ApplicationWindow>)
        {
            window.present();
            return;
        }
        ui::build_window(app, commands.clone(), events.clone());
    });

    let code = app.run();
    runtime.shutdown();
    code
}

/// Started from the app menu, the process may miss the PATH entries a shell profile
/// adds, and with them the agent CLIs, Piper and editors.
#[allow(unsafe_code)]
fn extend_path() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<_> = std::env::split_paths(&current).collect();
    for dir in [
        ".local/bin",
        ".cargo/bin",
        ".opencode/bin",
        ".bun/bin",
        ".npm-global/bin",
        ".local/share/pnpm",
    ] {
        let dir = home.join(dir);
        if dir.is_dir() && !paths.contains(&dir) {
            paths.push(dir);
        }
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        // SAFETY: called first thing in main, before any other thread exists.
        unsafe { std::env::set_var("PATH", joined) };
    }
}

/// The desktop portal names an app after its systemd scope (`app-<id>-*.scope`);
/// started from a terminal, VS Code or a bare launcher the process sits in its
/// parent's scope, so the global push-to-talk shortcut would be filed under the
/// parent. Re-run once inside our own scope; on any trouble just carry on.
fn enter_own_scope() {
    use std::os::unix::process::CommandExt;

    if std::env::var_os("ORCH_SCOPED").is_some() || std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        return;
    }
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").unwrap_or_default();
    if cgroup.contains(&format!("app-{APP_ID}")) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let unit = format!("app-{APP_ID}-{}", std::process::id());
    let error = std::process::Command::new("systemd-run")
        .args([
            "--user",
            "--scope",
            "--quiet",
            "--collect",
            &format!("--unit={unit}"),
            "--",
        ])
        .arg(exe)
        .args(std::env::args_os().skip(1))
        .env("ORCH_SCOPED", "1")
        .exec();
    // Only reached when systemd-run could not be started.
    eprintln!("orchestrator: running without its own scope: {error}");
}

/// To the terminal when there is one; started from the app menu, to
/// `~/.local/state/orchestrator/orchestrator.log` so a failed start leaves a trace.
fn init_logging() {
    use std::io::IsTerminal;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::Path::new(&home).join(".local/state"))
        });
    let file = (!std::io::stderr().is_terminal())
        .then_some(state)
        .flatten()
        .and_then(|state| {
            let dir = state.join("orchestrator");
            std::fs::create_dir_all(&dir).ok()?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("orchestrator.log"))
                .ok()
        });
    match file {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .init(),
        None => tracing_subscriber::fmt().with_env_filter(filter).init(),
    }
}
