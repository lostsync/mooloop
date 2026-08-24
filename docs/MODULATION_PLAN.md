# Modulation, parameters, and the effect suite

Status: approved design, August 2026. Supersedes the "Explicitly out of scope"
section of `docs/EFFECTS_PLAN.md` where the two disagree; everything else in
that document still stands.

Read `docs/PRODUCT.md` for why this exists ("One Automation Language"),
`docs/BUFFER_ENGINE.md` for the retained-audio device this has to stay
compatible with, and `AGENTS.md` before touching git.

## What this document decides

The filter shipped as a complete vertical slice, which proved the effect
plumbing. Before adding ten more effects we need to settle how parameters are
addressed, modulated, and automated — otherwise every new effect hardcodes its
own ranges and the modulation system becomes a per-effect special case.

These decisions are made. Implement them; don't re-litigate them.

## Parameter descriptors

Every effect kind publishes a static table of `ParamDescriptor`:

```rust
pub struct ParamDescriptor {
    pub id: u32,            // stable, per-kind, never renumbered
    pub name: &'static str,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    pub curve: ParamCurve,  // Linear | Exponential | Stepped(n)
    pub default: f32,
}
```

This is the single source of truth for a parameter's range and its
normalized (0..1) <-> natural (Hz, dB, bits) mapping. Automation lanes,
modulation depth, knob glue, and preset validation all read it. A range
written a second time anywhere else is a bug.

`id` values are stable and per-kind. They are persisted indirectly (automation
lanes will reference them) so they must never be renumbered once shipped —
append new ids, retire old ones by leaving gaps.

Events on the wire carry **natural** units, not normalized ones. Effects stay
ignorant of curves; the engine converts. This keeps `Event::ParamValue`
readable in tests and means a descriptor change can't silently reinterpret an
effect's internal state.

### Deferred: `ParamAddr`

An earlier sketch of this work introduced a general
`ParamAddr { channel, target, param }` type. It is **not** being built yet.
Today only effect slots consume `Event::ParamValue`, and
`EngineCommand::SetEffectParam { channel, slot, id, value }` already *is* that
address. A general address type with exactly one inhabited variant is the
speculative generality `EFFECTS_PLAN.md` warns against.

Introduce it in the modulation-rack pass, when modulators and strip parameters
give it a second and third target. The descriptor table is the part with value
now, and it is forward-compatible with the address type.

## Modulation architecture

### Modulator rack

Each channel gets a small fixed rack of modulator slots (4), each holding one
of `Lfo | StepSeq | EnvFollower | SampleAndHold | Macro`. This reuses the
install/reclaim/reorder plumbing already built for the effect chain — a
modulator is a node with no audio output, not a new subsystem.

Per-channel, not project-global. It matches the rack UI and keeps a channel a
self-contained instrument. Project-global modulators can be added later as a
distinct source kind; nothing here blocks them.

### Mod matrix

Entries are `(source_slot, destination, depth, polarity)`. The engine ticks the
modulator rack *before* the channel's effect chain, evaluates the matrix, and
emits `Event::ParamValue` into the destination slot's existing per-slot
`EventList`.

**No effect changes to support modulation. Ever.** Effects already split their
block at `ParamValue` offsets. That contract is the whole design; keep it.

### Base value plus offset

The engine owns the parameter table: the **base** value per destination (what
the knob sets) and the sum of active **modulation offsets**. It emits the
resolved value.

Effects store only resolved values. Do not let the matrix write absolute
values directly — the user's knob and the LFO would fight, and turning a
modulated knob would snap it back. The UI needs both numbers anyway to draw a
knob with a modulation arc.

### Control rate, not audio rate

Modulation is evaluated on a fixed subdivision of the block (32 or 64 frames),
not once per block and not per sample. Once per block stair-steps audibly on
fast LFOs; per sample is a cost we don't need.

This means no audio-rate FM of a filter cutoff. That is a deliberate limit.
Stepped, sequenced modulation is stylistically correct for the music this
instrument targets, and audio-rate modulation is a much larger engine change
that can come later if it earns its way in.

## Note-triggered effects

Effect slots currently receive only their own private parameter events
(`render.rs` — "generators never see effect events", and the reverse). Keep
that isolation for parameter events, but **give effect slots access to the
channel's note stream as a separate input**.

This is what makes the rack an instrument rather than a chain of processors:
an LFO that resets phase on note-on, a delay that flushes on a note, a
step-sequenced modulator that advances per note, a stutter fired from a rack
step. The sample-accurate note pipe already reaches every channel; it stops
one node short.

## Inter-device and inter-channel data

**Within a channel:** the mod matrix covers it. Effects may expose outlet
signals (a compressor's gain reduction, an envelope follower's output, a
gate's open state) as modulator sources. The dynamics effects already compute
exactly these internally; exposing them is a matter of publishing the value,
not of new DSP.

**Across channels: deferred, by decision.** Not in this pass. `ParamAddr` will
carry `channel` from the day it is introduced so that enabling it later is a
routing change rather than a retyping of every engine command.

**True audio sidechain: still deferred.** The mixer supplied the first
compiled audio graph, but not the complete sidechain contract. A sidechain is
a dependency edge in addition to ordinary audio routing: the source must be
scheduled before the consumer even though its signal is not summed into that
consumer's main input. `compile_bus_graph` currently models only each bus's one
audio destination, and `AudioNode::process` currently accepts only one in-place
stereo bus. Extend both through the process-buffer and typed-edge design in
`AUDIO_ARCHITECTURE.md`; do not retain a borrowed source bus inside an effect.

Latency compensation is also required and is not hypothetical. `AudioNode`
now reports integer latency, and the drive effect declares 15 frames for its
complete 2x oversampling path (both 32-tap FIR stages, including the retained
polyphase offset). Its internal dry path is aligned, but the graph does not yet
delay neighbouring shorter paths at a sum. **Build preallocated graph
compensation before parallel sends or true sidechain.**

Control-rate ducking still does not need any of this: publish modulator
outputs into a per-channel table read on the *following* block. One block of
latency, deterministic, identical offline and realtime, and it makes graph
order irrelevant. That remains the cheaper and more musical first move.

### Display telemetry is observation, not a route

Device displays may need a continuously changing view of their input or
output: spectrum, waveform, gain reduction, a buffer read head, and similar
information. These publish a fixed, bounded semantic vector into the engine's
device-stage telemetry bank. The UI reads only the newest snapshot through
atomics; it does not receive PCM or replay audio analysis itself.

Display telemetry is deliberately not a modulation outlet. It has no timing
guarantee beyond "latest available", cannot write parameters, and must not be
used by audio nodes as an input. When a device exposes a musical control
signal, it belongs in the modulator/matrix path above, where the engine can
give it a declared rate, latency, and destination semantics. This preserves a
single display path that any device can use without preempting the future
typed control graph.

**UI:** no patch cords. The assignment gesture is a labeled source chip on
each modulator: click it, assignable knobs light up, drag one to set depth.
Inlets and outlets exist in the data model without being drawn as wires.

## Effect build order

1. ~~**Foundation** — `EffectParams` tagged enum, descriptor tables.~~ Done.
2. ~~**Drive/saturation and bitcrush** — stateless, cheap, immediately
   useful.~~ Done.
3. ~~**Delay**~~ Done, on the shared primitive described below.
4. ~~**Dynamics: gate, compressor, limiter** — one shared envelope
   detector.~~ Done, sharing `mooloop_dsp::dynamics`.
5. ~~**EQ** — cheap; `Svf` already exists.~~ Done.
6. ~~**One modulation processor**~~ Done as a mode-selectable 3U insert:
   chorus, flange, phaser, ensemble, and ADT share stable parameter IDs and
   the ordinary effect event path. This is deliberately separate from the
   forthcoming modulator rack: its internal LFO is a sound algorithm, not a
   general control source. The future rack can modulate its descriptors like
   any other effect.
7. ~~**Reverb** — last; hardest to make good rather than merely present.~~ Done
   as a generated-room convolution IR player. `CONVOLUTION_REVERB.md` records
   its realtime/resource contract and measured-IR import boundary.

### The delay line is shared with the buffer device

This landed as `mooloop_dsp::delayline` (`DelayLine` + `ReadHead`). It is a
shared primitive, not part of the delay effect: `BUFFER_ENGINE.md`'s read-head
requirements are a superset of what a delay needs, so the buffer device should
build on it rather than growing a second ring.

What it provides: allocation only in the constructor, cubic Hermite
interpolation (linear aliases audibly under rate change and reverse),
equal-power crossfade on head discontinuity with a length-0 hard jump left
legal, and a documented minimum approach distance to the write head
(`MIN_READ_OFFSET`).

`ReadHead` deliberately does **not** know about playback rate or direction.
The caller passes how far the offset should drift per frame — `1 - rate`
forward, `1 + rate` reverse, because the write head is also moving. That one
decision is what lets a fixed delay tap, a repitching tape delay, a reverse
window, and the buffer device's detached heads all be the same type. The delay
effect exercises all three behaviors today, so the primitive is proven rather
than speculative.

What the buffer device still has to add: window length, explicit hold and
return-live operations, and freeze/snapshot. None of those need changes to
`DelayLine`.

## Anti-aliasing policy

Distortion and saturation are **2x oversampled**. A waveshaper run at base
rate folds its harmonics back down as inharmonic fizz, which is the difference
between a usable saturator and a bad one.

Bitcrush is **deliberately not oversampled**. Its aliasing is the effect.

State this per-effect in the DSP module docs so the choice reads as
intentional rather than inconsistent.
