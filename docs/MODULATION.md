# Modulation and parameters

Status: the approved design and its implementation contract, August–September
2026. Built: five module kinds, eight modules and sixteen routes per channel,
durable route identity, direct assignment on ordinary controls.

> Merged 2026-09-14 from `MODULATION.md` (the approved design) and
> `MODULATION.md` (the spec that expanded it). The split made sense
> while the thing was being designed and stopped making sense once it was
> built — two documents for one subject is two places to look and two places
> to drift. Dropped in the merge, as history rather than contract: the effect
> build order, every item of which was struck through as done; the spec's
> framing and "retain the existing foundation" sections; and its numbered
> delivery list, which had gone stale (outlets landed 2026-09-05). All of it
> is in git, and `JOURNAL.md` carries the narrative.

`AUDIO_ARCHITECTURE.md` owns preparation, execution and realtime lifecycle.
This document owns descriptors, addressing, ownership, and the resolution rule.


## What this document decides

The filter shipped as a complete vertical slice, which proved the effect
plumbing. Before adding ten more effects we need to settle how parameters are
addressed, modulated, and automated — otherwise every new effect hardcodes its
own ranges and the modulation system becomes a per-effect special case.

These decisions are made. Implement them; don't re-litigate them.

## Decisions

```text
CHANNEL (ownership)
│
├── source device → ordered insert rack → channel strip       audio path
│
├── modulation source collection                              control sources
│     LFO · step · random · macro · note value · device outlet · ...
│
└── explicit routes                                           control path
      source → transform → ParamAddr destination
```

- A **channel** owns its modulation sources and routes. A source is not the
  property of Mono, Buffer, an insert, or the strip when it is a reusable
  channel source or crosses a device boundary.
- A device may own modulation that is endemic to its synthesis algorithm:
  per-voice envelopes and note values, audio-rate oscillator relationships,
  or an authored instrument LFO. It persists those local routes with the
  device and may publish selected sources as typed outlets. This does not
  duplicate or transfer ownership of the channel rack.
- A device owns parameter descriptors and may publish named control outlets.
  It does not need to know which sources are connected to its parameters.
- `COMPOSABLE_DEVICE_UNITS.md` owns the general published/private port
  contract. This specification applies that contract to channel modulation:
  only deliberately published, typed control outlets enter the route system.
- The **common device frame** is the UI exposure point: it summarizes routes
  terminating in that device and opens the channel-owned shelf/inspector. It
  does not create a device-local modulator.
- A route adds an offset around a destination's base value; it never writes an
  absolute replacement value.
- The data is graph-capable, but the product is not graph-first. Patch cords
  and a full graph editor are deferred.

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

**Amended 2026-09-23, deliberately (Adam, MOO-74).** A hosted plugin's
parameters get their own owner, `ParamOwner::PluginParam { device }`, rather
than reusing `Effect { device }` with the plugin's id reinterpreted in `param`.
The rule above protects the *saved bytes* of existing addresses, and this
keeps them: no existing variant changes shape, and `size_of::<ParamAddr>()`
stays 16, because a variant whose only payload is a `DeviceId` fits in the
padding the other variants already have. What the rule must not be read to
forbid is an owner for a new *kind of namespace*. A plugin's `param` is the
plugin's own sparse `u32`, meaningful only against the instance's reported
parameter list, never against a `&'static` descriptor table. Folding it into
`Effect` would make `param` mean two different things with nothing on disk to
say which, and every exhaustive `owner` match (the integrity pass among them)
would judge a plugin id against a native table without a compiler error.
Adam's words: *"we don't want to not know what something belongs to"*.
`docs/plans/plugin-hosting/00-status.md`, "Parameters", has the ruling.

**The generator owner names its kind** (MOO-135, 2026-09-26, the same
ruling applied to a channel's source). A descriptor id is stable *per kind*,
and a channel's kind can change: id 12 is the sampler's Cutoff and the v1
drum synth's snare tone. So the owner is `ParamOwner::Source { kind }`, the
kind the address was made on, and the address still costs 16 bytes. A
resolver builds its addresses from the kind the channel runs now, so one
made on another kind matches nothing. It is **inert, never dropped**: kept,
saved back unchanged, and live again when the channel is switched back. This
is the "never drop" rule from MOO-74. On disk the owner is still spelled
`"source"`, and the kind is a sibling key (`PROJECT_FORMAT.md`), so the saved
bytes of every other address did not change.

This is deliberately a destination address, not a claim that every parameter
is already a legal modulation target. Descriptors declare range and curve;
destination metadata declares whether modulation is meaningful and how its
control signal is interpreted. A source, effect, Buffer, or strip should not
need to know which LFOs or other signals are currently connected to it.

## Ownership and data model

### Channel collection

`ChannelSetup` continues to contain one modulation collection. A channel owns
the sources it can play and all routes it makes. Project-global modulation is
not implied; a later global source is a new explicit scope/source kind.

The realtime representation may remain bounded and allocation-free. Capacity is
an engine protocol boundary, not a UI layout: the UI shows existing modules
plus **Add source**, never a fixed row of permanent empty bays. The grid's
rows follow the capacity constant and scroll, which is pinned by a test that
renders the shelf at eight and at sixteen. A larger
capacity must not alter persisted destination or route meaning.

That is now literally true rather than aspirational. `MAX_MODULATORS_PER_CHANNEL`
is a constant the layout obeys, modulation edits each name one fact so the
command ring no longer grows with capacity at all, and durable `ModSourceId`
means slot numbers are an implementation detail rather than something a saved
project depends on. Raising the number costs the DSP racks, the control outputs
and the meters -- all linear and all small. See
`docs/plans/archive/modulator-capacity/`.

### Sources and source metadata

A source produces a declared control signal. It is more general than an LFO and
is not necessarily a DSP device. A source must publish enough metadata for the
engine and UI to use it consistently:

```rust
struct ModSourceDescriptor {
    id: ModSourceId,             // stable within its channel/owner
    kind: ModSourceKind,
    name: String,                // user-renamable where useful
    signal: SignalShape,         // bipolar | unipolar | gate | stepped
    update: ControlRate,         // 32 frames, note event, block, ...
    latency: ControlLatency,     // explicit; outlets include one block
    trigger: TriggerPolicy,      // free, note-reset, note-advance, manual
}
```

`ModSourceId` is the durable source identity, and it landed. Rack devices have
since been given the same treatment for the same reasons — `DeviceId`, minted
on insertion, named by every route and lane, with the chain position derived —
so a modulation destination is now as reorder-proof as a modulation source.
See `docs/plans/archive/containers/01-a-device-is-an-identity.md`. It is minted when
a module is added, carried through reorders, and never reused. `source_slot`
survives only as the bounded runtime locator the realtime path indexes; it is
derived from `source` whenever the rack changes and is never authored. Legacy
routes saved before the id existed decode from their slot number. Reordering
the grid therefore moves a module without changing what any route means — the
one thing that did have to be remapped through the permutation is the Math
module's `input_slot`, a slot reference the user never sees.

| Kind | Meaning | Status |
| --- | --- | --- |
| LFO | Free-running or note-restarted periodic movement. | Implemented. |
| Envelope | Gate-driven attack, decay, sustain, and release contour. | Implemented with an explicit channel-note gate adapter; typed device gate outlets are planned. |
| Step / random generator | Clocked patterns, probability, and controlled variation. | Implemented as the Step and Random modules. |
| Macro / internal value | User macro, transport phase, velocity, key track, pressure, or another declared channel value. | Planned. The Math module covers user arithmetic over an existing module's output, not a channel value source. |
| Generator outlet | Generator-reduced values such as last-note velocity, gate, envelope, or Buffer state. | Implemented for the ML-P8's seven and DS-01's six, through `ModSourceRef::GeneratorOutlet` and the one-block control table, and offered by the shelf's own OUTLETS pane: a chip selects and arms exactly like a module, so the assign gesture builds the route. Only the control run is offered; an audio outlet is refused by domain. Note the range convention below. |
| Device outlet | Named effect signals such as gain reduction, envelope-following level, or gate state. | Planned. The vocabulary is shared with generator outlets; no effect declares one. |
| Audio-derived control | Explicit envelope follower, transient detector, or another control extractor. | Deferred until it has an outlet contract. |
| External / cross-channel control | Another channel's note gate is an explicit source-inlet adapter. MIDI/CV, buses, global sources, and general cross-channel outlets remain deferred by routing policy. | Note-gate adapter implemented; general routing deferred. |

A musical outlet is not display telemetry. Telemetry is best-effort observation
for meters, plots, and waveforms; it cannot drive parameters. A control outlet
has declared range, rate, and latency.

### Destinations and destination metadata

`ParamAddr` stays the stable destination identifier. A parameter descriptor
states how values map; it does not say whether modulation makes sense. Each
device kind and strip should therefore expose a sidecar declaration:

```rust
struct ModDestinationDescriptor {
    param: u32,                  // existing descriptor ID, not a new address
    allowed: bool,
    interpretation: ModInterpretation,
    default_polarity: ModPolarity,
    depth_limit: Range<f32>,
    smoothing: Option<Smoothing>,
}
```

The first interpretation is `NormalizedRange`: depth is a fraction of the
descriptor's entire normalized range. This exactly matches current
`ModRoute` behavior. A later musical mapping, such as bounded semitone pitch,
belongs here only when a real device needs it; it must still resolve via the
descriptor and emit the same ordinary `ParamValue` event.

Discrete modes, booleans, source selection, destructive actions, and structural
controls default to `allowed: false`. A stepped target must opt in and state
its hysteresis/quantization rules. This prevents an LFO from flapping a toggle,
switching an algorithm, or rebuilding a device.

The UI highlights only legal controls when a source is armed. The engine
rejects or ignores a route whose source, destination, or declaration is
invalid; project persistence retains it as an inspectable orphan rather than
silently deleting authored work.

### Routes and value resolution

Conceptually, a route is:

```rust
struct ModRoute {
    source: ModSourceRef,
    destination: ParamAddr,
    depth: f32,                  // signed normalized destination fraction
    polarity: ModPolarity,       // bipolar or unipolar
    // future: shaping, offset, and per-route smoothing
}
```

Initially, `ModSourceRef::LocalSlot(u8)` adapts to the current
`source_slot`. That is not a reason for LFO-only UI or a second matrix. One
source/destination pair is unique; reassigning it edits the existing route.
Different sources may share a destination and their offsets sum.

At each control tick:

```text
base = automation value when a lane is active; otherwise knob value
offset = sum(route_transform(source_output))
resolved = descriptor.from_normalized(clamp(to_normalized(base) + offset))
```

The base stays authored and visible. A knob changes the centre/floor underneath
active modulation; it neither removes a route nor fights the next LFO update.
Devices receive only `resolved`; the engine owns base and the route sum.

**Write precedence.** Three writers reach an effect parameter, and which one
the device hears is a stated rule, not an order of calls. The same table sits
on `control_events_for_slot` in `crates/mooloop-engine/src/render.rs`. The
base-plus-offset rule below is unchanged by `docs/plans/automation-curves/`;
what changed is the wire: rows one and two used to leave the engine as a
`ParamValue` event pushed at every control tick, and now leave as one curve —
a per-destination `[f32; ticks]`, handed to the node once a block through
`AudioNode::apply_curves` — with the same resolved value at the same tick.
**The carrier is now a curve, not a step of events.** A node without a native
curve path still receives the identical step of `ParamValue` events it always
did, converted from the curve by that trait method's default implementation,
so nothing downstream of a device's `process` had to change for this. Only
row three — the knob's own value, with no lane and no route — is still an
event on the wire, because it is not a curve: it fires once, at one offset,
not once a tick.

| Lane | Route | Base | Offset | Who writes the device |
| --- | --- | --- | --- | --- |
| yes | any | lane | routes, summed | the engine, every control tick |
| no | yes | knob | routes, summed | the engine, every control tick |
| no | no | knob | none | the knob's own value, queued once at the next block's first frame |

A lane is present when one with points covers the playhead, playing or
stopped. A route is present when the destination's policy accepts modulation.
A route to a destination that refuses it doesn't count. A knob edit always
updates the stored base. It sends the device a value only in the last row.
Under a lane, the knob isn't heard until the lane is cleared or stops covering
the playhead. The engine then hands the knob back. The engine answers "is a
lane present" in one place (`AutomationCurve::at`), so the per-tick resolution
and the knob edit can't disagree. A recorded lane, when recording lands, will
be written from the knob and will take over as the base on the next control
tick.

A bipolar route swings source `-1..1` about the base. A unipolar route maps
that output to `0..1`, making the base the floor. Signed depth inverts either
form without inventing another source. Clamp only after all offsets sum.

**The lift stands on the source's own span, not on the literal `1`.** A module
emits *into* `-1..1` and does not have to fill it: the envelope and the random
module scale by their own amount before lifting into the signed convention, so
they do fill it, while the LFO scales an already-signed waveform by `depth` and
so spans `-depth..depth`. Lifting that with `(v + 1) / 2` would rest the
destination `(1 - depth) / 2` above its base and — at depth zero — offset it
half a depth while producing no movement. `ModRack::wire_span` reads the span
off the module's params, which is also why an LFO's **fade-in** is outside it:
a fade is engine state and reading it per control tick would mean a second
table beside `ControlOutputs`.

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

The realtime implementation may use a fixed, bounded array (currently eight
module slots and sixteen routes per channel) because it makes the callback
predictable. That is an engine protocol boundary, not the product abstraction:
the UI presents a collection of existing sources plus an add action, never a
fixed row of permanent empty bays. Increasing capacity or admitting a new
source type must not change the persisted route vocabulary or the ordinary
interaction.

Per-channel, not project-global. It matches the rack UI and keeps a channel a
self-contained instrument. Project-global modulators can be added later as a
distinct source kind; nothing here blocks them.

A source is something that produces a normalized bounded control signal over
time, conventionally `-1..1` before route transformation. The first source was
an LFO; LFO, envelope, step, random, and math modules ship now. The taxonomy
is intentionally broader still: macros, note-derived values, named device
outlets, and eventually external control or audio-derived signals can all
participate if they declare their timing and value semantics. Do not make a
type or UI that assumes a modulator is only a little waveform generator.

### Mod matrix

Each explicit route is `(source_ref, ParamAddr, transform)`, where the
transform includes depth, polarity, and any later bounded shaping or offset.
Source references are stable source or outlet identities, not merely a
hard-coded slot number. Source metadata declares its label, signal shape
(bipolar, unipolar, gate, or stepped), control rate, and latency; destination
metadata declares that the parameter is legal to modulate.

The engine evaluates sources before their destinations at the declared control
rate, resolves the routes, and hands the destination a curve -- one resolved
value per control tick, through `AudioNode::apply_curves` -- rather than
pushing an event per tick onto the destination's list. The conceptual path is:

```text
source -> normalized control signal -> route transform -> ParamAddr
```

**No effect changes to support modulation. Ever.** That is still the whole
design, and it still holds under the curve path: `apply_curves`'s default
implementation turns the curve back into the exact `Event::ParamValue` step
an effect's ordinary block-splitting already knows how to consume, so a
device that has not opted into a native curve path never has to. Events with
sample offsets stay for what they are for -- notes, and the boundary a
future hosted plugin's parameter queue is built from -- and a curve becomes
events only there, or for a device that has not opted in.
`docs/plans/automation-curves/00-status.md`.

### Base value plus offset

The engine owns the parameter table: the **base** value per destination (what
the knob sets) and the sum of active **modulation offsets**. It emits the
resolved value.

Effects store only resolved values. Do not let the matrix write absolute
values directly — the user's knob and the LFO would fight, and turning a
modulated knob would snap it back. The UI needs both numbers anyway to draw a
knob with a modulation arc.

**A hosted plugin's parameter is the one exception to who holds the base**
(MOO-82, 2026-09-24). The plugin owns its values -- its own GUI moves them,
its presets load them -- so the engine keeps no base for it. A lane still
supplies the value, sent to the plugin in its plain units
(`Event::ParamValue`). A route is sent as an **offset** over whatever the
plugin holds (`Event::ParamMod`, CLAP's non-destructive parameter
modulation), so the rule is the same -- base plus offset, and they never
fight -- with the base kept by the plugin instead of the engine. The offset
is the route's normalized sum times the parameter's range, and the processor
sets it back to zero when the route goes, because CLAP's modulation holds
until it is changed. Whether a route may drive a plugin parameter is
`ModDestinationDescriptor::for_plugin_param`: continuous and marked
modulatable by the plugin. The engine finds a plugin's driven parameters by
walking the routes and lanes that name the device, not a descriptor table it
does not have (`EffectChain::plugin_curves`; MOO-195 generalizes the walk).

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

## Timing and realtime contract

Control sources run at the existing 32-frame subdivision. The final tick of a
block may be shorter and its event starts at the exact sample offset. This is
deliberately neither once-per-block nor audio-rate control: it avoids audible
fast-LFO stepping while keeping the callback bounded and allocation-free.

Source state advances once per subdivision before a device consumes its control
values. Reconfiguring a same-kind active source should preserve continuity (the
LFO currently preserves phase); deliberate resets follow the source trigger
policy.

An LFO may store either a free rate in hertz or a transport-relative cycle
duration. Musical divisions are durable values from `4/1` through `1/64T`, so
tempo changes bend the running oscillator without replacing its authored
setting or resetting phase. While the transport runs, a synced LFO that does
not retrigger on notes takes its phase from the song position -- beats over
its division, plus its phase offset, every control tick -- so Play, Seek and
an export all land it where the position implies, and its random steps are a
hash of the cycle number rather than a running generator (MOO-127,
2026-09-23). Stopped, it free-runs from where it was. Fade-in uses the same free/synced timing
vocabulary, begins when the source is installed, and restarts with a declared
note trigger. Output smoothing is a bounded one-pole slew at control rate;
square pulse width moves the high-to-low transition without changing the
route language. Note triggers are observed on the containing 32-frame control
subdivision, keeping the callback bounded and allocation-free.

An envelope stores an explicit input channel and ADSR values. Attack, decay,
and release use the same free/synced timing vocabulary as the LFO. Note On
restarts attack from the current value; the final held Note Off begins release,
so overlapping piano-roll notes keep the gate high. Runtime output stays in the
rack's signed convention and a new envelope route defaults to unipolar
polarity, making idle contribute no offset and sustain/peak rise above the
destination base.

Generator and device outlets publish into a per-channel control table. Consumers
read that table on the following block, with one declared block of latency.
This rule is mandatory: it makes realtime/offline results identical, prevents
graph-order accidents, and avoids same-block feedback exceptions. A generator
reduces per-voice values to a single named signal; the first policy may be
last-note, with alternatives added as explicit outlet modes. ML-P8 and DS-01
both reduce through a **focus** — the group or voice created by the most recent
Note On, held through its life so an envelope outlet has a coherent tail, and
falling to zero when it goes idle rather than stepping backward onto an older
sounding note.

**An outlet does not use the rack's signed convention, and a route's polarity
is about that convention.** A rack module always emits `-1..1`, which is why
`Unipolar` lifts it — `(v + 1) / 2` — so a one-way module contributes no
offset at idle. An outlet publishes in the range its `SignalShape` declares,
where a unipolar one is *already* `0..1`. So an outlet route takes the
destination's default polarity, `Bipolar`, which passes the value through:
zero rests at the base and one reaches full depth. Lifting a unipolar outlet
again would sit it half a depth above the base at idle and give it half the
swing. `Unipolar` stays meaningful on an outlet, but only for a genuinely
bipolar one such as ML-P8's `LFO`.

True audio-rate FM **through a channel route** and true audio sidechain are
excluded. A device's fixed/internal oscillator network is outside this
control-rate route contract. `AudioNode` currently has one in-place stereo bus;
true sidechain requires prepared typed auxiliary edges/process buffers and
graph latency compensation. Do not retain a borrowed source bus inside an
effect. A control-rate envelope follower exposed as an outlet is the correct
first audio-derived-control form.

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
complete 2x oversampling path (both 31-tap half-band FIR stages, 15 samples
each at the 2x rate, since MOO-250). Its internal dry path is aligned, but the graph does not yet
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

## User experience

### Shelf and common frame

The channel has one collapsed-by-default **MOD** shelf immediately beneath the
device rack. It lists existing source chips and **Add source**. Selecting a chip
opens its compact editor without arming assignment. The editor contains
source-owned controls (for an LFO: waveform, free/synced rate, free/synced
fade-in, phase, depth, smoothing, square pulse width, and retrigger; for an
envelope: gate input, free/synced attack/decay/release, sustain, and amount).
There is one shelf per channel, not a `MOD` page copied into Mono, Poly, Buffer,
and every effect.

Every common device frame shows `MOD n`, the number of routes that terminate
there, and may show source pills where more legible. Activating it opens the
same shelf focused on a destination-first route inspector. This is the UI
entry point where signal order is visible without falsely making sources
device-owned.

Source tiles are iconified summaries. Selecting one expands its source-owned
control surface without arming the rest of the rack. The expanded surface has
a separate **Assign** switch and any declared source inputs. For an LFO the
first input is reset/trigger: `Free` and channel `Note On` are current; a later
compatible-signal picker may add named generator, effect, Buffer, and
cross-channel outlets such as `Kick / Gate`. The picker binds a declared
control signal to a declared source inlet. It does not create a device-local
modulator or infer control data from telemetry.

Rate and fade-in place a clickable sync LED directly beside the knob. A dark
LED leaves the knob continuous; a lit LED turns that same gesture and readout
into the shared `4/1` through `1/64T` musical-division range. This compact
`O.` affordance is used consistently for source timing controls rather than
adding a second selector row for each one.

Today `Kick notes → Envelope / Gate → Sampler / Position` is readable and
playable in the ordinary rack. Later, `Kick / Gate → LFO / Reset → Sampler /
Position` uses the same inlet and route concepts once generators publish typed
outlets through the control table.

### Direct assignment

1. Open the MOD shelf and select a source tile to edit it.
2. Activate **Assign** for that source. Legal controls on the channel's source
   device and its inserts acquire a subtle assignable state; illegal controls
   do not.
3. Drag a normal control to create or adjust the armed source's route depth.
   Preserve the ordinary control's base value.
4. Keep the base readout; add a marker and modulation arc/range overlay for
   resolved excursion.
5. Clicking the marker opens a destination-first inspector listing incoming
   routes, source, polarity, depth, and a remove action.
6. Turning **Assign** off restores ordinary base-value editing while keeping
   the source selected for editing.

**Modulation targets devices, not the channel strip.** The strip's volume and
pan remain ordinary destinations in the engine and keep their descriptors, so
existing routes resolve and nothing needs migrating -- but no strip control
offers the assign gesture, and none is planned for now. "A modulator moves a
device parameter" is a rule the user can hold without exceptions, and the
mixer draws a strip per channel while routes belong to one channel, so an
assignable fader would have to explain which channel it meant.

The indicator carries four states, and each has to be legible at a glance
without a legend:

| State | Ring |
| --- | --- |
| Ordinary | Value arc in the accent colour |
| Assigning, unassigned | Track empty but for a short accent bar at the base |
| Assigning, assigned | The bar, plus the route's excursion span in the alert colour |
| Assigned, running | Value arc, plus a live alert-coloured arc out of its end, and one dot per route below |

Because the value arc and the modulation arc share a ring, a control may not
draw its *value* in the alert colour -- that would make "orange" mean two
things on the same knob.

The gesture is one undoable route edit, not a stream of unrelated parameter
edits. Re-dragging the same pair retunes it. Zero depth is a valid parked route;
the inspector offers explicit removal.

Patch cords are optional presentation, not a product taboo. The compact rack
and direct assignment remain the routine workflow; a future matrix/graph may
draw and edit the identical typed inlet and destination edges when a larger
patch benefits from it. It may not create parallel routes, implicit
modulation, or a new audio-rack model.

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

## Anti-aliasing policy

Distortion and saturation are **2x oversampled**. A waveshaper run at base
rate folds its harmonics back down as inharmonic fizz, which is the difference
between a usable saturator and a bad one.

Bitcrush is **deliberately not oversampled**. Its aliasing is the effect.

State this per-effect in the DSP module docs so the choice reads as
intentional rather than inconsistent.

## Scope boundaries and delivery order

This work includes the channel-owned model, destination metadata,
base-plus-offset resolution, current LFO continuity, the shelf/common-frame
interaction, and the direct-assignment inspector.

It excludes a general visual-programming environment; copying the same
general-purpose channel LFO into every device; general cross-channel/global
routing beyond the explicit channel-note gate adapter; true audio sidechain,
cross-device audio-rate FM, and control feedback cycles; and treating display
telemetry as control data. A device-specific LFO or per-voice modulation
system may remain local when it is an authored part of the instrument, works
without the channel rack, persists with the device, and publishes any
cross-device signal through the ordinary outlet contract. Transitional synth
LFOs that are merely generic channel modulators should still migrate instead
of growing a parallel system.

1. **Done.** Preserve `ModRack`/`ParamAddr`; add destination metadata and
   expose LFO routes in the channel shelf.
2. **Done.** Complete direct assignment, base/excursion feedback, destination
   inspector, and undo as the normal workflow.
3. **Done** for durable references and the step/random/math modules; macro and
   note-derived sources are not built. See
   `docs/plans/archive/modulator-modules/00-status.md`.
4. Add declared generator/effect/Buffer outlets through the one-block control
   table. **Next.**
5. After typed auxiliary graph edges and compensation exist, evaluate true
   sidechain and external routing. A graph UI, if useful, comes last.

## Acceptance criteria

- A source on one channel can target legal generator, multiple insert, and
  strip parameters on that channel without appearing as a device-owned LFO.
- Resolved values follow descriptor mapping, automation base, and summed route
  offsets at 32-frame resolution; devices receive normal timed `ParamValue`
  events only.
- Editing a modulated knob changes its base without deleting or fighting routes,
  and the UI communicates both base and excursion.
- Devices add parameters/outlets through metadata, not matrix special cases.
- Reordering the rack does not change durable destination identities; a future
  graph reconstructs every route from the same persisted data.
- The callback allocates nothing, locks nothing, performs no I/O, follows no
  implicit cross-channel edge, and never treats best-effort telemetry as
  modulation data.
