# 0012. The pet is a host agent, floating only where layer-shell exists

- **Status:** Accepted; the drawing is amended by 0015
- **Date:** 2026-10-28

## Context

The user needs one conversational entry point that listens, speaks and passes
orders to groups and agents. Floating a widget over the desktop on Wayland
requires the layer-shell protocol, which GNOME does not support.

## Decision

- The pet is a special agent (the host). It takes the place of the voice actor:
  STT in, TTS out.
- It runs like any agent (configurable provider, preferably a fast model), with app
  tools: list groups/agents, message an agent, create an agent, answer a permission
  request, summarize state.
- Its permission answers go through the broker like everything else
  (see [0005](0005-central-permission-broker.md)).
- Visual states: idle, listening, thinking, speaking, alert. It is drawn as an
  animated sprite sheet in a `gtk::DrawingArea`.
- Placement: in the center of the app window by default. Post-MVP, it floats on a
  transparent `gtk4-layer-shell` surface when `gtk4_layer_shell::is_supported()`
  (KDE, Hyprland, Sway), with input only on its body.

## Consequences

- Works on every desktop; the floating mode is an enhancement.
- Two placement code paths to maintain.
- Gives the user a single conversational surface without bypassing permissions.
