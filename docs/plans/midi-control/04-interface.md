# 04 — the interface

**Partly landed 2026-09-15.** The channel sidebar's IN and CH rows are live,
there is a record-arm button, and the pump carries control input and recorded
notes. Learn, the mapping editor and the transport mapping surface are not
built, so **a CC can be bound only by a project written by hand**.

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

## What is left

1. **Learn, and a mapping editor.** `Session::begin_control_learn` exists and
   is tested; nothing calls it. The obvious home for the gesture is the
   context menu that already reaches modulation assignment. A bound control
   wants a mark on it, and an unresolved binding wants to say so.
2. **A transport mapping surface.** `TransportControl::ALL` is the menu.
3. **`release_control_pickup_for` is not called.** Moving a control on screen
   should make its bound knob catch the new value; until this is wired, a knob
   that has already taken over will pull the parameter back to the knob's
   position on its next message. One call per control handler, which is why it
   is worth doing with the learn pass rather than scattered now.
4. **Preferences → MIDI**, still a placeholder page. The port list and a
   default takeover belong there.
5. **Nothing here has been run.** It compiles and the layers below it are
   tested; no MIDI device has been plugged into it. `scripts/mooloop-mcp` and
   a keyboard are the next step.

## One decision left open

Whether a learn gesture binds the port it heard (`ControlLearn::bind_port`).
On for a studio with several controllers, off for one. It is a preference, and
picking a default without a user is guesswork — ask Adam.
