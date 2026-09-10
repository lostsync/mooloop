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

`Notes` is a line or two, not a log. If an entry has grown into a diary,
trimming it back is a courtesy to whoever reads this next; the detail already
exists in the commit messages and `docs/JOURNAL.md`. Trim your own pair's
entry, not somebody else's.

`Sessions` is a rough count, not an audit — increment it once per distinct
work session you can recall being part of, and don't stress over precision.

## Roster

Alphabetical by model name, then by harness.

### Claude Fable 5 — Claude Code
- First seen: 2026-08-31
- Last seen: 2026-08-31
- Sessions: 1
- Notes: Wrote `docs/plans/modulator-modules/` — the modulator-grid plan —
  and started step 01: modulator params join the descriptor system.

### Claude Fable 5.1 — Claude Code
- First seen: 2026-09-02
- Last seen: 2026-09-04
- Sessions: 3
- Notes: Sanity pass over the sampler slice/commit push (commit bakes the
  stored ratio, revert re-provisions the stretch pool, slice-mode seed and
  rate fixes, slice note-offs, one-click re-bake). Then device ordering:
  one permutation per structural edit, run over routes and lanes on both
  the UI and engine sides, so modulation and automation follow a moved
  effect and die with a removed one; channel delete/paste renumber every
  channel-scoped address; effect add/move/remove became undoable; the
  integrity pass repairs stranded and dangling addresses. Then the preset
  system's steps 01-04: the effect-level preset end to end, with a factory
  bank for every effect kind.

### Claude Opus 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-09-10
- Sessions: 83
- Notes: Parameter descriptors and the modulation design; seven of the
  effects; the mixer bus graph; clip automation end to end; the Appearance
  rebuild on three colour seeds; the gain audit and its plan; the ML-M1,
  ML-P8, DS-01 and modulator-module plans, and the build of DS-01 and of
  ML-P8's own modulation, voice pool, outlets and paged face; DS-01's own
  paged face and factory kit; the
  `mooloop-session` extraction; `docs/LOOSE_ENDS.md`;
  the Slint MCP server behind `scripts/mooloop-mcp`; `scripts/antibox`,
  `scripts/slint-sketch` and `scripts/loop-profile`;
  `docs/WIDGET_INVENTORY.md`; `docs/ARCHITECTURE_REVIEW.md`; and the
  September 2026 documentation audit, the 2026-09-05 refresh that
  rewrote `FOCUS.md` around Adam's new list, the `v0.1.2` release, and the
  modulation shelf's OUTLETS pane that made a published outlet routable by
  hand, DS-01's own published outlets, ML-P8's factory bank, and the v1 drum
  synth's descriptor table, mixer latency compensation, and the typed audio
  edges with the Aux In device that consumes them, the rest/tail
  contract that lets idle devices and idle channels stop rendering, and the
  block-path measurement that found the control pass costing more than the
  audio, the sleeping-device shape evaluation behind it, and what an effect
  costs while it rests and while it works, the sleeping mixer, and the undo
  history that had no ceiling, and the double parse in front of every song
  that opened, the undo that re-rendered every commit it moved, and what the
  stretch costs, the device-container plan, the durable device identity that
  replaced `SlotRemap`, the container itself and the two clippy lints it
  shipped with, the root focus scope that was standing beside the interface
  instead of around it, and the interface-iteration plan that replaced the
  one-push mockup, and the verification note about what a piped exit code is
  actually reporting, and the browser tab that browses presets and adds a
  device rather than replacing one, and the device clipboard whose selection
  is an identity rather than a seat, and the container box that replaced the
  nesting bars once the rack turned out not to wrap after all, with the
  reorder drag that finally shows itself, and then that box again as the
  container's own chrome once the coloured tray turned out to be a backdrop
  rather than an enclosure, the emptied container that could not be refilled,
  and the generator that could not be selected, and the song loop the
  application was named after and did not have, folded into the transport so
  nothing below it had to learn that looping exists, with the grabbable
  playhead that turned out to be the same mechanism, and the audio callback
  that could not say whether it was late or merely never run, and the undo
  history whose ceiling was a count of things that are not the same size, and
  the gigabyte a one-channel project allocates before it holds
  anything, and then what that gigabyte costs every time a drag moves the
  mouse, and the UI consistency pass that found a clip light nothing cleared
  and a sampler envelope reading 2.5x what it played, and the test that
  showed that gigabyte was not what had been interrupting the audio after
  all, and the picker chip that closed its menu before it reported, which had
  left seven source and route controls drawing correctly and doing nothing
  since the day it was written, and the knob wrapper that kept the value
  string to itself, so 85 ML-P8 and DS-01 dials hovered as an empty box,
  and the sixteen-channel ceiling that only ever existed in the markup, and
  the pane-layout plan that turned four requests for one surface in one place
  into a set of views and three slots to put them in, and its first step --
  a work area that computes each slot's rectangle instead of nesting layouts,
  so a view needs one instance wherever it is drawn, and the bottom pane's two
  stacked toolbars folded into the one row per view that made possible, and
  then the split itself, whose divider closes what it is dragged out of
  existence, and the status-bar chip that had been drawing a left-docked panel
  for a sidebar docked on the right since the day it was written, and the zoom
  that is a double-click on the thing naming the pane rather than a button per
  pane, with the dock's divider generalised from a named page to the one fact
  underneath it -- that a device face declares a height and nothing else does,
  and the tab drag that moves a view between panes, whose grab and landing
  live in elements that do not contain each other -- the split of labour the
  rack's reorder had already had to learn, and then the tab's own right-click
  menu -- recorded a commit earlier as wanting machinery that did not exist,
  which was a claim made from the absence of submenus in the menu bar and
  three existing `PopupWindow` menus away from being true -- and the pane
  arrangement persisted into settings, sanitized rather than validated so a
  hand-edited file cannot produce a window with no pane in it, and the pane
  that went blank when the view it was showing was dragged out of it, which
  had been two bugs in four lines and no test covering pane moves at all, and
  the drag's own geometry tested at last, led by the check that says the
  measured tab coordinates still land on tabs, and the clippy lint that had
  been failing CI for two days while a bare `cargo clippy` reported it green,
  and the console plan, which is what four items on one morning's list turn
  into once the policy `MIXER_PLAN.md` was missing is written down -- that a
  mixer strip is created by a musical act rather than an administrative one --
  and its first step, the channel reorder that could not be composed from a
  removal and an insertion, which found six things the session had been
  mis-keying through every channel edit there has ever been, and then its
  second -- console summing, where the requirement turned out to be that the
  decoding bus is invisible, and where normalizing the curve to put its knee
  on full scale was built, measured, and rejected because it is worst exactly
  where music is loudest, and the harmonic target that turns "we want it to
  sound like iron" into a number a test can check, which found on the way that
  there is no simple level law once a profile has more than one term, and then
  `docs/TERMINOLOGY.md`, after the console plan's own first decision turned out
  to be a spreadsheet's idea of a mixer rather than a console's, and then the
  default new song as the specification for what a track is, which is the
  question "why did you want channel groups" turning out to have a concrete
  answer, and then how you actually model a preamp, which is mostly one
  structure -- tilt, shape, untilt -- because a transformer's flux goes as V/f
  and so it saturates from the bottom up -- and then the filter sandwich that
  does it, where boosting the bottom into the curve turned out to be the wrong
  half of the obvious idea, and the protocol for measuring the plugins those
  numbers should have come from -- whose first version priced hosting a plugin
  against writing a JUCE host and reached for rendered WAVs, when Adam knew it
  was a pip install and a for-loop, and then taking the analog-sum switch back
  off the channels, because a console puts its Channel stage on a mixer strip
  and mooloop's mixer strip is a track, and then the mixer becoming a list of
  tracks somebody made rather than a fixed bank of seventeen -- which turned up
  a render loop that walked a prefix of a whole-address-space permutation, so a
  short bank would have dropped tracks silently, and then sends -- where the
  step's own plan turned out to have named the wrong three files, so the
  change that was scheduled last for forcing a rewrite of the realtime
  schedule did not touch it, and then verifying that step, which found the
  audio complete against its own acceptance list and the gaps everywhere else
  -- one picker behind four controls calling every destination a send, two
  acceptance cases true but untested, and four documents still saying sends
  were absent, and then four interface regressions off one screenshot, of
  which the fourth was that a channel had never been renameable at all and
  the real fault under the other three was that a Slint `TextInput` replaces
  its binding the moment somebody types into it, so every rename field in the
  application had been going stale on its first keystroke, and then a detail
  pass over the metering and gain vocabulary, which found the shared dB
  formatter dropping the tenth on a whole value -- so a fader read `+6 dB` at
  full throw and `-12.4 dB` a nudge below it, against a `GAIN_STRUCTURE.md`
  that had specified one decimal all along -- the master peak readout
  spelling that number a second time and rounding for itself, a gain-reduction
  meter whose vertical bar grew from the middle of its track, and a meter
  scale drawing its tick marks through its own labels so `-12` read as `+2`,
  all three because a sized child of a plain `Rectangle` is centred in it
  rather than placed at its origin, and then the agreement test that guards
  that vocabulary, which parsed its own source two different ways and could
  have read a comment recording a value in place of the value, and the
  automation lane's own readout, the one value path in the program that
  printed a descriptor's raw units, so a lane on a five-millisecond envelope
  attack showed `0.01 s` and everything faster than it showed `0.00 s`, and
  then the half of the face-agreement test that was never written: the
  generators had their ranges checked against the table and the effects did
  not, though eight of their faces spell a range out as a ratio inside a
  `pow()`, where it does not look like a range and nothing draws the eye to
  it when the table moves, and the modulation shelf, which reports every
  parameter twice -- once as the knob's value-text and once as the caption
  under it -- and had drifted on one of the fifteen, so the LFO's Smoothing
  said `150 ms` to the tooltip and `150ms` to the eye, and then the piano
  roll's two test suites, which each held their own copy of `TICKS_PER_STEP`
  and so would have gone on passing while the roll drew every note in the
  wrong place.
  Longer accounts of most of this are in
  `docs/JOURNAL.md` and the commit messages, which is where they belong.

### Claude Sonnet 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-09-01
- Sessions: 18
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
  Sampler bugfix pass: fixed a browser-load race where a channel's own
  default-sample reset could overwrite the file the user had just picked,
  reworked the pitch-and-speed group's layout after a knob/field composition
  bug clipped its own controls off the face, and made a sampler's tuning
  live -- it was baked into a voice's playback rate once at trigger and
  never revisited, so retuning a held or looping note (by hand or by
  modulation) silently did nothing until the next note-on. Added an opt-out
  toggle for the old per-trigger behavior, defaulting to live.

### GLM 5.3 Flash (glm-5.3-flash) — opencode
- First seen: 2026-08-23
- Last seen: 2026-09-01
- Sessions: 17
- Notes: Effect-container refactor: latency-aligned dry path, one dB trim
  knob everywhere, bus-effect metering, and the shared effect-device shell.
  Extracted the shared DraggablePoint handle and gave the EQ band points
  and the Filter cutoff/resonance point a common drag + wheel interaction.
  Docked the transient hover/status overlay as an always-visible bottom
  status bar. Made the piano roll's dock resizable via a draggable
  splitter, with a moving-origin drag integrator and a snapshot-tested
  clamp/restore contract. Added the browser sidebar shell: right-docked
  column in flow with the work area, status-bar toggle chip, and an
  ew-resize grip on the same integrator. Filled the sidebar with the
  sample browser: locations persisted in settings.toml, zenity folder
  picker through the pump, VS Code-style tree with expand/collapse,
  wav-only listing, and right-click location removal. Browser pass two:
  playable-children filtering behind a format predicate, an autoplay arm
  and preview-volume trim knob, and a header-stats info pane fed by a
  dedicated engine preview voice with live shared gain. Started the
  gain-structure plan: characterization tests pinning today's source
  peaks, summing, reverb wet-path gain, and fader travel identity, then
  the shared gain module (`mooloop-core/src/gain.rs` + `GainMath` in
  `gain.slint`) with the fader taper and its cross-boundary agreement
  test, then the fader taper and dB readouts across mixer strips, the
  bus output stage, and oscillator level knobs, then the -12 dBFS
  operating level: calibrated every generator against it, set channels
  genuinely at unity, and wrote docs/GAIN_STRUCTURE.md as the standing
  reference, then pinned the per-oscillator unity reference and made
  drive level-compensated (reference-anchored saturation shared by every
  drive stage), then energy-normalized the reverb IR, level-matched the
  plate, and switched the host wet/dry blend to equal-power, then put
   the meters on IEC 60268-18 with the warning threshold at -10 and
   pixel-verified colour transitions, completing the gain-structure plan.
   Opened the modulator-system branch and laid its metadata groundwork:
   `mooloop-core/src/mod_metadata.rs` with durable `ModSourceId` refs and
   source descriptors (shape, rate, latency, trigger), a legacy local-slot
   LFO decode, and `ModDestinationDescriptor` defaults that derive from each
   `ParamCurve` so stepped targets refuse modulation until they opt in.
   Added the bitcrush style row: a `BitcrushStyle` param (crush, TPDF
   dither, µ-law companding, interpolated hold) threading core descriptors,
   the DSP branch, and the device-face SelectorBank, with per-style DSP
   tests pinning the signal behaviors the styles exist for.

### GPT-5 — Codex
- First seen: 2026-08-21
- Last seen: 2026-09-02
- Sessions: 74
- Notes: Audio-core architecture, realtime project swaps, compiled bus graphs,
  latency/gain hardening, device-host controls, command-history foundation, and
  realtime capacity policy, mixer signal-slot design, CI, packaging, and the
  retained-audio buffer device/event path and off-realtime tempo/config ring
  replacement; release README revisions. Refined the channel modulation shelf
  into compact source, source-editor/input, and destination modules, with an
  explicit assignment mode separate from source selection. Augmented the
  channel LFO with free/synced rate and fade-in, clickable sync LEDs, smoothing,
  pulse width, and note-triggered realtime reset. Added the first second source
  type: a tempo-syncable ADSR envelope with explicit cross-channel piano-roll
  gate input, unipolar route defaults, realtime note-gate handling, and its
  compact shelf editor. Made the compact and expanded modulator faces follow
  the configured ADSR and LFO signal shapes instead of generic source icons.
  Refocused the active work on distinct Mono and Poly identities followed by
  the Buffer's ordinary composition workflow. Integrated the composable-device
  unit contract with the existing realtime and modulation architecture.
  Reconciled and merged the local ML-P8 work with the newer sampler
  time-stretch history on GitHub main.

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

### Kimi k3-256k — opencode
- First seen: 2026-08-21
- Last seen: 2026-08-21
- Sessions: 1
- Notes: Implemented the effects-chain vertical slice (filter effect, end to end).

### ox-alpha — opencode
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 2
- Notes: Rescued the ZoomScrollBar widget from an abandoned WIP branch and
  wired it into the piano roll's time and pitch axes.
