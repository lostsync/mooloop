# Architecture Review

Status: review of the implementation against an external reference, 2026-09-02.

Measures Mooloop against `docs/reference/ANATOMY_OF_A_DAW.md`, a reference
architecture for DAW subsystems that ends with a section written about this
project. The question it was read to answer was Adam's: *how divergent is
Mooloop from a good design, and is a UI toolkit change a good moment to rebuild
what exists?*

The short answers are **barely** and **no**. This document says where that
verdict comes from, and separates the one thing that is genuinely unbuilt from
the several that are deliberately deferred.

`docs/AUDIO_ARCHITECTURE.md` remains the standing engine contract. This review
does not replace it; it grades against it, and the two agree almost everywhere.

## Verdict

The engine matches the reference closely enough that the reference's own
prediction about the project is wrong. It says a groove sequencer at this stage
"probably does not yet have" the realtime boundary, and names it "the piece
whose absence gets more expensive every week."

Mooloop has it. Bounded rings both ways, no allocation on the callback, no
drops on the callback, sample-accurate event dispatch, and a control plane that
never lets the audio thread see the project model.

Where the implementation differs from the reference, it usually differs
deliberately, and the deliberate choice is usually defensible on the reference's
own terms. There is one real gap and one small correctness debt. Everything else
on the reference's list is scheduled work that has not come up yet.

## The four decisions the reference says to settle first

Its Figure 8 names four choices as "each is a rewrite later." All four are
settled.

### 1. Does `process()` take a sub-range or a whole block?

**Settled: a whole block plus a sorted event list, with nodes splitting
internally.**

`AudioNode::process` (`crates/mooloop-dsp/src/node.rs:125`) hands the node the
block and an `EventList` ordered by sample offset. Nodes that respond to events
render in segments — `PolySynth::process`
(`crates/mooloop-dsp/src/polysynth.rs:359`) walks the events and calls
`render_range` between them.

This is the shape CLAP and VST3 use, and it is the better of the two options the
reference offers: it keeps the host from paying a virtual call per event on
nodes that do not care, and it maps a future plugin host onto the trait
one-to-one.

It is also load-bearing rather than aspirational. `Sequencer::schedule` converts
tick positions to real per-block sample offsets
(`crates/mooloop-engine/src/sequencer.rs:568`), including swing, and the tests
assert ordering and drift behaviour rather than just non-emptiness.

### 2. Fixed pipeline or free graph?

**Settled: fixed pipeline, with the bus tree compiled off the audio thread.**

`ChannelStrip` (`crates/mooloop-engine/src/render.rs:925`) is generator → effect
chain → gain/pan → bus. Bus destinations and their render order are compiled
together into a `CompiledBusGraph` on the control thread; the executor only
installs or walks it (`crates/mooloop-core/src/mixer.rs:257`).

This is the Ableton and FL answer, and it is right for a groovebox. The
reference's warning — that the organising idea propagates everywhere and
retrofitting one is a rewrite — cuts in favour of having chosen early.

### 3. Node state behind `Arc`, or owned by the engine?

**Settled: owned by the engine, and the reference's Figure 3 question is
therefore moot rather than unanswered.**

Mooloop does not publish copy-on-write graph snapshots at all. `StructuralCommand`
(`crates/mooloop-engine/src/lib.rs:52`) carries a `Box<dyn AudioNode + Send>`,
allocated on the control thread, into the callback through the ordered command
stream; displaced nodes leave by the reclaim ring and are dropped off-thread.
The comment on that enum states the rule explicitly: the realtime thread never
drops a `Box`.

The reference's stated reason for `Arc`-shared nodes is that DSP state must
survive a graph swap — a reverb tail must not restart because an EQ was inserted
three slots earlier. Here nothing is swapped except the slot being edited, so
the tail survives for a simpler reason. **This divergence should not be
"fixed."** Adopting `arc-swap` graph publication would add a mechanism to
protect an invariant the current design already holds structurally.

The cost of the choice is that a structural edit is a point mutation with an
ordering requirement, which is why `Graph::pending_command`
(`crates/mooloop-engine/src/graph.rs:41`) exists to hold the stream when
reclamation backpressures. That is the right price.

### 4. Is the sample counter the only clock?

**Mostly, and this is the small correctness debt.**

`Transport` (`crates/mooloop-engine/src/transport.rs`) keeps both
representations the reference asks for. `frames_played` is ground truth and
never accumulates float error. But `position_ticks` is an `f64` accumulator
advanced per block —

```rust
let end = start + frames as f64 * self.ticks_per_sample();
```

— and `position_ticks` is what scheduling actually reads. Musical time is
therefore an accumulator that runs *beside* the sample counter rather than a
mapping *of* it, which is the specific arrangement the reference identifies as
producing long-session drift.

The module comment already flags the intended fix ("the future tempo-map /
playlist layer will anchor on this"). Severity today is low: at fixed tempo the
error is bounded and inaudible over any realistic session, and `ProcessContext`
already carries `position_frames` so tempo-synced nodes can use ground truth.
Severity rises the moment a tempo map exists, because that is when the
conversion stops being a multiply and starts being an integral that must not be
computed twice from two different sources.

**Recommendation:** derive `position_ticks` from `frames_played` when the tempo
map lands, not before. Doing it now is a one-line change that buys nothing; doing
it as part of the tempo map is where it belongs.

## Where Mooloop is ahead of the reference's growth path

Two items the reference schedules for later already exist, and are better than
the sketch it gives.

**Unified modulation and automation.** The reference's item 4 treats automation
lanes and modulators as a thing to add once sample-accurate dispatch exists, and
holds up Bitwig's any-modulator-to-any-parameter system as the fullest
expression of the idea. Mooloop already resolves both through one destination
space: `ParamAddr` names a target, and modulation and automation compose into
the same per-subdivision control segments each block
(`resolve_strip_segments`, `crates/mooloop-engine/src/render.rs:767`ff, and the
`ModulationBlock` / `AutomationBlock` pair above it). A strip's fader is an
ordinary destination, addressed the same way a device parameter is.

The reference's own analysis says this is the choice that must be made early
because it forces the parameter subsystem to be control-rate from day one.
That has already happened here.

**Arrangement over patterns.** The reference closes by naming its item 5 — "the
one thing worth arguing about" — as where the product's identity is decided, and
warns that inheriting the answer by accident means a linear timeline that fights
the step data forever. Mooloop chose pattern-first deliberately: the playlist is
tick-addressed placements of patterns
(`crates/mooloop-engine/src/sequencer.rs:661`ff), and `docs/PRODUCT.md` argues
the position rather than assuming it.

Nothing to do here. It is worth knowing that the decision the reference
considers most dangerous to defer is the one already closed.

## The one real gap: graph-wide delay compensation

Per-device dry alignment exists. `DryAlign` is built from
`node.dry_path_latency_frames()` at install time
(`crates/mooloop-engine/src/render.rs:613`), so a device with an internal
parallel path never mixes time-misaligned wet and dry.

**Cross-channel and cross-bus compensation does not exist.** Two channels
summing into a bus are not aligned against each other. If one carries a
latency-reporting device and the other does not, they sum out of phase, and the
symptom is the comb filtering in the reference's Figure 5.

Today this costs almost nothing, and that is exactly the argument for doing it
now:

- `effects/drive.rs:151` is the only effect that reports nonzero latency, so
  there is presently at most one way to trigger the bug.
- `docs/AUDIO_ARCHITECTURE.md` already specifies the fix as migration step 5
  ("introduce preallocated compensation delays and compile cumulative latency
  for the existing mixer tree"), and steps 1–4 are done.
- The compile site already exists. `CompiledBusGraph` is built off-thread and
  owns the render order; cumulative latency is another quantity to compile into
  it, not a new subsystem.
- The price of deferring is not linear. Every lookahead dynamics device, every
  oversampled effect, and the whole plugin-hosting item make the graph wider
  *and* make the bug reachable. Compensating a one-destination mixer with four
  latency sources is a contained job; compensating a general DAG with sends and
  sidechains after the fact is the reference's "fixed point" version of it.

**Recommendation: this is the next infrastructure step, and it should be taken
before any work that adds latency-reporting devices.** It is the only item in
this review that is cheaper today than it will ever be again.

## Deliberate deferrals, correctly deferred

None of these are divergences. They are the reference's growth path beyond the
point Mooloop has reached, and the ordering matches.

| Reference item | State | Note |
| --- | --- | --- |
| Plugin hosting (item 3) | Absent | `AudioNode` is already shaped to take CLAP one-to-one. Deferring costs nothing while the trait stays honest. |
| Audio tracks, disk streaming, the butler (item 6) | Absent | No capture path and no media pool. Correctly behind the instrument work. |
| Live looper (item 7) | Partly, differently | The buffer device is retained-audio, not a looper state machine. `docs/BUFFER_ENGINE.md` owns that design. |
| External sync — Link, MIDI clock, MTC (item 8) | Absent | The reference says this is cheap if the clock decision went well. It mostly did; see decision 4. |
| Multi-core graph execution | Absent, single-threaded serial render | The reference's own point applies: every parallelism scheme buys throughput with lookahead, and a groovebox wants the low-latency path. Not a gap at this scale. |

One reference suggestion is closer than it looks. It proposes shipping the
engine as a CLAP plugin as a cheap way to find out whether the node interface is
clean. `RenderState` is already documented as "JACK-independent render state
shared by realtime playback and file export"
(`crates/mooloop-engine/src/render.rs:1`), and only `driver.rs` binds to JACK.
The experiment is available whenever it is wanted.

## Undo: a knowing divergence, low priority

The reference argues for a command log over the project model, on the grounds
that snapshots of a large session are too big to take on every fader move.

Mooloop uses snapshots: `History<ProjectSnapshot>`
(`crates/mooloop-session/src/history.rs`), where a snapshot is the whole
`Project` plus a vector of sample handles.

The reference's objection is about scale, and does not bite yet. Samples are
`Arc` clones rather than buffer copies, project documents are small, and the
drag problem the reference raises is already solved a different way — gesture
tokens coalesce a whole pointer drag into one entry (`history.rs`, `Entry::gesture`).

Worth revisiting if project size grows by an order of magnitude. Not worth
revisiting now, and not a reason to restructure anything.

## The UI layer, and the rebuild question

Adam's framing was that a move to egui would be a good opportunity to rebuild
what exists, properly. The evidence says the opposite: **the engine is the part
that is already built properly, and the UI layer is the part that is not.** A
rebuild of both would discard the strongest code in the repository to fix a
problem located somewhere else.

The measurements below are from this review, before
`docs/plans/session-layer-extraction/` ran. They are the diagnosis, and are
left as they were written; what happened next is under "Extraction: done"
after the recommendation.

The coupling is real, but it is concentrated:

| Layer | Lines | Depends on a UI toolkit |
| --- | --- | --- |
| `mooloop-core` | 14,205 | No |
| `mooloop-dsp` | 23,650 | No |
| `mooloop-engine` | 9,126 | No |
| `mooloop-project` | 4,787 | No |
| `mooloop-ui` (Rust) | 16,490 | Yes |
| `mooloop-ui` (43 `.slint` files) | 20,451 | Yes |

Sixty-eight percent of the Rust has no opinion about the toolkit and would
survive a migration untouched. The remaining third is where everything hard
lives, and three measurements describe it:

1. **`crates/mooloop-ui/src/lib.rs` is 13,411 lines in one file**, carrying 589
   property set/get calls and 187 callback registrations.
2. **`UiState::new` runs from line 4370 to line 12078** — a 7,700-line
   constructor that registers every callback inline. Edit logic lives in closure
   bodies interleaved with `window.set_*` calls.
3. **Session state is stored inside toolkit containers.** `UiState` holds
   roughly a dozen `Rc<VecModel<...>>` fields. That is not rendering state; it
   is the application's live model kept in Slint's data structures, and it is
   the specific thing an immediate-mode toolkit cannot carry across.

The reference's remark that Slint belongs on the UI side of the realtime
boundary is satisfied — meters and playhead come from wait-free snapshots polled
on a timer (`crates/mooloop-engine/src/meters.rs`), exactly as it prescribes.
The problem is the opposite boundary, the one between the session model and the
view, and the reference does not discuss it because it is not an audio problem.

The good news is that the seam is cleaner than the file size suggests.
`ChannelState` (`lib.rs:586`) is already entirely toolkit-free. `UiState`'s
fields are roughly ninety percent plain Rust with the models mixed in among
them. And its 58 methods divide along an obvious line: `sync_*` / `refresh_*` /
`show_*` project into Slint, while `automation_lane`, `effect_chain`,
`retarget_effect_slots`, `select_note`, `prune_note_selection`,
`placement_covering`, `allowed_destinations` and their neighbours are model
logic that has no business knowing about a toolkit.

**Recommendation: extract before migrating, and treat the two as separate
decisions.** `docs/plans/session-layer-extraction/` lifts a toolkit-free session
crate out of `lib.rs`; `docs/plans/egui-view-layer/` builds a view against it.
The first is worth doing whether or not the second ever happens, because it is
what makes a 13,411-line file testable and what turns "rewrite the app" into
"write a view layer."

### Extraction: done, 2026-09-03

`mooloop-session` exists and has no `slint` in its dependency tree. It owns the
model, the edits, undo, and engine command emission; `mooloop-ui` owns the
window, the models, the callbacks and the projection. `lib.rs` is 9,797 lines,
`cargo test -p mooloop-session` is 87 tests in under a second, and the third
measurement above -- session state stored inside toolkit containers -- no
longer holds: `Session` owns the plain fields and `UiState` keeps only the
`Rc<VecModel<...>>` projections of them.

Two things stayed on the view's side deliberately, both recorded with reasons
in `docs/plans/session-layer-extraction/00-status.md`: `UiState::new` is still
long, but what is left in it is callback registration rather than decisions;
and the pump's meter polling stayed where it is, because its per-row change
detection is what keeps the pump cheap.

One tension worth naming rather than discovering later: `docs/FOCUS.md` says to
prefer changes that produce a musical decision over changes that merely add
capacity. A toolkit migration is the largest available non-musical change. That
is Adam's call, but the project's own standing rule argues against it, and it
should be answered deliberately rather than by drift.

## Summary of actions

| Action | Priority | Where |
| --- | --- | --- |
| Compile graph-wide latency compensation into the bus plan | **Next infrastructure step** | `docs/AUDIO_ARCHITECTURE.md` step 5 |
| Extract a toolkit-free session layer from `mooloop-ui/src/lib.rs` | High, and independent of egui | `docs/plans/session-layer-extraction/` |
| Derive `position_ticks` from `frames_played` | With the tempo map, not before | This document, decision 4 |
| Build an egui view layer | Adam's call; only after the extraction | `docs/plans/egui-view-layer/` |
| Rebuild the engine | **No** | — |
| Adopt `arc-swap` graph publication | **No** | This document, decision 3 |
| Replace snapshot undo with a command log | Not now | This document, undo |
