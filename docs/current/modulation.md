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
  route naming its parameter, device and depth, song inlet tags, and the
  wires; double-click
  empty canvas to type a box (`lfo tri 1/4`, `counter 4`, `* -0.5`; a
  completion list offers the vocabulary) and double-click a box to retype
  it; drag between jacks to wire, click an outlet to arm Assign, click an
  inlet to pick what feeds it, marquee and drag to move, Delete to remove.
  A name the vocabulary lacks makes a box outlined in red that keeps its
  text and does nothing. The canvas is larger than the pane and scrolls.
- **A box's settings are on its face** (song patch step 05). The arrow at
  the right of a box opens it in place: a small knob and readout per
  setting, a stepped one (a shape, a division, a mode) clicking through its
  values. A knob drag is one undo step, and an armed outlet's drag on a
  knob routes it there: one box can move another's rate or depth, as any
  destination. A box cannot route onto its own knobs. The face being open
  is saved with the song.
- **Cables bend by hand.** Drag a cable's middle and the run moves along the
  axis it crosses; a double-click on it goes back to automatic routing. The
  bend is saved with the song, and a drag is one undo step.
- **Song inlets** (song patch step 06). Right-click empty canvas to make an
  **Inlet**, **Notes in** or **Notes out** tag; a new inlet opens its list at once, and clicking an inlet tag later
  opens it again. An inlet reads the transport -- **Beat** and **Bar** (a
  ramp 0 to 1 that fires each beat or bar, counted from the pattern's start
  in Pattern mode and the song's in Song mode), **Pattern position** (a ramp
  across the playing pattern, firing as it starts) and **Pattern** (the
  playing pattern's number as 0 to 1 across the song's patterns, firing on
  each change; in Song mode the topmost playlist row's) -- or a channel's
  notes as a gate, one of its generator's control outlets, or its mod wheel
  or aftertouch. Stopped, the transport tags hold and fire again on play;
  where no pattern is placed, Pattern and Pattern position hold. An outlet
  tag reads a block late and its tag says **late**. A box's inlet picker
  lists every bound inlet tag beside the gates.
- **Notes through the patch** (song patch step 07). A **Notes in** tag reads
  a channel's notes and a **Notes out** tag plays notes on a channel; a new
  one asks which channel at once, and a click on it later asks again. A
  wire between them plays the first channel's part on the second as well,
  in the same block at the same offsets, live and in a render. A Notes in
  tag copies by default: its list's **Take its notes** turns the take on,
  and its tag reads **takes**, so the channel's own notes reach only the
  patch (its chokes, bends and parameters still reach it). A notes-in tag
  reads the channel's own part, not what the patch sends it. Every note the
  patch plays has an id of its own, and a NoteOff, a choke, a seek, a panic,
  a stop, a mute, a pattern switch or a loop fold releases it; deleting the
  wire or rebinding a tag releases what went through it on the next block.
  A wire holds 64 notes at once; one more is refused and the Notes out tag
  goes red. A note wire cannot close a loop. Note boxes come with step 08.
- **Cable activity** (Preferences > Appearance): Off, Subtle (the default) or
  Full. A control wire tints toward the accent with its level and a note
  wire thickens for a moment on each note; Subtle is 30 % of Full.
- **A channel's outlets and keyboard are read through tags.** There is no
  outlet band: an inlet tag bound to a generator's outlet or a channel's mod
  wheel or aftertouch is the source, armed by clicking its outlet like a
  box's, and a route from it reads exactly what a route from the outlet
  read. A song saved before this opens with each such route reading a tag
  made for its source, one per source, in the tag column; copying a channel
  and saving a channel preset still carry the outlet itself, so a paste
  finds or makes its own tag. Any bound tag can be routed: a Bar ramp onto
  a cutoff needs no box.
- Under the canvas, the selected box's name, edited in its header, or what
  the selected tag reads; and the selected source's routes from the whole song, each naming its
  chain, device and parameter ("Kick Filter 1 · Cutoff"), with polarity and
  remove. Eight box kinds ship (LFO, Envelope, Step, Random, Math as the
  arithmetic boxes `+ - * / min max clip`, Counter, Select and Slew), each
  a descriptor table plus a tick, so a box's settings undo and persist like
  an effect's. Selection and arming hold the source itself, so they survive
  a channel change, a reorder and a new box.
- **A box's inlets are fed by wires**, or by clicking an inlet and picking
  what feeds it: none, then every channel's notes as a `gate` tag, or
  another box. A new box is named for the channel it was made on and keeps
  that channel as its home seat, which a channel preset reads.
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

- Every open lane of the clip is shown, stacked under the roll (described in
  [sequencing.md](sequencing.md)). The velocity lane is a separate fixed lane
  rather than one entry in that stack, and a pattern holds at most eight
  lanes per channel.
