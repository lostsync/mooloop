# Modulation and automation

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## Parameters and the Modulation pane

- `ParamAddr` addresses parameters owned by a source, a rack device, or the
  strip, within its channel-or-track scope. A rack device is named by a
  durable `DeviceId` minted when it is inserted, so reordering, inserting
  into or deleting from a chain changes no saved address at all; the
  position is derived from the chain on each read. A channel is still named
  by index, so routes and lanes are rescoped on every channel and track edit.
  Song modulation and clip automation resolve through it. They compose
  rather than compete: a lane supplies the base a knob would otherwise
  supply, and the matrix adds its offsets on top, so an LFO wobbles around a
  drawn curve. Both resolve at the 32-frame control rate into one curve per
  destination, handed to the device once a block. A device with no curve
  path of its own (every native device but the EQ) receives it as the
  parameter events it already took, so no effect needed a change.
- **Modulation belongs to the song, not to a channel.** The song holds one
  set of modules and routes (`SongModulation`), with no limit on how many of
  either. A route reaches any channel's generator, inserts and strip, and
  any track's inserts and strip, the master's included, so one LFO can move
  a filter on the kick and a send return's EQ at once. The modules tick once
  per control tick, in list order, before anything renders.
- **The Modulation pane** is a view of its own: **View > Show Modulation**
  or **Ctrl+6** (`view.pane-modulation`). It opens in the bottom slot and
  moves between panes like any other view. It holds the song's patch canvas:
  boxes where they were put, `gate` tags at the left edge, a tag under each
  route naming its parameter, device and depth, and the wires; drag between
  jacks to wire, click an outlet to arm Assign, click an inlet to pick what
  feeds it, marquee and drag to move, Delete to remove. The canvas is larger
  than the pane and scrolls. Beside it, the OUTLETS of every channel that
  publishes some, under the channel's name; an **Add** list on the right;
  the selected module's surface, with its name edited in its header; and
  beside it the selected source's routes from the whole song, each naming
  its chain, device and parameter ("Kick Filter 1 · Cutoff"), with polarity
  and remove. Five module kinds ship (LFO, Envelope, Step, Random and Math),
  each a descriptor table plus a tick, so a module's parameters undo and
  persist like an effect's. Selection and arming hold the source itself, so
  they survive a channel change, a reorder and a new module.
- **A module's input is picked from a list**: none, then every channel's
  notes for the LFO's reset, the Envelope's gate, the Step's advance and
  the Random's trigger, or every other module in the song for Math. A new
  module is named for the channel it was made on and keeps that channel as
  its home seat, which a channel preset reads.
- Routes carry durable `ModSourceId`s, so reordering the grid moves a module
  without changing what any route means. **A reorder, and any other edit,
  keeps each module's running state**: an LFO keeps its phase, its smoothing
  and its fade position, an envelope keeps its stage and level, and a Random
  module keeps its sequence. An Envelope whose input changed releases.
- **Assigning.** Arming a module's or outlet's **Assign** makes legal
  controls assignable on whatever chain the rack shows, any channel's or
  track's; dragging one sets route depth while the control keeps its base
  value, and re-dragging retunes the route. Route-count dots and armed
  depths count the song's routes onto each control. Removing a route
  restores the destination's base, on generator parameters as well as
  effect ones.
- **Copying a channel copies its routes.** A pasted or cloned channel gets
  the routes that point into the original, aimed at the copy, from the same
  modules, so one LFO drives both; routes from its own outlets and keyboard
  follow the copy. A channel preset carries the modules its routes use and
  adds them to the song as new ones; a kit replaces the song's modulation.
- **Not built yet:** a fader or pan control does not arm a route (a route
  onto a strip plays, but cannot be made by dragging); clicking a face's
  route dot does not select its module or reveal the pane; and a route onto
  a plugin parameter is named only while its channel is selected, reading
  "Unavailable destination" otherwise, though it still plays. Device
  (effect) outlets and macros remain planned.

## Published outlets

- **A device's published outlets can drive other devices.**
  `mooloop_core::outlet` states the vocabulary — control versus audio domain,
  the tap point an audio outlet is taken at, and the one-block latency every
  control outlet carries — and the ML-P8 declares fourteen outlets and
  publishes its seven control values, reduced through the group of its most
  recent note. A modulation route names its source through `ModSourceRef`, so
  it may be a module or a generator outlet; both resolve into one flat
  control address space, and an outlet route persists by outlet id. The
  latency is an ordering fact rather than a delay: the control table is filled
  before the strips render, so a route necessarily reads what the generator
  published in the previous block, live and offline alike.
  The Modulation pane offers them: every channel whose generator publishes
  control outlets has a group in its OUTLETS list, under the channel's name,
  one named chip per outlet with the same live meter a module tile carries. A chip selects and arms like a
  module, so the ordinary assign-then-drag gesture builds an outlet route, and
  the route it writes names the outlet by its durable id. An outlet has no
  editor, because the device that publishes it owns its behaviour; the pane
  beside it shows the declaration instead. A generator that publishes nothing
  has no group at all rather than an empty one. The audio outlets never appear
  in that list: they are not control sources, and `OutletDomain` is what
  refuses them structurally rather than a rule the picker remembers. An Aux In
  channel is where they are read instead.
  **The two kinds of source publish in different ranges, and a route's
  polarity is about the module convention.** A module emits into
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
