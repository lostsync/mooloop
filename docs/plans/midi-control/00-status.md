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

**Per-channel routing did not reach the live engine until 2026-09-17.**
`EngineHandle::install_project` reattached every shared cell but the routing
one, and the app installs a project at startup, so the input setting was
written to a cell no renderer read and every channel followed the selection.
The engine tests attached routing by hand and passed throughout. The install
path and startup now share one attach list (`SharedCells::attach`), and
`install_tests` drives the renderer an install builds.

**Recording was only right on the first pass until 2026-09-17.** The engine
reported the unfolded transport tick, which in pattern mode grows forever,
and the session clamped it onto the pattern's last tick. The engine now folds
it with the sequencer's own wrap (`Sequencer::recording_tick`). In song mode it
records at the offset into the selected pattern's placement under the
playhead, and not at all where none covers it.

**Record arm did not survive an install until 2026-09-17.** Every structural
edit, undo and load builds a fresh renderer, and it started disarmed while the
button still read armed. The arm now travels with the install
(`mooloop_engine::InputState`).

**Routing was not republished after an install until 2026-09-17.** A channel
removal, move, paste or undo left later channels reading another channel's
input setting. Each install now carries the incoming project's routing into a
routing cell of its own, so no renderer ever reads another project's channel
order.

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
