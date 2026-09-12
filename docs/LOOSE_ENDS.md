# Loose Ends

Small known gaps that agents flagged when handing work back, gathered into one
place so they stop living in chat scrollback. The file and line named is where
to start.

Everything here as of **2026-09-06** was re-verified against the tree that
day. Entries added since carry their own date, and entries older than that
sweep have not been checked against the tree since it — the spike list below
was still claiming thirty-nine unpushed commits on `main` a day after `main`
was pushed, which is what this paragraph is now careful about.

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

**The preamp face has no transfer-curve display, where Drive's has one.**
`preamp-device.slint` leaves the panel Drive fills with
`DriveTransferDisplay` empty. Drawing this stage's curve needs the
coefficients `HarmonicShaper::new` solves for, and reaching them means
widening `EffectSlotRow` in `main.slint` -- which is what the EQ, the
dynamics trio and the Buffer already do for `eq-spectrum-data`,
`gain-reduction-db` and `buffer-collisions`, so the path exists and is
ordinary. The alternative, computing them in markup from the voicing index,
would spell `harmonics.rs`'s profile numbers a second time, which is the
duplication `AGENTS.md` names as this codebase's characteristic fault.

**A transfer curve is probably the wrong display for it anyway.** Adam,
2026-09-10: the interesting question is *where on the spectrum* the stage is
distorting, which a curve cannot show and which is the whole point of the
tilt -- obvious on a kick, nearly clean on a hat. `SpectrumAnalyzer` already
produces exactly the right thing (48 log bands, a Goertzel bank rather than
an FFT, published once a hop and only while a display subscribes), and this
device is unusual in having both the dry and the wet signal in hand at the
same sample. Two analyzers and a per-band difference would make the tilt
visible on the same axis `colour_harmonics_vs_freq.csv` plots.

**The oscillator Level knob works in dB; its descriptor is linear 0–1.**
`device-oscillator.slint:95` drives the knob through `GainMath.linear-to-db`,
while `generator.rs:75`'s `unit()` helper declares the parameter as a linear
`0.0..1.0`. A modulation depth is a fraction of the *descriptor's* range, so
the drawn excursion arc on that one knob is in dB-space and misrepresents its
own width. Assignment and the audible result are both correct. Fixing it
properly means giving Level a gain curve, which touches automation and project
files — which is why it was left.

**~~Reverse and ping-pong refuse to stretch, and slice mode does too.~~**
Closed 2026-09-08. `mooloop-dsp/src/sampler.rs:495` still gates
`stretch_is_active` on `!reverse`, `loop_mode != Pingpong` and
`play_mode != Slice` — the behaviour is unchanged and correct. What was
missing was the explanation: the stretch toggle names which of the three it
is, and says to commit, in the status bar. The recorded fix was a
`hover-hint` property threaded through `main.slint`; `StatusHint` made it a
line on the toggle instead.

---

**The drop gap cannot show which side of a container's edge it lands on.**
The rack opens a one-row gap where a dragged device will land, and the gap
falls inside a container's box when the landing is inside it -- except at a
run's last row, where "just inside the box" and "just after the box" are the
same index and the gap is drawn in the same place either way. Which one a drop
means is decided by `move_effect` in `mooloop-session`, and the rack does not
know that rule: `main.slint`'s cell works the box out from `depth` and
`children` alone. Showing the resolved answer needs the session to publish the
depth a drop at index N would produce -- one `int` property and a callback on
each target change, not a redesign. Everything else about the drag is visible;
this one case still asks for trust.

**Dragging a container opens a one-row gap, not a run-sized one.** The rack's
drop gap is the width of the row being dragged, and dragging a container
moves its whole run -- so the gap it opens is right for a leaf and too small
for a box, and the devices inside the box do not travel with its face while
the pointer is down. `move_effect` does the right thing on release; it is the
picture during the gesture that is wrong. The cell publishes its own width
into `RackDrag.source-width` (`main.slint`), and a run-sized figure would have
to come from the session, which is the only thing that knows where the run
ends.

## Wired but unreachable

**The channel strip's parameters are not automation or modulation
destinations.** Every one has a stable id (`mooloop_core::strip`) and the
engine applies them by id, so the values are addressable; what is missing is
that a lane's target is an `EffectTarget` plus a *slot* and a strip is not a
slot. The ids start at 16 for this: `modulation::STRIP_PARAM_VOLUME` and
`STRIP_PARAM_PAN` are 0 and 1 of what is conceptually the same strip
(`ParamOwner::Strip`, already addressable by a route), so the two tables can
become one without renumbering anything automation has persisted. Recorded
2026-09-11 with step 03.

**Buffer MIDI mapping has no UI.** `EngineHandle::set_buffer_midi_map`
(`mooloop-engine/src/lib.rs:622`) is the only way to install one, and neither
`mooloop-ui` nor `mooloop-session` calls it. MIDI is decoded and routed; it is
just not reachable from the app.

**There is no way to hear a track before its own fader.** Solo is in place
as of 2026-09-11, which silences the others rather than opening a monitor
path, so a soloed track is still heard through its fader, its pan and its
analog-sum switch. `MIXER_PLAN.md` records the AFL tap as later work and
names what it needs: the tap points that pre-fader sends also want.

**A channel cannot be soloed, only its track.** `MixerBus.solo` is per track,
which is the same scope the analog-sum switch has and for the same reason
(`docs/TERMINOLOGY.md`: a mixer strip is a track). Soloing one channel of
several on a track has not been asked for, and would need the per-channel
strip the sends work also wants — so it is one control on one face away, not
a design question.

---

## Focus

**A text field is left with Enter, and by nothing else.** The toolbar's search
and rename fields and the knob/fader numeric entries all call `clear-focus()`
on `accepted` (`toolbar.slint:395`, `controls.slint:959`,
`controls.slint:2005`; the three line numbers this entry carried were all
stale by 2026-09-10) and have no Escape handler, so clicking into one and
then clicking away leaves the caret in it. While it is there, Space types a
space instead of starting the transport — which is correct for a field being
edited and wrong for a field nobody is editing. The 2026-09-07 focus fix made
every *control* transparent to shortcuts; text fields are the remaining case,
and they need a way out rather than a change to what they consume.

**A name is renamed where its subject is edited, and nowhere nearer to it.**
A channel is renamed on the `DEVICES` toolbar and a track on its own device
face, so renaming either means opening the view that owns it. The obvious
alternative — double-clicking the rack plate or the mixer strip — was not
built: the plate already carries a press that selects, a drag that reorders
and a right-click that opens a menu, and a fourth gesture on it needs a
decision about which one loses rather than an implementation. Nothing is
blocked by this; it is one more click than a user coming from FL will expect
(`main.slint`, the `NameField` beside `CHANNEL PRESET`).

## Edits that do not undo

**Sampler slice and marker edits are not undoable.** `add_slice`,
`move_slice`, `remove_slice`, `divide_slices`, `clear_slices` and
`snap_all_markers` in `mooloop-session/src/sampler.rs` never touch history —
the file has no reference to it. This is pre-existing rather than introduced
by the slice work; wiring sampler params into undo is its own change.

**Send edits are not undoable, because routing never was.** `add_send`,
`remove_send`, `set_send_level`, `set_send_tap` and `set_send_enabled` in
`mooloop-session/src/mixer.rs` mark the document dirty and return an
`EngineCommand`, the same shape `set_bus_output` beside them has always had.
A track *add* is undoable, because it goes through the project-edit path — so
one mixer face now has both behaviours on it, which is the part worth fixing.
Unifying them means routing joining `ProjectEdit`, not a per-callback patch.

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

## One name, two policies

**One descriptor id decodes to two enums, and they do not have the same
number of variants.** `EQ_PARAM_CHARACTER` -- the EQ's "Shape" -- means
`EqSlope` when a pass filter is the selected target and `EqQProfile` when a
band is. `EqSlope` has five variants (Db6..Db36) and `EqQProfile` has two,
and the descriptor is sized for the slope: `min: 0, max: 4,
ParamCurve::Stepped(5)` (`effect.rs:340`).

So on a band, positions 1, 2, 3 and 4 all mean `Proportional`, and all four
read back as position 1. The face never sends the other three --
`eq-device.slint:80` drives the band's control as a two-state toggle sending
0 or 0.25 -- so it is not reachable by hand. **An automation lane is not the
face.** A lane drawn at half travel writes position 2; what `get` returns is
position 1. The lane and the parameter disagree about what the lane says, and
the readout snaps somewhere the line is not.

`mooloop-core`'s `stepped_round_trip_tests` covers every stepped parameter in
every table -- effects, generators, modulators and the strip -- and this is
the one id excluded, with `one_id_decodes_to_two_enums_of_different_arity`
pinning exactly what it does instead. So it cannot get quietly worse, and the
day it is fixed that test fails and says so.

Fixing it means a second descriptor id, because a shipped id is frozen
("never renumber a shipped id -- append instead", `effect.rs:198`). That is a
decision about the project format and about which of the two meanings keeps
`EQ_PARAM_CHARACTER`, plus whether a migration rewrites existing lanes --
which is why it is recorded rather than done. Found 2026-09-12.

**`from_index` answers out-of-range input two different ways depending on
which enum you ask, and nothing currently reaches it.** Forty-five enums
convert a selector index to a variant under one name, in two conventions: the
`Self::ALL.get(index.clamp(0, len - 1))` body, thirteen times, clamps to the
nearest end; a hand-written `match` with a `_ =>` arm, thirty-one times,
falls through to the default variant. `NotePriority::from_index(99)` is
variant 0 where an ML-P8 enum's is its last.

**This was recorded as more dangerous than it is, earlier the same day, and
the correction is the useful part.** Every parameter path runs through a
`set` that calls `descriptor.clamp_natural` first, so an out-of-range index
never reaches `from_index` from automation, modulation, a project file or the
UI. All thirty-one hand-written pairs were checked and every one round-trips.
The divergence is real and unreachable, which makes unifying the forty-five
bodies churn rather than a fix -- and is why the pass that found it wrote a
test instead.

What the two conventions do cost is a reader: the same call means two things
depending on the enum, and neither says so. A sentence on each, or one shared
trait, would settle it whenever one of these files is open anyway.

**The five generators each split their own block at note events.**
`mlm1.rs:572`, `mlp8.rs:2494`, `monosynth.rs:314`, `polysynth.rs:393` and
`drumsynth.rs:437` carry the same loop the twelve effects carried until
2026-09-12, when it became `effects::process_param_split`. The generator
version is not a candidate for the same treatment without reading all five
first: its `match` covers note-on, note-off and the events a synth ignores,
and whether the differences between the five are deliberate is exactly the
question. `render_range` is the per-synth method that would sit under it.

**`render_blocks` is written about seven times.** `audio_edge_tests.rs`,
`container_tests.rs`, `ds01_tests.rs`, `idle_skip_tests.rs`,
`console_tests.rs`, `gain_structure_tests.rs` and `strip_tests.rs` each
declare their own "render N seconds in blocks of M and collect the output".
They are all `#[cfg(test)]` modules inside `mooloop-engine/src`, so unlike
the `mooloop-ui` integration tests they can share a plain module without any
`tests/common/` arrangement. The cheapest of the duplication items here.

**The Slint testing backend is set up eighteen times.** Fifteen
`mooloop-ui/tests/*.rs` files spell out the same
`TestingBackend::new(TestingBackendOptions { mock_time, threading,
renderer_name: "software" })`, `source_snapshot.rs` eleven times on its own.
This is the same shape as the piano-roll grid constants below, wants the same
`tests/common/` module, and would be worth doing in the same pass.

## Housekeeping

**The piano roll's grid geometry is a constant in two test files and nothing
holds them together.** `piano_drag.rs:32` and `piano_tools.rs:16` each declare
`GRID_ORIGIN_X` / `GRID_TOP_Y` / `ROW_HEIGHT` / `STEP_WIDTH` / `HIGH_NOTE`,
measured off a software render of the 960x760 window, and `piano_tools.rs`
says "matching `piano_drag.rs`" in a comment that nothing enforces. Both moved
on 2026-09-08 when the dock's two toolbars merged; fixing the first and
running the suite reported the second as nineteen fresh failures, which is how
the copy was found. The cost is one wasted five-minute remote run per toolbar
change, so it is small — but it is exactly the shape `slint_face_agreement.rs`
exists to prevent for faces, and a shared `tests/common/` module or one
element-derived origin would end it. `rack_tools.rs:28` has a third
`GRID_ORIGIN_X` for the step grid; that one is genuinely a different grid.

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

**Six dB readouts still round for themselves.** `GainMath.format-db` now
covers every readout that is a *gain*, but six sites spell their own number
because they are not gains and the shared formatter's signed, `-inf`-floored
output would misreport them: a `+` on a knee width or a gate range is wrong,
and `±0.0 dB` on a limiter ceiling parked at full scale reads oddly.
`compressor-device.slint:70,142`, `gate-device.slint:68,140`,
`limiter-device.slint:62`, `device-displays.slint:545`. Three of them use
`round(x * 10) / 10`, which drops the tenth on a whole value — the same
width-jitter this pass took out of `format-db` itself. What is missing is an
unsigned one-decimal formatter to sit beside `format-db`; that is a decision
about the dB vocabulary rather than a typo, which is why it was left.

**Ticks-per-step is twenty-five bare `24`s in the markup.** `mooloop_core`
owns `TICKS_PER_STEP`; the roll spells it as a literal `24` fifteen times in
`piano-grid.slint` and ten in `main.slint`, because Slint has no shared
constant for it the way `GainMath` and `DeviceRackMetrics` are shared for
gain and rack geometry. Drift is now *detected* -- `piano_tools.rs` and
`piano_drag.rs` compute from the engine's value since 2026-09-10, so a table
edit that the markup does not follow fails them -- but acting on that
failure means finding twenty-five literals by hand. A `RollMetrics`
global beside the other two, with a test asserting it against
`mooloop_core::TICKS_PER_STEP`, would make it one edit. Mechanical, and
larger than a detail fix, which is why it is here.

**A rack unit is two different widths.** A device's total width -- face plus
both rails -- is computed twice and not the same way. An effect slot uses
`unit-width * units + half-gap * (units - 1) + rail-width * 2`
(`main.slint:3806`); the source device uses `unit-width * units + half-gap +
rail-width * 2` (`main.slint:3283`), with the gap term not multiplied. They
agree only at two units. A three-unit source is 724px where a three-unit
effect is 728px, and a four-unit source is 944px against 952px -- so
"3U" on a sampler and "3U" on a delay are not the same measurement.

Nothing is visibly misaligned: the two sit side by side rather than stacked,
and the drag hit-tests each row from its own `absolute-position + width`
(`main.slint:3838`), which is what `CURRENT.md` means by "measured from that
row's own bounds". The cost is only that the unit is not a unit.

Which one is wrong is a design call rather than a reading of the code, which
is why this is a note. Two of the three sites that size a face use
`* (units - 1)`, so it has the majority. But `JOURNAL.md` records the
three-unit source face at its inner 664px -- exactly what the source formula
gives -- as measured and deliberate, and ML-P8 moved to four units on the
finding that three had "no slack anywhere". Correcting the source formula
widens every three-unit source face by 4px and every four-unit one by 8px,
against faces that were sized by eye and signed off. Found 2026-09-10.

**Three unmerged spikes.** `spike/egui-view-layer` (3 commits),
`spike/slint-split-build` (5) and `spike/pattern-bank-cost` (1) are answers
rather than candidates — none is waiting to land. Adam's call whether any
goes anywhere.

There is also `claude/device-identity-rack-addressing-99yt4o` on the remote,
one commit that is not in `origin/main` and has no local branch. Nobody has
said whether it is wanted.

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
