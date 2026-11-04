//! Full-page content: help, the getting-started guide and the setup view (F5).

use orch_core::model::{AppState, Provider};

use super::format::format_tokens;
use super::{ACCENT, DIM, FAINT, OK, TEXT, TITLE, WARN, s, title};
use crate::ui::grid::{Grid, Lines, Rect, Style, Weight, fit};

pub(super) fn draw_help(grid: &mut Grid) {
    let lines: [(&str, &str); 47] = [
        (
            "Enter / i",
            "message the selected agent (or \"agente X: …\")",
        ),
        (
            ": / Ctrl+P / Space",
            "command palette (type a :command there too)",
        ),
        ("n", "new agent wizard"),
        ("o", "open a project: recent, found on disk, or Browse…"),
        ("@ in a message", "mention a file of the project (or @pet)"),
        (
            "Shift+Enter  ←→↑↓",
            "new line · move in a message (long ones open an editor)",
        ),
        (
            "F1-F6  1-6",
            "overview · agents · perms · log · setup · read",
        ),
        ("r", "read the selected agent's conversation full screen"),
        ("j k  ↓ ↑", "select agent · s start/stop · e open in editor"),
        (
            "a  l  g  d",
            "allow · always (agent) · always (group) · deny",
        ),
        ("c", "acknowledge the caution light"),
        (
            "PgUp PgDn  wheel",
            "scroll the console · Shift: the event log · End: newest",
        ),
        ("Alt+1-9  [ ]", "switch between open projects"),
        ("Ctrl+Space", "hold to talk"),
        ("", ""),
        (
            ":open <path>",
            "open a git repository (next to the open ones)",
        ),
        (
            ":project <n>  :close",
            "focus an open project · close the focused one",
        ),
        (
            ":group <n>[: <goal>]",
            "create a group (the goal is optional)",
        ),
        (
            ":new <g> <n> [p]: <task>",
            "create an agent (p = provider label, F5) · \" | persona\"",
        ),
        (
            ":color [agent] <color>",
            "its lines and name in a colour · auto · none",
        ),
        (":theme <name>", "mono · amber · green · nord · light"),
        (
            ":persona [agent]: <who>",
            "who the agent is; agents ask teammates with ask_agent",
        ),
        (":start/:stop [agent]", "run or stop an agent"),
        (":remove <agent>", "forget an agent (its worktree stays)"),
        (
            ":edit [editor]",
            "open the agent's worktree · :edit project",
        ),
        (
            ":key <service> [delete]",
            "store an API key in the Secret Service (paste with Ctrl+V) · or remove it",
        ),
        (
            ":compact <n%|off>",
            "context fill at which agents go on with a summary (85%)",
        ),
        (
            ":model [id]  :fallback",
            "the agent's model · order on usage limits (auto, reset)",
        ),
        (
            ":provider <p|svc/model>",
            "switch the agent mid-task · :update CLIs",
        ),
        (
            "@pet … / :pet <model>",
            "ask the pet · choose the model it thinks with",
        ),
        (
            ":pet float / :pet dock",
            "the pet walks on your desktop (layer-shell)",
        ),
        (
            ":wake on|off",
            "say \"ei <pet>\" instead of holding Ctrl+Space",
        ),
        (
            ":lang pt|en  :voice [id]",
            "language of speech and the pet · Piper voice",
        ),
        (
            ":email [url]  :term",
            "open your email (or set webmail) · a terminal",
        ),
        (
            ":allow <Rule>",
            "e.g. :allow Bash(cargo test:*) for the agent",
        ),
        (
            ":timeout <min>",
            "how long a request waits before it is denied",
        ),
        (
            ":limit <group> <n|off>",
            "at most n agents of the group working at once",
        ),
        (
            ":voice install",
            "download the whisper model and the pt_BR voice",
        ),
        (
            ":sandbox on|off",
            "bubblewrap: agents only write their worktree",
        ),
        (":merge [agent]", "commit its worktree and merge its branch"),
        (":lead <group> [n]", "the pet splits the goal into n agents"),
        (":view <name>  :ping", "switch view · runtime round trip"),
        ("", ""),
        (
            "permitir / sempre / negar",
            "spoken or typed: answer the open request",
        ),
        ("resumo", "the pet reads the room state out loud"),
        ("grupo X, agente Y: …", "send to a named agent"),
        ("? Esc", "close this help"),
    ];
    let w = 78.min(grid.cols - 4);
    let h = (lines.len() + 4).min(grid.rows - 2);
    let rect = Rect::new((grid.cols - w) / 2, (grid.rows - h) / 2, w, h);
    grid.fill(rect, ' ', TEXT);
    let mut frame = Lines::new(grid.cols, grid.rows);
    frame.rect(rect, Weight::Double);
    frame.draw(grid, ACCENT);
    title(grid, rect.x, rect.y, &[s("HELP // KEYS & COMMANDS", TITLE)]);
    for (row, (keys, what)) in lines.iter().enumerate().take(h.saturating_sub(4)) {
        grid.spans(
            rect.x + 3,
            rect.y + 2 + row,
            w - 6,
            &[s(fit(keys, 27), ACCENT.bold()), s(*what, TEXT)],
        );
    }
}

/// First steps, ticked off as the project fills up.
pub(super) fn draw_guide(grid: &mut Grid, area: Rect, state: &AppState) {
    let provider = state
        .providers
        .iter()
        .find(|p| p.ready)
        .map_or("opencode", |p| p.label.as_str());
    let new = format!(":new API Tests {provider}: write the /pix tests");
    let steps: [(&str, &str, bool); 4] = [
        (
            "o  or  :open ~/code/my-repo",
            "open a repository · or n: agents need no project",
            state.project().is_some(),
        ),
        (
            ":group API: payments backend",
            "a group shares one goal",
            !state.groups.is_empty(),
        ),
        (
            &new,
            "an agent: group, name, provider, task",
            !state.agents.is_empty(),
        ),
        (
            "type + Enter  ·  hold Ctrl+Space",
            "talk to the selected agent",
            false,
        ),
    ];
    let x = area.x + 1;
    grid.text(x, area.y, area.w, "GETTING STARTED", TITLE);
    for (row, (command, what, done)) in steps.iter().enumerate() {
        let y = area.y + 2 + row * 2;
        let (mark, style) = if *done { ("✓", OK) } else { ("○", DIM) };
        grid.spans(
            x,
            y,
            area.w.saturating_sub(2),
            &[
                s(format!("{mark} {} ", row + 1), style),
                s(fit(command, 44), if *done { DIM } else { ACCENT.bold() }),
                s(format!(" {what}"), FAINT),
            ],
        );
    }
}

/// Writes a page top to bottom, one line at a time, clipped to its area.
struct Page {
    x: usize,
    y: usize,
    width: usize,
    bottom: usize,
}

impl Page {
    fn line(&mut self, grid: &mut Grid, spans: &[(String, Style)]) {
        if self.y < self.bottom {
            grid.spans(self.x, self.y, self.width, spans);
        }
        self.y += 1;
    }

    fn section(&mut self, grid: &mut Grid, name: &str, hint: &str) {
        self.line(grid, &[s(name, TITLE), s(format!("   {hint}"), FAINT)]);
    }
}

/// F5: what this machine offers and how to wire the rest.
pub(super) fn draw_setup(grid: &mut Grid, area: Rect, state: &AppState) {
    let mut page = Page {
        x: area.x + 1,
        y: area.y,
        width: area.w.saturating_sub(2),
        bottom: area.bottom(),
    };
    setup_providers(&mut page, grid, state);
    page.line(grid, &[]);
    page.section(
        grid,
        "API KEYS",
        ":key anthropic  → typed masked, stored in the Secret Service, never in SQLite",
    );
    page.line(grid, &[]);
    let editors: Vec<&str> = state.editors.iter().map(|e| e.name.as_str()).collect();
    page.section(
        grid,
        "EDITORS",
        "e or :edit [name] opens the selected agent's worktree · :edit project",
    );
    let editors = if editors.is_empty() {
        "none found".into()
    } else {
        editors.join(" · ")
    };
    page.line(grid, &[s("  ", TEXT), s(editors, TEXT)]);
    page.line(grid, &[]);
    setup_sandbox_and_pet(&mut page, grid, state);
    page.line(grid, &[]);
    setup_voice(&mut page, grid, state);
}

fn setup_providers(page: &mut Page, grid: &mut Grid, state: &AppState) {
    page.section(
        grid,
        "AGENT PROVIDERS",
        "use the label in :new <group> <name> <label>: <task>",
    );
    if state.providers.is_empty() {
        page.line(
            grid,
            &[s(
                "  none found: install opencode, gemini or an ACP adapter, or run Ollama",
                DIM,
            )],
        );
    }
    for option in &state.providers {
        let (lamp, status) = if option.ready {
            (s("● ", OK), s("ready", DIM))
        } else {
            (
                s("○ ", WARN),
                s(format!("needs a key: :key {}", option.label), WARN),
            )
        };
        let kind = match &option.provider {
            Provider::CliAcp { command } => format!("ACP  {command}"),
            Provider::CliMcp { command } => format!("MCP  {command}"),
            Provider::Api { model, .. } => format!("API  {model} (label/<model> for another)"),
            Provider::Local { url, .. } => format!("LOCAL {url}"),
        };
        page.line(
            grid,
            &[
                s("  ", TEXT),
                lamp,
                s(fit(&option.label, 22), TEXT.bold()),
                s(fit(&kind, 52), DIM),
                status,
            ],
        );
    }
    let spent = spent_by_provider(state);
    let spent = if spent.is_empty() {
        "none yet".into()
    } else {
        spent.join(" · ")
    };
    page.line(grid, &[s("  tokens this session: ", DIM), s(spent, TEXT)]);
}

/// "claude 12.3k", one entry per provider (and the pet) that spent tokens.
fn spent_by_provider(state: &AppState) -> Vec<String> {
    let mut spent: Vec<(String, u64)> = Vec::new();
    for agent in &state.agents {
        let name = agent.provider.short_name().to_owned();
        match spent.iter_mut().find(|(n, _)| *n == name) {
            Some((_, total)) => *total += agent.usage.total(),
            None => spent.push((name, agent.usage.total())),
        }
    }
    if state.host.usage.total() > 0 {
        spent.push(("pet".into(), state.host.usage.total()));
    }
    spent
        .iter()
        .filter(|(_, t)| *t > 0)
        .map(|(n, t)| format!("{n} {}", format_tokens(*t)))
        .collect()
}

fn setup_sandbox_and_pet(page: &mut Page, grid: &mut Grid, state: &AppState) {
    page.section(
        grid,
        "SANDBOX",
        "bubblewrap · :sandbox on|off · ORCH_SANDBOX_BIND adds writable paths",
    );
    page.line(
        grid,
        &[if state.sandbox {
            s(
                "  ● on: the system is read-only for agents; only their worktree is writable",
                OK,
            )
        } else {
            s("  ○ off: agents can write anywhere your user can", WARN)
        }],
    );
    page.line(grid, &[]);
    page.section(
        grid,
        "PET",
        "the host: voice without a name, @pet …, or no agent selected · :pet <model>",
    );
    match &state.host.provider {
        Some(model) => page.line(grid, &[s("  ● ", OK), s(model.clone(), TEXT.bold())]),
        None => page.line(
            grid,
            &[s(
                "  ○ no model: only fixed phrases (permitir, negar, resumo, grupo X agente Y)",
                WARN,
            )],
        ),
    }
}

fn setup_voice(page: &mut Page, grid: &mut Grid, state: &AppState) {
    let voice = &state.voice;
    let lamp = |on: bool| {
        if on {
            s("● online ", OK)
        } else {
            s("○ offline ", WARN)
        }
    };
    page.section(
        grid,
        "VOICE",
        "hold Ctrl+Space (global shortcut through the portal when available)",
    );
    let language = voice.language.code();
    let current = voice.voice.clone().unwrap_or_else(|| "none".into());
    page.line(
        grid,
        &[
            s("  speech-to-text  ", DIM),
            lamp(voice.stt),
            s(format!("whisper-rs · {language} · :lang pt|en"), FAINT),
        ],
    );
    page.line(
        grid,
        &[
            s("  text-to-speech  ", DIM),
            lamp(voice.tts),
            s(format!("piper · {current} · :voice <id>"), FAINT),
        ],
    );
    page.line(
        grid,
        &[
            s("  wake word       ", DIM),
            lamp(voice.wake),
            s("\"ei <pet name>\" · :wake on|off", FAINT),
        ],
    );
    if let Some(detail) = &voice.detail {
        for part in detail.split(" · ") {
            page.line(grid, &[s("  ", TEXT), s(part, WARN)]);
        }
    }
}
