# Task: Control and Modulation — picking up from Stage 1

`01-the-whole-thing.md` is done. This document is the handoff for what comes
next. It is **not** a new design: `docs/MODULATION_PLAN.md` is the approved
design and `docs/FOCUS.md` step 3 is the work order. Read both. This file
records what already landed, the one amendment that plan needs, and the
order to build in.

## Where things stand

Everything below is merged to `main`.

| Commit | What |
|---|---|
| `2a0a814`, `298c5ee`, `6fff8dd` | Buffer device core, engine insert, off-thread ring resize |
| `e0bf382` | Collision telemetry; block-size and crossfade acceptance tests |
| `6869acc`, `055ac02` | Buffer device face with debug triggers; insert-menu entry |
| `2f5f8ce` | Reverse as a held, looping gesture |
| `ea59b7a` | MIDI input port, decoding, and a buffer control mapping |
| `b576ad1` | `ParamAddr`, the mod matrix, and an LFO source |

Stage 1's acceptance list is complete except **RT hygiene (test 8)** — no
allocations or locks in the callback, verified. That needs an
allocation-tracking harness, not a reading of the code, and is still open.

### What is audible today

A Buffer insert with three debug triggers on its face: JUMP (latching, -1
beat), REV (held; runs backward from the press and loops the two bars behind
it), STUT (latching, 1/16 window, eight repeats). Collisions show on the face.

### What exists but is not reachable from the UI

- **MIDI.** The engine registers a `midi_in` port and decodes it, and
  `BufferMidiMap` maps note/velocity/CC onto the buffer. There is no way to
  *configure* a mapping — only `EngineHandle::set_buffer_midi_map`
  programmatically. Adam has said MIDI *device* input is not a near-term
  priority; the value here is the event vocabulary, not the port.
- **Modulation.** `ParamAddr`, `ModRack`, and `ModulatorRack` (LFO) exist and
  are tested. Nothing ticks them. This is the next step.

## Amendment to MODULATION_PLAN.md: generator outlets

The approved plan covers **effects** exposing outlet signals (a compressor's
gain reduction, a gate's open state) as modulator sources. It does not cover
**generators** publishing note-derived signals, and that is the piece that
lets note data reach an effect at all.

Adam's model:

> in your synth, you say 'put velocity on outlet 1'. on your effect, you add a
> modulator, something like `device in`, which lets you pick an available
> outlet from a list, and gives you a trim knob, maybe smoothing too.

This resolves a problem that otherwise has no good answer. A channel has one
filter and one buffer shared by every voice, so "this note's velocity opens
*this note's* cutoff" is not expressible at an effect — per-note modulation of
a shared device is a category error. An outlet sidesteps it: **the generator
reduces per-voice note data to one channel-rate control signal**, and
downstream devices consume plain CV. No effect needs per-voice state.

What this requires:

- An outlet address — `(channel, outlet index)` — and a name for the list.
- A published reduction per outlet. Which reduction (last note, highest,
  loudest) is a real choice; last-note is the obvious default.
- A `DeviceIn` modulator kind alongside `Lfo`, with trim and smoothing.
  Smoothing matters: velocity is a step per note, and unsmoothed it will
  click when it lands on a cutoff.
- Timing follows the plan's existing rule for inter-device data: publish into
  a per-channel table **read on the following block**. One block of latency,
  deterministic, identical offline and realtime, and graph order stops
  mattering. Do not try to make outlets same-block.

Fold this into `MODULATION_PLAN.md` proper rather than leaving it here.

## Build order

### 1. Engine wiring — one modulator to one destination, audible

This is FOCUS.md's own instruction: *LFO to filter cutoff, knob to ear*,
before any general editor.

- `RenderState` holds a `ModulatorRack` and `ModRack` per channel.
- Tick the modulator rack **before** the channel's effect chain, at
  `CONTROL_RATE_FRAMES` (32) subdivisions of the block.
- Resolve `base + sum(offsets)` per destination and emit `Event::ParamValue`
  into the destination slot's existing per-slot `EventList`.
- **No effect changes. Ever.** Effects already split their block at
  `ParamValue` offsets; that contract is the whole design.
- The engine owns the base value. Never let the matrix write absolute values
  — the knob and the LFO would fight, and turning a modulated knob would snap
  it back. The UI needs both numbers anyway to draw the modulation arc.

Done when an LFO visibly and audibly moves a filter cutoff, knob and
modulator do not fight, and it survives save and reload.

### 2. Generator parameter descriptors

Effects are descriptor-addressed: `(EffectTarget, slot, param_id)` with
ranges and curves. **Generators are not** — they ship whole structs
(`SetChannelSamplerParams { channel, params: SamplerParams }`). So
`song.channel().generator().property` is not addressable and cannot be a
modulation destination.

Closing this is mechanical but broad: descriptor tables for sampler, drum,
mono, and poly params mirroring the effect ones. Do it before the UI grows an
assignment gesture, or the gesture will only work on half the project.

### 3. Generator outlets and `DeviceIn`

Per the amendment above.

### 4. UI

The plan already specifies the gesture, and it matches what Adam described
independently: **no patch cords**. A labeled source chip on each modulator —
click it, assignable knobs light up, drag one to set depth. Inlets and
outlets exist in the data model without being drawn as wires.

### 5. Refactor `BufferMidiMap` onto `ParamAddr`

Right now the MIDI map names buffer tuple fields directly. Once `ParamAddr`
has real destinations it should become one instance of the general
source→destination system rather than a parallel one. Do this *after* step 1
proves the general path, not before.

## Decisions already made, do not re-litigate

- **Control rate, not audio rate.** 32-frame subdivisions. No audio-rate FM.
  A deliberate limit; stepped sequenced modulation is stylistically correct
  for this instrument.
- **Cross-channel modulation is deferred**, but `ParamAddr` already carries
  the channel so enabling it is a routing change.
- **Display telemetry is not a modulation outlet.** Spectrum, gain reduction
  as *display*, and the buffer's collision counter publish into the telemetry
  bank with no timing guarantee beyond "latest available". They must never be
  read by an audio node as input. A musical control signal belongs in the
  modulator path where it gets a declared rate and latency.
- **The modulator rack is inline, not boxed nodes.** This deviates from
  MODULATION_PLAN.md, which suggested reusing the effect chain's
  install/reclaim plumbing. No modulator kind allocates, so that machinery
  buys nothing and would put a `Box` drop on the path of every rack edit.
  Reverse it if a future modulator kind needs heap state.

## Gotchas found the hard way

- **Reverse collision timing.** A detached head is only overtaken at the
  ring's *trailing edge*, so on an 8-bar ring a -2 beat reverse runs ~7.5
  seconds before force-returning. `01-the-whole-thing.md` acceptance test 3
  reads as though the collision is prompt; it is not. This is why reverse
  became a held gesture with a 2-bar loop instead.
- **A window extends backward for a reverse head.** Extending it forward in
  both directions points a reverse window at samples the writer has not
  written yet, and the gesture plays silence. Easy to reintroduce.
- **`EffectTypeMenu` in `device-rack.slint` is a hardcoded catalog**, not a
  projection of `EffectKind::ALL`. A new effect kind added everywhere else
  still cannot be inserted, and nothing fails at compile time. This already
  bit once. A test asserting the menu covers `EffectKind::ALL` would earn its
  keep.
- **Relative CC encodings genuinely disagree.** Byte 65 is `+1` in binary
  offset and `-63` in two's complement. It must be configured, never sniffed.
- **Scrub is position mode.** The head chases a target and the closing speed
  *is* the playback rate. Holding at rate zero repeats one sample forever,
  which is a DC step rather than silence, so gain follows speed down.
