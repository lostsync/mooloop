# Effects units — implementation plan

Status: implemented historical plan. The ordered control stream and prepared
project-state ownership in `AUDIO_ARCHITECTURE.md` supersede this document's
separate structural-ring transport details. Read `AGENTS.md` first
(worktree/git rules) and `docs/UI_DESIGN.md` before touching any `.slint` file.

The implementation narrative below is retained to explain how the effect
slice was built; it is not the current transport API. Follow
`AUDIO_ARCHITECTURE.md` and `CURRENT.md` when extending the engine.

This document tells you exactly what to build and in what order. Where a
design question could go multiple ways, a decision has already been made
below — don't re-litigate it, just implement it. If something in here turns
out to be flat wrong once you're in the code (a type doesn't exist, a
signature differs), fix the plan's assumption and keep going; don't stall on
it.

## What we're building

Effects units that slot in after a channel's generator (sampler / drum synth
/ mono synth), chainable, reorderable by drag, and built so they're portable
to a future mixer/send-bus system and a future CLAP plugin host. The first
effect to actually ship is a simple filter (low-pass/high-pass), because the
DSP for it (`Svf`) already exists and is unused — this gives a complete
vertical slice (DSP → core types → engine → persistence → UI → drag-reorder)
without inventing new DSP math along the way.

Do not build a second effect type until the filter is fully working
end-to-end, tested, and playable in the running app. The filter is the
template every later effect copies.

## Why the design looks like this

Two hard constraints from the existing codebase, non-negotiable:

1. `crates/mooloop-dsp/src/node.rs` documents the realtime contract: code
   running inside `AudioNode::process` (the JACK thread) must never
   allocate, free, lock, or block. This is enforced by convention, not the
   compiler, so violating it won't fail to build — it'll cause audio
   glitches/xruns under load. Take it seriously.
2. `EngineCommand` (`crates/mooloop-core/src/bridge.rs`) is a
   `#[derive(Copy)]` POD enum drained on that same realtime thread. Anything
   that needs to hand a *heap-allocated* object (a `Box<dyn AudioNode>`) to
   the audio thread cannot go through it — `Box` isn't `Copy`, and building
   one on the RT thread would allocate.

Everything odd-looking below (the two extra ring buffers, the reuse of
`Event::ParamValue`) exists to satisfy these two constraints. If you're
tempted to simplify by putting a `Box` inside `EngineCommand`, don't —
you'll pass compilation, then cause real audio dropouts.

## Architecture

### Slot storage: dynamic, not preallocated-per-kind

Each channel's effect chain is an addressable array of *optional, dynamically
allocated* nodes:

```rust
// crates/mooloop-engine/src/render.rs, inside ChannelStrip
effect_chain: [Option<Box<dyn AudioNode + Send>>; MAX_EFFECTS_PER_CHANNEL],
```

Use the complete `u8` slot address space (`MAX_EFFECTS_PER_CHANNEL == 256`) in
`crates/mooloop-core/src/channel.rs`, right next to `MAX_CHANNELS` /
`MAX_PATTERNS`. This is a realtime bridge boundary, not a product cap.

We do **not** preallocate one instance of every effect kind in every slot.
That was considered and rejected: it wastes memory per slot per kind and
doesn't scale as more effect kinds (or a future CLAP host, which can't be
preallocated at all) get added. Instead, an effect instance is constructed
once, on the GUI thread, when the user adds it — same as how a real rack
instrument (think Reason, Bitwig) allocates a device object when you drop it
in, not before.

### Moving a `Box` onto/off the RT thread: two extra ring buffers

Because `Box<dyn AudioNode>` can't ride the existing `EngineCommand` queue,
add a second pair of lock-free SPSC ring buffers (same `rtrb` crate already
used for `EngineCommand`/`EngineEvent`, just carrying a non-`Copy` payload —
`rtrb` does not require `Copy`, only `Send`):

```rust
// crates/mooloop-core/src/bridge.rs — new types alongside EngineCommand/EngineEvent

/// GUI -> audio. Carries ownership of a heap-allocated effect node into the
/// realtime thread. Never carry this in `EngineCommand` — `Box` isn't `Copy`.
pub enum StructuralCommand {
    /// Install `node` at `slot` on `channel`, replacing whatever was there.
    /// The replaced node (if any) comes back via `StructuralReclaim` below —
    /// the RT thread must never drop a `Box` itself (that's a deallocation
    /// on the audio thread, which is exactly what we're avoiding).
    InstallEffect {
        channel: u8,
        slot: u8,
        node: Box<dyn AudioNode + Send>,
    },
    /// Remove whatever is at `slot`, if anything. Also reclaimed, not dropped.
    RemoveEffect { channel: u8, slot: u8 },
}

/// audio -> GUI. Hands back a displaced node so the GUI thread can drop it
/// (deallocate) safely, off the realtime thread.
pub enum StructuralReclaim {
    Node(Box<dyn AudioNode + Send>),
}
```

Wire these exactly like `EngineCommand`/`EngineEvent` are wired today in
`crates/mooloop-engine/src/lib.rs` and `graph.rs` — same `rtrb::RingBuffer`
construction pattern, just a second producer/consumer pair. The RT thread
drains `StructuralCommand` at the same point it currently drains
`EngineCommand` (top of `Graph::process`), and pushes any displaced box into
the `StructuralReclaim` producer. The GUI thread already has a timer polling
`EngineEvent` — drain `StructuralReclaim` there too and just let the `Box`
drop (that's it, that's the whole reclaim step; no special cleanup needed).

**Reordering does not use this channel.** Swapping two array entries
(`effect_chain.swap(a, b)`) moves pointers, allocates nothing, and is safe to
do directly on the RT thread. So reordering stays a plain `Copy`
`EngineCommand`:

```rust
SwapEffectSlots { channel: u8, slot_a: u8, slot_b: u8 },
```

### Parameter changes on a live effect: reuse `Event::ParamValue`, add nothing new

`crates/mooloop-dsp/src/event.rs` already defines
`Event::ParamValue { id: u32, value: f32 }` — a generic, node-defined,
sample-accurate parameter event, unused anywhere today but built for exactly
this. Use it instead of inventing a new command type:

- When the user turns a filter knob, the GUI thread builds a `TimedEvent`
  with `Event::ParamValue { id, value }` (pick small stable `id` constants
  per effect, e.g. `0 = cutoff_hz`, `1 = resonance`) and it flows through the
  channel's existing per-block `EventList` alongside note events, straight
  into `FilterEffect::process`'s `events_in` argument. `FilterEffect` reads
  `ParamValue` events at their sample offset and updates its internal state.
- This means `AudioNode`'s signature does **not** need to change at all.
  Every effect type just handles the `ParamValue` ids it cares about and
  ignores the rest.
- Practically: you'll need a place in the GUI-side event submission path to
  inject one-off `ParamValue` events per channel/slot (separate from the
  sequencer's per-pattern note events). Look at how the sequencer currently
  builds each channel's `EventList` per block in
  `crates/mooloop-engine/src/sequencer.rs` / `render.rs` and find the
  smallest way to merge in a small queue of pending param events per
  channel — a `Vec<TimedEvent>` drained each block is fine, this is
  non-realtime-critical in size (a handful of knob twiddles per block, not
  audio data).

If merging param events into the existing per-channel `EventList` construction
turns out awkward, the fallback is a small additional `Copy` `EngineCommand`
per effect kind (`SetFilterParams { channel, slot, params: FilterParams }`,
mirroring `SetChannelSamplerParams` exactly) applied directly to the boxed
node via a trait method. Prefer the `ParamValue` approach first since it's
more general and avoids one bespoke command per effect kind going forward,
but don't burn more than an hour fighting it before falling back.

### Processing loop

In `RenderState::process_block_inner` (`crates/mooloop-engine/src/render.rs`),
the effects loop already exists and is already correctly placed (after the
generator, before gain/pan) — it's just iterating an empty `Vec` today:

```rust
for effect in &mut strip.effects {
    effect.process(&context, &mut strip.bus, &self.empty_events, None);
}
```

Change this to iterate `effect_chain`, skipping `None` slots, and pass the
channel's real per-block events (needed for `ParamValue` delivery) instead of
`self.empty_events`:

```rust
for slot in strip.effect_chain.iter_mut() {
    if let Some(node) = slot {
        node.process(&context, &mut strip.bus, &self.events[index], None);
    }
}
```

## Core types (`mooloop-core`)

Add to `crates/mooloop-core/src/synth.rs` (or a new `effect.rs` module if
that reads cleaner — follow whichever the existing file organization nudges
you toward):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    Filter,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FilterParams {
    pub cutoff_hz: f32,
    pub resonance: f32,
    // low-pass vs high-pass: an enum field, or a 0..1 blend — your call,
    // just give it a sane Default.
}
```

Persisted per-slot state (`ChannelSetup.effects`, see below):

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectSlotState {
    pub kind: EffectKind,
    pub params: FilterParams, // widen to an enum once a second effect kind exists
    pub bypassed: bool,
}
```

Note: with only one effect kind so far, don't build a tagged `EffectParams`
enum yet — that's speculative generality for a kind that doesn't exist. Wait
until effect #2 is being added, then widen `FilterParams` into an
`EffectParams` enum wrapping per-kind params (mirror `ChannelSource`'s
tag+content serde shape at that point). Keep it concrete now.

Add to `ChannelSetup` in `crates/mooloop-core/src/project.rs:117`:

```rust
pub struct ChannelSetup {
    pub channel: Channel,
    pub source: ChannelSource,
    #[serde(default)]
    pub effects: Vec<EffectSlotState>,
}
```

`#[serde(default)]` is required so existing saved songs (which predate this
field) still load — this is the same back-compat pattern already used for
`MonoSynthParams.lfo`. Don't skip it.

## DSP (`mooloop-dsp`)

New module `crates/mooloop-dsp/src/effects.rs` (or `effects/filter.rs` if you
want room for siblings later — either is fine, don't agonize over it):

```rust
pub struct FilterEffect {
    left: Svf,
    right: Svf,
    params: FilterParams,
    sample_rate: u32,
}

impl AudioNode for FilterEffect {
    fn process(&mut self, ctx: &ProcessContext, bus: &mut StereoBus,
               events_in: &EventList, _events_out: Option<&mut EventList>) {
        // split at ParamValue event offsets, update self.params on each one,
        // run self.left/self.right.next_sample(...) per-frame over bus,
        // same shape as how MonoSynth/Sampler already split at NoteOn/NoteOff.
    }
}
```

`Svf` already exists at `crates/mooloop-dsp/src/filter.rs` — use it directly,
don't write new filter math. Look at how `MonoSynth` (`monosynth.rs`) already
uses `Svf` for its own filter for the exact calling convention.

Update the crate doc comment in `crates/mooloop-dsp/src/lib.rs` (currently
says effects "arrive in later phases") and `pub mod effects;` / re-export.

## Engine (`mooloop-engine`)

- `ChannelStrip::new` initializes `effect_chain` to `[None, None, ...]` (an
  array of `None` — `Default` derive won't work for `[Option<Box<dyn
  Trait>>; N]` directly since trait objects aren't `Default`; just write it
  out with `std::array::from_fn(|_| None)` or similar).
- `RenderState::apply_command` gains a `StructuralCommand` drain step (new,
  separate from the existing `EngineCommand` drain) and handles
  `SwapEffectSlots` in the existing `EngineCommand` match.
- `load_project`/`from_project`: when loading a project, existing effect
  slots need to be rebuilt from `ChannelSetup.effects`. Since project loading
  itself is not a hot per-block path (it happens on transport
  stop/load/import), it's fine to construct the `Box<dyn AudioNode>`
  instances directly here rather than routing through the structural ring
  buffer — check how `InstallProject`/project snapshot loading already works
  in `render.rs`/`lib.rs` to see whether it runs on the RT thread or is
  already special-cased as a non-hot-path operation, and match that.

## Persistence (`mooloop-project`)

No format/envelope change needed beyond the new `ChannelSetup.effects` field
and its `#[serde(default)]`. Don't add an `Effect` `DocumentKind` (mirroring
the existing `Generator` one) unless you finish everything else first and
still have time — it's a nice-to-have for shareable effect presets, not part
of the MVP.

## UI (`mooloop-ui`)

Read `docs/UI_DESIGN.md` before touching any `.slint` file — it's the
acceptance contract for this codebase's UI work, not optional background
reading.

1. New `crates/mooloop-ui/ui/filter-device.slint` exporting
   `FilterDeviceFace`, built from the existing `DeviceFrame` / `DeviceHeader`
   / `DeviceRackMetrics` primitives in `device-rack.slint` — copy the
   structure of `mono-device.slint` or `drum-device.slint` for how a device
   face wires knobs up to Slint properties + `-changed` callbacks.
2. In `crates/mooloop-ui/ui/main.slint`, find the currently-disabled
   `DeviceAddSlot { enabled: false; }` right after the source `DeviceFrame`
   (search for `DeviceAddSlot` — there's exactly one live usage). Make it
   live (`clicked => { root.add-effect-clicked(); }`), and render the
   effect chain between the source device and this add-slot: a repeated
   `DeviceFrame` + `DeviceJoin` per active effect slot, sourced from a new
   Slint model property (e.g. `effect-slots: [{kind: int, bypassed: bool,
   cutoff: float, resonance: float}]`) that the Rust side keeps in sync with
   `UiState`'s project mirror.
3. Rust glue in `crates/mooloop-ui/src/lib.rs`: new callbacks
   `add_effect_clicked`, `remove_effect_clicked(slot)`,
   `set_effect_bypassed(slot, bypassed)`, `filter_cutoff_changed(slot, v)`,
   `filter_resonance_changed(slot, v)`, `reorder_effect(from, to)`. Follow
   the exact pattern the generator callbacks already use: update the local
   project mirror, then send the matching `EngineCommand`/`StructuralCommand`
   to the audio thread. `add_effect_clicked` is the one that constructs the
   `Box::new(FilterEffect::new(...))` and sends `StructuralCommand::InstallEffect`.
4. Drag-to-reorder: there is no existing reorderable-list widget anywhere in
   this codebase — every current drag interaction (knobs, the sequencer step
   grid) is value-editing via `TouchArea`, not list reordering. Build a small
   new component: a `TouchArea` on each effect's `DeviceHeader` that tracks
   press-x, computes a live delta against sibling slot widths on move, and
   fires `root.reorder-effect(from, to)` on release. Model the press/move/
   release handling directly on the step-grid drag code in `main.slint` (grep
   for the step-paint/step-stretch `TouchArea` handlers) — same idiom, just
   producing an index instead of a value. Don't reach for an external Rust
   crate for this; it doesn't exist in this dependency set and isn't needed
   for a single-axis reorder.

## Test/verify

Follow `AGENTS.md`'s proportional-verification rule — this touches
`mooloop-core`, `mooloop-dsp`, `mooloop-engine`, `mooloop-project`, and
`mooloop-ui`, which is cross-crate, so once everything's wired end-to-end run
the full workspace test suite once, not per-commit. Along the way:

- `cargo test -p mooloop-dsp` after `FilterEffect` exists — add a unit test
  that feeds it a known signal (e.g. white noise or a sum of two known
  frequencies) and asserts the high-frequency content is attenuated more
  than the low-frequency content after processing. Look at existing
  `#[cfg(test)]` blocks in `sampler.rs`/`monosynth.rs` for the harness style
  already in use (likely constructs a `ProcessContext`/`StereoBus` by hand).
- `cargo test -p mooloop-engine` after the render loop change — assert a
  channel with a filter installed produces different output than the same
  channel without one, for the same input.
- `cargo test -p mooloop-project` — round-trip save/load a project with a
  non-empty `effects` vec and confirm it survives serialization, and
  separately confirm an *old*-format song file (no `effects` field) still
  loads (this is what `#[serde(default)]` is for — write the test).
- For the UI, use the headless software-rendering path documented in
  `AGENTS.md` ("Checking the UI / taking screenshots") rather than trying to
  eyeball it live — `SLINT_BACKEND=winit-software` plus the playlist
  snapshot test pattern, or the control-gallery example if you add the new
  `FilterDeviceFace` to it.
- Actually running the live app (real JACK audio, turning the filter knob
  and hearing it work) is worth doing once at the end, using the headless
  `agent` Hyprland workspace described in `AGENTS.md` — don't skip straight
  to calling it done on tests alone; hearing it work is the real signal for
  an audio feature.

## What "done" looks like

You can: add a filter after a channel's generator from the UI, hear it
change the sound in the running app, drag it to a different position in a
multi-effect chain (even with just one effect kind, the chain array/reorder
plumbing should work for N slots), save the song, reload the app, and see
the filter (with its params) still there, on a channel that still plays
correctly.

## Explicitly out of scope for this pass

- A second effect kind (delay, reverb, etc.) — build these after the filter
  is fully solid, by copying its pattern exactly.
- The mixer / send-bus system Adam mentioned wanting eventually. Nothing in
  this plan blocks it: keep `FilterEffect` (and every future effect)
  ignorant of channel-strip concepts like gain/pan/mute — it should only
  touch `ProcessContext`/`StereoBus`/its own params, per `node.rs`'s existing
  doc comment about routing living in "the engine's bus management, not this
  trait." That discipline is what makes the same effect reusable later on a
  master bus or a send/return bus without a rewrite.
- CLAP hosting. Also unblocked by this plan: `AudioNode` doesn't change, and
  a future CLAP-hosting node just becomes another kind of
  `Box<dyn AudioNode>` installed the same way through `StructuralCommand`.
  Nothing here needs to anticipate CLAP further than "don't paint yourself
  into a corner," which this design already doesn't.
