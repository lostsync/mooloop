# Containers plan status

**Finished 2026-09-23 and archived**, when steps 09 and 10 landed.

**Step 06 decided 2026-09-18: build a layer device** (Adam: *"i do want a
layer device"*). **The work order was written 2026-09-21 as steps 07-10** and
Adam ordered it next the same day. Steps 01-05 are unchanged.

Linear: project [Containers and the layer device](https://linear.app/mooloop/project/containers-and-the-layer-device-2a0a16bb6801), one issue per step.

| Step | What | Issue | State |
| --- | --- | --- | --- |
| [07](07-a-branch-is-a-run.md) | One container predicate, latency as a tree, `EffectKind::Layer` landing silent | [MOO-69](https://linear.app/mooloop/issue/MOO-69) | **landed 2026-09-22** — see below |
| [08](08-the-chain-splits-and-sums.md) | Branch buffers, alignment, the sum — the engine | [MOO-70](https://linear.app/mooloop/issue/MOO-70) | **landed 2026-09-23** — see below |
| [09](09-the-rack-draws-branches.md) | Per-branch Level, Mute and Solo in the engine, the list face, and the selected branch drawn to the right. **Rewritten 2026-09-23** around Adam's answer (Bitwig's FX Layer) | [MOO-71](https://linear.app/mooloop/issue/MOO-71) | **landed 2026-09-23** — see below |
| [10](10-the-gestures-and-the-preset.md) | Wrap-as-layer, add/remove a branch, a preset with branches | [MOO-72](https://linear.app/mooloop/issue/MOO-72) | **landed 2026-09-23** — see below. **The plan is finished and archived.** |

**09's answer, 2026-09-23.** Adam chose none of 09's three height options:
*"like container, but with a list (of the layers). pretty much just copy
bitwig"*, with `reference/img/bitwig-fxlayer.png` (untracked, under the
top-level `reference/`) as the mock-up. The layer's face is one rack unit: a
list of branches, each row with a name, S, M and a level meter, a `+` to add
one, and Gain and Mix. The **selected** branch's chain continues to the right
in the rack under a coloured bracket. What that adds beyond 08 is per-branch
solo, mute and level in the engine. **09 was rewritten around it the same
day**: the three controls are descriptors on every container (ids 1-3 after
Mix), which makes a Chain the unit of a branch, and 10's add and remove
gestures became the list's `+` and a row's context menu.

**The one thing 06 priced that turned out not to be true:** it said the span
representation could not express parallel branches, and it can. A branch is a
direct child's *run*, which `wrap_in_container` already walks to validate a
selection. No second representation, no new field on `EffectSlotState`. What
does change, and 02 recorded the opposite in good faith, is that
**`chain_latency` stops being a sum** — the time a signal spends inside a
layer is its longest branch, not the total of all of them.

## Step 10 — the gestures, and a layer is a preset

Landed 2026-09-23. **A layer can be made, filled, emptied and saved from the
rack**, and the plan is finished.

### Where it went

- **Wrap offers a choice.** The rail's wrap button opens a two-row menu,
  Chain or Layer (`DeviceFrame::wrap-requested(kind)`). Wrapping in a layer
  makes a layer of one branch that already has its switches: the run goes
  into a Chain and the Chain into the Layer, two rows installed in turn and
  one undo step (`Session::wrap_effects_in`). A run that already is one Chain
  becomes the branch as it stands. When the nesting cap leaves room for one
  box and not two, the Layer goes straight round the run.
- **A branch row has a menu.** A right press on a list row offers *Remove
  branch*, which takes the head and its whole run as one removal and one undo
  step. The last branch can go too, leaving an empty layer.
- **A layer is a preset.** A run preset headed by any container is saved,
  listed and loaded under its head's kind, so a layer's preset lists on the
  layer's rail. Before this the save, the listing and the preset capture all
  asked `== Chain`.
- **A bank of three**, as runs (`effect_factory::runs`): parallel drum
  compression, clean and distorted, and a three-way filter split, every
  branch a Chain. It is seeded by `seed_effect_run_bank` under its own marker,
  `.factory-runs-v1`. Every install since 07 had already written the layer's
  empty row bank and its `.factory-v1`, so a bank behind that marker would
  never have been written.

### Acceptance

- `wrapping_as_a_layer_makes_one_chain_branch` and
  `removing_a_branch_takes_its_whole_run` (`session/src/effects.rs`): the two
  rows a layer wrap makes and the order the engine mirrors them in, and a
  removal that takes the head and its run, down to an empty layer.
- `the_wrap_menu_reports_the_kind_each_row_offers`
  (`ui/tests/effect_preset_menu.rs`) and `a_rows_menu_removes_that_branch`
  (`ui/tests/layer_face.rs`) click the two new menus and require the
  callback to land with the right kind and the branch head's rack index.
  `popup-close-order` reads the five leads it read before; neither menu is
  among them.
- `the_layer_bank_is_layers_of_chain_branches` (`core/src/effect_factory.rs`)
  and `the_layer_run_bank_seeds_past_the_row_banks_marker`
  (`project/src/factory.rs`): three well-formed layers whose every branch is
  a Chain, written past the marker every install already has, listed on the
  layer's rail and read back unchanged with no repairs.
- Every gesture records one undo step through `record_project_history`, and
  `unrecorded-edit` reads the one pre-existing lead it read before (the
  plugin pump, not this plan).

### What is not done

- Nobody has listened to it (`FOCUS.md`, first on the list).
- MOO-210 (a container's output trim is inert) is open.
- A device dropped between two branches is a leaf branch with no S or M.
  Wrapping it in a chain as it lands is the fix, if that matters in use.
- **Selectors stay unbuilt.** `06` prices one as nearly free once layers exist, and that is still not a reason to build one.

## Step 09 — the rack draws branches

Landed 2026-09-23, built from the rewritten work order. **A layer now draws
like Bitwig's FX Layer**: a one-unit face listing its branches (name, S, M,
meter), a `+` that appends an empty chain branch, Gain and Mix, and only the
selected branch's devices in the rack, under an accent bracket.

### Where it went

- **The controls** are descriptors 1-3 on every container (`Level`, `Mute`,
  `Solo`), fields on `ContainerParams` with serde defaults. Level ramps on a
  sixth `HostRamps` smoother and is applied in `close_run` before the blend
  (`apply_run_level`). Mute and Solo resolve to a per-branch **gate**, a
  seventh smoother on the branch head's slot, aimed and advanced only in
  `finish_branch`. `OpenRun::any_solo` is read when the layer opens its run.
- **The gate is primed, not settled.** Settling a chain when a document
  arrives cannot settle the gate, because its target belongs to the layer, so
  it clears `gate_primed` instead and the first `finish_branch` jumps the gate
  to its target. Without that, a branch saved muted would sound for its first
  five milliseconds, and `a_muted_branch_is_absent_and_a_soloed_one_is_alone`
  (exact equality from frame 0) would fail. The clearing sits in the chain's
  `settle_ramps` and not the slot's, because the slot's runs on **every block**
  for an empty box. The first cut put it there, and an empty branch's mute
  then switched in one sample.
- **An empty chain is a clean branch, so it has a Level too.** An empty box
  used to pass its input untouched and settle every ramp it had. It now
  passes its input at its Level, ramped, unless it is bypassed. Its Mix
  blends the input with itself, which stays the identity it always was.
- **The drawing** is derived in `mooloop-ui/src/layer_view.rs`
  (`rack_view`): which rows a layer hides, each drawn row's next *drawn*
  depth, which boxes close at each drawn row, the branch list, and the
  bracket. It replaced `containers_closing_at`, which answered "where does
  this box end" from the next row, and so would have left a layer whose last
  branch is hidden with no right cap and no output rail.
  `a_chain_with_no_layer_is_drawn_as_before` pins that nothing changes for a
  chain without a layer.
- **The list reads the branch heads' own rows.** `LayerBranchListRow` is
  instantiated in `main.slint`'s layer arm as the face's children, so S, M
  and the meter read `effect-slots[branch.slot]` live, and S and M write
  through `effect-param-changed(branch.slot, 2|3, ..)`. That path already
  records undo. The only new callbacks are `layer-branch-selected` (view
  state, sends nothing) and `layer-branch-added` (one undo step, "Branch
  added").
- **`append_into_container`** (`structure.rs`) is `insert_into_container`
  at the other end of the run, so the list's `+` adds the *last* branch.
- **The chain's face gained a Level knob**, which is a branch's fader.

### What it cost

`EffectSlot` went from 592 bytes to 616, for two more `Smoothed` (Level and
the gate), per occupied slot, pinned by `the_render_graph_costs_what_the_project_uses`.
`ContainerParams` grew by six bytes inside a payload the poly synth's
parameter block floors, so `EffectParams`, `EffectSlotState` and the command
ring did not move (`capacity_no_longer_moves_the_command_ring` still passes
unchanged).

### What the doing found

- **A container's output-trim knob does nothing.** The output rail every
  container draws past its run carries an output trim wired to
  `SetEffectOutputTrim`, and the container arm of `EffectChain::process`
  settles the leaf ramps and never applies it. Found while deciding where
  Level goes, and filed rather than fixed here (open: MOO-210).
- **The export and the live loop differ in the last frame of a pattern**, by
  1.5e-22, where one ends and the other wraps to its top. That is unrelated
  to the layer, and the parity test bounds it rather than ignoring it.

### Acceptance

In `crates/mooloop-engine/src/container_tests.rs`:
`a_muted_branch_is_absent_and_a_soloed_one_is_alone`,
`a_solo_stays_inside_its_layer`,
`a_branchs_level_and_the_layers_gain_scale_what_they_say`, and
`layer_parallel_drums_render_offline_as_they_play`. The last is the case to
play: authored, saved, reopened with no repairs, rendered offline, and
identical to the live loop. It also writes the listening file `FOCUS.md`
names. `a_layer_branchs_mute_solo_and_level_are_continuous`
(`continuity_tests.rs`) holds every switch to the click bound.
`a_container_saved_before_branch_controls_reads_them_at_rest` (`effect.rs`)
pins the old-file defaults. Every pre-09 container render test still passes
bit for bit. In `mooloop-ui`: `layer_view`'s seven (a nested layer's
bracket among them), `layer_face.rs`, which clicks the list and requires every
press to name the branch head's rack index rather than the row's place in the
list, the face agreement test reading the layer arm, and the insert-menu
cover test asking both predicate arms.

**Nobody has listened to it.** It is first on `FOCUS.md`'s list.

## Step 08 — the chain splits and sums

Landed 2026-09-23 in one commit. **A layer now runs its direct children as
parallel branches**: each starts from the layer's input, the shorter ones are
held back to meet the longest, they sum at unity, and the layer's Mix blends
the sum against its dry copy exactly as a chain's does.

**The listening pass** (a drum loop into a clean branch and a Drive →
Bitcrush branch, mix swept) was closed 2026-09-23 on Adam's instruction to
treat outstanding listening passes as done with nothing heard. Nobody has
listened to it. It is still the case to play first.

### Where it went

- **The loop.** `OpenRun` learned `parallel`, the branch now running and
  where it ends. At a layer's row the input is copied to `branch_in[depth]`
  and `branch_sum[depth]` is cleared; the first branch runs on the bus as it
  stands. At the top of every row, after closing the runs that end there, the
  run on top is asked whether its *branch* ends there too -- a branch is a
  whole run, so that is only ever true once everything inside it has closed.
  `finish_branch` delays the bus by the branch's alignment and adds it to the
  sum; `start_branch` restores the input. `close_run` finishes the last
  branch, puts the sum on the bus, and then blends exactly as before.
- **The buffers** hang off `ContainerScratch` as `branches:
  Option<Box<BranchScratch>>`, so a chain of plain boxes still pays 256 KiB
  and a chain that has ever held a layer pays **512 KiB more**, not 2 MiB (the
  work order's figures were four times too high; see the note in 08).
  `ContainerScratch::for_chain` is the one rule for which a chain gets, read
  by the loader and by `publish_container_spans`, and a chain holding the
  smaller one trades up when a layer arrives and hands the old one back to be
  dropped. **The depth cap stays at 4**: 512 KiB per layered chain did not
  need the lever.
- **Alignment** is `EffectSlot::branch_align`, on the branch head beside
  `container_align`, sized by `mooloop_core::branch_alignment` and sent on a
  new `StructuralCommand::SetBranchAlign` for every direct child of every
  layer, after every structural edit. The longest branch gets `None`, so a
  layer whose branches declare nothing allocates no ring at all.
  `EffectSlot` grew from 512 to 520 bytes, paid per occupied slot.
- **The latency walk's `max` arm.** `latency_of_runs` takes the flow, and
  `run_latency` of a layer is its longest branch -- which is both what its
  dry copy waits for and what `chain_latency` declares to the mixer. It
  landed with the render, as 07 said it had to.

### What the doing found

- **The bypass order needed nothing new.** The container bypass arm already
  runs before the generic one (03's fix), and a bypassed layer is not pushed
  on the open-run stack, so no branch boundary is ever asked of it.
  `a_bypassed_layer_is_its_input` is the test that would fail on the unfixed
  order: a layer of a Drive and a pass-through would come out near twice its
  input.
- **A stale branch ring is harmless by construction.** A row that stops being
  a branch keeps whatever `branch_align` it last had, and nothing reads it:
  the loop only asks for it at a layer's branch boundary, and every edit that
  makes a row a branch republishes it. So the publisher sends every branch of
  every layer, `None` included, and nothing tries to clear the others.
- **A layer past the depth cap runs in series.** It cannot open a run, so
  its rows play in the enclosing context one after another -- the same inert
  box `docs/LOOSE_ENDS.md` already records for a chain past the cap, and only
  reachable from a hand-edited file, because every gesture refuses it.
- **Branch rings do not advance while their layer is bypassed**, so the first
  block after un-bypassing a layer with a Drive in one branch replays up to 15
  frames of the other branches from before the bypass. A container's inner
  rings have always behaved this way (a bypassed box skips its run); noted
  rather than fixed.

### Acceptance

In `crates/mooloop-engine/src/container_tests.rs`:

- `a_layer_of_one_branch_is_a_chain` -- a lone Drive, and a whole
  Filter → Drive → Delay chain, as the one branch of a layer and as a chain,
  at mix 1.0 and 0.6, at 64, 128 and 512 frames, sample for sample. It
  replaces 07's `a_layer_running_in_series_is_a_chain`.
- `two_identical_branches_are_exactly_six_db` -- two Filters are one Filter
  doubled, every sample exactly.
- `a_branch_that_declares_latency_does_not_comb_against_one_that_does_not`
  -- a Drive beside a pass-through equals the Drive plus the input delayed 15
  frames, and is shown to be far from the unaligned sum.
- `a_bypassed_layer_is_its_input` -- the input, delayed by the layer's
  declared latency, and the chain's latency unchanged.
- `a_layer_inside_a_chain_inside_a_layer` -- four open runs, the cap, and the
  answer is the Drive plus the input twice.
- `a_layer_allocates_nothing_on_the_callback` -- `CountingAllocator`, 20
  blocks of the nested layers and a `SetBranchAlign` applied between them,
  zero allocations and zero frees.

And `a_layer_declares_its_longest_branch` in `mooloop-core`'s `mixer.rs`
pins the walk and `branch_alignment`.

## Step 07 — a branch is a run, and a layer is a device

Landed in three commits, the first two on 2026-09-21 and the third on
2026-09-22. All three are behaviour-neutral for every project that existed
before them, and the third is the one a musician can see: a **Layer** row at
the bottom of the insert menu, which runs what it holds **in series**,
exactly as a Chain does, until step 08.

- **`96848cb` — one container predicate.** `EffectKind::is_container`,
  `EffectParams::{is_container, container_children, set_container_children}`,
  and every span primitive in `structure.rs` reading them. The sweep found
  four sites the survey had not counted, and the one that mattered is
  `integrity.rs`: its three `EffectKind::Chain` comparisons are the repair
  for a malformed span, so a layer would have opened unrepaired. Also the
  renderer's `container_dry` allocation test, `load_effect_run`'s two guards,
  and the `SetContainerSpan` publish loop in `mooloop-ui`.
  `container_predicates_agree_about_every_kind` sweeps `EffectKind::ALL`;
  flipping `Chain` to `false` fails it with *"Chain answers is_container two
  different ways"*.
- **`d7ec7e4` — latency is a tree walk.** `latency_of_runs` adds sibling
  runs, `latency_of_run` asks what a container holds, and `run_latency` walks
  too. No behaviour change on a serial tree, which is what
  `the_latency_walk_agrees_with_the_flat_sum_on_every_serial_arrangement`
  pins over five shapes. Changing the sibling combine to `max` fails it at
  15 against 30.
- **The third commit — `EffectKind::Layer`.** The fifteenth kind, with the
  persistence, the insert-menu row, the kind index and preset slug, and a
  face. The rack's two local commits from 2026-09-21 (`is-container` on the
  row instead of `kind == 13` seven times, and the test that the published
  row says so) went with it.

### What the third commit found

- **There is no `LayerParams`.** The work order asked for one with "the same
  two fields `ChainParams` has, for the same two reasons". Two structs with
  the same fields meaning the same things is the fault this codebase keeps
  finding, so `ChainParams` became **`ContainerParams`** and both kinds hold
  it. The Rust name is not on the wire -- `EffectParams` is tagged, so a
  chain is `type = "chain"` and a layer `type = "layer"` around the same
  `state` -- and the rename cost no compatibility. `CHAIN_PARAM_MIX` became
  `CONTAINER_PARAM_MIX` for the same reason, and a layer's frozen id table
  is the chain's (`param_id_freeze_tests.rs`).
- **`ContainerFlow` is where `is_container` comes from now.**
  `EffectKind::container_flow` is the one match that tells `Chain` from
  `Layer` (`Series`, `Parallel`), and `is_container` is its `is_some()`. So
  the reason the predicate commit refused a one-variant enum is answered the
  other way round: the enum is not a second spelling of the predicate, the
  predicate is a reading of the enum. `a_chain_runs_in_series_and_a_layer_in_parallel`
  pins which kind answers which.
- **The latency walk does not read it yet, and that is a correction to this
  file.** 07 said the latency arm would change with `Layer`. It cannot while
  a layer runs in series: the walk's number sizes both the channel's
  compensation and the layer's own dry-path ring, and a layer holding two
  Drives that declared 15 frames while the renderer took 30 would comb at any
  partial mix and sit the channel 15 frames late. The `max` arm lands in 08,
  with the render that makes it true. A latency change is still never
  bundled with a *new kind*, which was the bisection argument; it is bundled
  with the behaviour it describes.
- **The container face said "Chain" whatever it was drawing.** Its title
  was a literal in `container-device.slint`, so a layer would have been
  titled a chain. `EffectSlotRow` gained `label` -- `EffectKind::label`,
  published beside `is-container` -- and the face reads it. Telling the two
  apart by `kind == 14` in the markup would have been the comparison the
  `is-container` commit had just retired.
- **The `is-container` commit had quietly taken a guard off.**
  `every_face_reads_its_parameters_by_id` found face arms by searching
  `main.slint` for `if slot.kind == `, and the container's arm stopped
  matching the moment it became `if slot.is-container :`. Its Mix binding
  was then checked by nothing, and the test's tripwire (at least 60 of the
  66 bindings) went on passing at 65. It reads the predicate arm now, once
  per container kind, and asserts there is exactly one.
- **Both container kinds are the same to every span primitive, word for
  word.** `every_container_kind_is_reported_and_moved_the_same_way` sweeps
  the container kinds through `span_problem`'s two malformations and a move
  of a boxed run, and requires identical answers.

### What it cost

Measured on the tree either side of the commit, as `CAPACITY_POLICY.md`
asks of anything that adds to a stored type: **nothing moved.**

| Type | Before | After |
| --- | --- | --- |
| `EffectParams` | 140 | 140 |
| `EffectSlotState` | 160 | 160 |
| `EngineCommand` | 136 | 136 |

A layer's payload is the chain's payload and the discriminant had room, so
the command ring's 136 bytes are still floored by the poly synth's parameter
block, and `capacity_no_longer_moves_the_command_ring` still says so.

### Acceptance

- `a_layer_running_in_series_is_a_chain` (`container_tests.rs`) -- a Filter
  → Drive → Delay run wrapped in a Layer and in a Chain renders sample for
  sample the same at 64, 128 and 512 frames, at full wet and at 0.6, where
  the Drive's 15 frames would comb a misaligned dry path.
- `a_layer_round_trips_through_the_document` -- a layer inside a chain saves,
  reopens as a layer at the same depth with the same mix and span, and
  renders the same.
- `a_layer_is_a_new_tag_on_the_same_state` -- an empty `layer` state decodes
  to the defaults, and a chain written before `Layer` decodes unchanged.
- The two the work order named under other names are the first two
  commits': `container_predicates_agree_about_every_kind` (now also asking
  for a flow) and `the_latency_walk_agrees_with_the_flat_sum_on_every_serial_arrangement`.
- `the_insert_menu_offers_every_kind` clicks fifteen rows and gets fifteen
  kinds back.

### Not in this step

The rack draws a layer as a chain's box, with "Layer" on its face -- the
drawing is 09's, and waits on Adam's mock-up. The wrap button always makes a
Chain, and a layer's preset carries its own row (its mix), not its run: both
are step 10.

## Step 01 — a device is an identity, not a position

Landed on `feat/containers` (2026-09-06). A rack device now carries a
`DeviceId` minted when it is inserted, and every persisted address names that
id instead of a slot number. **Reordering, inserting and deleting rack rows
rewrite no address anywhere.**

In plain terms: dragging a device around the rack used to be an edit to the
song's saved wiring, because a route said "the device in slot 3" and slot 3
kept changing. It now says "device 3", and the position is worked out from the
rack when it is needed. The saved file does not change when you drag.

Where it is:

- `DeviceId` and `EffectSlotState.id` — `crates/mooloop-core/src/effect.rs`
- `ParamOwner::Effect { device }` — `crates/mooloop-core/src/modulation.rs`
- the mint, the derivation and the load-time fill —
  `mint_device_id`, `device_slot`, `slot_of`, `assign_device_ids` in
  `crates/mooloop-core/src/structure.rs`
- the engine's half — `EffectSlot::device` and `EffectChain::slot_of` in
  `crates/mooloop-engine/src/render.rs`

### What `SlotRemap` had left, which the brief asked to be named

**Nothing, and it is gone.** The plan said the answer was a finding rather
than a decision to take in advance; the finding is that the type had no
responsibility that survived. Everything it did was rewrite an address after a
chain edit, and no address is a position any more.

What went with it: `SlotRemap` itself, `SlotRemap::address`,
`structure::retarget_lanes`, `ModRack::retarget_effect_slots`,
`PatternChannel::retarget_lanes`, `Sequencer::retarget_lanes`,
`RenderState::retarget_effect_slots`, and `Session::retarget_effect_slots`
with all five of the in-memory fixups it ran.

What replaced it is one narrower verb, spelled the same way at each layer —
`forget_device` — called on removal and on nothing else, because a removal is
the only chain edit that can still make an address stop meaning something.

`ChannelEdit` stays exactly as it was. A *channel* is still addressed by
position, and a route scoped to channel 4 still has to become channel 3 when
channel 1 is deleted. That is a separate problem and this step did not touch
it. `structure.rs`'s module doc now says so, in place of the argument it used
to carry against durable device ids.

### Three things the doing turned up

- **The two places that authored a slot number rather than deriving one were
  exactly where the brief predicted.** `Session::pending_preset_save` and
  `Session::effect_preset_names` both held positions across an edit and both
  had to be remapped; they are now keyed by `DeviceId` and are not remapped at
  all. `Session::automation_target` was a third, but it derives.

- **A preset had to be stripped of identity on the way out, and refused it on
  the way in.** `take_preset_save` clears the id when it lifts a row off the
  chain, and `load_effect_preset` writes `preset.with_id(existing)` rather than
  `*effect = *preset`. Without the second of those, loading a preset over a row
  would have silently deleted that row's identity and orphaned every route
  pointing at it — a preset changes what a device sounds like, not which device
  it is.

- **`DeviceId` had to be `u32`, which is what forces the wire-format change**
  rather than the nesting the brief expected to force it. `ParamOwner::Effect`
  carried a `u8`, and identity has to be monotonic and never reused, so a
  256-slot chain edited enough times would exhaust the space and hand a
  departed device's routes to a newcomer. `#[serde(alias = "slot")]` is what
  makes the widening free: in a project written before this step, a device's
  position *was* its identity, so both keys name the same device.

### What it cost

Measured rather than assumed, as `docs/plans/archive/modulator-capacity/`
requires of anything that widens a stored type.

`ParamAddr` went from 12 bytes to 16 and `ModRoute` from 28 to 32, because
`ParamOwner::Effect` now holds a `u32` where it held a `u8`. That is 64 bytes
per channel's modulation rack, 16 KiB across the reserved 256 channels — the
same order and the same argument as the last time an address grew, and it is
storage rather than ring traffic.

**The command ring did not move.** `size_of::<EngineCommand>()` is still 136,
still floored by the poly synth's parameter block, and
`capacity_no_longer_moves_the_command_ring` now pins the new numbers with the
reasoning beside them.

### Acceptance

`a_reorder_does_not_change_one_byte_of_the_saved_addresses`
(`crates/mooloop-session/tests/structure.rs`) is the case the plan asked for:
it snapshots the saved routes and lanes of a project, performs a reorder, an
insert and a second reorder, and asserts the saved addresses are identical.
That assertion could not be written under the slot scheme by construction —
there, every one of those edits had to change them.

Beside it, `reordering_effects_leaves_every_address_untouched` and the engine's
`a_route_and_a_lane_follow_their_device_through_a_reorder_and_die_with_it`
both changed from "the permutation ran correctly" to "there was nothing to
permute", which is the difference this step is for.

## Step 02 — the container is a device

Landed on `feat/containers` (2026-09-07). `EffectKind::Chain` is the
thirteenth effect kind. It takes a rack row, an identity, a bypass, a preset
slot and a place in the saved project, and it is **audibly nothing**: a chain
with containers in it renders bit-identically to the same chain with the
containers removed.

In plain terms: the box exists and you can put devices in it. It does not do
anything to the sound yet — that is step 03 — and the point of building it
silent first is that step 03's "at 100% wet it sounds the same" test needs
something to sound the same *as*.

### The representation, and why

A container does not hold its children. `ChainParams.children`
(`crates/mooloop-core/src/effect.rs`) says how many of the rows *after* it are
inside it, and those rows live in the same `Vec<EffectSlotState>`. The tree is
derived from the flat list, the way a device's position is derived from its
identity.

That keeps `EffectParams`, `EffectSlotState` and `EngineCommand` `Copy`, keeps
the engine's `[Option<Box<dyn AudioNode>>; 256]` flat, and — the part that
matters — makes "the rack cannot tell the difference between a container and
a leaf device" structurally true rather than an invariant to be maintained.

`chain_latency` needed no change at all: the children are rows of the same
chain, so the sum already counts them, and `EffectKind::Chain::latency_frames`
is `0` rather than a sum that would count them twice.

### Where it is

- `ChainParams`, `EffectKind::Chain`, `CHAIN_PARAM_MIX` —
  `crates/mooloop-core/src/effect.rs`
- the span primitives and the two invariants — `span_of`, `run_of`,
  `depth_at`, `parent_of`, `span_problem`, `wrap_in_container`,
  `unwrap_container` in `crates/mooloop-core/src/structure.rs`
- the transparent slot occupant — `crates/mooloop-dsp/src/effects/container.rs`
- the transparent *slot* — `EffectChain::is_container` and its branch in
  `crates/mooloop-engine/src/render.rs`
- the face — `crates/mooloop-ui/ui/container-device.slint`

### One thing the doing turned up, and it is load bearing

**A transparent node in a slot is not a transparent slot.** The first version
gave the container an ordinary rack row and let the chain host blend it like
any other device. It failed the null test immediately: the per-slot wet/dry is
an *equal-power* crossfade, so even at unity a transparent node leaks a
`cos(pi/2)` of the dry alongside itself — about 6e-8, inaudible, and not
bit-exact. The buffer device's Follow mode had already recorded exactly this,
and the note was read as a quirk of that device rather than as a fact about
the blend.

So a container slot now skips the leaf blend entirely, which is also the
correct design rather than a workaround: its `mix` is a blend across a *span*,
not across a device, and step 03 is where the chain host learns to apply one.

### Acceptance

`a_container_that_is_not_doing_anything_is_not_doing_anything`
(`crates/mooloop-engine/src/container_tests.rs`) renders a Filter → Drive →
Delay chain, wraps parts of it in containers at three mixes and two depths,
and asserts the master is sample-identical at three block sizes. Drive is in
the chain deliberately: it is the only kind that declares a latency, so a
container that disturbed the compensation plan would show as a shift rather
than as a difference in timbre.

Beside it: a bypassed container does not move the chain in time, a nesting
survives a save and reload and still renders the same, and a `chain` params
table with neither field set decodes to the defaults.

### Not in this step

The rack draws the container's face but not a *frame* around its run, and
there is no gesture to make one — `Session::wrap_effects_in_container` and
`unwrap_container_at` exist and are tested, but nothing in the interface calls
them yet. `EffectSlotRow` already carries `children` and `depth` so that the
face contract crosses `main.slint` once rather than twice. That is step 04.

## Step 03 — the chain mixes

Landed on `feat/containers` (2026-09-07). **A container now has a dry path.**
It keeps a copy of what goes into it, runs its devices, and crossfades the two
on the way out — delaying the copy by the run's declared latency so the blend
does not comb.

In plain terms: this is the wet/dry knob that did not exist. Every device has
had one for a while, but it blends that *one* device against its own input. A
container blends a whole **run** — Drive into Delay into Reverb, mixed once,
against the signal that went in. That is the thing the brief opens by asking
for, stated correctly.

### Where it is

- the run's latency — `mooloop_core::run_latency` (`mixer.rs`)
- the realtime pass — the open-run stack and `EffectChain::close_run` in
  `crates/mooloop-engine/src/render.rs`
- the state it needs — `EffectSlot::container_children` /
  `container_align`, and `EffectChain::container_dry`
- how it gets there — `StructuralCommand::SetContainerSpan`, published by
  `UiState::publish_container_spans` after every structural edit

### One dry buffer per open box, not per box

The plan said one preallocated copy per container, hung on the container's
slot. It is one per **nesting depth**, hung on the chain, and that is strictly
better: what a chain needs at once is a copy per box it is currently *inside*,
not one per box it holds. Ten sibling containers share one buffer; four nested
ones need four. The cap (`MAX_CONTAINER_DEPTH`, four) therefore bounds a real
allocation rather than a data structure, which is the distinction
`CAPACITY_POLICY.md` is about.

A chain that holds no container pays one pointer. The buffers arrive with the
first container a chain is ever given and stay, the way channel storage does.

### Two things the doing turned up

- **The generic bypass branch ran before the container branch**, so a
  bypassed box delayed its own row and then let its run play anyway. Bypassing
  a container has to skip the *run*, so the container branch had to come
  first. Caught by the acceptance test rather than by reading, which is the
  argument for that test being a render comparison rather than a unit check.

- **A container moves a run, and the engine mirrors a reorder one row at a
  time.** One `MoveEffect` could not express it. Rather than invent a
  run-move command, `move_effect_to` now reports the single-row sequence that
  gets the engine's flat chain to the model's order — an insertion sort over
  device identities (`mooloop_core::move_sequence`), which is correct for any
  rearrangement rather than for the ones a container happens to produce, and
  which only reads because step 01 gave devices identities to sort by.

### What it cost

`EffectSlot` 504 bytes to 512 — a byte of span that falls in padding, and the
`Option<Box<IntegerDelay>>` for the dry ring, `None` on every leaf. Per
*occupied* slot. `ChannelStrip` gained eight, the pointer to the dry buffers.
128 bytes across a sixteen-channel project, which does not move the graph
figure at all.

### Acceptance

`crates/mooloop-engine/src/container_tests.rs`, and the two that matter are a
pair:

- `a_container_at_full_wet_is_its_devices` — bit-identical to the same
  devices with no box, at three block sizes. Without this every other claim
  about the control is a claim about nothing.
- `a_container_at_zero_mix_is_its_input_delayed_by_its_run` — identical to the
  same run *bypassed*, which the compensation work already made time
  transparent. The run holds a Drive, the only kind that declares a latency,
  so an unaligned dry path would comb rather than cancel.

Beside them: the mix lands on neither end at 50%, nesting composes, bypassing
a box skips its run without moving the chain in time, and — the one that
states the gap rather than the mechanism —
`a_run_blended_once_is_not_two_devices_blended_in_turn`.

## Step 04 — the rack draws the box

Landed on `feat/containers` (2026-09-07). **Containers are reachable.** A
device's left rail has a wrap button and a container's right rail has an
unwrap button, and every row inside a container wears one bar per level of
nesting across its top edge.

### Two gestures, not five

The plan listed wrap, unwrap, collapse, and a drop target with an inside
edge. Two of those turned out to be enough to make containers usable, and one
of them turned out to already work:

- **Wrap** puts a box around the clicked row's *run*. Around a leaf that is
  one device; around a container it is that whole container, which is how
  nesting is reachable from a single button without a selection model having
  to exist first.
- **Unwrap** takes the box away and leaves what was inside where it is. It is
  what makes `remove_effect`'s "a box goes with its contents" safe to have.
- **Dropping into a run needed nothing.** The existing drag reports a landing
  index, and `move_effect` already decides what that index falls inside — so
  dragging a device onto a row that is already in a box puts it in the box,
  and dragging it past the run's end takes it out. The plan expected a new
  drop target; the model had already answered the question.
- **Collapse is not built.** It is for a container wider than the viewport,
  and until a musician has made one that wide there is no evidence about what
  it should collapse *to*. Recorded rather than guessed.

### Bars, not a box — and then a box after all

`docs/plans/archive/containers/04` asked for a frame around the run. It shipped as one
accent bar per enclosing container across the top of each row instead, and the
reason given was the rack's own shape: that it is a horizontal sheet which
*wraps*, so a run could begin on one line and end on the next, and a rectangle
around it would have to be two rectangles reading as two containers.

**The rack does not wrap.** `rack-row` is a single `HorizontalLayout` with
`alignment: start` inside a horizontally scrolling viewport
(`device-chain-scroll`), and it has never been anything else. A run is always
contiguous, so a box is always one box. The argument was sound and its premise
was not checked against the layout it described — which is the same shape of
mistake as the unreachable insert menu above it, and it survived longer
because the drawing it justified *worked*, so nothing failed.

Adam found it the way the other one was found: by using the thing and saying
the containment did not read. Superseded on `feat/container-ui`
(2026-09-07) by `ContainerEnclosure` in `device-rack.slint` — a real box,
drawn one slice per row, with the head capping the left, the row before a
shallower one capping the right, and an empty container capping itself. The
bars and the left-edge stripes are gone.

The constraint the bars were also carrying still holds and is not affected:
**vertical adjacency means the chain continues.** A layer's branches, if they
are ever built, need a treatment that is not adjacency.

### The box was drawn twice, and the first one was wrong

The first version drew the run as an accent-coloured tray under the rows,
with rails above and below. It read as green everywhere and still did not
read as containment, which is the verdict Adam gave it after a day. The
reason is worth keeping: **a wash under a row of opaque cards is a backdrop,
and a backdrop says "these are related", not "these are inside".** The cards
stayed cards.

So the cards went instead. `DeviceFrame::contained` turns off a row's own
background and border, and `ContainerEnclosure` draws the one that used to be
theirs, once, around the whole run -- the container's own chrome, extended.
What is left inside is rails, headers and faces sitting directly on the
container's surface, which is what a device inside a device looks like.
Nesting steps up the surface ramp rather than through a colour per level,
because the ramp is already the depth vocabulary and does not run out.

### And then a third time, which is the one that is right

The second version dissolved the contained devices' cards and drew the
container's frame around them, which was closer and still not it: the
container's *output rail* was still standing against its own face, in the
middle of the run, so the box read as a surround drawn around the devices
rather than as a device holding them.

Adam settled it with a mock-up. The container's chrome is one object: its
input rail at the head, its output rail **past everything it holds**, and the
recessed space between them. Both rails take the box's colour rather than a
device's, the fill is darker than a device rather than the same, and the top
and bottom borders hug the faces instead of standing clear.

What that needs, and it is the one thing the flat list could not answer from a
single row: **which boxes end here.** `depth` and `children` say what a row is
inside, not what closes at it, and a repeater item cannot walk the model. So
`EffectSlotRow` gains `closing: [int]` -- the rack indices of the containers
whose run ends at this row, innermost first, computed by
`containers_closing_at` in `mooloop-ui`. Each entry draws one
`DeviceOutputRail` after the row, wired to the *container's* index rather than
the row's, because it is that device's control standing at the far end of its
own run.

`DeviceFrame::tail-rail` is what lets a container not draw one against its own
face, and it shortens `content-width` by a rail so the face keeps the space.
The drag had to give something up for it too: `EffectDeviceShell` used to
work out the pitch of the row it was grabbing as "face plus two rails plus a
join", and no row is reliably that any more, so the *cell* publishes its own
width when it sees the grab land on it.

### Every device in the rack wore a container's band for three iterations

`for level[lvl] in [1, 2, 3, 4]` binds `level` to the **value** and `lvl` to
the **index**. The enclosure used `lvl` as the level, so the levels ran 0..3
instead of 1..4 -- and `depth >= 0` is true of every row, so a level-0
enclosure was drawn behind *every device in the rack*, in or out of a
container.

It shipped in the first version of the box and survived two redesigns. It is
also most of what Adam was reacting to each time: the first complaint was
"this green stuff everywhere", and it was everywhere because a full-width
accent band was being drawn across the whole rack. Both redesigns treated
that as a matter of taste and restyled the band, which made it quieter
without making it correct.

What let it hide: the band was drawn behind opaque device cards, so all that
showed of it was a line above and below each row -- which is exactly what the
correct drawing looks like for a row that *is* in a container. It only became
unmistakable when a device outside every container still had one, and Adam
pointed at that line and said it should not be there.

The lesson is narrower than "check your loops": **a wrong thing that looks
like a plausible thing gets restyled instead of fixed.** Two rounds of
judgement went on the appearance of a band whose existence nobody had
questioned.

### An emptied box was a sealed box

Dragging the last device out of a container left something that could not be
refilled by dragging anything back in. Not an oversight in the drag: an empty
container's span is `c+1..c+1`, which contains no index at all, so
`resize_enclosing` had nothing to grow and there was no `to` anywhere on the
chain that meant "inside this box".

It is the same ambiguity `insert_into_container` exists for, and it has the
same answer: the *gesture* says what the index cannot.
`move_effect_into_container` is the drop's version of that operation, and
`Session::move_effect_to` routes to it when the row dropped on is an empty
container. Deliberately only when it is empty -- for a box that still holds
something, its own row goes on meaning "before this box", because its
children are there to be aimed at and taking that index away would leave
"just before a container" with no gesture of its own.

### What the box needed from Rust: nothing

`EffectSlotRow` already carried `children` and `depth` — step 02 sent them
across `main.slint` early, for the frame step 04 was going to build. The one
extra fact a box needs is where a run *ends*, and a Slint repeater item can
index its own model: `effect-slots[index + 1].depth` says whether the row
after this one is shallower, which is exactly the right cap. So the drawing
that step 04 called too expensive to be worth it cost one component and no
change to the face contract at all.

### Shipped unreachable, and fixed the same day

**The first cut of this step had no way to add a container at all**, which
Adam found by looking for one. Three faults, and they compound:

- `EffectTypeMenu` is a hand-written list in `device-rack.slint` and
  `EffectKind::ALL` is a list in Rust, with nothing holding them together. The
  thirteenth kind was never added to the menu, so the obvious route did not
  offer a container -- and on a chain with no devices there was nothing to
  wrap either, so there was no route at all.
- `+` on a container inserted *before* the box rather than into it, so even a
  container made by wrapping could not be filled from its own button.
- An empty container's span was empty, so no index counted as inside it. An
  inserted container was a dead end.

The third one took two attempts and the first was wrong in an instructive way.
Making `resize_enclosing` treat "the position just after a container's row" as
inside it fixed the symptom and left two rules disagreeing: `span_of`,
`depth_at` and `parent_of` still called that position outside, so wrapping the
row after an empty box silently pulled an unrelated device two levels into a
box it had never been in. The ambiguity is inherent -- position `N+1` means
both "first device inside the box at `N`" and "next device after the box", and
for an empty box those are one integer -- so it is an *operation* now rather
than an index: `insert_into_container` names the box, and the span rule stays
strict everywhere. `the_row_after_an_empty_container_is_not_inside_it` fails
on the version that did not.

The guard is `the_insert_menu_offers_every_kind`
(`crates/mooloop-ui/tests/effect_preset_menu.rs`), which clicks the menu's
last row and asserts it reports `EffectKind::ALL.len() - 1`. It fails for any
kind that exists in Rust and cannot be put on a chain, which is the class of
bug rather than the instance. The lesson is the one this repository keeps
learning: a hand-written list that mirrors an enum needs a test that says so,
because every other test in the suite passes while the feature is unreachable.

### Acceptance

`wrapping_and_unwrapping_leave_every_device_where_it_was`
(`crates/mooloop-session/src/effects.rs`) is the round trip: wrap a device,
wrap the container that made, check every original device is still on the
chain in order and at the depth it should be, then unwrap the inner box and
check its child stayed. `a_container_draws_its_run_and_its_nesting`
(`crates/mooloop-ui/tests/source_snapshot.rs`) renders a real rack holding a
nested pair, which is what fails if `main.slint` stops passing `children` and
`depth` or the container branch stops matching its kind — a container that
drew as an empty frame would pass every other test in that file.

Not verified in the live window. `scripts/mooloop-mcp` needs a real window and
this machine's headless path is a separate setup; the drop-target behaviour in
particular is an interaction, and a musician should put a box round something
before anyone claims it feels right.

## Step 05 — a container is a preset

Landed on `feat/containers` (2026-09-07), **with half of it blocked and the
reason recorded rather than worked around.**

A container saves as one preset: the box, everything in it, in rack order,
with every host control. It loads onto any other container on any chain in any
project, and loading it twice onto the same chain gives two independent runs.
`contains = ["effect_params", "effect_run"]` — an entry **added** rather than
`effect_params` redefined, which is the condition
`docs/plans/archive/preset-system/00-status.md` set on having built the one-row preset
first, and it holds: a reader that predates runs meets `effect_run`, does not
know it, and refuses the bundle instead of loading the first device and
silently dropping the box.

### What is blocked, and why it is not a shortcut

The step asked for *"a container holding two devices and a modulator route
between them"*. **The route cannot travel, and building it would mean deciding
something the brief explicitly reserves.**

A route's source is a module in the *channel's* modulation rack, not in the
container. A run saved with its routes would load onto a channel whose rack
has no such module, so the route would resolve to nothing — the preset would
carry a promise it could not keep. Making it keep the promise means the
container carries the modulator modules too, which means a modulator can live
in a container, which `reference/CONTAINERS.md` records under "what this brief
deliberately does not decide": *"Adam has not settled this and it does not
block the container work. Design containers so that becomes possible; do not
build it."*

So the run travels and the modulation does not (open: MOO-160). `EffectRun` is a struct with
one field for exactly this reason, and `contains` is a list for exactly this
reason: when the modulator question is answered, both grow rather than change.
The ML-M1 bank's complaint — that a device preset cannot carry the modulation
that makes the patch mean anything — is therefore **not closed by this step**.
It is now one decision away from being closable, rather than one architecture
away.

### Where it is

- `EffectRun` — `crates/mooloop-core/src/effect.rs`
- `replace_run` — `crates/mooloop-core/src/structure.rs`, one function
  because the boxes around a replaced run lose one length and gain another
- `save_effect_run_preset`, `EFFECT_RUN_PRESET_CONTAINS`,
  `DocumentKind::EffectRun` — `crates/mooloop-project/src/lib.rs`
- `repair_effect_run` — refuses a headless or straddling run rather than
  flattening it, which is the opposite of what a *song* gets and deliberate:
  a song with a malformed span still holds work in the right order, and a
  preset that cannot describe a box is just a file
- `Session::load_effect_run` — `crates/mooloop-session/src/effects.rs`

### Acceptance

`a_container_saves_as_one_preset_and_lands_twice_independently`: a box holding
a bypassed filter and a dialled-in drive, saved, reloaded, and loaded **twice**
onto a bus — six devices, six distinct identities, host controls intact. Twice
on purpose: two copies sharing identities would be two rows no route or lane
could tell apart, and that is exactly what carrying the preset's own ids would
produce. Beside it, a headless run and a non-container target are both refused
with the chain unchanged.

## Step 06 — layers and selectors

**Cannot be executed by an agent, and that is what the step says.** It builds
nothing; it closes when chain containers have been lived in and Adam has ruled
on whether layers are worth un-parking parallel routing. The page prices them:
N branch buffers instead of one dry copy, branch alignment to the longest,
a second representation in `EffectSlotState` because a span cannot describe
parallel paths, and a drawing problem step 04 closed off by making vertical
adjacency mean "the chain continues".
