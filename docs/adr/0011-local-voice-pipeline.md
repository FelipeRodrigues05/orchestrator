# 0011. Local voice: push-to-talk, whisper-rs, Piper

- **Status:** Accepted
- **Date:** 2026-10-28

## Context

The user talks to the app and hears who is asking for what. Speech is in Brazilian
Portuguese. Recognition quality and a natural voice matter more than features like
wake words. Cloud speech services would send audio off the machine.

## Decision

- Input: push-to-talk first. Capture with `cpal` (PipeWire) at 16 kHz mono and
  transcribe with `whisper-rs` in `spawn_blocking`, language fixed to `pt`. Use the
  `small` model on CPU and `medium` when a GPU feature is available.
- Commands: a pure parser in `core`, tested with phrase tables. Input is normalized
  first (lowercase, no accents, no trailing punctuation).
- Output: Piper as a subprocess (`pt_BR-faber-medium`), played with `rodio`, from a
  speech actor with a priority queue: permission > error > agent done > rest.
- The TTS stays silent while the user is speaking.
- Wake word and LLM-based routing come later.

## Consequences

- Everything runs locally, but whisper on CPU can be slow; short push-to-talk
  phrases keep it manageable.
- Users need to install the Piper binary and download the voice and whisper models.
- The spoken phrases and voice commands stay in Portuguese, since that is the
  user's language. The code stays in English (see [0013](0013-english-codebase.md)).
