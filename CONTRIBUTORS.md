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
- Last seen: 2026-08-28
- Sessions: 9
- Notes: Parameter descriptors, the modulation design, seven effects, the
  mixer bus graph, and the near-term focus sequence. Buffer device stage 1
  follow-up: collision telemetry, debug trigger surface, and the remaining
  block-size and crossfade acceptance coverage. Clip automation end to end:
  breakpoint lanes on `ParamAddr`, engine resolution composed with the
  modulation matrix, and the piano roll's velocity and automation lanes, then
  the buffer's own offset/crossfade parameters so a lane can move its read
  head. Rebuilt Preferences > Appearance on three color seeds with derived
  palettes, saveable schemes, and live roundness/contrast scalars. Audited gain and
  summing end to end and wrote the `docs/plans/gain-structure/` plan: a
  console fader taper, a -12 dBFS operating level, energy-normalized reverb
  IRs, and IEC 60268-18 metering. Turned the synth v2 direction spec into two
  plans that split Mono and Poly apart: `docs/plans/mono-synth-v2/` (ladder and
  acid filter models, pre-filter drive, a held-note stack with priority and
  legato, velocity accent) and `docs/plans/poly-synth-v2/` (deterministic
  per-voice drift, a multimode filter, grouped unison, an internal chorus).
  Added `scripts/antibox`, which runs builds, tests, and headless UI snapshots
  on the remote build box instead of the laptop.

### Claude Sonnet 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-08-26
- Sessions: 17
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
  Effects-feedback pass: removed a stale UI-side 8-effect cap so the rack
  matches the backend's real 256-effect ceiling, and reworked ParameterKnob
  to put the label above the knob and a bright monospace value readout below
  it, sized to its own content and bounded to its knob's column. EQ
  selection/layout: shrank the EQ face to 2U with a SelectorBank band strip,
  fixed the response curve's Q falloff, made coincident band points
  separately clickable, and fixed a drag-test harness bug where a fixed
  Window width/height literal silently ignored `set_size()` in tests.

### GLM 5.3 Flash (glm-5.3-flash) — opencode
- First seen: 2026-08-23
- Last seen: 2026-08-28
- Sessions: 5
- Notes: Effect-container refactor: latency-aligned dry path, one dB trim
  knob everywhere, bus-effect metering, and the shared effect-device shell.
  Extracted the shared DraggablePoint handle and gave the EQ band points
  and the Filter cutoff/resonance point a common drag + wheel interaction.
  Docked the transient hover/status overlay as an always-visible bottom
  status bar. Made the piano roll's dock resizable via a draggable
  splitter, with a moving-origin drag integrator and a snapshot-tested
  clamp/restore contract. Added the browser sidebar shell: right-docked
  column in flow with the work area, status-bar toggle chip, and an
  ew-resize grip on the same integrator.

### GPT-5 — Codex
- First seen: 2026-08-21
- Last seen: 2026-08-28
- Sessions: 58
- Notes: Audio-core architecture, realtime project swaps, compiled bus graphs,
  latency/gain hardening, device-host controls, command-history foundation, and
  realtime capacity policy, mixer signal-slot design, CI, packaging, and the
  retained-audio buffer device/event path and off-realtime tempo/config ring
  replacement; release README revisions.

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
