# MIDI and control surfaces — status

Started and finished 2026-09-15. The work `SCOPE.md` §2 item 2 calls "MIDI I/O
— the configurable half", plus items it does not: controller mapping,
transport control, and MIDI recording (§2 item 5's first half).

`docs/CONTROL_SURFACES.md` is the design. This file says what has landed.

| Step | State |
| --- | --- |
| [01 — the control model](01-control-model.md) | **Landed** 2026-09-15 |
| [02 — routing, transport and capture in the engine](02-engine.md) | **Landed** 2026-09-15 |
| [03 — the control thread's half](03-control-thread.md) | **Landed** 2026-09-15 |
| [04 — the interface](04-interface.md) | **Landed** 2026-09-15 |

## What works now

A channel picks its MIDI input and its channel filter from the sidebar. The
engine routes notes by them at each message's own frame offset, forwards
control and transport messages to the control thread, and captures played
notes into the pattern while the transport is armed and running. LEARN beside
the transport maps a controller to any parameter modulation reaches: press the
control, move the knob. Preferences > MIDI lists the inputs, every mapping,
and all seven transport gestures with whatever is bound to each. The map is
saved with the project, because its targets name channels of this song.

## What is not built, and why

- **MIDI output does not exist.** The sidebar's OUT row is inert and says so.
  `SCOPE.md` §2 item 2 puts it in 0.2.0.
- **A mapped control carries no mark on its face.** It needs a per-parameter
  model on all fifty device faces; the mapping page is where a binding is
  visible instead. `04-interface.md` has the reasoning, `LOOSE_ENDS.md` the
  entry.
- **A binding's mode is whatever learn chose.** The editor switches takeover
  and inversion, not the mode. Relative encoders are what would want it and
  they cannot be told apart without hardware.
- **Clock is dropped**, as the design says: twenty-four messages a beat and
  nothing to sync to them yet.

## Two things a reader should not have to discover from the source

- **JACK presents one merged MIDI port.** Every hardware source is
  auto-connected to `mooloop:midi_in` and a message arriving on it carries no
  record of which keyboard sent it. So under JACK the input picker has exactly
  one entry, honestly named "All Hardware Inputs"
  (`mooloop_engine::MERGED_MIDI_IN_LABEL`, which the preferences page also
  recognises in order to explain it), and two keyboards are told apart by MIDI
  channel rather than by port. Core MIDI connects per source and really does
  distinguish them. A port per source under JACK is a driver change, not a
  mapping one; see `CONTROL_SURFACES.md`.
- **The Core MIDI half of step 02 is unverified.** `coreaudio_driver.rs` is
  behind `#[cfg(target_os = "macos")]` and this work was done on Linux, so it
  has not been compiled, let alone run. The changes are mechanical — a port id
  threaded through `MidiBytes` and the connection list — but "mechanical" is
  not "checked". **Build it on macOS before believing it.**

## And the one that matters most

**None of it has been run against a MIDI device.** Every layer is tested, the
application compiles, and the mapping page draws; no keyboard has been plugged
into it. `scripts/mooloop-mcp` and a controller are the check, and until that
happens this plan is finished on paper rather than in a room.
