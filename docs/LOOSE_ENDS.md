# Loose Ends

Small known gaps that agents flagged when handing work back, gathered into one
place so they stop living in chat scrollback. Each one was **re-verified
against the tree on 2026-09-06**; the file and line named is where to start.

This is not a roadmap and not a bug list. Everything here was a deliberate
stopping point rather than an oversight, and none of it blocks the sequence in
`FOCUS.md`. It exists so that a thing already known does not get rediscovered
as a surprise, and so an idle half hour has somewhere to look.

Scope rule: an item belongs here if it is **small, specific, and true of the
code right now**. A gap large enough to need a plan belongs in `docs/plans/`;
a wish belongs in `ENHANCEMENTS.md`; a described behaviour gap belongs in
`CURRENT.md`. When an item is fixed, delete the row — do not annotate it.

---

## Wrong-looking UI over correct behaviour

**The oscillator Level knob works in dB; its descriptor is linear 0–1.**
`device-oscillator.slint:95` drives the knob through `GainMath.linear-to-db`,
while `generator.rs:75`'s `unit()` helper declares the parameter as a linear
`0.0..1.0`. A modulation depth is a fraction of the *descriptor's* range, so
the drawn excursion arc on that one knob is in dB-space and misrepresents its
own width. Assignment and the audible result are both correct. Fixing it
properly means giving Level a gain curve, which touches automation and project
files — which is why it was left.

**Reverse and ping-pong refuse to stretch, and slice mode does too.**
`mooloop-dsp/src/sampler.rs:495` gates `stretch_is_active` on `!reverse`,
`loop_mode != Pingpong`, and `play_mode != Slice`. The UI disables the
combinations, so this is belt-and-braces rather than a silent failure, but the
reason is only in the source comment — nothing tells a user *why* the control
went grey.

---

## Wired but unreachable

**Buses cannot be renamed.** `MixerBus.name` is in the project format, saves
and loads fine, and defaults to `"Bus 1".."Bus 16"` (`mixer.rs:64`). There is
no command, action, or Slint field that sets it — grep finds no `RenameBus` or
`SetBusName` anywhere. For a drum bus you would want "Drums".

**Buffer MIDI mapping has no UI.** `EngineHandle::set_buffer_midi_map`
(`mooloop-engine/src/lib.rs:622`) is the only way to install one, and neither
`mooloop-ui` nor `mooloop-session` calls it. MIDI is decoded and routed; it is
just not reachable from the app.

**Solo is a button with nothing behind it.** `SoloButton` exists in
`controls.slint:1856` with a `soloed` property; `mooloop-core` has no solo
state at all. `MIXER_PLAN.md` specifies the intended behaviour (an AFL-style
monitor tap, not a routing change). Also standing in `ENHANCEMENTS.md`.

---

## Focus

**A text field is left with Enter, and by nothing else.** The toolbar's search
and rename fields and the knob/fader numeric entries all call `clear-focus()`
on `accepted` (`toolbar.slint:316`, `controls.slint:922`,
`controls.slint:1796`) and have no Escape handler, so clicking into one and
then clicking away leaves the caret in it. While it is there, Space types a
space instead of starting the transport — which is correct for a field being
edited and wrong for a field nobody is editing. The 2026-09-07 focus fix made
every *control* transparent to shortcuts; text fields are the remaining case,
and they need a way out rather than a change to what they consume.

## Edits that do not undo

**Sampler slice and marker edits are not undoable.** `add_slice`,
`move_slice`, `remove_slice`, `divide_slices`, `clear_slices` and
`snap_all_markers` in `mooloop-session/src/sampler.rs` never touch history —
the file has no reference to it. This is pre-existing rather than introduced
by the slice work; wiring sampler params into undo is its own change.

---

## Ceilings and one-shots

**`MAX_MOD_ROUTES_PER_CHANNEL` is 16** (`modulation.rs:1290`) — two routes per
module across eight slots. It was left there deliberately, to be raised once
the modulator grid has been lived in. The price of raising it is now measured
and linear, so this is a one-line decision when the answer is known.

**Factory banks self-seed once and can never update.**
`mooloop-project/src/factory.rs` writes `.factory-v1` (and `.ml1-factory-v1`)
markers on first run and never rewrites the directory. Editing a factory patch
in `effect_factory.rs`, `ds01_factory.rs`, `mlm1_factory.rs` or
`mlp8_factory.rs` will not reach a machine that has already seeded unless the
marker or the directory under `presets/` is deleted by hand.

**Per-slice loop points do not exist.** `loop_mode` sits on `SamplerParams`
(`sampler.rs:552`), so looping is all slices or none. Explicitly deferred.

---

## Meter and time

**The project is 4/4 end to end.** `render.rs:1709` hardcodes
`const BEATS_PER_BAR: f32 = 4.0`, and `integrity.rs:480` rewrites any other
meter back to 4/4 on load with a doctor message. `Project.beats_per_bar`
exists and is settable in memory, so the field promises more than the engine
delivers. Not a silent bug any more — but a 3/4 project is not a thing.

**`position_ticks` is an accumulator, not derived from `frames_played`**
(`mooloop-engine/src/transport.rs:24`). `ARCHITECTURE_REVIEW.md`'s action
table calls this out and says to fix it *with* the tempo map, not before.
Recorded so the deferral stays deliberate.

---

## Decisions whose reason expired

**The limiter still has no lookahead, and the code's stated reason is now
false.** `mooloop-dsp/src/effects/dynamics.rs:391` says "Add lookahead when
the engine can compensate for it, not before." The mixer became latency
compensated on 2026-09-05, so the condition is met. `CURRENT.md` already
records this as an open decision rather than a settled no; the source comment
does not.

**`BUFFER_ENGINE.md` still specifies Buffer as an ordinary insert** "at the
useful point in a chain" (lines 12, 45, 66). Adam has since said that framing
is partly wrong — the device belongs at the end of a rack with its own lane.
Nothing in the repository captures the rethink; the doc reads as settled.

---

## Cannot currently be tested

**The modulator plan's acceptance test 8 — RT hygiene, no allocations or
locks in the audio callback — still has no harness that can express it.**
There are two `#[global_allocator]`s in the tree and neither one does this
job: `spikes/time-stretch/src/main.rs:52` is outside the workspace, and
`mooloop-session/src/lib.rs:55` is `#[cfg(test)]`, counts *live bytes* to
measure undo-history footprint, and lives in the session crate rather than
in engine or DSP where the callback actually runs. Counting a steady-state
total is not the same as trapping an allocation on the audio thread. Until
something is, the test is satisfiable only by reading code — which is the
thing it exists to replace.

---

## Consistency questions, not bugs

**A departed producer and a departed device are handled oppositely.** Aux In
sends a subscription whose source channel was deleted to `DEPARTED_SOURCE`
(`aux_in.rs:147`), keeping it inert and inspectable. The modulation rack drops
routes whose device is gone (`modulation.rs:1875`) while keeping *illegal*
routes inert (`modulation.rs:1932`). Both behaviours were chosen on purpose in
their own passes; nobody has decided whether they should match.

---

## Housekeeping

**`mooloop-ui` had never been linted, and two things had ridden in on that.**
Fixed 2026-09-07, recorded because the *shape* of it will recur: `cargo
clippy` walks the dependency graph, `mooloop-core` had been failing since
`df52933`, and a run that dies there never reaches the crate you were asking
about. The two it was hiding were a redundant rebinding and — the one that
mattered — a `#[test]` attribute that had come adrift from its function, so
`effect_rack_scrolls_horizontally_to_reach_a_long_chain` had stopped being a
test. **A disabled test does not fail; it stops existing**, and `dead_code`
was the only thing that could have said so. When clippy is red anywhere,
nothing downstream of it is being checked at all.

**The README hero screenshot predates effects.** `mooloop-screenshot.png`,
captioned "channel rack and Mono Synth" — accurate, but no longer showing the
most interesting part of the app. A fresh one can be rendered headlessly.

**`CURRENT.md` has two bullets spliced into one line.** At line 554 the
limiter-lookahead sentence runs straight into "Each kind publishes a static
`ParamDescriptor` table", which belongs to a separate bullet that lost its
list marker in the 2026-09-05 edit.

**Two unmerged spikes and 39 unpushed commits on `main`.**
`spike/egui-view-layer` and `spike/slint-split-build` are answers rather than
candidates — neither is waiting to land. Adam's call whether either goes
anywhere, and when `main` gets pushed.

---

## Closed since being raised

Kept briefly so the same thing is not re-reported. Delete freely once stale.

- Buffer had no parameter descriptors and could not be automated — it has
  `BUFFER_DESCRIPTORS` now (`effect.rs:119`).
- The oscillator Semis descriptor disagreed with its knob's travel — both are
  `-48..48`, with a comment saying why (`generator.rs:428`).
- Clear Pattern and Select All were disabled menu rows — both are wired
  (`main.slint:1485`, `main.slint:1508`).
- The v1 mono synth shipped alongside "Mono 2" — the picker shows one Mono
  (`main.slint:2555`).
- The last recorded listening pass was stale at ML-M1 / 2026-08-31 —
  `FOCUS.md:337` now records DS-01 and ML-P8.
- Whether `BufferParams` references audio, which would have forced effect
  presets down the asset-collection path — it is three scalars
  (`effect.rs:1785`), so it does not.
- `scripts/antibox` refilling with caches for checkouts that no longer exist —
  `--prune` and `--prune-age` exist.
- Stray remote-tracking refs and empty `mooloop-worktrees/` directories — gone.
