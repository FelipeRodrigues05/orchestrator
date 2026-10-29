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

## Control room (ADR 0014)

- The window is one ASCII board drawn on a character grid: masthead (clock, runtime RTT, agent counts), view tabs F1–F4 with the caution light, sector map (groups, agents, links to the pet), permission queue, event log, agent console, voice strip (push-to-talk placeholder until step 7) and the command line on the bottom border.
- `ui/grid.rs` (cells + box-drawing line routing), `ui/room.rs` (layout, input, hit regions), `ui/render.rs` (Cairo + Pango painter), `ui/pet.rs` (pet frames). Grid, room and pet are GTK-free and unit tested.
- Every indicator shows real state; anything not wired yet says which step brings it. Keys: F1–F4/1–4, `j/k`, `a/l/d`, `c` (ack), `/` command line, `?` help, Ctrl+Space push-to-talk.
- No docking: panels do not float. Keep `gtk4` 0.11 / `libadwaita` 0.9 / `pangocairo` 0.22 in step (glib 0.22).
- Layout saved per project in SQLite and restored on reopen.

## Open in editor

- "Open in" button in the project header (opens the root) and in each agent's header (opens its worktree).
- The last editor used becomes the default.
- Terminal editors (`nvim`, `helix`) open inside the system's default terminal.

## Wayland

- The app cannot stay on top or steal focus: use `gio::Notification` with buttons + voice; the full popup opens when the window regains focus.
- Linux + Wayland only for now.

## Pet host and palette (ADR 0015, ADR 0014 amendment)

- Spoken/typed text: fixed phrases first (`voice::commands`), then the host LLM (`core::host`) for speech without a name, `@pet …`, or no agent selected. Host actions return to the orchestrator as `HostAction`s; permission answers go through the broker.
- `:pet <model>` picks the host model; default is Anthropic Haiku with a key, else a local model.
- Permission popup is an in-grid modal (`a`/`l`/`g`/`d`/`Esc`); `gio::Notification` when unfocused.
- Palette: `:` `Ctrl+P` `Space`; wizard `n`. Unmatched text runs as a `:command`.
- `:voice install` downloads whisper small and the pt_BR Piper voice into `~/.local/share/orchestrator`; the voice actor reloads without restarting.
