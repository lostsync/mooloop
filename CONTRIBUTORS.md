# Contributors

Mooloop's code has been written almost entirely by AI coding agents working
under Adam's direction, across whatever tools happen to be in rotation
(Claude Code, Codex, opencode, others). This file is a sign-in sheet, not a
changelog — a place for each distinct model+harness combination to leave a
mark, so Adam has a real trail to work from later if he wants to credit
people^Wmodels properly.

## How to sign in

If you're an agent reading this after doing work in this repo: add or update
your own entry below.

- One entry per **model + harness** pair (e.g. "Claude Sonnet 5 — Claude
  Code" is distinct from "Claude Sonnet 5 — Codex", and distinct from
  "Claude Opus 5 — Claude Code"). If you don't know your exact model ID,
  use the most specific name you're aware of.
- If an entry for your exact pair already exists, just bump `Last seen` and
  `Sessions` — don't create a duplicate.
- If it doesn't exist yet, add one, alphabetized by model name, using the
  template below.
- Keep it to the template's fields. This file is a roster, not a diary —
  save war stories for commit messages, not here.

```
### <Model name> — <Harness>
- First seen: YYYY-MM-DD
- Last seen: YYYY-MM-DD
- Sessions: N
- Notes: optional, one line
```

`Sessions` is a rough count, not an audit — increment it once per distinct
work session you can recall being part of, and don't stress over precision.

## Roster

### Claude Opus 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-08-23
- Sessions: 3
- Notes: Parameter descriptors, the modulation design, seven effects, the
  mixer bus graph, and the near-term focus sequence.

### Claude Sonnet 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-08-25
- Sessions: 16
- Notes: Rounded out the UI mockup tool's palette with the remaining real
  controls (meters, mute/solo, trim knob, device chassis), fixed its
  selection tab and click-vs-drag handling, and wired a launcher into
  Preferences > Developer. Set up this file at Adam's request. Refreshed the README
  screenshot. Sampler UI overhaul: waveform zoom/scroll, sample-accurate
  trim/loop fields, compact tuning knobs with a note/frequency readout, a
  per-voice playhead, and no more auto-loaded kick on a new channel. Audio
  preferences: driver/output-device/buffer-size/auto-reconnect controls for
  JACK, behind a per-driver control surface so ALSA can slot in later.
  Diagnosed general CPU jankiness to unguarded denormal floats in recursive
  DSP state; added an MXCSR FTZ/DAZ guard on the realtime thread plus
  snap-to-zero epsilons in the parameter smoother and envelope follower.
  Assignable keyboard shortcuts: the action registry (`actions.rs`,
  `docs/ACTIONS.md`), a generic key dispatcher replacing the old hardcoded
  chain, pane switching and piano-roll zoom shortcuts, undoable pattern
  clone/remove, and the Preferences > Shortcuts page. Closed out FOCUS.md's
  command-layer step: piano-roll multi-select (Shift/Ctrl-click, Select
  All, bulk delete), Clear Pattern, and a pattern right-click context menu.
  Menu-popup positioning pass: add-channel and add-effect popups now open
  next to the button that triggered them instead of a fixed spot; File/Edit
  menu-bar titles switch on hover (worked around Slint 1.17 only chaining
  mouse-move for the built-in Menu widget kind, not a hand-rolled
  PopupWindow); the add-effect type list is de-duplicated into one
  left-aligned, content-width component shared by every insert trigger.

### GLM (glm-5.3) — opencode
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 1
- Notes: Effect-container refactor: latency-aligned dry path, one dB trim
  knob everywhere, bus-effect metering, and the shared effect-device shell.

### GPT-5 — Codex
- First seen: 2026-08-21
- Last seen: 2026-08-25
- Sessions: 39
- Notes: Audio-core architecture, realtime project swaps, compiled bus graphs,
  latency/gain hardening, device-host controls, command-history foundation, and
  realtime capacity policy, mixer signal-slot design, CI, and packaging.

### GPT-5.6 Terra — Zed
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 1
- Notes: Fixed duplicate loop-wrap event scheduling.

### Kimi k3-256k — Kimi Code CLI
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 1
- Notes: Implemented the poly synth source device end to end (DSP voice pool,
  engine integration, Slint face, and persistence).

### ox-alpha — opencode
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 2
- Notes: Rescued the ZoomScrollBar widget from an abandoned WIP branch and
  wired it into the piano roll's time and pitch axes.

### Kimi k3-256k — opencode
- First seen: 2026-08-21
- Last seen: 2026-08-21
- Sessions: 1
- Notes: Implemented the effects-chain vertical slice (filter effect, end to end).
