# Automation curves: status

Plan D from `reports/fable-2026-09-22.md` (finding 3, deepening 09-21's
finding 5). Worked in a single pass by a Sonnet-class agent, 2026-09-22, in a
worktree alongside four other concurrent agents each owning disjoint files.
**No `cargo` command was run in the first pass** (a hard constraint of the
session, not a choice) — everything below was originally written from reading
the code, with every uncertain spot called out for the coordinator's build.

**Update, same day, second pass:** the coordinator's build reported all
"uncertain compile spots" fine, plus 7 pre-existing test regressions, and
unblocked `cargo` for this session to fix them (sole builder, one command at a
time). Root cause, fix, and the final green test runs are in "Round two: the
stale-curve-pool bug" near the end of this file. `mooloop-dsp` (700 tests) and
`mooloop-engine` (315 tests) are both fully green, and `cargo clippy -p
mooloop-dsp -p mooloop-engine --all-targets -- -D warnings` is clean for every
file this plan owns (one unrelated, pre-existing clippy error remains in
`effects/filter.rs`, out of scope -- see that section).

## What landed

**Step 1 — the trait method and the curve type.** `crates/mooloop-dsp/src/node.rs`:

- `pub struct ControlCurve<'a> { pub id: u32, pub values: &'a [f32] }`, one
  value per control tick, `Default` (empty slice) so a fixed-size buffer of
  them can be built without one being live yet.
- `pub const MAX_CONTROL_TICKS_PER_BLOCK: usize = MAX_BLOCK_SIZE /
  CONTROL_RATE_FRAMES;` — moved here from a private constant
  `mooloop-engine/src/render.rs` used to recompute from the same two inputs.
  Both crates now import the one definition; `render.rs` keeps a
  `const _: () = assert!(...)` next to its own former derivation as a
  compile-time note that the two are the same number, since the duplicate
  computation is gone rather than merely equal.
- `AudioNode::apply_curves(&mut self, curves: &[ControlCurve<'_>], tick_frames:
  usize, fallback: &mut EventList)` — **see "Deviation from the literal
  signature" below**, the one place this round did not implement the plan
  literally. The default converts each curve into the same
  `Event::ParamValue` events the engine used to push directly, timed at each
  tick's frame offset, into `fallback`. No node overrides this and behaves
  any differently on day one; the default is exercised by
  `node.rs::the_default_apply_curves_reconstructs_the_events_a_caller_would_have_pushed`.

**Step 2 — the curve pool in `render.rs`, at both sites.**

- `struct CurvePool<const N: usize> { ids: [u32; N], ticks: [[f32;
  MAX_CONTROL_TICKS_PER_BLOCK]; N], count: usize, active_ticks: usize }`,
  with `empty`, `clear(ticks)`, `begin(id) -> Option<&mut [f32;
  MAX_CONTROL_TICKS_PER_BLOCK]>` (refuses past `N` destinations, does not
  drop silently), and `fill(&self, buf: &mut [ControlCurve]) -> usize`
  (writes a caller-owned on-stack array so `apply_curves` can be handed a
  slice).
- `type EffectCurvePool = CurvePool<MAX_EFFECT_CURVE_DESTINATIONS>` where
  `MAX_EFFECT_CURVE_DESTINATIONS = mooloop_core::effect::EQ_DESCRIPTOR_COUNT`
  (50 — EQ's seven bands of six fields plus two pass filters of four, the
  largest of the fourteen effect kinds' tables by a wide margin. Verified
  against every kind, not asserted: `mooloop-dsp`'s
  `no_effect_kinds_descriptor_table_exceeds_the_curve_frames_capacity`.)
- `type SourceCurvePool = CurvePool<MAX_SOURCE_CURVE_DESTINATIONS>` where
  `MAX_SOURCE_CURVE_DESTINATIONS = mooloop_core::ds01::DESCRIPTORS.len()`
  (92 — DS-01's table, the largest of the eight generator kinds').
- **Effects site**: `EffectChain` gained `curve_scratch: Box<EffectCurvePool>`
  and `curve_refusals: u64`. `EffectChain::control_events_for_slot` (was:
  push one `Event::ParamValue` per tick per destination into `event_scratch`)
  now writes each destination's tick values into its own `curve_scratch`
  row via `begin`/refusal-counted, and returns the tick count. The call site
  (`EffectChain::process`) builds a small on-stack `[ControlCurve;
  MAX_EFFECT_CURVE_DESTINATIONS]`, fills it from `curve_scratch`, and calls
  `node.apply_curves(...)` before `node.process(...)` — skipped entirely
  when nothing is driven, the common case. `event_scratch` now holds only
  the once-per-block queued knob/buffer commands
  (`PendingEffectParams::copy_to`), never a per-tick automated value.
- **Sources site**: `RenderState` gained `source_curves: Vec<Box<SourceCurvePool>>`
  (grows in lockstep with `strips`/`events`/`control_outputs` — see
  `ChannelStorage`, `build_channel`, `push_channel`, all three updated) and
  `source_curve_refusals: u64`. The channel-generator descriptor loop
  (`process_block_inner`, the "generator's control events" comment) writes
  into `source_curves[index]` the same way. `ChannelStrip::process` gained
  two parameters: `events` is now `&mut EventList` (not `&`) and a new
  `curves: &[ControlCurve<'_>]`; it calls `node.apply_curves(curves,
  CONTROL_RATE_FRAMES, events)` before `node.process_source(...)` when
  `curves` is non-empty. Since no generator overrides `apply_curves`, this
  reconstructs the exact events they always received, into the exact list
  they always received them on — **day-one behaviour is unchanged for every
  generator**, and the fix (no more competing with the channel's own notes
  for room in a 256-slot list) is latent, ready for the day a generator
  opts in.
- **Deliberately NOT converted**: the generator's *internal route* amounts
  (`Event::SourceRouteAmount`, addressed by a route id rather than a
  descriptor id) still go through `push_ordered` into `events[index]`,
  unchanged, with its original `let _ =`. `ControlCurve`/`apply_curves`'s
  default only knows how to reconstruct `Event::ParamValue`; giving a curve
  a second meaning for a non-descriptor destination is a design decision
  this pass left alone rather than made in passing. Documented in place at
  the loop (`render.rs`, the "Deliberately still event-based" comment).

**Step 3 — `process_curve_split`, and dirty flags for EQ, plate, reverb.**

- `crates/mooloop-dsp/src/effects/mod.rs`: `CurveFrame` (a node's own owned
  copy of one block's curves, since the borrow `apply_curves` receives does
  not outlive that call) and `process_curve_split(node, bus, events_in,
  curves: &CurveFrame, tick_frames, frames)`, the curve-aware twin of
  `process_param_split`. It applies `events_in`'s once-off queued entries
  first (documented and `debug_assert`ed to be at offset zero, per
  `PendingEffectParams::queue`'s own contract), then splits the block at
  each control tick a curve holds a value for, calling `apply_param` at most
  once per tick per parameter. Tested directly against a `Recorder`
  (mirroring the existing `process_param_split` tests) including the
  flagship test described below.
- **EQ** (`eq.rs`) is the one effect wired end-to-end: it overrides
  `apply_curves` (captures into a `CurveFrame`) and its `process` branches
  to `process_curve_split` when that frame is non-empty. Per-parameter
  dirty flags: `dirty: [bool; STAGES]`/`live: [bool; STAGES]` replace the
  old `update_coefficients()` that rebuilt every one of the 19 biquads on
  any single parameter change. `apply_param` now only marks the affected
  band or pass filter dirty; `resolve_dirty` (called from
  `process_range`, not from `apply_param`) redesigns only what is marked,
  and refolds the compact `active` list only if anything changed. Calling
  the resolve from `process_range` rather than `apply_param` is what makes
  it *coalescing*: several parameters changing within one control tick
  (which is exactly what `process_curve_split` and `process_param_split`
  both do — every tick's `apply_param`s, then one `process_range`) resolve
  into one pass, not one rebuild per `apply_param` call.
- **Plate and reverb** (`plate.rs`, `reverb.rs`) got the *dirty-flag half*
  only, not the curve override. Both already dispatched per-parameter in
  `apply_param` (unlike EQ), so there was no cross-parameter rebuild to
  remove; what the old eager scheme still paid for was the `resize()` →
  `rebuild_feedback()` cascade running *twice* when `size` and `decay_s`
  changed within the same control tick (both curve-driven is the common
  case for a reverb whose size is swept and decay tracks it). Both now
  have `size_dirty`/`feedback_dirty`/`damping_dirty`(/`modulation_dirty` for
  reverb) flags, marked by `apply_param` and resolved once per
  `process_range` call by `resolve_dirty`, in dependency order (size before
  feedback, since feedback's RT60 formula reads the `target_len` a size
  change moves). `width`/`predelay_ms`/`diffusion`/`low_cut_hz` are direct
  smoother-target writes with no per-line loop behind them and were left
  eager (no flag needed). Both effects still use `process_param_split`
  (the event path only) for the reason in the next section.

## Deferred, and why (conservatism clause)

**Plate and reverb do not override `apply_curves`.** The dirty-flag work
(the measurable perf payoff for both, per finding 3's "the whole comb bank")
is complete either way, since it lives in `apply_param`/`resolve_dirty` and
runs identically under the event path or a hypothetical curve path. Wiring
the curve override itself was judged the higher-risk, lower-return half for
this pass specifically because — unlike EQ, whose `RangeProcessor` state is
simple (an array of `Biquad`s keyed by a flat stage index) — verifying the
`CurveFrame`/`process_curve_split` plumbing against two more independently
structured effects, without being able to run a single test, was assessed
as more likely to introduce a subtle mis-wiring than to add value beyond
what EQ already demonstrates end-to-end. Wiring them is mechanical (copy
EQ's `apply_curves` override and the `process` branch, verbatim in shape)
and is the natural next step; nothing about the dirty-flag work done here
needs to be redone to add it.

**The generator internal-routes loop** (`Event::SourceRouteAmount`) is
unconverted, as described above.

**No generator overrides `apply_curves`.** All eight are outside this
round's file ownership (`sampler.rs`, `filter.rs`, `monosynth.rs`,
`polysynth.rs`, `mlm1.rs`, `mlp8.rs` were another agent's this round; DS-01
and DrumSynth were not explicitly excluded but were left alone for the same
reason plate/reverb's curve override was — no file in this round's list
holds their `RangeProcessor`-equivalent state, and inventing one to wire a
curve path through was out of scope). The source-side plumbing
(`source_curves`, the descriptor loop, `ChannelStrip::process`'s new
signature) is real and load-bearing today only in the sense that it
reconstructs the identical event stream generators already consume; the cap
removal for sources is latent until a generator opts in.

## Deviation from the literal signature

The plan's own text (both the finding and the numbered step) gives
`apply_curves`'s signature as `apply_curves(&mut self, curves:
&[ControlCurve], tick_frames: usize)` — two parameters, no way back out.
A **default** trait method with that signature cannot do what the same
sentence asks of it ("turns each curve into `ParamValue` events... so every
node compiles and behaves identically"): the events it constructs have to
go somewhere, and the only list `process` will read afterward
(`event_scratch` at the effects site, `events[index]` at the source site)
belongs to the caller, not to `&mut self`. Handed only `&mut self`, a
default implementation has no lawful way to deliver anything.

The three ways to reconcile this, and why the third is what's implemented:

1. **Return `bool`** ("did you consume it") and have the *caller* do the
   fallback conversion when the answer is `false`. Rejected: it makes the
   default do nothing (misrepresenting the doc comment that says otherwise)
   and pushes the "safe default" property onto every call site remembering
   to check the return, rather than onto the trait.
2. **Store curves as node state via a second required method** (no default
   possible) forcing every implementor to add a buffer. Rejected outright:
   this is a removal of the working path for every node that has not opted
   in, which the plan explicitly forbids.
3. **Add `fallback: &mut EventList`** — the engine's about-to-be-used event
   list, passed straight through. The default writes into it exactly what
   the engine used to write there itself; a node with a native path (EQ)
   ignores the parameter and writes nothing. **Implemented.**

Every other part of the contract — the destinations, `tick_frames`'
meaning, "a node that does not override this keeps working unchanged," the
existence of the default at all — is exactly as specified. The full
reasoning is in `node.rs`'s doc comment on `apply_curves` itself, next to
the code it explains.

## The `block_cost.rs` measurement this plan could not add

**Do not add this without reading `crates/mooloop-engine/src/block_cost.rs`
first — the shapes below are written to match its existing helpers
(`loaded_project`, `filtered_project`, `playing_effect_cost`,
`filter_engaged_cost`) but that file was owned by a concurrent agent this
round and was not read past its first ~60 lines.**

A project builder, next to `filtered_project`:

```rust
/// [`loaded_project`], with an EQ on every channel and `destinations`
/// distinct band-gain parameters under continuous LFO modulation each --
/// the case `reports/fable-2026-09-22.md` finding 3 and
/// `docs/plans/automation-curves/00-status.md` are about: several
/// simultaneously driven destinations on one slot, which the old event
/// path made compete for one 256-slot list and the curve pool does not.
fn automated_eq_project(count: usize, destinations: usize) -> Project {
    // Same shape as `filtered_project`: `loaded_project(count)`, then for
    // each channel, `channel.setup.push_effect(EffectSlotState::of_kind(EffectKind::Eq))`,
    // then install an LFO modulator on `channel.setup.modulation` and add
    // `destinations` routes via `ModRoute::to_slot`, each targeting a
    // different EQ band's gain id (`mooloop_core::eq_band_param(band,
    // EQ_BAND_GAIN)` for `band in 0..destinations`), each a `ModPolarity::Bipolar`
    // route at some nonzero depth (0.3-0.5) so every tick actually resolves
    // a moving value rather than a constant one. The device id an
    // `EffectSlotState::of_kind` install mints needs reading from the slot
    // state it returns, the way `render.rs`'s own test helpers
    // (`install_effect_of_kind`, added this round) resolve it for a
    // hand-installed effect -- `push_effect` may already assign one
    // differently, so read what it actually minted rather than assuming
    // slot-position addressing.
}
```

A test, next to `playing_effect_cost`:

```rust
/// Four EQ bands simultaneously automated, before and after Plan D:
/// `render.rs`'s pushes into the shared 256-event `event_scratch` list
/// versus the curve pool. At a block large enough that this exceeds the
/// list's capacity even once (`>= MAX_BLOCK_SIZE / destinations` control
/// ticks a destination, i.e. any block at or above 2048 frames for four
/// destinations), the "before" tree does not merely cost more -- it drops
/// automation silently, which this file cannot see (it measures time, not
/// correctness; `EffectChain::curve_refusals` and the drop this replaces
/// are what a correctness test reads). This is the perf half only: ns per
/// block, on the unfixed tree first, then on this plan's tree, recorded in
/// this status file the way Plan A's own step 1 recorded
/// `scheduling_cost`'s baseline.
#[test]
#[ignore = "measures wall time; run deliberately in release"]
fn automated_eq_cost() {
    let channels = 8;
    println!();
    println!("  {channels} channels, EQ with 4 automated bands");
    println!("  frames   ns/block");
    for frames in [64usize, 512, 2048, 8192] {
        let nanos = per_block_nanos(&automated_eq_project(channels, 4), frames, 200);
        println!("  {frames:>6}  {nanos:>9}");
    }
}
```

Run **on the unfixed tree first** (before this plan's `render.rs`/`eq.rs`
changes are merged, or against `HEAD~1` after they land) and record the
baseline numbers in this file, the way Plan A's own step 1 insists on
("this is the step that makes the rest honest") — then again after, in the
same table. Neither number exists yet; this section is the specification
for producing them, not the numbers themselves.

## Tests added

- `crates/mooloop-dsp/src/node.rs`: `the_default_apply_curves_reconstructs_the_events_a_caller_would_have_pushed`,
  `a_curve_consuming_override_leaves_the_fallback_list_untouched`.
- `crates/mooloop-dsp/src/effects/mod.rs`: `a_curve_lands_its_value_on_every_tick_boundary`,
  `two_curves_at_the_same_tick_share_one_range_split`,
  `a_queued_once_off_event_is_applied_before_the_first_curve_tick`,
  `an_empty_curve_frame_renders_the_block_in_one_call`,
  `many_simultaneous_curve_destinations_do_not_drop_where_the_shared_event_list_would`
  (the flagship correctness test: four destinations at
  `MAX_CONTROL_TICKS_PER_BLOCK` ticks each demonstrated directly against a
  real `EventList` — accepts only ~256 of the 1,024 pushes attempted, the
  exact defect finding 3 names — versus the curve path, which delivers all
  1,024 `apply_param` calls with the right values at the right ticks),
  `no_effect_kinds_descriptor_table_exceeds_the_curve_frames_capacity`.
- `crates/mooloop-dsp/src/effects/eq.rs`: `construction_designs_every_stage_exactly_once`,
  `changing_one_band_only_redesigns_that_band`,
  `changing_the_high_pass_does_not_redesign_the_low_pass_or_any_band`,
  `two_params_changing_at_one_tick_coalesce_into_one_resolve`,
  `the_curve_path_and_the_event_path_agree` (bit-identical output between a
  block driven through `apply_curves` and the same values delivered as
  ordinary timed events).
- `crates/mooloop-dsp/src/effects/plate.rs`: `construction_resolves_every_dependency_exactly_once`,
  `apply_param_alone_defers_the_rebuild`,
  `changing_damping_alone_does_not_rebuild_size_or_feedback`,
  `changing_decay_alone_does_not_rebuild_size_or_damping`,
  `size_and_decay_changing_at_one_tick_rebuild_feedback_only_once`,
  `width_and_predelay_never_touch_the_dirty_flagged_dependencies`.
- `crates/mooloop-dsp/src/effects/reverb.rs`: the same six, plus
  `changing_modulation_alone_does_not_rebuild_the_others` (reverb has a
  fourth dependency plate does not: `modulation_dirty`).
- `crates/mooloop-engine/src/render.rs`: `two_simultaneously_modulated_eq_bands_survive_a_maximal_block`,
  `two_simultaneously_modulated_source_params_survive_a_maximal_block` —
  both end-to-end through a real `RenderState`/`Project`, asserting
  `curve_refusals`/`source_curve_refusals` read zero after an 8192-frame
  block with two simultaneously driven destinations (which the old event
  path, at that block size, could not have carried past the first
  destination alone).
- `event.rs`'s existing `parameter_changes_precede_note_ons_at_one_offset`
  was not touched and was re-read to confirm it still exercises exactly the
  event path this plan leaves intact (queued knob edits, notes, and the
  plugin/CLAP boundary).

The plan's own required test — "drives one destination at an
8192-frame block and asserts nothing is dropped" — is implemented as the
*two*-destination form in both `effects/mod.rs` and `render.rs`, because a
single destination alone exactly fills (does not exceed) the old list's
256-slot capacity at 8192 frames (`MAX_BLOCK_SIZE / CONTROL_RATE_FRAMES ==
MAX_EVENTS == 256`, a numeric coincidence in the existing constants) — see
finding 3's own wording, "one destination emits 256 events and fills the
list alone; **a second is silently dropped**." The two-destination form is
the one that actually demonstrates the defect and its fix; a literal
one-destination version would have passed on the unfixed tree too and
proven nothing.

## Every changed or added public signature

- `mooloop_dsp::node`: `ControlCurve<'a> { id: u32, values: &'a [f32] }`
  (`Clone, Copy, Default`); `MAX_CONTROL_TICKS_PER_BLOCK: usize`;
  `AudioNode::apply_curves(&mut self, curves: &[ControlCurve<'_>],
  tick_frames: usize, fallback: &mut EventList)` (default provided; see the
  deviation section above for the third parameter).
- `mooloop_dsp` crate root (`lib.rs`): now re-exports `ControlCurve` and
  `MAX_CONTROL_TICKS_PER_BLOCK` alongside the existing `node::*` re-exports.
- `mooloop_dsp::effects`: `pub(crate) const MAX_NODE_CURVE_DESTINATIONS`,
  `pub(crate) struct CurveFrame` (`empty`, `capture`, `is_empty`, `curves`,
  `Default`), `pub(crate) fn process_curve_split(...)` — all crate-internal,
  no external API change.
- `mooloop_dsp::effects::eq::EqEffect`: gained a field `curve_frame:
  CurveFrame` and `#[cfg(test)] rebuilds: [u32; STAGES]`; `AudioNode::apply_curves`
  is now overridden. `EqEffect::new`'s behaviour is unchanged (same eager
  full build); its *implementation* moved from an unconditional
  `update_coefficients()` to `dirty = [true; STAGES]` + `resolve_dirty()`.
  No public signature changed; `update_coefficients` (private) no longer
  exists, renamed/restructured into `resolve_dirty`/`mark_dirty`.
- `mooloop_dsp::effects::plate::PlateEffect` /
  `mooloop_dsp::effects::reverb::ReverbEffect`: private methods `resize`,
  `rebuild_feedback`, `rebuild_damping`, (`rebuild_modulation` for reverb)
  renamed to `do_resize` etc. and made side-effect-only; the dirty-flag
  bookkeeping (`resolve_dirty`, `*_dirty` fields) is new and private. No
  public signature changed.
- `mooloop_engine::render` (crate-internal, `pub(crate)`/private — nothing
  here is public API): `EffectChain` gained `curve_scratch: Box<CurvePool<50>>`,
  `curve_refusals: u64`. `EffectChain::control_events_for_slot` now returns
  `usize` (was `()`). `RenderState` gained `source_curves:
  Vec<Box<CurvePool<92>>>`, `source_curve_refusals: u64`. `ChannelStorage`
  gained `source_curves: Box<CurvePool<92>>` (and `build_channel`/
  `push_channel` were updated to match — both are `pub(crate)`, called only
  from within `mooloop-engine`). `ChannelStrip::process`'s signature
  changed: `events: &EventList` → `events: &mut EventList`, plus a new
  `curves: &[ControlCurve<'_>]` parameter, inserted between `events` and
  `source`. `ChannelStrip::process` is private to `render.rs`; its one
  caller (inside the same file) was updated in the same edit.

## Which nodes override `apply_curves` vs. use the default

**Overrides:** `EqEffect` only.

**Uses the default** (every other node in the tree, unchanged behaviour):
all eight generators (`Sampler`, `DrumSynth`, `MonoSynth`, `PolySynth`,
`MlM1`, `MlP8`, `Ds01`, `AuxIn`), and every effect but EQ (`FilterEffect`,
`DriveEffect`, `PreampEffect`, `BitcrushEffect`, `DelayEffect`,
`ReverbEffect`, `PlateEffect`, `GateEffect`, `CompressorEffect`,
`LimiterEffect`, `BufferDevice`, `ContainerEffect`, `ModulationEffect`).

## Uncertain compile spots (prominent, as asked — no `cargo` was run)

Ranked by how worried I am, most first:

1. **`crates/mooloop-engine/src/render.rs`, the borrow shape at both call
   sites** (`EffectChain::process`'s curve_buf/node/event_scratch triple,
   and the source loop's curve_buf/ports/strip/events[index]/aux_scratch
   quintuple). Each argument comes from a *different, directly-named field*
   of `self`, which NLL's disjoint-field borrowing should accept without
   trouble — and the source site's shape (several disjoint-field borrows
   feeding one call) already existed before this change, just with one
   fewer field and an immutable rather than mutable `events` borrow — but
   I have not compiled it. If it fails, the fix is almost certainly
   reordering which borrow is taken first, not a structural rework.
2. **`ChannelStrip::process`'s `&*events` reborrow** (`&mut EventList` ->
   `&EventList` passed to `process_source`). I believe this is a completely
   ordinary explicit reborrow and compiles as written; flagged because it's
   new and load-bearing.
3. **`#[cfg(test)] { ... }` as a block-expression statement** inside
   `resolve_dirty`/the sample-rate branches (eq.rs, plate.rs, reverb.rs). I
   believe attributes on block-expression statements are ordinary stable
   Rust; flagged because I could not compile-check it.
4. **`std::array::from_fn(|_| ControlCurve::default())` typed as
   `[ControlCurve<'_>; N]`** at both render.rs call sites, and the lifetime
   unification with `CurvePool::fill`'s `&'a self -> &'a mut
   [ControlCurve<'a>]`. I believe this is standard lifetime elision/inference
   and compiles; flagged because it is the one place a lifetime is inferred
   across a `fn` boundary rather than written out.
5. **`const _: () = assert!(...)` in render.rs.** Standard stable const-eval
   assert; low risk, flagged for completeness.
6. **Everything in `mooloop-core`** (`EQ_DESCRIPTOR_COUNT`,
   `ds01::DESCRIPTORS`, `eq_band_of`/`eq_pass_of`, `EQ_HIGH_PASS`/
   `EQ_LOW_PASS`, `eq_band_param`/`eq_pass_param`) was read, not edited, and
   every path used was confirmed present and `pub` by grep — low risk.

None of the above touched `block_cost.rs`, `sampler.rs`, `filter.rs`,
`effects/filter.rs`, `monosynth.rs`, `polysynth.rs`, `mlm1.rs`, or
`mlp8.rs`, per the hard constraint. `docs/MODULATION.md`'s precedence table
and two adjoining paragraphs were updated to say the carrier is a curve;
the base-plus-offset rule itself is unchanged.

**All six of the spots above compiled exactly as guessed** -- the
coordinator's build confirmed it and reported zero errors against them.
The seven failures it reported next were behavioural, not compile-time; see
below.

## Round two: the stale-curve-pool bug

**Root cause, one bug in two places.** Both curve pools
(`EffectChain::curve_scratch` and `RenderState::source_curves[index]`) were
only ever `clear()`ed *inside* the "something is driven" branch
(`if modulation.rack.has_routes() || automation.is_some() { ... }` at each
site). The moment a destination stopped being driven at all for a slot or a
channel -- the last route removed, the last lane cleared or switched off,
a device moved out of a slot the route followed -- that whole branch, clear
included, was skipped. The pool kept whatever it held from the last block it
*was* driven, `count > 0` and all, and the call site's `if
self.curve_scratch.count > 0 { apply_curves(...) }` (or the source site's
unconditional call) dutifully handed the node that stale row again, replaying
an automation value the engine had already stopped resolving. Every one of
the seven failures was one symptom of this:

- `a_generator_parameter_reaches_the_device`: the *first* half passed (the
  lane reaches the sampler); the *second* half -- clear the lane, expect
  `drive == 0.0` -- failed, because `strip_source_base()`'s direct restore
  was immediately overwritten by the stale curve replaying `1.0` on the very
  next block.
- `clearing_a_lane_returns_the_destination_to_its_knob`,
  `clearing_a_module_restores_what_it_was_driving`,
  `a_narrow_route_removal_restores_the_destinations_base`,
  `switching_off_an_automated_pattern_hands_the_knob_back{,_while_stopped}`:
  same shape on the effects side -- `clearing_a_module_restores_what_it_was_driving`
  reads `event_scratch` directly and expects *exactly one* restoring event
  (the queued knob push); the stale `curve_scratch` row was contributing a
  second, unwanted one through `apply_curves`'s default fallback.
- `render::footprint::the_render_graph_costs_what_the_project_uses`: not the
  stale-pool bug at all -- three hardcoded `size_of`/total-byte literals
  needed updating for real, deliberate size changes (`EffectChain` +16,
  `ChannelStrip` +80, `SourceCurvePool` added at 94,592 bytes a live
  channel) plus one unrelated, already-landed concurrent change
  (`MlP8` +64 from `docs/plans/filter-coeffs/`, not this plan).

**The fix.** One line at each site, moved to run unconditionally before the
"is anything driving this at all" gate rather than after it:
`EffectChain::control_events_for_slot` now calls `self.curve_scratch.clear(0)`
as its first statement, before either early return; the source loop in
`process_block_inner` calls `self.source_curves[index].clear(0)` immediately
after building `modulation`, before the `if modulation.rack.has_routes() ||
automation.is_some()` gate. Both pools already had a second, later
`clear(ticks)` call inside the driven branch (to set the real tick count when
something *is* driving); that call is now redundant-but-harmless on the path
where it still runs, and is the only clear that runs on the path where it
does not. This is the "safe default" contract's other half, made explicit by
the bug: a curve pool has to go empty on the same schedule the old event push
did (every block, unconditionally), or a node that used to fall silent the
moment nothing drove it starts hearing an echo of the last block that did.

**Footprint literals**, updated with the real, deliberate numbers and prose
explaining each (in the same style the rest of that test already uses):
`EffectChain` 20,552 -> 20,568; `ChannelStrip` 42,376 -> 42,456; `per_live`
gained `+ size_of::<SourceCurvePool>()` (94,592 bytes) and its assertion
60,816 -> 155,488; the final reserved-plus-sixteen-live-channels total 1,437
-> 2,916 KiB. **That last number is worth the coordinator's attention on its
own merits, separate from whether the test passes**: `SourceCurvePool` sized
to DS-01's 92-descriptor table costs 92.4 KiB *per live channel*, all of it
reserved whether or not that channel's source is ever actually modulated.
Sixteen live channels alone doubles the project's reserved footprint. The
bound follows the plan's literal instruction ("size N = the largest number of
driven destinations a channel can have -- derive it, don't guess"), but
whether *every* live channel should pay DS-01's worst case, versus e.g. a
per-kind-sized pool or a smaller shared cap with its own counted refusal, is
a product/architecture call this pass did not have standing to make and is
recorded here rather than decided.

**One unrelated, pre-existing clippy finding, left alone.** `cargo clippy -p
mooloop-dsp -p mooloop-engine --all-targets -- -D warnings` is clean for
every file this plan touched (after fixing three `needless_range_loop`s this
plan's own code introduced, and one `derivable_impls` on `ControlCurve`'s
hand-written `Default` -- now `#[derive(Default)]`). It still fails on one
pre-existing `needless_range_loop` in `crates/mooloop-dsp/src/effects/filter.rs:465`,
inside a test helper, unrelated to automation curves and outside every file
this round was allowed to touch (`docs/plans/filter-coeffs/` owns it).
Reported here so it is not lost, not fixed here.

**Final test results**, both full suites, after the fix:

```
$ cargo test -p mooloop-engine
test result: ok. 315 passed; 0 failed; 17 ignored; 0 measured; 0 filtered out
   Doc-tests mooloop_engine
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo test -p mooloop-dsp
test result: ok. 700 passed; 0 failed; 4 ignored; 0 measured; 0 filtered out
   Doc-tests mooloop_dsp
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## After the plan: the fallback thins (MOO-73, 2026-09-23)

Every device but the EQ still takes its curves through `apply_curves`'s
default fallback, into a 256-event list it shares with the channel's notes.
Sixteen routes and eight lanes at 512 frames therefore still overflowed it,
freezing one destination mid-block and starving the rest. The default now
thins every curve evenly when they would not fit (`fallback_stride`,
`node.rs`), counted back from each curve's last tick so a block still ends
on the resolved value. The ML-P8's internal route amounts, still one event
per tick as recorded above, are thinned the same way against half of the
list's room (`control_tick_stride`, `render.rs`). Every `EventList` counts
its own refusals now, which puts the sequencer's note and choke pushes into
`RenderState::refused_events`. A native curve path per device is still the
structural answer. Thinning only keeps the others correct in the meantime.
