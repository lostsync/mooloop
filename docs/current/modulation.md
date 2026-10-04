# Modulation and automation

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## Parameters and the modulation shelf

- `ParamAddr` addresses parameters owned by a source, a rack device, a
  modulator slot, or the strip, within its channel-or-bus scope. A rack device
  is named by a durable `DeviceId` minted when it is inserted, so reordering,
  inserting into or deleting from a chain changes no saved address at all; the
  position is derived from the chain on each read. A modulator is still named
  by slot inside the rack, and a channel by index. The per-channel `ModRack` and clip
  automation resolve through it. They compose rather than compete: a lane
  supplies the base a knob would otherwise supply, and the matrix adds its
  offsets on top, so an LFO wobbles around a drawn curve. Both resolve at the
  32-frame control rate into one curve per destination, handed to the device
  once a block. A device with no curve path of its own (every native device
  but the EQ) receives it as the parameter events it already took, so no
  effect needed a change.
- The channel modulation rack is a shelf pinned to the bottom of the editor
  dock, under the device rack and outside its scroll, collapsed by default.
  It is therefore never wider than the window, however long the device chain
  grows, and the chain's horizontal scrollbar sits above it. Open, it is a
  module grid beside the selected module's full surface. Five module kinds ship — LFO, Envelope, Step, Random, and Math —
  each a descriptor table plus a tick, so a module's parameters automate,
  undo, and persist like an effect's. Capacity is eight modules and sixteen
  routes per channel; the eight is a constant with a measured price rather
  than a layout assumption, and the grid scrolls to whatever it is set to.
  Routes carry durable `ModSourceId`s, so reordering the grid moves a module
  without changing what any route means, and `MoveModulator` remaps the Math
  module's `input_slot` across the same permutation. **A reorder also moves
  each module's running state**: an LFO keeps its phase, its smoothing and its
  fade position, an envelope keeps its stage and level, and a Random module
  keeps its sequence. Arming a module's Assign
  switch makes legal controls assignable; dragging one sets route depth while
  the control keeps its base value. Removing a route restores the
  destination's base, on generator parameters as well as effect ones. The
  envelope's gate input is an explicit channel-note picker — the first
  adapter for a typed generator `Gate` outlet, which does not exist yet.
  A published generator outlet is a source in the same shelf, in its own pane
  beside the modules. Device (effect) outlets, cross-channel sources, and
  macros remain planned.

## Published outlets

- **A device's published outlets can drive other devices.**
  `mooloop_core::outlet` states the vocabulary — control versus audio domain,
  the tap point an audio outlet is taken at, and the one-block latency every
  control outlet carries — and the ML-P8 declares fourteen outlets and
  publishes its seven control values, reduced through the group of its most
  recent note. A modulation route names its source through `ModSourceRef`, so
  it may be a rack module or a generator outlet; both resolve into one flat
  control address space, and an outlet route persists by outlet id. The
  latency is an ordering fact rather than a delay: the control table is filled
  before the strips render, so a route necessarily reads what the generator
  published in the previous block, live and offline alike.
  The shelf offers them: a channel whose generator publishes control outlets
  grows an OUTLETS pane beside its module grid, one named chip per outlet with
  the same live meter a module tile carries. A chip selects and arms like a
  module, so the ordinary assign-then-drag gesture builds an outlet route, and
  the route it writes names the outlet by its durable id. An outlet has no
  editor, because the device that publishes it owns its behaviour; the pane
  beside it shows the declaration instead. A generator that publishes nothing
  has no pane at all rather than an empty one. The audio outlets never appear
  in that pane: they are not control sources, and `OutletDomain` is what
  refuses them structurally rather than a rule the picker remembers. An Aux In
  channel is where they are read instead.
  **The two kinds of source publish in different ranges, and a route's
  polarity is about the module convention.** A rack module emits into
  `-1..1`, and `Unipolar` lifts that into `0..1` so a one-way module rests at
  the destination's base. The lift stands on the module's **own amount**, not
  on the full range: an LFO at half depth swings `-0.5..0.5`, and a unipolar
  route from it still rests on the base and reaches half the route's depth.
  Turning that amount to zero therefore contributes nothing. An LFO still
  fading in is the one case the lift cannot see, because the fade is engine
  state rather than a parameter: for the length of the fade a unipolar route
  from it rises from half the module's depth instead of from the floor. An
  outlet publishes in its *declared* range, where a
  unipolar one is already `0..1`, so an outlet route takes the destination's
  own default — `Bipolar`, which passes the value through. `Unipolar` on an
  outlet remains meaningful, but only for a genuinely bipolar one such as
  ML-P8's `LFO`.

## Automation lanes

- Clip automation is per (pattern, channel), lives in the clip that drew it,
  and may address a bus. Two clips automating one destination is not
  prevented; the lowest channel wins at render time.
- **A lane that stops driving a destination gives the knob back.** Deleting or
  clearing a lane does it, and so does moving the playhead off it: switching
  pattern, switching between pattern and song mode, and seeking all hand every
  destination the outgoing position was driving — and the incoming one is not
  — back to the value its knob shows. Without that a filter goes on playing at
  wherever the last curve left it while its face reads something else.
  **A song-mode clip boundary is the case this does not cover**, because it is
  not a command: the playhead moves out from under a lane on its own, and the
  destination latches until something touches it. That one is in
  `docs/LOOSE_ENDS.md`.

- One automation lane is visible at a time (its picker is described under
  the piano roll, in [sequencing.md](sequencing.md)): several lanes cannot be shown at once, the velocity lane
  is a separate fixed lane rather than one entry in that list, and a pattern
  holds at most eight lanes per channel.
