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

Every addressable device kind and strip publishes a static table of
`ParamDescriptor`:

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

### Stable destinations: `ParamAddr`

`ParamAddr` is the stable destination address used by automation and
modulation. It combines a channel-or-bus scope, an owning surface (source,
effect slot, modulator slot, or strip), and that owner's stable descriptor id.
It is persisted and must never be retyped merely because a new routing surface
is added.

This is deliberately a destination address, not a claim that every parameter
is already a legal modulation target. Descriptors declare range and curve;
destination metadata declares whether modulation is meaningful and how its
control signal is interpreted. A source, effect, Buffer, or strip should not
need to know which LFOs or other signals are currently connected to it.

## Modulation architecture

### Modulator rack

Each channel owns one modulation rack and routing matrix. Neither belongs to
an individual source or insert. A device supplies parameters and may publish
named control outlets; the channel owns the source collection that can use
those outlets and the routes that terminate in devices or the strip.

This governs reusable channel sources and every route that crosses a device
boundary. It does not strip an authored instrument of endemic modulation. A
polysynth may own per-voice envelopes, velocity/key/gate relationships,
audio-rate oscillator routing, and a device-specific LFO with saved internal
routes. Those cannot in general be reproduced after the channel has reduced a
chord to one control value. Selected internal signals become channel sources
only by being published through the typed outlet contract below.

The realtime implementation may use a fixed, bounded array (currently four
local source positions) because it makes the callback predictable. That is an
engine protocol boundary, not the product abstraction: the UI presents a
collection of existing sources plus an add action, never four permanent empty
bays. Increasing capacity or admitting a new source type must not change the
persisted route vocabulary or the ordinary interaction.

Per-channel, not project-global. It matches the rack UI and keeps a channel a
self-contained instrument. Project-global modulators can be added later as a
distinct source kind; nothing here blocks them.

A source is something that produces a normalized bounded control signal over
time, conventionally `-1..1` before route transformation. The initial source
is an LFO. The taxonomy is intentionally broader: step and random generators,
macros, note-derived values, envelopes, named device outlets, and eventually
external control or audio-derived signals can all participate if they declare
their timing and value semantics. Do not make a type or UI that assumes a
modulator is only a little waveform generator.

### Mod matrix

Each explicit route is `(source_ref, ParamAddr, transform)`, where the
transform includes depth, polarity, and any later bounded shaping or offset.
Source references are stable source or outlet identities, not merely a
hard-coded slot number. Source metadata declares its label, signal shape
(bipolar, unipolar, gate, or stepped), control rate, and latency; destination
metadata declares that the parameter is legal to modulate.

The engine evaluates sources before their destinations at the declared control
rate, resolves the routes, and emits `Event::ParamValue` into the
destination's existing event path. The conceptual path is:

```text
source -> normalized control signal -> route transform -> ParamAddr
```

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

This means no audio-rate FM of a filter cutoff **through a channel route**.
That is a deliberate limit. Stepped, sequenced modulation is stylistically
correct for the music this instrument targets, and cross-device audio-rate
modulation is a much larger engine change that can come later if it earns its
way in. Fixed or authored audio-rate paths inside one prepared DSP device are
not routed by this matrix and are not prohibited by it.

### Rack semantics, graph-capable model

The ordered device rack remains the normal presentation and audio workflow.
The modulation model is graph-capable only in the useful, narrow sense that
cross-device sources, destinations, routes, timing, and latency are explicit
data. Authored device-local modulation also has persisted, inspectable source
and destination identities, but may execute inside a voice where channel-rate
routing cannot preserve its semantics. A future zoomed-out graph view can
visualize published boundaries and the channel routes alongside the audio
chain; it must not introduce a parallel cross-device modulation engine or
redefine the rack model.

Do not build that graph editor in this pass. Routine modulation is a
source-selection and direct-manipulation interaction, not a matrix or a field
of patch cords. A full matrix may later serve inspection and expert editing,
but it is not the ordinary workflow.

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

### Generator outlets

Generators also publish named, channel-rate outlets. This is how note-derived
data reaches an effect without pretending a shared channel effect can own
per-voice state: a generator reduces its voices to one musical control signal,
then a downstream effect consumes ordinary CV. A sampler or synth may, for
example, assign velocity to an outlet; the channel adds a `DeviceIn` source,
chooses that named outlet, and supplies trim and smoothing for routes to any
legal destination.

An outlet address is `(channel, outlet index)` plus its user-facing name.
The first reduction is last-note; a later explicit outlet mode can add highest
or loudest note without changing routing. `DeviceIn` is a sibling of `Lfo`,
not telemetry: its smoothing is part of its musical contract, because an
unsmoothed velocity step can click a filter cutoff.

Generators publish outlets into a per-channel table. Consumers read the table
on the following block, with exactly one block of declared latency. That makes
offline and realtime behavior identical and leaves graph order irrelevant; do
not add a same-block exception. These outlets remain distinct from the display
telemetry bank below, which is observation-only and has no audio timing
contract.

Buffer outlets follow the same rule if and when the Buffer earns them. Useful
candidates include normalized playhead position, distance from the write head,
window or loop phase, amplitude, transient state, and slice state. They are
musical control signals only when declared with a rate and latency; the UI
must never infer them by sampling a waveform display or telemetry snapshot.

**Across channels: deferred, by decision.** Not in this pass. `ParamAddr`
already carries a channel-or-bus scope, so enabling cross-channel control
later is a routing-policy change rather than a retyping of every engine
command.

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

## Modulation UI

The channel has one collapsed-by-default modulation shelf beneath its device
rack. It lists the channel's existing source chips and an add-source action;
it is not a page inside Mono, Poly, Buffer, or an effect. The common device
frame exposes the shelf where users are already reading signal order.

Every device header shows a compact `MOD n` summary for the number of routes
that terminate in that device, with optional source pills when that is clearer
than a count. Activating the summary opens an inspector filtered to that
device; it does not move or duplicate the modulation sources. The inspector
is destination-first, for example `LFO 1 -> Cutoff +28%`, and is where a route
can be reviewed or removed without opening a general matrix.

Selecting a source chip arms it. Every legal ordinary control becomes visibly
assignable; dragging that control establishes or adjusts the selected source's
route depth. The control retains its base value. A modulation marker or
overlay shows the resulting excursion, and a parameter inspector can list its
base value and all incoming routes. Deselecting the source returns ordinary
control manipulation to normal.

There are no patch cords in this workflow. Inlets and outlets are explicit in
the model, but their routine presentation is source selection, destination
markers, overlays, and inspectors. The future matrix/graph view is an expert
view of the same routes, not a prerequisite for using them.

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
   as an eight-line feedback delay network. `REVERB.md` records its realtime
   contract. It shipped first as a generated-room convolution IR player, which
   was the one device that could not honour "no effect changes to support
   modulation" above: a convolution node cannot take a parameter event, so
   routes aimed at its knobs did nothing. That is the concrete reason it was
   replaced rather than tuned.

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
