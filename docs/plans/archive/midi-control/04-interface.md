# 04 — the interface

**Landed 2026-09-15.** The channel sidebar's IN and CH rows, a record-arm
button, the pump in both directions, a learn gesture on every parameter
control modulation reaches, and a mapping editor with the transport list on
Preferences > MIDI.

## What landed

- **The sidebar's MIDI rows.** IN lists *Follow Selection*, *Off*, *All
  Inputs* and then the driver's ports; CH lists Omni and 1–16. Neither list is
  spelled in markup: `MidiInputSource::picker_rows`/`from_row`/`row` and
  `MidiChannelFilter::from_row`/`row` own the rows and the mapping, with
  round-trip tests, and the markup hands back an index. A stored port that is
  not plugged in says so under the picker rather than leaving the channel
  silently unplayable — the row falls back to *Follow Selection*, so without
  that line the panel would be misreporting what the channel is set to.
  **OUT is still inert**, and now means what a disabled row here has always
  meant: MIDI output does not exist (`SCOPE.md` §2 item 2, 0.2.0).
- **A record-arm button** beside play and stop, using the `"record"` role
  `TransportButton` already had. Arm, not record: the transport does the
  recording, which is why arming while stopped works.
- **The pump.** `EngineEvent::ControlInput` and `EngineEvent::RecordedNote`
  are collected during the drain and answered after it, because acting on
  either needs the session *and* the handle while the handle is borrowed for
  the drain. One republish for the whole drain rather than one per message: a
  fader sweep is a hundred messages a second.
- **A port scan once a second**, which rebuilds the routing table and
  re-resolves the control map when — and only when — the list moves. A
  keyboard plugged in mid-session is a channel whose stored port name resolves
  for the first time and a binding that stops being inert.
- **LEARN, beside the transport.** Arms the gesture; a press on any parameter
  control names it; the next control moved on the desk binds to it. The arm
  stays on through a binding landing, so a desk is mapped control after
  control in one pass, and the status bar names what was just bound. While it
  is armed a press writes nothing at all — not the value, not a modulation
  depth, not an undo gesture — because a mapping must land on the value that
  was there before the press. The button reads LISTENING while a gesture is
  pending and names its target, because the status bar cannot: a hover hint
  outranks a status message there, so the sentence naming the pending
  parameter vanished the moment the pointer crossed any control.
- **The mapping editor and the transport list**, on Preferences > MIDI, with
  the ports the driver is offering and the one preference that is the user's
  rather than the song's. A row relearns, removes, switches pickup/jump and
  inverts. A transport gesture is learned from its own row: it has no
  on-screen control to press, and `TransportControl::ALL` is the menu.
- **Pickup releases itself.** See below; this replaces item 3 of what was
  left, and it is the one place the plan was wrong about its own codebase.

## The three decisions that changed the shape

1. **The learn press rides on `modulation-edit-started`.** The plan said the
   gesture's home was "the context menu that already reaches modulation
   assignment". There is no such menu — modulation is assigned by arming a
   source and dragging a control, and no parameter control in this interface
   has a context menu at all. What the faces *do* have is one callback that
   already knows which parameter was pressed, forwarded through the rack to
   `main.slint` from about fifty call sites. Overloading it costs a branch in
   three Rust handlers; a second callback beside it would have cost fifty
   lines of markup and an eight-minute build for each site missed.

2. **The arm is a Slint global**, `ControlAssign`, not a property threaded
   down beside `modulation-armed`. Modulation's is threaded correctly —
   arming it is per-destination. Learn reaches every parameter, so there is
   nothing per-control to say.

3. **`release_control_pickup_for` is still not called, and should not be.**
   The plan asked for "one call per control handler". There is no such set of
   handlers: a generator's parameters are written by three dozen individually
   named callbacks that assign the field directly, so the call would have been
   scattered across every device face and forgotten by the next one. Instead a
   caught binding records the value the parameter *reads back* after it writes
   it, and releases itself when it next finds the parameter somewhere else.
   That covers the on-screen knob, undo, a preset, automation and another
   binding, with one comparison and no call sites. The read-back rather than
   the request is the load-bearing half — a stepped parameter quantizes, so
   comparing against the request releases on every message and nothing follows
   anything. `a_quantized_parameter_does_not_release_its_own_control` fails
   against that version.

## What is left

1. **A mapped control carries no mark.** Deliberate: it needs a per-parameter
   `[bool]` on every device face, beside the `modulation-route-counts` model
   that already goes to all of them, which is the fifty-file markup edit
   decision 1 exists to avoid. The mapping page is where a binding is visible
   today. Recorded in `LOOSE_ENDS.md`.
2. **A binding's mode cannot be changed.** Learn picks it — Toggle for a note,
   Absolute for anything else — and the editor switches takeover and inversion
   but not the mode. Relative encoders are what would want it, and their three
   conventions cannot be told apart without a device to try them on.
3. **Nothing here has been run.** It compiles, it draws, and every layer below
   it is tested; no MIDI device has been plugged into it. `scripts/mooloop-mcp`
   and a keyboard are the check.

## The decision that was open

Whether a learn gesture binds the port it heard (`ControlLearn::bind_port`).
**Off by default**, as a preference on the MIDI page rather than a per-binding
field. Off is right for one controller and wrong for several, and the tie was
broken on what each failure looks like: with it off two controllers fight over
one parameter, which is visible on the mapping page and fixable there; with it
on, a device that comes back under a different name takes every mapping with
it and nothing on screen says why. Adam can move it in one click.
