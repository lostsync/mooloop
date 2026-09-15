# 01 — the control model

**Landed 2026-09-15.**

The types, in `mooloop-core`, with no interface and no engine attached.

## What landed

`midi.rs` grew the facts a router needs:

- **Transport messages are decoded.** Start, Continue, Stop and Song Position
  (`0xFA`, `0xFB`, `0xFC`, `0xF2`). Clock is still dropped, deliberately: it is
  twenty-four messages a beat forever, nothing syncs to it yet, and decoding it
  would only spend the block's MIDI scratch. System status is matched *before*
  the channel nibble, because `0xFA` is Start and not a note-off on channel 10.
- **A message carries the port it arrived on.** `MidiPortId` is this run's
  numbering, not a persisted identity — a port's index moves when a device is
  unplugged, so a project stores the name the driver reported and resolves it
  once per run.
- **`SYSTEM_CHANNEL`**, which no channel filter accepts. A system message has
  no channel nibble; giving it a value that no filter matches is how a
  transport message is kept out of an Omni channel's notes by construction
  rather than by everybody remembering.
- **`ChannelMidiInput`** — a source (follow the selection, off, all ports, or
  one named port) and a channel filter (Omni, or 1–16). Its default is *follow
  the selection on every channel*, which is what mooloop did before the field
  existed, so a project written earlier opens behaving identically.

`control.rs` is new, and is the mapping layer.

## The shape, and why it is this shape

One binding is three questions, and only the first is about MIDI:

```
ControlSource  →  ControlValue  →  ControlMode  →  ControlOutcome
 what a desk       protocol-free    how movement    set a value,
 sends             movement         is read         or fire
```

Everything from `ControlValue` rightwards is protocol-free. That is the whole
design: **an OSC surface adds a variant to `ControlSource` and a decoder that
produces a `ControlValue`, and reuses the rest unchanged** — takeover, the
three relative-encoder conventions, toggles, momentary pads, ranges,
inversion, the transport gestures, and the whole of `ControlMap`.
`docs/CONTROL_SURFACES.md` states what that costs and what it does not.

Three decisions worth keeping:

- **Pickup is the default takeover.** A potentiometer left at zero must not
  slam a filter shut the first time it is touched after a load. Jump is there
  for endless and motorised controls, where the surface *is* the truth.
- **Conflict is overlap, not equality.** Learning CC 7 on channel 1 over a
  binding of CC 7 on every channel takes the knob; it does not sit beside it.
  The first version compared sources for equality and left both in the map,
  and one knob then drove two things with no way to tell from the desk.
- **Bindings live in the project.** A `ParamAddr` names a device on a channel
  of *this* song, so a map stored beside the application would point at the
  wrong channels the moment another song opened. A surface template that
  outlives a song is a different document and does not exist yet.
