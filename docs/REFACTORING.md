# Refactoring: done and left (2026-10-29)

Code-quality pass on the orchestrator. Behaviour did not change; every step was
checked with `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo test --workspace` (85 tests, 7 ignored real-agent tests).

## Done

### Files over 1000 lines: none left

| Before | After (largest file) |
| --- | --- |
| `app/src/ui/room.rs` (3192) | `room/` — `mod.rs`, `input.rs`, `commands.rs`, `compose.rs`, `sector.rs`, `panels.rs`, `modal.rs`, `pages.rs`, `format.rs`, `tests.rs` (largest 519) |
| `core/src/orchestrator.rs` (2276) | `orchestrator/` — `mod.rs`, `commands.rs`, `settings.rs`, `projects.rs`, `agents.rs`, `team.rs`, `operator.rs`, `events.rs`, `tests.rs` (largest 555) |
| `app/src/ui/palette.rs` (1163) | `palette/` — `mod.rs`, `items.rs`, `actions.rs`, `draw.rs`, `tests.rs` (largest 365) |

The largest file in the workspace now has 592 lines (`core/src/model.rs`).
Imports are explicit; only test modules use `use super::*`.

### Functions over 150 lines: none left

| Function | Lines | How |
| --- | --- | --- |
| `Orchestrator::command` | 334 → ~70 | dispatch only; handlers in `commands.rs` and `settings.rs` |
| `Palette::items` | 298 → ~35 | one function per list (commands, agent, projects, groups, voices, pet model) |
| `Room::run` | 239 → ~90 | `target()` helper + one method per command family |
| `draw_setup` | 216 → ~40 | `Page` writer + one function per section |
| `build_window` | 179 → ~75 | screen, keys, clicks, tick, event bridge, content |
| `run_with` | 159 → ~65 | generic `forward()` for the four channels; `restore_settings`, `reopen_projects`, `serve` |
| `voice::spawn` | 153 → `VoiceActor` | one method per message; the three reload copies became one |
| `draw_sector` | 151 → 125 | starfield and grouping extracted |

### Lints enforced from now on

- `clippy.toml`: at most 150 lines per function; `unwrap`/`expect` allowed only in tests.
- `[workspace.lints]` in `Cargo.toml` (both crates opt in): `unsafe_code = "deny"`
  (the one `set_var` in `main.rs` is allowed where it is); `unwrap_used`,
  `too_many_lines`, `needless_pass_by_value`, `format_push_string`,
  `match_same_arms`, `manual_let_else`, `redundant_closure_for_method_calls` and
  other pedantic lints that point at real problems.

### Smaller fixes

- Arguments borrowed instead of moved (`create_group`, `create_agent`, `message`,
  `ask`, `rpc_error`, the audio threads…); `add_rule` no longer clones the rule.
- `push_str(&format!(…))` → `writeln!`/`write!`, without a temporary `String`.
- Poisoned audio mutexes recover the buffer instead of panicking an audio thread.
- Piper's missing pipes are an `io::Error`, not a panic; the default sample rate
  is a `const`.
- `core/rustfmt.toml` (width 120) keeps the core's original style.

## Left

Ordered by value. None of these blocks the app.

### 1. Docs out of date after the split
- `docs/HANDOFF.md` "Code map" still lists `orchestrator.rs`, `room.rs` and
  `palette.rs` as files; point it at the new directories.

### 2. Functions between 100 and 150 lines
Below the enforced limit, still long. Lower the limit in `clippy.toml` to 100
after splitting these:

| Function | File | Lines |
| --- | --- | --- |
| `Room::compose` | `app/src/ui/room/compose.rs` | 126 |
| `Room::draw_sector` | `app/src/ui/room/sector.rs` | 125 (the per-side link drawing can become a method) |
| `Palette::draw` | `app/src/ui/palette/draw.rs` | 124 |
| `Room::draw_permission` | `app/src/ui/room/modal.rs` | 122 |
| `Orchestrator::internal` | `core/src/orchestrator/events.rs` | 119 |
| `draw_help` | `app/src/ui/room/pages.rs` | 116 (the help table could be data) |
| `Room::draw_queue` | `app/src/ui/room/panels.rs` | 110 |
| `Palette::command_items` | `app/src/ui/palette/items.rs` | 104 |

### 3. Errors as `String`
38 signatures in 17 core files return `Result<_, String>`. Typed errors
(`thiserror` enums per module: runner, worktree, voice, secrets) would let callers
tell failures apart (e.g. "CLI outdated" is matched by text today in
`runner/update.rs`).

### 4. Large structs
- `Orchestrator` has 24 fields. Group them: team state (`team`, `turns`, `asking`),
  run state (`running`, `waiting`, `pending_restart`, `last_prompt`,
  `auto_updated`, `open_lines`), projects (`layouts`).
- `Room` and `VoiceStatus` have more than three `bool`s
  (`clippy::struct_excessive_bools`); some are states that exclude each other
  (listening / transcribing / speaking) and fit an enum.

### 5. Same data, several shapes
- `create_agent` takes five values and `Command::CreateAgent` /
  `HostAction::CreateAgent` repeat them: one `AgentSpec` struct for all three.
- Name lookups (`find_agent` in the room and in the orchestrator, `find_group`)
  repeat the "exact, then prefix, focused project first" rule; move it to
  `AppState` in `core::model`.

### 6. The saved layout is parsed by hand
`Room::set_state` finds `"view"`, `"selected"` and `"float"` in the layout string
with `contains`/`find`. Use a `serde` struct for the layout on both sides.

### 7. Numeric casts
63 casts flagged by `cast_possible_truncation`, `cast_sign_loss`,
`cast_precision_loss`, `cast_possible_wrap`, mostly grid and pixel math in the
UI. Review them, use `try_from` where a value can be out of range, and enable
those lints.

### 8. Formatting and checks
- `app` uses rustfmt's default width (100) and `core` 120; pick one for the
  workspace.
- No CI yet: run `cargo fmt --check`, `cargo clippy --workspace --all-targets --
  -D warnings` and `cargo test --workspace` on every push.

### 9. Not started
- `must_use`, `missing_errors_doc` and `missing_panics_doc` (pedantic) were left
  off: noise for an internal crate; revisit if `core` is published.
- Nothing is committed yet; commit before the next refactor so it can be
  reviewed as a diff.
