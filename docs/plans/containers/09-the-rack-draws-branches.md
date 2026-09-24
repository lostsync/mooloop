# 09 — The rack draws branches

Rewritten 2026-09-23 around Adam's answer, which chose none of the three
height options the first version of this step offered:

> like container, but with a list (of the layers). pretty much just copy
> bitwig.

The mock-up is Bitwig's FX Layer (`reference/img/bitwig-fxlayer.png`, under
the top-level `reference/`, untracked). What it shows, and what this step
builds:

- **The layer is one rack unit tall.** Nothing grows vertically and no face
  is scaled. The first version's three options (the rack grows, the branches
  shrink, tabs) are moot.
- **Its face is a list of its branches**, one row each: a name, **S**, **M**
  and a small level meter. A `+` sits under the list. **Gain** and **Mix**
  sit beside it.
- **The selected branch's chain continues to the right in the rack**, as
  ordinary faces, under a coloured bracket spanning that branch's devices.
  Selecting another row shows that branch instead. Every branch stays
  visible, soloable and mutable at a glance in the list, which answers the
  objection the first version made to tabs.

Linear: MOO-71. The gestures that make and edit a layer are 10's (MOO-72);
this step builds the controls, the engine behind them, and the drawing.

## What it adds that 08 did not build: solo, mute and level

A branch gets a **level before the sum**, a **mute**, and **solo within its
layer**. These are engine parameters, not drawing: stable ids, undo,
persistence, a 5 ms ramp on every move, and one code path for the realtime
and offline renders.

### Where they live: on the container, as descriptors

The decision is that **every container carries three more parameters beside
its Mix**, in `ContainerParams`, with ids after `CONTAINER_PARAM_MIX`:

| id | Name | Curve | Default | Stored as |
| --- | --- | --- | --- | --- |
| 0 | Mix | Linear 0..1 | 1.0 | unchanged |
| 1 | Level | `Fader` (the mixer's taper) | unity | linear gain |
| 2 | Mute | `Stepped(2)` | off | bool |
| 3 | Solo | `Stepped(2)` | off | bool |

Why the container, and not the other two places they could go:

- **Not an array on the layer, indexed by branch.** A branch's position
  changes whenever a branch is added, removed or dragged, so a lane or a route
  on "branch 2's level" would follow the position rather than the branch,
  which is the address problem step 01 spent a whole step removing. It would
  also put per-branch arrays into `EffectParams`, which is `Copy` and sizes
  the command ring.
- **Not host fields on every slot, beside wet/dry and the trims.** Those have
  no descriptor id, so nothing could automate, modulate or MIDI-learn them,
  and a face knob would need its own callback instead of
  `effect-param-changed`. "Real parameters with stable ids" rules it out.
- **On the container, the id is `ParamAddr::effect(branch device, 1..3)`.**
  It is stable through every reorder, because it is the branch's own device
  identity, and every existing mechanism (the face's `effect-param-changed`,
  gesture undo, the saved `ContainerParams`, automation and modulation
  addressing) carries it with no new plumbing.

That makes a **Chain the unit of a branch**, which is Bitwig's model too:
each of its layers is a chain. A branch of more than one device is already a
Chain (a branch is one direct child's run, 07). The gestures in 10 make every
new branch a Chain, including a one-device one.

A **leaf** directly inside a layer is still a legal branch (a hand-edited
file, or a device dropped between two branches). It sums at unity and has no
controls of its own, and a sibling's solo silences it like any other branch.
Its list row shows no S or M. That is a cost of the decision, recorded rather
than hidden. 10 can wrap a dropped leaf in a Chain as it lands.

### What each one means

The rules are chosen so that one reading covers every container. There are no
separate meanings for "inside a layer" and "outside a layer":

- **Level scales a container's run before its Mix blend.** For a layer that
  is the sum of its branches, which is Bitwig's Gain: parallel compression is
  Gain and Mix on the layer. For a Chain it is the run's output. In a branch
  whose Mix is at 1.0 (the default) that is the branch's fader. A layer's face
  labels it **Gain**, and a chain's labels it **Level**. It is one parameter
  under two captions, the way Bitwig captions it.
- **Mute and Solo gate a branch into its layer's sum.** They are applied
  where a branch joins the sum (`finish_branch`), after its own blend, so a
  muted branch contributes nothing at any Mix. **Outside a layer they are
  inert.** A plain chain's face does not draw them, and a lane drawn on one is
  a lane on a control that does nothing there, the same as a chain past the
  depth cap.
- **Solo is within the layer.** When any branch of a layer is soloed, every
  branch that is not soloed is silent. A solo in one layer does not touch
  another layer, nor the rest of the rack.

### In the engine

- **Level** ramps through a sixth `HostRamps` smoother on the container's
  slot, aimed from its `base_params` exactly as Mix already is, and applied in
  `close_run` to the run's output before `blend_run`. At unity and settled it
  is skipped, which keeps every existing container render bit-identical.
- **Mute and solo** resolve to one gate per branch, 0 or 1, ramped by a
  seventh smoother on the **branch head's** slot. The layer's row learns
  whether any of its direct children is soloed when it opens its run (a walk
  of its direct children's `base_params`, bounded by the span), and carries
  the answer on `OpenRun`. `finish_branch` sets the head's gate target from
  that and from the head's own mute and solo, then advances it per frame
  while adding the branch to the sum.
  The gate is aimed and advanced **only** in `finish_branch`. A row that stops
  being a branch keeps whatever gate it last had and nothing reads it. It is
  the same argument 08 made for a stale `branch_align`, and it means a muted
  chain outside a layer never leaves a ramp travelling that would keep the
  chain awake. The gate is left out of `ramps_settled` for the same reason.
- **Realtime and offline** run the same `EffectChain::process`, so parity is
  a render comparison and not a second implementation.

### In the document

`ContainerParams` gains `level`, `mute` and `solo`, each `#[serde(default)]`,
so every song and preset written before this step reads as unity, unmuted,
unsoloed. `PROJECT_FORMAT.md` records the three fields. The layer's and the
chain's frozen id tables (`param_id_freeze_tests.rs`, Control's) grow by the
three ids. The size tables in `00-status.md` are re-measured, because
`ContainerParams` grows by six bytes inside a payload that the poly synth's
parameter block floors.

## The drawing

### The layer's face

One rack unit, `LayerDeviceFace` in a new `layer-device.slint` (Effects'),
drawn for `EffectKind::Layer` where `ContainerDeviceFace` is drawn today. The
chain keeps `ContainerDeviceFace`, which gains its Level knob.

- **The list**, one row per branch, in rack order: the branch's name (its
  preset name if it has one, otherwise the label of the first device inside
  it, otherwise its kind), **S** and **M** toggles, and a level meter. The
  selected row is highlighted. Clicking a row selects that branch.
- **S, M and the meter read the branch head's own row** in `effect-slots`
  (`effect-slots[branch].p2`, `.p3`, `.output-*-db`), not a copy. The list
  carries only which rows are branches. So a mute pressed in the list, or
  undone, or automated, redraws without anyone remembering to republish the
  layer's row. S and M write through the existing `effect-param-changed`,
  with the branch head's index and ids 3 and 2, so they are undoable by the
  path every knob already takes.
- **`+`** under the list adds a branch (10 wires it).
- **Gain** (id 1) and **Mix** (id 0) knobs beside the list.

### Only the selected branch is in the rack

`EffectSlotRow` gains the facts a row cannot read off itself, all derived in
`mooloop-ui` beside `containers_closing_at`, and none of them session state:

- **`hidden`**: the row lies in a branch of some layer that is not that
  layer's selected branch. The cell draws nothing and takes no width.
- **`branches`**, on a layer's row: the rack indices of its direct children,
  with each one's name and whether it has controls (is a container).
- **`selected-branch`**, on a layer's row: the rack index of the branch shown.
- **`bracket`**: whether the row lies in the selected branch of its innermost
  layer, and whether it is that branch's first or last row, for the coloured
  bracket across the top of the branch's devices.
- **`next-depth` becomes Rust's**, and **`closing` becomes visible-aware**.
  Both answered "where does a box end" from the *next row*, and with hidden
  rows the next row may not be drawn. So both are asked of the next *visible*
  row. A layer whose last branch is hidden still caps its box and draws its
  output rail after the last row that is drawn.

Which branch is selected is **view state**: `UiState`, keyed by the layer's
`DeviceId` and holding the branch head's `DeviceId`. It is not saved, not in
undo, and never crosses to the engine. Selecting is looking, not an edit
(`dupe-audit navigation-sends`). A layer with no remembered selection, or one
whose remembered branch has gone, shows its first branch.

### The bracket

Bitwig draws a teal bar over the selected branch's devices. Here it is an
accent bar in the enclosure's top margin, one slice per row like
`ContainerEnclosure`, with the first and last rows turning its ends down. It
is drawn by the rows of the selected branch, whatever their depth inside it,
so a Chain branch's own box sits under the bracket.

### What is already there to build on

- `ContainerEnclosure` (`device-rack.slint`): the box, drawn one slice per
  row. The head caps the left, and the row before a shallower one caps the
  right.
- `EffectSlotRow.depth`, `.children`, `.closing`, and `containers_closing_at`
  (`mooloop-ui/src/lib.rs`).
- `DeviceFrame::contained` and `tail-rail`.
- `mooloop_core::layer_branches`, the direct children of a layer, which the
  engine's alignment publisher already walks.

## The bug this step is most likely to repeat

`for level[lvl] in [1, 2, 3, 4]` binds `level` to the **value** and `lvl` to
the **index**. The enclosure used `lvl`, so every device in the rack wore a
level-0 container band for three iterations, and it hid because it was drawn
behind opaque cards. The list is a `for branch[i] in slot.branches`, and the
same trap is there: `i` is the row's position in the list, and `branch.slot`
is the rack index every callback wants.

**Instrument rather than reason** when a layer does not look right. `04`'s
three rounds went on appearance while the defect was arithmetic.

## Acceptance

- **The engine**, in `container_tests.rs`:
  - a branch at Level −6 dB sums at exactly half its amplitude;
  - a muted branch contributes nothing, and a soloed one silences its
    siblings only, not a second layer;
  - a Mute toggled mid-render ramps rather than steps (the block-edge jump is
    bounded, as MOO-108's continuity checks bound a bypass);
  - Level, Mute and Solo at their defaults are bit-identical to the tree
    before this step, for a chain and for a layer;
  - the parallel-compression case (a drum-like loop into a clean branch and a
    Drive → Bitcrush branch, the layer's Mix swept) renders sample-identical
    through the realtime loop and `OfflineRenderer`, and is measured: branch
    levels, and the S and M effect.
- **The document**: the three fields round-trip, and a `ContainerParams`
  table written before them reads as unity, unmuted, unsoloed.
- **The drawing**:
  - a layer of two branches draws its list with both. The rack shows only the
    selected branch's devices, with the bracket over them, and the device
    after the layer is drawn after its output rail;
  - selecting the other row swaps the branch shown and sends nothing to the
    engine;
  - a chain inside a branch reads as inside the branch, not as a third
    branch;
  - an empty layer caps itself, the way an empty container already does.
- `slint_face_agreement.rs` covers the layer face, and
  `scripts/dupe-audit unchecked-face` reports it as seen. `AGENTS.md`: the
  list is hand-written for a stated reason, and the failure it keeps having is
  a face nobody extended it to.
- **The case to play** is added to `FOCUS.md`'s listening list with its render
  command. An agent cannot hear it. Nobody has, and nobody will be said to
  have until Adam does.
