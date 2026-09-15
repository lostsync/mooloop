# MIDI and control surfaces — status

Started 2026-09-15. The work `SCOPE.md` §2 item 2 calls "MIDI I/O — the
configurable half", plus items it does not: controller mapping, transport
control, and MIDI recording (§2 item 5's first half).

`docs/CONTROL_SURFACES.md` is the design. This file says what has landed.

| Step | State |
| --- | --- |
| [01 — the control model](01-control-model.md) | **Landed** 2026-09-15 |
| [02 — routing, transport and capture in the engine](02-engine.md) | **Landed** 2026-09-15 |
| [03 — the control thread's half](03-control-thread.md) | **Landed** 2026-09-15 |
| [04 — the interface](04-interface.md) | **Partly landed** 2026-09-15 — sidebar rows, record arm, the pump. Learn and the mapping editor are not built. |

## What works now, and what does not

Steps 01–03 are the spine: everything below the interface. A project can
*hold* a channel's MIDI input, a channel filter, and a table of control
bindings; the engine routes notes by them, forwards control for mapping, and
captures notes into patterns; the session resolves a binding onto the same
parameter write the on-screen control uses.

Step 04 then made the first half of it reachable: the channel sidebar's IN and
CH rows are live, there is a record-arm button, and the pump carries control
input and recorded notes both ways. **Controller mapping still has no
interface** — `Session::begin_control_learn` is tested and nothing calls it, so
a CC can be bound only by a project written by hand, and there is no transport
mapping surface. That is the honest state.

**None of it has been run against a MIDI device.** Every layer is tested and
the application compiles; no keyboard has been plugged into it.

Two things a reader should not have to discover from the source:

- **JACK presents one merged MIDI port.** Every hardware source is
  auto-connected to `mooloop:midi_in` and a message arriving on it carries no
  record of which keyboard sent it. So under JACK the input picker has exactly
  one entry, honestly named "All Hardware Inputs", and two keyboards are told
  apart by MIDI channel rather than by port. Core MIDI connects per source and
  really does distinguish them. A port per source under JACK is a driver
  change, not a mapping one; see `CONTROL_SURFACES.md`.
- **The Core MIDI half of step 02 is unverified.** `coreaudio_driver.rs` is
  behind `#[cfg(target_os = "macos")]` and this work was done on Linux, so it
  has not been compiled, let alone run. The changes are mechanical — a port id
  threaded through `MidiBytes` and the connection list — but "mechanical" is
  not "checked". **Build it on macOS before believing it.**
