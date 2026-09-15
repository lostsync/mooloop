# 03 — the control thread's half

**Landed 2026-09-15.**

`mooloop-session/src/midi.rs`: what a forwarded message does, and where a
recorded note lands.

## What landed

- **`Session::midi_routing`** resolves every channel's stored input against
  the driver's current port list, which is what `EngineHandle::set_midi_routing`
  installs. Call it when a channel's setting changes *and* when the port list
  does — a keyboard plugged in mid-session is a channel whose stored port name
  resolves for the first time.
- **`Session::record_note`** writes a captured note into the pattern it was
  played over, on the channel the engine says — not the selected one, because
  recording follows the routing rather than the screen. The engine has already
  positioned it on the looping playhead, so what this adds is the pattern's
  bounds: a note held past the end is trimmed to it.
- **`Session::apply_control_input`** is the one entry point. Order is
  deliberate: a learn gesture in progress consumes the message, because the
  point of learn is that the next control you touch is the one you meant —
  including one already bound to something else, which is how a mapping is
  corrected. Only when nothing is learning does the map get the message.
- **`Session::apply_transport_control`** is the single place a transport
  gesture is carried out, whether a mapped pad, an external Start message, a
  shortcut or a toolbar button asked for it. One place, so they cannot come to
  disagree about what Play means.
- **`param_descriptor` / `param_natural` / `set_param_normalized`** resolve a
  `ParamAddr` to a value and back. Reader and writer share one `match` over
  `ParamOwner`, so a fourth owner kind cannot be given a reader and left
  without a writer. Every write goes through the *same* project mutation and
  the same `EngineCommand` the interface's own control produces.

## Two bugs the tests found, both worth recording

- **Song Position did nothing.** It was folded into `external_transport`,
  which returns the gesture a message asks for — and a locate is not one of the
  gestures, so it returned `None` and took the whole arm with it. An external
  sequencer's locate was silently ignored. Fixed by asking `is_transport()`
  first and handling the seek beside the gesture rather than inside it.
- **Learn stacked instead of replacing.** `ControlMap::bind` compared sources
  for equality, so learning CC 7 on channel 1 over CC 7 on *every* channel left
  both bindings live and one knob drove two things. Fixed in step 01's types:
  conflict is now overlap.

## What is deliberately not reached

Modulator slots and a generator's internal routes. A route's amount belongs to
a patch rather than to a device's control surface, and binding a knob to one is
a question nobody has asked. They are outside the modulation shelf's pass for
the same reason, and `set_param_normalized` returns `None` for them rather than
writing somewhere approximate.

**MIDI's Stop is a pause.** It holds position, and Continue resumes from where
it stopped, so it maps to `Pause` and not to `Stop` — which returns to the
start. Getting that backwards would make an external sequencer's stop button
silently rewind the song.
