# Plans

A plan is a directory of dated, numbered step documents. `00-status.md` records
what has landed and what the doing changed about the plan; the numbered files
are the steps, worked in order. **When every step is done, the whole directory
moves to `archive/`** — that move is what marks a plan finished, so an active
directory should always contain live work.

`docs/FOCUS.md` decides which of these is next. This file only says what state
each one is in.

`incremental-structure/` was added 2026-09-17, **not started**, out of Adam's
reaction to hearing `channel-identity/04`: the engine already changes its
graph without missing a sample for every device edit and for adding a channel,
and ought to do the same for the rest. It is the continuation of
`channel-identity/` rather than a new idea -- that plan removed most of the
reason positional identity made this hard.

`coreaudio-driver/` was added 2026-09-13 and is **in progress, outside the
`FOCUS.md` sequence**, because Adam asked for it directly: he wants to develop
on a Mac as well as on Fedora. It builds a Core Audio driver beside JACK,
chosen at compile time, and stops short of packaging or a macOS release. Every
step has landed; the macOS CI job is the one thing that has not yet run.

`channel-identity/` and `audio-recording/` were added 2026-09-17, **outside
the `FOCUS.md` sequence**, from Adam's answers to the architecture section of
`reports/fable-2026-09-17.md`. `channel-identity/` gives a channel a durable
id, the way devices already have one, and ends with an engine that keeps
strips across an install. **All five steps landed 2026-09-17**; step 06,
three saved fields that still name another channel by position, is what is
left, and half of it has been waiting on the engine map step 05 built.
`plugin-hosting/` step 06 is thereby unblocked. `audio-recording/` is not
started.
`audio-recording/` is `SCOPE.md` items 3 and the audio half of 5, in Adam's
shape: a take goes into the channel's sampler, and one input menu chooses
between audio and MIDI. Its five open questions were all answered by Adam on
2026-09-17, the day the plan was written; they are kept in its `00-status.md`
as a record beside the step each one settles.

`plugin-hosting/` was added 2026-09-16 and is **not started, outside the
`FOCUS.md` sequence**, because Adam asked for it directly: CLAP effects and
instruments now, then VST3, then AU if ever. Thirteen steps. The first four
build the host-neutral contract (#26) with a fake plugin before any protocol
code, and the rule running through all of them is that no format type leaves
`mooloop-plugin-host`. Adam settled four questions while it was written: plugin
state goes in the project TOML, plugin GUIs open in their own windows,
plugins without a GUI get a real face, and the scanner runs out of process.
Linux is the acceptance bar.

`mixer-track-reorder/` was added 2026-09-16, **all four steps landed the same
day**, and it archived. Adam asked for it directly: a mixer track moves by
dragging its strip's name plate, the master stays first, and everything that
named the track follows it. Writing it turned up two older gaps, and both are
fixed: MIDI control bindings followed no structural edit, including the
channel reorder that had already shipped, and a track removal left the
session's track-keyed state on the old numbering. The Track menu it added is
new too. The drop and its undo were checked in the live app; nobody has
listened across one yet.

**`FOCUS.md` was rewritten on 2026-09-12 and it decided five of the entries
below.** The sequence it set was: finish `interface-iteration/` (steps 03 and
04), then `eq-v2/`, then Buffer. The first of those closed on 2026-09-14 and
archived, and `eq-v2/` closed on 2026-09-15 when Adam declined its one optional
step, so the live sequence is Buffer. That step's standing
instruction was to raise the device's *shape* before building, because
`BUFFER_ENGINE.md`'s insert model was not settled; **it was settled on
2026-09-15 and Buffer stays a device**, with
`buffer-implementation/03-freeze-and-the-grid.md` as the work order that
followed. **Every step of `03` landed 2026-09-16, and the acceptance suite
followed on 2026-09-17**: `buffer_workflow_tests.rs` carries six of
`FOCUS.md`'s seven clauses, and writing it found three defects in the
quantized gesture path, all fixed in the same run. What is left of the plan is
the seventh clause and Adam's judgement, both of which need ears.
`device-registry/`,
`theming/`, `pattern-bank-floor/` and `egui-view-layer/` are all parked under
"deliberately not now", each with its reason and with what would unpark it. One
piece is exempt: take `device-registry/`'s face host component if a device step
already has `main.slint` open.

`musical-time/` was added 2026-09-15, **all three steps landed the same day**,
and it archived. A `rust-slint-boundary/` run, written out of
`buffer-implementation/03`, whose BBT readouts had no shared thing to reuse.
"Four beats to the bar" had been spelled **nine times across six crates**;
`time::BEATS_PER_BAR` is the only place that says so now. `BbtPosition` and
`BbtDuration` are two types rather than one with a flag, because a position
counts from one and a duration from zero, and a one-bar length printed through
the wrong one reads `2:1:0`. `BbtText` in `controls.slint` is the one
formatter. Nothing on screen changed.

Two things in `00-status.md` outlive it. **The guard was written before the
fix**, because the plan's mutation table asked for it -- and it then reported
ten sites where the survey that commissioned it had found eight, including two
nobody had looked at. A check written after its fix is shaped to report what
its author already knows. And it shipped **narrower than the plan specified**,
with the reason in its docstring: the wider sweep had one permanent false
positive, and a check that is never clean stops being read.

`control-plane-seams/` was added 2026-09-15, **all five steps landed the
same day**, and it archived the same day. It came from an outside architectural
review that read the system end to end rather than reading a diff, and whose
verdict on the layering was good: the crate stack is one-way, live and offline
rendering share one prepared `RenderState`, and device identity survives rack
movement. What it faulted was the **control-plane boundary** -- the place where
logically atomic operations are written as sequences of independent mutations
and fallible queue writes. Four of the five shared one shape, and it is the
shape this repository keeps producing: **the code knew the right rule and
applied it one layer too shallow.**

Three things in `00-status.md` outlive the plan. The **allocation-tracking
harness** `FOCUS.md` had been asking for since `buffer-implementation/` Stage 1
turned out to be three lines on an allocator `mooloop-engine` already had, and
it was validated against the defect it was written for. **Two steps could not
be given the test they specified**, both because an `EngineHandle` cannot be
built without opening an audio driver -- which is a gap worth a decision, and
is recorded as one. And the fix shapes are worth copying: three of the five
made the defect *unrepresentable* rather than handled -- one publish call
instead of four, a bank taken by value, a `#[must_use]` delivery result -- so
the guarantee is carried by the signature rather than by a test.

The review's sixth item, that the UI/session seam is becoming a god layer, is
in the plan's `README.md` as a direction rather than a step, for the same
reason `device-registry/` has no steps.

`pattern-bank-floor/` was added 2026-09-08 and is **not started, and not on
anyone's list**. It fell out of an audio-dropout investigation whose actual
cause was `rtkit` demoting the machine's realtime threads. What it records is
that every project reserves 1.00 GiB of pattern storage before it holds
anything, and that an ordinary edit rebuilds it at 20 ms a time. Whether that
is worth a step was a `FOCUS.md` question, and the answer as of 2026-09-12 is
**not now** -- safely, because the measurements to judge it by are committed
either way.

Last swept 2026-09-15, and amended five times later the same day -- the fifth
when `musical-time/` was worked start to finish and archived, leaving the
sequence as Buffer alone. The other four: when
`control-plane-seams/` was written; when Buffer's shape was settled and
`buffer-implementation/03` and `musical-time/` were added; and when five
finished plans moved to `archive/` -- `control-plane-seams/` itself,
`pane-layout/`, `session-layer-extraction/`, `preset-system/` and
`poly-v1-mono-mode/`, the last two of which had been sitting under "Queued,
not started" while being done; and a fourth time when Adam **closed the EQ** --
step 04 declined -- and `eq-v2/` archived with it. `eq-v2/` steps 01 and 03 landed on the 14th and step 02
seven minutes into the 15th. 01 gave every EQ band and both pass filters their own stable ids --
the one native device whose parameter model a CLAP host could not have
expressed -- and the thing it proved is that a face showing one band at a time
was never the problem: resolving the selection one layer earlier left the
markup untouched. 03 found a band's Q knob doing nothing at all while that
band was a shelf, and deleted a private copy of the core Q law that had been
running instead of it. 02 stopped the response plot approximating: the curve
is the bank's own coefficients evaluated now, held to a sine through a real
bank, and an `EqSpec` global took the last range out of `eq-device.slint` --
which closed a double-click defect and turned up a pass-slope selector that
had been wrong by a factor of two since it shipped. Earlier the same day,
`interface-iteration/` step 04 landed and the directory archived. The keyboard pass was supposed to be about coverage --
no stop, nothing device-level, no browser focus -- and 63 actions is that
coverage. What it found is that **the registry and the keyboard had drifted
apart and nothing could notice**: `transport.loop-toggle` had been dead since
it shipped on 2026-09-07, bound to a bare L that no branch of `main.slint`'s
key ladder forwarded, with the registry, the prefpane and the whole suite
green over it. Two tests read the markup now rather than the registry. The
decision the step existed to make -- what `Ctrl+C` means -- is that a chord
resolves to one action id and therefore cannot mean three things: it is one
action carrying a `Scope`, resolved against the focused panel, falling back
to the channel because that is what it meant before there was a second
clipboard. Before that, 2026-09-13, when `interface-iteration/` step 03 landed: a channel
has a name and a colour, a pattern has both, and a colour is content the song
owns rather than an index into a palette -- so a song looks the same under
every theme. The step that only meant to add a colour found that **a pattern's
name had never been persisted**, which is the shape `FOCUS.md` names as the
class of this month's work: a claim the source no longer supported. Before that,
2026-09-11, when `console/` finished and moved to `archive/`: step
03 put a channel strip on every track -- an input stage, four EQ bands, a
compressor and a polarity switch, all out by default, under one strip-wide
voicing -- and closed step 06 with it, since the preamp it models had landed
two days earlier and was waiting for somewhere to live. Two things in it
outlive the feature. **A voicing selects laws, never values**: the plan had
one choosing band frequencies and Q, which would have made four voicings four
sets of lying knobs, so what a voicing owns is the harmonic profile, the
tilt, the slew, the Q law, the curve above the knee and the programme
dependence -- each either invisible or *drawn*. And **no control on the strip's faces
declares a range**: `install_strip_spec` hands the markup the whole
descriptor table once at startup, so there is no second copy to drift and
`tests/strip_face.rs` fails if a bound is ever spelled in the markup. That is
the stronger form of what `slint_face_agreement.rs` does for every other
face. `adopt-shared-biquad-in-eq/` went to `archive/` with it, absorbed as
the step said it would be.

Before that, 2026-09-09, when `console/` steps 01, 02, 04 and 05 landed and step
05 was verified: the mixer became a list of tracks somebody made rather than a
fixed seventeen, a track routes a copy of itself to another track, and any
summing point can glue what feeds it. The plan's own `README.md` was rewritten
mid-plan, which is the thing worth carrying: its first pair of decisions
described a derived *view of strips* where grouping took a channel's strip
away, and a console does not do that -- the mixer is tracks, all of them,
always, with bus and send as roles rather than species. Before that,
2026-09-08, when `pane-layout/` steps 01 and 02 landed: the work
area is five views in three slots, the top pane splits, the dock stopped
carrying two toolbars where one would do, and `Ctrl+1`..`Ctrl+5` started
meaning "reveal this view wherever it lives" without their registry entries
changing at all. Earlier the same day, when that plan was written: Adam asked for
the top pane to split, and the four requirements he gave turned out to be one
requirement -- *this view, in that place, at that size* -- asked about four
surfaces, so the plan answers it once. Earlier the same day, when
`ui-consistency-pass/` finished: Adam's standing
list turned into an audit, and the audit found controls that were saying
things the engine was not doing -- a clip light nothing could clear, a sampler
envelope reading 2.5x what it played, a delay whose `1/2` was an eighth note,
and eighteen envelope stages laid across a linear range their own descriptors
call exponential. Before that, 2026-09-07, when `interface-iteration/` step 02
landed: a rack
device can be copied, cut, pasted and duplicated, and the selection that makes
that reachable from the keyboard is an identity rather than a slot. Earlier
the same day, step 01: the browser sidebar browses presets, and an effect
preset loaded from it adds a device rather than replacing one. Earlier the same day, when that plan was written
and the root focus scope was fixed -- it had been standing beside the
interface rather than around it, which is why a shortcut needed a click on the
background first.
Earlier the same day, when `containers/` steps 02-05 landed: a container is a
device that holds a run of devices, blends it, and saves as one preset. Before
that, 2026-09-06, when its step 01 landed: a rack device is now an identity
rather than a position, and `SlotRemap` is gone. Before that,
2026-09-05, seven times, the last when `auto-offline-idle-devices/`
finished: `AudioNode` can say it has nothing to do, and a thirty-two channel
project with one channel playing now costs a tenth of what it did per block.
Before that, when `typed-audio-edges/` finished
and took `poly-synth-v2/` and `drum-synth-v2/` into `archive/` with it — three
directories closing on one mechanism, which is why it was worth building once
rather than twice. Earlier the same day, five times: when the plan was
written, when DS-01's step 09 closed, when `FOCUS.md` was rewritten around the
debt those two instruments left, when ML-P8's step 07 listening pass closed,
and when `latency-compensation/` finished and archived.

`docs/archive/ARCHITECTURE_REVIEW.md` is where the two newest plans came from, and it
is worth reading before either.

## Active

| Plan | State |
| --- | --- |
| `midi-control/` | **All four steps landed 2026-09-15; ready to archive once a keyboard has been plugged into it.** Adam's brief: a channel picks its MIDI input and channel or Omni, CCs assign to knobs and faders, transport controls, record MIDI to a pattern -- with OSC kept in mind. All of it is built: per-channel input routing in the engine, a protocol-free mapping layer whose MIDI half is one of three enum variants, transport gestures carried out in one place wherever they are asked for, note capture into patterns, the sidebar's IN and CH rows, a record-arm button, LEARN beside the transport, and a mapping editor with the transport list on Preferences > MIDI. **None of it has been run against a MIDI device**, which is the only thing between it and the archive. `docs/CONTROL_SURFACES.md` is the design and states where OSC attaches: one `ControlSource` variant, one decoder, and a transport -- nothing else in the layer changes. Three things not to rediscover: JACK presents **one merged MIDI port**, so under JACK keyboards are told apart by MIDI channel rather than by port; the Core MIDI half has never been compiled, because it is behind a macOS `cfg` and this was written on Linux; and step 04 was wrong about this codebase twice in ways worth reading before the next interface pass -- there is no per-control context menu to hang a gesture on, and there is no set of parameter handlers small enough to call `release_control_pickup_for` from. `04-interface.md` says what was done instead. |
| `buffer-implementation/` | **Stage 1 done, Stage 2 open.** Stage 1's acceptance test 8 (RT allocation hygiene) is still unverified, but **no longer for want of an instrument**: `control-plane-seams/04` added `CountingAllocator::allocations()` to `mooloop-engine` on 2026-09-15 and proved it against a real defect. What is left is coverage -- one block on the preview path is measured, the Buffer operations are not, and the locks half of the claim is not measured at all. |
| `mono-synth-v2/` | **Complete and played**, one finding deliberately left open (Acid's cutoff corner). Kept out of the archive only because that finding needs Adam's ear, not because a step is unbuilt. |
| `edit-loop/` | **Steps 01 and 02 landed, 03 closed unstarted, 04 waiting on one measurement.** Six of every ten working hours went on `cargo`. `scripts/antibox` now picks incremental compilation for dev builds and sccache for release builds (64% off `cargo test --workspace`), `AGENTS.md` carries a verification ladder, the mockup tool is behind a Cargo feature, and `scripts/mooloop-run` is one command from edit to running application. Splitting device faces was measured and rejected: 79% of face commits also edit `main.slint`. What is left is `main.slint` itself, which no Slint arrangement reaches -- read `04-decide.md` before `egui-view-layer/`. |
| `egui-view-layer/` | **Written, not decided; argument 4 tested and upheld; `edit-loop/` now points at it.** `edit-loop/04-decide.md` fixed the Rust half of the loop and found the UI half unreachable from inside Slint, which is the argument this plan was waiting for; one post-change `scripts/loop-profile` run closes it. No longer blocked: `session-layer-extraction/` is done, so a view layer would inherit a session rather than reproduce one. Still gated on step 01's spike. `00-status.md` states the case both ways. Compile cost was assumed to be the argument against and measured as an argument for: `build.rs` expands `ui/main.slint` into a single 39 MB Rust module, which is where the four minutes and the 3.4 GB go. What is left to decide is frame time and interaction feel. |
| `ui-consistency-pass/` | **All six steps landed 2026-09-08; ready to archive once Adam has played it.** Not a feature plan: it is the audit Adam's standing list asked for, and every finding is a control that disagreed with the table it is drawn against. Its `README.md` records the method so the sweep can be re-run. Three things came out of it that outlive the fixes: `slint_face_agreement.rs` now holds every envelope stage to its descriptor (the drum synth had been the only device with such a test), `StatusHint` gives any control at any depth a line in the status bar without a property threaded through its device face, and the toolbar rules in `UI_DESIGN.md` -- a switcher leads the toolbar it decides, a pane's setting lives in that pane's header once, and a button per member of a growing set does not survive the set growing. |
| `containers/` | **Steps 01-05 landed 2026-09-06/07; 06 is Adam's call and builds nothing.** A container is a device that holds a run of devices: it wraps, nests four deep, blends its whole run against the signal that went in, bypasses as a unit, and saves as one preset. Its `README.md` answers the brief's four source questions and records three places `reference/CONTAINERS.md` was wrong -- the largest being that per-device wet/dry already existed, so the gap was wet/dry across a *run*. Step 01 gave rack devices durable `DeviceId`s and deleted `SlotRemap` outright. **One thing is blocked rather than done:** a container preset cannot carry the modulation that drives it, because a route's source lives in the channel's rack, and fixing that means deciding whether a modulator can live in a container -- which the brief reserves. 06 prices layers and defers them. |

## Queued, not started

These have steps but no `00-status.md`, because nothing has landed to record.
`device-registry/` is the other way round -- a survey with a status and no
steps, because whether any of it is worth doing is a `FOCUS.md` question and
writing steps would presume the answer.

| Plan | Why it is queued |
| --- | --- |
| `device-registry/` | **A survey, written 2026-09-11, not a work order. Parked 2026-09-12**, except for the face host component, which `FOCUS.md` says to take if a device step already has `main.slint` open. Adam's "we should basically be loading these like plugins we get to have native conversations with", priced. Adding a device kind touches fourteen files, nine of which hold a one-line arm stating one fact; those nine are the registry, spread out. Three findings worth having before anyone tries: the typed `EffectParams` enum is the reason the DSP reads well and should not be flattened into function pointers; a table spanning crates cannot exist, because `mooloop-dsp` depends on `mooloop-core` and so a node constructor is not nameable beside the params it builds from; and Slint has no dynamic component instantiation, so `main.slint` holds one arm per kind under every design short of generating the markup. What *is* reachable is that those arms are 444 lines of which 245 are the same eleven bindings fourteen times -- a face host component would take an arm from twenty-seven lines to eight without a Rust change. |
| `extract-mid-level-dsp-blocks/` | The primitives-to-devices ladder has no middle rung on the DSP side, and `device-displays.slint` holds eight visualizers with no shared canvas. |
| `theming/` | **Parked 2026-09-12**, with the note that it gets cheaper to defer and more expensive to do. Written 2026-09-09 from Adam's question about skins rather than color schemes. `Theme` in `ui/theme.slint` already is the stylesheet -- Slint has no cascade and does not need one -- and the survey in its `README.md` says which axes it is missing: color and radius and motion are tokenized, type and stroke are not at all (313 literal `font-size`, 81 literal `border-width`), and metrics are half-started in `toolbar.slint`'s `ToolbarMetrics`. Queued because it costs more with every device face added, not because it is urgent. **The reason to build it is accessibility rather than the homage**: the working type size is 7-11px and is not adjustable, and nothing checks that a chosen palette is readable. Step 02 (a relief primitive, because a Slint `Rectangle` has one border colour and a bevel needs four) is the only design problem in it; 01 and 03 are a token sweep and a file format, and are worth having on their own. |

## Archived

`archive/` holds finished plans with their status audits intact. Each is worth
reading before reopening the area it covers, because several record *why* a
tempting change was rejected:

`eq-v2/` (steps 01-03, closed 2026-09-15 when Adam declined step 04) is the
newest. It came out of one bug -- the EQ's Shape control was two settings of
different arity behind one automatable id -- and out of Adam's question about
what that implied: mooloop intends to host CLAP, a plugin exposes arbitrary
independently automatable parameters, and our own devices should reach
automation the same way. Under that test the EQ was not a device with a missing
feature but **the one native device whose parameter model a plugin host could
not express**. Step 01 is the only one that argument forces and it landed
2026-09-14: 50 per-band descriptors, no `FORMAT_VERSION` bump (the new ids
start at 16, so a stale lane on a retired one lands in a hole rather than on a
neighbour), and **no change to the face at all** -- the finding worth carrying
into plugin work, because a face showing one band at a time was never the
problem and resolving the selection one layer earlier left the markup
untouched. 03 found a band's Q knob doing nothing while that band was a shelf
and deleted a private copy of the core Q law that had been running instead of
it; 02 stopped the response plot approximating, and turned up a pass-slope
selector wrong by a factor of two since it shipped. **Step 04 was declined
rather than unbuilt** -- band-dependent saturation, the measured character from
five reference EQs -- and the step file is kept whole with its VEQ4 data,
because it is the work order if that sound is ever wanted. The reason is its
own: 01 to 03 correct things the device claimed and did not do, and 04 adds a
claim. Two listening passes are still owed and are recorded in
`LOOSE_ENDS.md`.

`control-plane-seams/` (all five, closed 2026-09-15): five
confirmed control-plane defects from an outside architectural review that read
the system end to end rather than reading a diff, and whose verdict on the
layering was good. Four of the five shared one shape, and it is the shape this
repository keeps producing: **the code knew the right rule and applied it one
layer too shallow.** Read `00-status.md` before touching command delivery,
sample publication or project install. Three things in it outlive the plan: the
**allocation-tracking harness** `FOCUS.md` had been asking for since
`buffer-implementation/` Stage 1, which turned out to be three lines on an
allocator `mooloop-engine` already installed; **two steps that could not be
given the test they specified**, both because an `EngineHandle` cannot be built
without opening an audio driver, which is a gap recorded as a decision rather
than closed; and three fixes that made the defect *unrepresentable* rather than
handled -- one publish call instead of four, a bank taken by value, a
`#[must_use]` delivery result -- so the guarantee is carried by the signature
rather than by a test. Its sixth finding, that the UI/session seam is becoming
a god layer, is a direction in its `README.md` rather than a step.

`pane-layout/` (all four, closed 2026-09-08, archived 2026-09-15) -- five
views, three slots, a view in exactly one slot at a time; the top pane splits,
any pane zooms, a tab drags between panes, and `editor-page` is gone. Read its
`README.md` before touching the work area and `00-status.md` for the four
Slint constraints step 01 found, one of which -- a nested model index that
type-checks and does not evaluate -- cost a wrong render. Adam's split-view
request, answered as a set of views and three slots rather than a control per
requirement.

`session-layer-extraction/` (all six, closed 2026-09-03, archived 2026-09-15)
-- lifted `mooloop-session` out of `mooloop-ui/src/lib.rs`, which went from
14,157 lines to 9,797, and gave the edit logic its first coverage. Two
departures are recorded in `00-status.md`: `UiState::new` is still long
(callback *registration* now, no longer decisions), and the pump's meter
polling stayed in the view on purpose.

`preset-system/` (all four, closed 2026-09-04, archived 2026-09-15) -- a
preset's unit is a device, with relative addressing. Read `00-status.md` before
adding a preset class: it records that an effect preset is a **rack edit, not a
document load**, and that the rule deliberately does *not* generalise to a
generator preset, which references audio that has to be decoded off the UI
thread.

`poly-v1-mono-mode/` (its one step, closed 2026-09-15, archived the same day)
-- the v1 poly has a mono mode, so `DeviceKind::MonoSynth` is deletable and
`MlM1` can take the plain name. Two things the build settled that the step file
had not: mono mode is its own toggle rather than `Voices = 1`, because
`Voices = 1` already means a pool of one voice in every saved project; and id
18 carries `EnvTrigger` rather than `GlideMode`. **The migration itself is a
separate branch and wants a listen before it opens.**

`interface-iteration/` (all four, closed 2026-09-14): a browser
that browses presets, a device clipboard keyed by identity rather than slot, a
channel that has a name and a colour, and the keyboard pass. Read its
`00-status.md` before touching the action registry or either key-decoding
ladder -- it records that a registry entry and a key that can reach it are two
different claims, and that the one action Adam had asked for most loudly had
shipped bound to a key nothing forwarded. Step 01's finding that there was no
"selected rack row" for a preset to land on, and step 03's that a pattern's
name had never been persisted, are both in there too.

`console/` (all six, closed 2026-09-11 and **played the same day** -- *"three distinct and musical characters"* on headphones, with the studio still to come) is the widest: four
items from one morning's list -- reorder channels, group them, make the mixer
work like a console, proper sends -- which turned out to be one design with a
character layer on top. Read its `00-status.md` before touching the mixer,
the strip or the summing: it records the two premises the plan got wrong and
had to retire (a derived *view of strips* where grouping takes a channel's
strip away, and a voicing that moves the numbers its own knobs show), the
step whose whole justification -- that sends force a rewrite of the realtime
schedule -- was false, and the two measurements that changed the design of
console summing. `THE-STRIP.md` beside it is Adam's mockup and the rulings on
it, and is still the reference for the strip's shape.

`adopt-shared-biquad-in-eq/` (absorbed by `console/` step 03 and closed with
it; the EQ's private copy of the RBJ cookbook is gone and the shared one
gained the `is_at_rest` it lacked) ·
`auto-offline-idle-devices/` (all three, closed 2026-09-05; read its status
before touching anything a device runs on the clock rather than on its input,
and before assuming a frozen delay line reads the same as a running one) ·
`typed-audio-edges/` (all five, closed 2026-09-05; the first half of
`AUDIO_ARCHITECTURE.md`'s migration step 6, and the plan that closed the two
below. Read it before building parallel sends or a sidechain input: the
compiled order and the refusal table are what they hang off, and its status
records why same-block delivery was forced rather than chosen) ·
`poly-synth-v2/` (all six, closed 2026-09-05; the ML-P8, its own modulation,
its voice pool, its five-page face and its eight-patch bank) ·
`drum-synth-v2/` (all nine, closed 2026-09-05; the DS-01, its six-page face
and its seventeen-patch kit) ·
`latency-compensation/` (all five, closed 2026-09-05; it is
`AUDIO_ARCHITECTURE.md`'s migration step 5, and the reason typed audio edges
could be built against an aligned tree) ·
`effects-feedback/` (all fourteen, closed 2026-09-02) ·
`gain-structure/` (all eight; `docs/GAIN_STRUCTURE.md` is the standing
reference) · `modulator-modules/` · `modulator-capacity/` ·
`share-dsp-primitives/` · `amortize-reverb-partition-cost/` ·
`reduce-ui-pump-overhead/` · `reduce-audio-jack-buffer-size/` ·
`skip-empty-effect-slots/`
