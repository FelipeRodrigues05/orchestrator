# 0021. Colour themes

- **Status:** Accepted
- **Date:** 2026-11-04

## Context

The room was one palette: black, greys and white, with red only for what needs the
operator (ADR 0014). People work at night and in daylight, and some prefer the
look of a phosphor terminal.

## Decision

- The grid keeps drawing with abstract tones (`Frame`, `Text`, `Dim`, `Warn`…); a
  theme maps each tone to a colour (`app/src/ui/theme.rs`). Layout never changes
  with the theme.
- Five themes: **mono** (the original), **amber** and **green** (phosphor
  terminals), **nord** (cold blues) and **light** (dark text on a light
  background). Agents' colours have a darker set for the light theme.
- In every theme red is still reserved for what is urgent; a test checks it.
- `:theme <name>` (Portuguese names too: `âmbar`, `verde`, `claro`) or "Theme" in
  the palette. The choice is a runtime setting (`AppState.theme`, saved in SQLite),
  so it survives restarts; the window's CSS follows it.
- Scanlines are drawn only in the dark phosphor-style themes.

## Consequences

- One more setting; the palette is data, so another theme is a new entry.
- The floating pet uses the same theme over its dark backing.
