# Control surfaces

Status: the design behind `mooloop-core::control`, written 2026-09-15 with
steps 01–03 of `docs/plans/archive/midi-control/` built and the interface not, and
brought up to date the same day when step 04 landed.

`MODULATION.md` owns parameter addressing and the modulator rack. This
document owns how something *outside* mooloop moves something inside it — a
knob on a desk, a pad, a transport button, a message from another sequencer.

## The one idea

A control surface is three questions, and only the first is about MIDI.

```
ControlSource   →   ControlValue   →   ControlMode   →   ControlOutcome
what a desk         protocol-free      how movement      set a value,
sends               movement           is read           or fire a gesture
```

`ControlSource` is where a protocol lives. Everything to the right of it is
protocol-free, and that is deliberate: takeover, the three relative-encoder
conventions, toggles, momentary pads, ranges and inversion, the transport
gestures, learn, and the whole of `ControlMap` are written once and are not
MIDI's.

## Where OSC attaches

**OSC adds one variant to `ControlSource` and a decoder that produces a
`ControlValue`.** That is the whole of it. Concretely:

- `ControlSource::Osc { address: String }`, beside `Cc`, `Note` and
  `PitchBend`. Its `read` turns an incoming argument into
  `ControlValue::Absolute` for a float in `0..=1`, `Press`/`Release` for a
  bang or a boolean, and `Relative` for a delta — the same three shapes the
  MIDI variants already produce.
- `ControlSource::conflicts_with` gains an arm: two OSC sources conflict when
  their addresses match. It must stay *overlap* rather than equality if OSC
  ever grows address patterns.
- A transport for it. This is the real work and none of it is in this module:
  a UDP socket, a bundle parser, and somewhere to put it. It is not the audio
  thread — see below — so it can be an ordinary thread feeding the same
  `apply_control_input` the engine's forwarded MIDI feeds.
- A surface identity, if several OSC clients should be told apart, playing the
  part `MidiPortId` plays. `MidiPortFilter`'s shape — any, or one named — is
  the shape that generalises; the name resolution is already a single function
  (`resolve_port_name`) rather than a rule spelled per type.

What OSC does **not** need: any change to `ControlTarget`, `ControlMode`,
`Takeover`, `ControlBinding`, `ControlMap`, `ControlMapState`,
`TransportControl`, `Session::apply_control_input` past its first few lines,
`Session::set_param_normalized`, or the project format's `control_map`.

What is *not* claimed: that OSC is free. Addresses are strings with structure,
which a `u8` controller number is not; a project that maps `/mooloop/ch/1/vol`
wants the address to survive a channel being renamed, and `ParamAddr` already
handles the other half of that. Nothing here has been built or tested against
a real OSC client.

## Where each part runs

**Notes are realtime. Control is not.**

The engine routes notes inside the audio callback, at each message's own frame
offset, because a note's timing is the thing. It *forwards* control changes,
pitch bends and transport messages up to the control thread, which maps them
and issues the same `EngineCommand` and the same project edit that moving the
on-screen control produces.

That second half is the load-bearing decision. A CC that moved a parameter by
a private realtime path would leave the project holding the old value, the
interface drawing the old value, and undo unable to see the move — and it
would be a second implementation of every parameter edit. The cost is one pump
of latency on a knob turn, which is the right trade for a mapping layer. A
performance subset that needs tighter timing can be given a realtime fast path
later against these same types; the buffer insert's note mapping already is
one, for exactly that reason.

## Decisions that are made

- **Pickup is the default takeover.** A potentiometer left at zero must not
  slam a filter shut the first time it is touched after a load. `Jump` exists
  for endless and motorised controls, where the surface is the truth.
- **Conflict is overlap, not equality.** Learning CC 7 on channel 1 over a
  binding of CC 7 on every channel takes the knob rather than sitting beside
  it. One physical control drives one thing.
- **A learn gesture does not hand the knob's position to the parameter.** It
  binds; the knob still has to catch the value. Otherwise mapping a knob
  somebody left at zero onto a centred pan slams it hard left. The on-screen
  half of the same rule is that *pressing* a control to name it does not move
  it either: the whole gesture writes nothing, and the mapping lands on the
  value that was there before the press.
- **A learn gesture binds the port it heard only if asked to**, and the
  default is not to. With it off, replacing a keyboard keeps every mapping and
  a second controller sending CC 7 moves the same fader; with it on, a studio
  with a desk *and* a keyboard stops the two colliding. Off is the right guess
  for one controller and the wrong one for several, and the case that decided
  it is the failure each way looks like: off, two controllers fight over one
  parameter, which is visible and fixable from the mapping page; on, a device
  plugged into a different socket can come back under another name and every
  mapping silently stops working, with nothing on screen to say why. It is a
  preference on Preferences > MIDI rather than a per-binding field, because a
  studio is one answer or the other.
- **Pickup is released by the binding noticing, not by the interface telling
  it.** A caught binding writes the parameter and records the value the
  parameter then *reads back*; the next message that finds it somewhere else
  knows something has overtaken it and starts catching again. The alternative
  -- every on-screen control, preset recall and undo step calling
  `release_control_pickup_for` -- was the original plan and does not survive
  contact with this codebase: a generator's parameters are written by three
  dozen individually named callbacks that assign the field directly, so the
  call would have been scattered across every device face and forgotten by the
  next one. The read-back rather than the requested value is the load-bearing
  half: a stepped parameter quantizes, so comparing against the request would
  find a difference on every message and nothing would ever follow anything.
- **Bindings live in the project**, because `ParamAddr` names a channel of
  *this* song. A surface template that outlives a song — a desk's transport
  row, say — is a separate document that stamps bindings into a project. It
  does not exist.
- **MIDI's Stop is a pause.** It holds position and Continue resumes from it,
  so it maps to `TransportControl::Pause`. Mapping it to `Stop`, which returns
  to the start, would make an external sequencer's stop button rewind the song.
  **Start is the other half**: it plays from the top, so it is
  `ReturnToStart` then `Play`. Mapped to a bare `Play`, as it was until
  2026-09-22, it resumed wherever mooloop had paused and the two machines ran
  bars apart.
- **Clock is dropped.** Twenty-four messages a beat, forever, and nothing
  syncs to it. It lands the day there is a clock to drive; the transport
  messages beside it are decoded now because they are gestures rather than a
  stream.

## What the drivers can and cannot tell you

**JACK presents one merged MIDI port.** Every hardware source is
auto-connected to `mooloop:midi_in`, and a message arriving there carries no
record of which keyboard sent it. So under JACK the input picker has exactly
one entry, honestly named, and two keyboards are told apart by MIDI channel.
Giving each source its own port is a driver change — registering and
unregistering JACK ports as devices come and go — and belongs with issue #9's
`MidiBackend` rather than with mapping.

**Core MIDI connects per source** and really does distinguish them, which is
what makes the input picker worth having there. Its port ids are monotonic
rather than positional, because the connection list is pruned when a device is
unplugged and a reused position would point a project at somebody else's
keyboard.

The MIDI preferences page says this in words when it sees the merged port, so
a one-entry input list does not read as a bug. The name it recognises is
`mooloop_engine::MERGED_MIDI_IN_LABEL`, which is why that constant lives in
the engine's `lib.rs` rather than in the JACK driver: the driver and the
interface have to agree on it, and a second spelling is the copy that drifts.

A project stores a port by the **name** the driver reported and resolves it
once per run. A name no current port carries leaves the channel silent for that
run and the name in the project, so plugging the device back in restores the
routing rather than requiring it to be set up again.

## The gesture, and why it is where it is

**Learn is armed from the toolbar and aimed by pressing a control.** LEARN
sits beside the transport buttons rather than on the preferences page,
because the thing it arms is a press on a control in the main window and a
modal dialog is covering those. It stays on after a binding lands, so a desk
is mapped control after control in one pass.

**The press arrives on `modulation-edit-started`.** That is a deliberate
overload of modulation assignment's callback, and it is worth stating why,
because the obvious reading is that learn should have one of its own. Every
device face already forwards that callback with the parameter that was
pressed, through the rack, to `main.slint` -- about fifty call sites. A
second callback beside it would be fifty more lines of markup, and each face
missed is an eight-minute build to discover. The Rust side tells the two
gestures apart from its own arm, which is a fact it already holds.

**The arm reaches the controls through a Slint global**, `ControlAssign`,
rather than a property threaded down. Modulation's `modulation-armed` *is*
threaded down, and correctly: arming modulation is per-destination, since a
source only reaches its own channel and a face has to say which parameters
accept it. Learn reaches every parameter there is, so there is nothing
per-control to say and nothing to thread.

**Its reach is exactly modulation's reach**, and that is not a coincidence:
both stop where `Session::param_descriptor` stops -- a generator's controls, a
device's, and the strip's volume and pan. A modulator slot's own parameters
and an instrument's internal routes are outside both.

## What is not built

- **A mapped control carries no mark.** See `LOOSE_ENDS.md`; it needs a
  per-parameter model on every face and was left out of step 04 on purpose.
- **A binding's mode cannot be changed from the editor.** A row switches
  takeover and inversion; the mode itself is whatever learn chose, which is
  Toggle for a note and Absolute for anything else. Relative encoders are the
  case that would want it, and their three conventions cannot be told apart
  without a device to try them on.
- **Nothing here has been run against a MIDI device.**
