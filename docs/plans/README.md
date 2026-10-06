# Plans

A plan is a directory of dated, numbered step documents. `00-status.md` records
what has landed and what the doing changed about the plan; the numbered files
are the steps, worked in order. **When every step is done, the whole directory
moves to `archive/`** — that move is what marks a plan finished, so an active
directory should always contain live work.

`docs/FOCUS.md` decides which of these is next. **Linear holds the state**:
every plan here has a project there, and every step still to do an issue, and
those are the copy to trust and to update (`AGENTS.md`, *Tracking work: Linear*). This
file is the index: what each plan is, and what is worth reading before
reopening it. A plan's own `00-status.md` says how it got where it is.

## Active

- `plugin-hosting/` (added 2026-09-16) — CLAP effects and instruments now,
  then VST3, then AU if ever. No format type leaves `mooloop-plugin-host`.
- `icon-pass/` (planned 2026-09-26) — one registry, `icons.slint`, and one
  `Icon` component for every icon. Read its README for the survey, and
  `00-status.md` for Adam's four rulings.
- `song-patch/` (planned 2026-10-06) — the Modulation pane becomes one
  patching canvas for the song: typed boxes, control and note wires, and
  the song's sources and destinations as tags. Read its README for Adam's
  words, the survey and what it reverses, and `00-status.md` for the
  defaults it picked that he can overturn.
- `extract-mid-level-dsp-blocks/` — only step 03 is left: `device-displays.slint`
  holds eight visualizers with no shared canvas. `00-status.md` explains why
  step 02 is cancelled.

## Queued, not started

`device-registry/` is a survey with a status and no steps, because whether any
of it is worth doing is a `FOCUS.md` question and writing steps would presume
the answer.

| Plan | Why it is queued |
| --- | --- |
| `song-automation/` | Planned 2026-09-30, in for 0.2.0: automation lanes on the song's timeline, in a panel under the playlist. Read its README for the survey and what it reverses and keeps. |
| `device-registry/` | **A survey, written 2026-09-11, not a work order. Parked 2026-09-12**, except for the face host component, which `FOCUS.md` says to take if a device step already has `main.slint` open. |
| `pattern-bank-floor/` | Parked 2026-09-12 (`FOCUS.md`). Every project reserves 1.00 GiB of pattern storage before it holds anything. With per-track clips ruled out (2026-09-17), it is a prerequisite for raising `MAX_PATTERN_STEPS`; its `00-status.md` records that. |

## Archived

`archive/` holds finished plans with their status audits intact. Each is worth
reading before reopening the area it covers, because several record *why* a
tempting change was rejected:

- `plugin-browser/` (done 2026-10-06, for 0.1.8) — the PLUGINS tab's filter
  takes the keys, instruments and effects are listed apart, and plugins filter
  by format, category and a favourites star. Its README says what each plugin
  format reports about a plugin's kind; VST3 categories wait on VST3 hosting.
- `song-modulation/` (planned 2026-10-05, done 2026-10-06, for 0.1.7) —
  modulators belong to the song, not a channel, and have the Modulation
  pane. Read its README for what it reversed in `MODULATION.md`, and
  `00-status.md` for where each step differed from the plan and the three
  things still open.
- `theming/` (done 2026-10-04, on Adam's word) — skins rather than colour
  schemes, built for accessibility rather than the homage. What it left open
  (MOO-154, MOO-157, MOO-205) is in Polish backlog.
- The Fable 2026-09-22 plans, each landed in one pass that day:
  `sequencer-cursor/` (A), `filter-coeffs/` (B; its measurement is MOO-505),
  `block-constant-work/` (C), `automation-curves/` (D; Plate, Reverb and the
  generators' curve paths are MOO-502 and MOO-503) and `generator-face-rows/`
  (E, a pilot on Aux In; the other seven faces are MOO-504).
- `containers/` (finished 2026-09-23) — a container is a device that holds a
  run of devices, blends it, and saves as one preset; a layer is drawn after
  Bitwig's FX Layer. Selectors stay unbuilt and priced (step 06).
- `master-bus-compressor/` (added and finished 2026-09-23) — the three laws as
  data (Grip, Punch, Tube), the section on the master's strip, and its face
  and needle meter. Its `00-status.md` records what fitting the laws found.
- `audio-recording/` (finished 2026-09-23) — `SCOPE.md` items 3 and the audio
  half of 5: a take goes into the channel's sampler. Adam's answers to its
  questions are in its `00-status.md` beside the step each one settles.
- `coreaudio-driver/` (finished 2026-09-22) — a Core Audio driver beside
  JACK, chosen at compile time, short of packaging or a macOS release. Its
  status file says which hardware check was made and which was closed on the
  record.
- `edit-loop/` (archived 2026-09-22) — six of every ten working hours went on
  `cargo`; it fixed the Rust half (`scripts/antibox`, the verification ladder
  in `AGENTS.md`, `scripts/mooloop-run`).
- `egui-view-layer/` (archived unstarted 2026-09-22) — Adam: *"we can
  archive. i think QT would be better than egui if we do switch toolkits."*
  Its `00-status.md` has what `main.slint` costs.
- `gesture-undo/` (2026-09-21) — every edit reaches the undo history, because
  undo installs a whole-project snapshot and destroys an edit that never
  reached it. Read its `00-status.md` before reopening any of it.
- `transport-discontinuity/` (2026-09-20) — held notes cut off by switching
  the pattern on screen, and the mechanism whose absence caused it: a command
  that lands at a musical boundary, an `AudioNode` that can be told time
  moved, and the rule that navigation must not reach the audio thread. Read its `00-status.md` before
  reopening any of it, for Adam's two rulings and the two things it leaves
  open.
- `channel-identity/` (finished 2026-09-18) — a channel has a durable id, and
  nothing in a song names a channel by its seat.
- `incremental-structure/` (finished 2026-09-18) — tracks got a `TrackId`, and
  an install carries every strip the edit is not about. Step 05 records why
  its audio-thread structural commands were decided against.
- `buffer-implementation/` (closed 2026-09-18) — the whole device, control and
  modulation, then freeze and the grid, and a gesture rebuild. Read its
  `00-status.md` for the lesson it paid most for.
- `midi-control/`, `ui-consistency-pass/` and `mono-synth-v2/` (archived
  2026-09-18 on Adam's word, each having waited only on him).
- `mixer-track-reorder/` (2026-09-16) — a mixer track moves by dragging its
  strip's name plate, the master stays first, and everything that named the
  track follows it.
- `eq-v2/` (steps 01-03, closed 2026-09-15 when Adam declined step 04) — the
  EQ was the one native device whose parameter model a plugin host could not
  express. Step 04 is kept whole with its VEQ4 data, because it is the work
  order if that sound is ever wanted.
- `musical-time/` (2026-09-15) — `time::BEATS_PER_BAR` is the only place
  that says four beats to the bar, and `BbtText` is the one formatter.
- `control-plane-seams/` (closed 2026-09-15) — five control-plane defects from
  an outside architectural review. Read `00-status.md` before touching command
  delivery, sample publication or project install.
- `pane-layout/` (closed 2026-09-08, archived 2026-09-15) — five views, three
  slots, a view in exactly one slot at a time. Read its `README.md` before
  touching the work area and `00-status.md` for the four Slint constraints
  step 01 found.
- `session-layer-extraction/` (closed 2026-09-03, archived 2026-09-15) —
  lifted `mooloop-session` out of `mooloop-ui/src/lib.rs`. `00-status.md`
  records two departures.
- `preset-system/` (closed 2026-09-04, archived 2026-09-15) — a preset's unit
  is a device, with relative addressing. Read `00-status.md` before adding a
  preset class.
- `poly-v1-mono-mode/` (closed 2026-09-15) — the v1 poly has a mono mode, so
  `DeviceKind::MonoSynth` is deletable. **The migration itself is a separate
  branch and wants a listen before it opens.**
- `interface-iteration/` (closed 2026-09-14) — a browser that browses presets,
  a device clipboard keyed by identity, a channel with a name and a colour,
  and the keyboard pass. Read its `00-status.md` before touching the action
  registry or either key-decoding ladder.
- `console/` (closed 2026-09-11) — the mixer as a console, with a character
  layer on top. Read its `00-status.md` before touching the mixer, the strip
  or the summing. `THE-STRIP.md` beside it is Adam's mockup and the rulings on
  it, and is still the reference for the strip's shape.
- `adopt-shared-biquad-in-eq/` (absorbed by `console/` step 03) ·
  `auto-offline-idle-devices/` (closed 2026-09-05; read its status before
  touching anything a device runs on the clock rather than on its input) ·
  `typed-audio-edges/` (closed 2026-09-05; read it before building parallel
  sends or a sidechain input) · `poly-synth-v2/` (the ML-P8, closed
  2026-09-05) · `drum-synth-v2/` (the DS-01, closed 2026-09-05) ·
  `latency-compensation/` (closed 2026-09-05; `AUDIO_ARCHITECTURE.md`'s
  migration step 5) · `effects-feedback/` (closed 2026-09-02) ·
  `gain-structure/` (`docs/GAIN_STRUCTURE.md` is the standing reference) ·
  `modulator-modules/` · `modulator-capacity/` · `share-dsp-primitives/` ·
  `amortize-reverb-partition-cost/` · `reduce-ui-pump-overhead/` ·
  `reduce-audio-jack-buffer-size/` · `skip-empty-effect-slots/`
