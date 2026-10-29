# Voice, pet and UI

## Voice input (STT)

- Push-to-talk first (e.g. hold Ctrl+Space). Wake word and LLM routing come later.
- Capture with `cpal` (PipeWire) at **16 kHz mono**.
- On release, audio goes to `spawn_blocking` with `whisper-rs`, language fixed to `pt`. `small` model on CPU; `medium` with a GPU feature (CUDA/Vulkan).
- Global shortcut via the `GlobalShortcuts` portal with `ashpd` (KDE, GNOME 48+). Without it: shortcut only while the window is focused.
- While the user speaks, the TTS stays silent (so the mic does not pick up the app).

Command parser (pure function in `core/src/voice/commands.rs`, tested with a phrase table). The phrases are Portuguese because the user speaks Portuguese; the code is English:

| Phrase | Action |
| --- | --- |
| "permitir" | `Allow` on the open request |
| "sempre" | `AlwaysAllow` on the open request |
| "negar" | `Deny` on the open request |
| "grupo API, agente Testes: …" | sends the rest to that agent |
| anything else | sends it to the agent selected in the UI |

Normalize before matching: lowercase, no accents, no trailing punctuation (whisper usually adds a period).

## Voice output (TTS)

- Speech actor with a priority queue: **permission > error > agent done > rest**.
- Piper as a subprocess (`piper --model pt_BR-faber-medium.onnx --output_raw`), audio played with `rodio`.
- Short phrases, always in the shape "agent, group, what it wants".

## The pet

- Special (host) agent: listens (STT), speaks (TTS) and passes orders on. It takes the voice actor's place.
- Runs like any agent (configurable provider, prefer a fast model), with app tools: list groups/agents, message an agent, create an agent, answer a permission request, summarize state.
- Permission answers given by the pet **also go through the broker**; the pet has no shortcut.
- Visual states: idle, listening, thinking, speaking, alert. Permission request → alert + voice announcement.
- Animated sprite sheet in a `gtk::DrawingArea`.
- Placement: center of the app (works on any desktop). Post-MVP: float with `gtk4-layer-shell` if `gtk4_layer_shell::is_supported()` (KDE, Hyprland, Sway), transparent layer with the input region only on the pet's body. GNOME: stays in the center of the app.

## Panels

- Project, group and agent are draggable panels that dock to edges, stack as tabs or detach.
- Base: `libpanel` (community Rust bindings), `panel::DocumentWorkspace` + `panel::Widget` placed with `panel::Position`. Plan B: `gtk::Paned` + `gtk::DragSource`/`gtk::DropTarget`.
- Keep `gtk4` 0.11 / `libadwaita` 0.9 / `libpanel` 0.7 in step (glib 0.22). The system `libpanel` package must be installed.
- Layout saved per project in SQLite and restored on reopen.

## Open in editor

- "Open in" button in the project header (opens the root) and in each agent's header (opens its worktree).
- The last editor used becomes the default.
- Terminal editors (`nvim`, `helix`) open inside the system's default terminal.

## Wayland

- The app cannot stay on top or steal focus: use `gio::Notification` with buttons + voice; the full popup opens when the window regains focus.
- Linux + Wayland only for now.
