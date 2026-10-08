# Modulation and parameters

Status: the approved design and its implementation contract, August–October
2026. **Modulation is song-wide since 2026-10-06** (0.1.7): the song owns
every module and route, a route reaches any channel or track, and the modules
have a pane of their own. Adam, 2026-10-05: *"channels wont have
modulators"*; asked how many modulators a song can have, *"all of them"*; and
*"it will just go in its own pane"*. Built: five module kinds with no count
limit, durable route identity, direct assignment on ordinary controls, and
the Modulation pane.

`AUDIO_ARCHITECTURE.md` owns preparation, execution and realtime lifecycle.
This document owns descriptors, addressing, ownership, and the resolution rule.

## What this document decides

How parameters are addressed, modulated, and automated — settled once, so that
no new effect hardcodes its own ranges and the modulation system never becomes
a per-effect special case.

These decisions are made. Implement them; don't re-litigate them.

## Decisions

```text
SONG (ownership)
│
├── channels: source device → insert rack → strip             audio path
├── tracks:   insert rack → strip, the master's included
│
├── modulation source collection                              control sources
│     LFO · step · random · macro · note value · device outlet · ...
│
└── explicit routes                                           control path
      source → transform → ParamAddr destination, on any channel or track
```

- The **song** owns its modulation sources and routes. A source is not the
  property of Mono, Buffer, an insert, the strip or a channel when it is a
  reusable source or crosses a device boundary.
- A device may own modulation that is endemic to its synthesis algorithm:
  per-voice envelopes and note values, audio-rate oscillator relationships,
  or an authored instrument LFO. It persists those local routes with the
  device and may publish selected sources as typed outlets. This does not
  duplicate or transfer ownership of the channel rack.
- A device owns parameter descriptors and may publish named control outlets.
  It does not need to know which sources are connected to its parameters.
- `COMPOSABLE_DEVICE_UNITS.md` owns the general published/private port
  contract. This specification applies that contract to song modulation:
  only deliberately published, typed control outlets enter the route system.
- The **common device frame** is the UI exposure point: it summarizes routes
  terminating in that device, while the modules themselves live in the
  song's Modulation pane. It does not create a device-local modulator.
- A route adds an offset around a destination's base value; it never writes an
  absolute replacement value.
- The data is graph-capable, but the product is not graph-first. Patch cords
  and a full graph editor are deferred; the Song Patch proposal
  (`docs/plans/song-patch/README.md`) is the direction they would take.

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
lanes reference them) so they must never be renumbered once shipped —
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

A plugin **instrument**'s parameters use the same owner. The `device` is
the id the channel's source slot is given,
`ChannelSetup::source_device`, minted from the channel's own device ids when
a plugin becomes its source, so it never names one of the channel's effects.
There is no "source arm" of `PluginParam` and no `Source` owner holding a
plugin's id. Replacing the instrument forgets its lanes and routes, as
deleting an effect does, and undo brings them back; a plugin missing on load
keeps them.

**The generator owner names its kind** (2026-09-26, the same ruling applied
to a channel's source). A descriptor id is stable *per kind*, and a
channel's kind can change: id 12 is the sampler's Cutoff and the v1
drum synth's snare tone. So the owner is `ParamOwner::Source { kind }`, the
kind the address was made on, and the address still costs 16 bytes. A
resolver builds its addresses from the kind the channel runs now, so one
made on another kind matches nothing. It is **inert, never dropped**: kept,
saved back unchanged, and live again when the channel is switched back. This
is the "never drop" rule from MOO-74. An inert lane keeps its slot among a
pattern's eight, so `Session::inert_source_lanes` lists it for the lane
picker to show as missing, MOO-74's treatment, where it can be removed. An
existing one is reopened; `Session::lane_allowed` refuses to make a new one.
On disk the owner is still spelled `"source"`, and the kind is a sibling key
(`PROJECT_FORMAT.md`), so the saved bytes of every other address did not
change.

This is deliberately a destination address, not a claim that every parameter
is already a legal modulation target. Descriptors declare range and curve;
destination metadata declares whether modulation is meaningful and how its
control signal is interpreted. A source, effect, Buffer, or strip should not
need to know which LFOs or other signals are currently connected to it.

## Ownership and data model

### Song collection

The song holds one modulation set: `Project.modulation`, a `SongModulation`
(`crates/mooloop-core/src/modulation.rs`) of every module (`SongModule`) and
every route. A channel holds none. A route runs from one source to one
`ParamAddr` anywhere in the song: any channel's generator, inserts and strip,
and any track's inserts and strip, the master's included. The set is a
patch (`plans/song-patch/`): the modules are boxes, the song's tags bring
its sources in (a gate tag is one channel's notes), and wires run from an
outlet to an inlet, one wire per inlet. The patch ticks once per control
tick, before anything renders, in a compiled order: topological over the
wires, tags first, ties in list order (`CompiledModulation::compile`).

**What a control wire carries.** A value, and the events of the tick it was
sent. Only a gate tag sends events today: its channel's NoteOns, NoteOffs
and choke, the counts an Envelope's gate has always taken. Its value is 1
while any note is held on the channel and 0 otherwise.

- A **trigger inlet** (the LFO's `retrigger`, the Step's `advance` and
  `reset`, the Random's `trigger`) fires once a tick, on a NoteOn or when
  its wire rises through 0.5. A box made while its wire is already high
  does not fire for that. The LFO's retrigger, the Step's advance and the
  Random's trigger still act only in the modes that follow notes.
- The Envelope's **gate** counts a gate tag's notes as before (so
  overlapping notes and chokes behave as they did), and treats any other
  wire as a gate by its level: held while at or above 0.5.
- The LFO's **rate** adds its wire in octaves: ±1 moves the rate two
  octaves either way.
- Math's **in** is its operand.
- A gate box (song patch step 08) sends what a gate tag does from the notes
  that reach it, on its first outlet; its **pitch** and **velocity**
  outlets carry the latest NoteOn's, each `0..1`, and no events.

**Note boxes read a control inlet once a block.** The note pass runs before
the patch ticks, so a `transpose`'s semitones and a `chance`'s probability
inlet are read as the patch ticked them last, at the end of the block
before. A wire into transpose adds `round(value * 12)` semitones, so ±1 is
an octave; into chance it adds to the probability, clamped to `0..1`.

**A loop runs a tick late.** A wire that closes a loop reads its outlet as of
the previous control tick, which keeps every patch bounded and identical
realtime and offline (`AUDIO_ARCHITECTURE.md`'s rule that feedback is an
explicit delayed edge). The loop is broken at the first box in list order
still waiting. A wire saved `late` reads the previous tick too: songs from
before the patch read a module listed at or after the reader that way, and
keep doing so.

**There is no count a user meets.** The set is sized from the song, not
reserved: any edit that changes its shape (a module or route added, removed
or reordered, an input repointed, a channel or track moved) installs a whole
new set built off the audio thread (`StructuralCommand::SetModulation`), and
the callback carries each surviving module's state into it by id and hands
the old set back to be dropped off the audio thread. Only a module's params
and a route's depth and polarity are retuned in place. This is
`CAPACITY_POLICY.md`'s preallocate-and-grow-by-replacement rule. The UI shows
existing modules plus an **Add** list, never a fixed row of empty bays.

**A module keeps a home seat**, `SongModule::rack`: the channel it was made
on, and a slot there. A new module is named for that channel
(`<channel name> <kind> <n>`), and a channel preset saved from that channel
carries the modules seated there, plus any module whose route lands on it;
the engine does not read the seat. The first eight modules made on a channel
get one, and a module past that, or one whose channel has gone, has none and
is the song's alone.
A channel preset still carries its modulation as a `ModRack`, so the
`MAX_MODULATORS_PER_CHANNEL` (8) and `MAX_MOD_ROUTES_PER_CHANNEL` (16)
constants bound what one preset holds, not what a song holds.

A durable `ModSourceId` names every module, so its position in the list is an
implementation detail rather than something a saved song depends on.

### Sources and source metadata

A source produces a declared control signal. It is more general than an LFO and
is not necessarily a DSP device. A source must publish enough metadata for the
engine and UI to use it consistently:

```rust
struct ModSourceDescriptor {
    id: ModSourceId,             // unique in the song
    kind: ModSourceKind,
    name: String,                // user-renamable where useful
    signal: SignalShape,         // bipolar | unipolar | gate | stepped
    update: ControlRate,         // 32 frames, note event, block, ...
    latency: ControlLatency,     // explicit; outlets include one block
    trigger: TriggerPolicy,      // free, note-reset, note-advance, manual
}
```

`ModSourceId` is the durable source identity. Rack devices have the same
treatment for the same reasons — `DeviceId`, minted on insertion, named by
every route and lane, with the chain position derived — so a modulation
destination is now as reorder-proof as a modulation source.
See `docs/plans/archive/containers/01-a-device-is-an-identity.md`. It is minted when
a module is added, carried through reorders, and never reused. `source_slot`
is a runtime locator derived from `source` and never authored. Legacy routes
saved before the id existed decode from their slot number. Reordering the
module list therefore moves a module without changing what any route means,
and a Math module names the module it reads by id as well
(`InputSource::Module`).

**A source names its channel.** A module's input is an `InputSource`: none,
one channel's notes (`ChannelNotes`, by `ChannelId`), or, for Math, another
module (`Module`). A generator outlet or performance source
(`ModSourceRef::GeneratorOutlet`, `ModSourceRef::Performance`) carries the
`ChannelId` whose generator or keyboard it is. Nothing means "my channel"
implicitly.

| Kind | Meaning | Status |
| --- | --- | --- |
| LFO | Free-running or note-restarted periodic movement. | Implemented. |
| Envelope | Gate-driven attack, decay, sustain, and release contour. | Implemented with an explicit channel-note gate adapter; typed device gate outlets are planned. |
| Step / random generator | Clocked patterns, probability, and controlled variation. | Implemented as the Step and Random modules. |
| Macro / internal value | User macro, transport phase, velocity, key track, pressure, or another declared channel value. | Planned. The Math module covers user arithmetic over an existing module's output, not a channel value source. |
| Generator outlet | Generator-reduced values such as last-note velocity, gate, envelope, or Buffer state. | Implemented for the ML-P8's seven and DS-01's six, through `ModSourceRef::GeneratorOutlet` and the one-block control table, and offered in the Modulation pane's OUTLETS list, under each publishing channel's name: a chip selects and arms exactly like a module, so the assign gesture builds the route. Only the control run is offered; an audio outlet is refused by domain. Note the range convention below. |
| Device outlet | Named effect signals such as gain reduction, envelope-following level, or gate state. | Planned. The vocabulary is shared with generator outlets; no effect declares one. |
| Audio-derived control | Explicit envelope follower, transient detector, or another control extractor. | Deferred until it has an outlet contract. |
| Another channel's control | Any channel's notes as a module's input, and any channel's generator outlets and performance controls as a route's source, each naming its channel. | Implemented. |
| External control | MIDI/CV and other sources from outside the song. | Deferred. |

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
descriptor into the same natural-unit value every other route delivers.

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
on `control_events_for_slot` in `crates/mooloop-engine/src/render.rs`. Rows
one and two leave the engine as one curve — a per-destination `[f32; ticks]`,
handed to the node once a block through `AudioNode::apply_curves` (see "Mod
matrix") — with the resolved value at each tick. Only row three — the knob's
own value, with no lane and no route — is an event on the wire, because it is
not a curve: it fires once, at one offset, not once a tick.

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
and the knob edit can't disagree. Nothing records a knob into a lane: points
are drawn by hand. A lane written from the knob, when automation write modes
exist (open: MOO-478), will take over as the base on the next control tick.

A bipolar route swings source `-1..1` about the base. A unipolar route maps
that output to `0..1`, making the base the floor. Signed depth inverts either
form without inventing another source. Clamp only after all offsets sum.

**The lift stands on the source's own span, not on the literal `1`.** A module
emits *into* `-1..1` and does not have to fill it: the envelope and the random
module scale by their own amount before lifting into the signed convention, so
they do fill it, while the LFO scales an already-signed waveform by `depth` and
so spans `-depth..depth`. Lifting that with `(v + 1) / 2` would rest the
destination `(1 - depth) / 2` above its base and — at depth zero — offset it
half a depth while producing no movement. `CompiledModulation::wire_span`
reads the span off the module's params, which is also why an LFO's **fade-in**
is outside it: a fade is engine state and reading it per control tick would
mean a second table beside the modules' outputs.

## Modulation architecture

### Modulator rack

The song owns one set of modules and one routing matrix (see "Decisions" and
"Song collection"). It does not strip an authored instrument of endemic
modulation: per-voice envelopes, velocity/key/gate relationships, audio-rate
oscillator routing, and a device-specific LFO with saved internal routes
cannot in general be reproduced after a chord has been reduced to one control
value.

Song-wide, not per channel. Adam, 2026-10-05: *"channels wont have
modulators"*. A copied or pasted channel brings the routes that point into
it, aimed at the copy, from the same modules (*"assignments would
copy/paste"*); a channel preset brings the modules its routes use and adds
them to the song.

A source is something that produces a normalized bounded control signal over
time, conventionally `-1..1` before route transformation. The first source was
an LFO; LFO, envelope, step, random, and math modules ship now. The taxonomy
is intentionally broader still: macros, note-derived values, named device
outlets, and eventually external control or audio-derived signals can all
participate if they declare their timing and value semantics. Do not make a
type or UI that assumes a modulator is only a little waveform generator.

### Mod matrix

The engine evaluates sources before their destinations at the declared control
rate, resolves the routes, and hands the destination a curve -- one resolved
value per control tick, through `AudioNode::apply_curves` -- rather than
pushing an event per tick onto the destination's list. The conceptual path is:

```text
source -> normalized control signal -> route transform -> ParamAddr
```

**No effect changes to support modulation. Ever.** That is the whole
design, and it holds under the curve path: `apply_curves`'s default
implementation turns the curve back into the exact `Event::ParamValue` step
an effect's ordinary block-splitting already knows how to consume, so a
device that has not opted into a native curve path never has to. Events with
sample offsets stay for what they are for -- notes, and a hosted plugin's
parameter queue, where a lane arrives as `ParamValue` and a route's offset as
`ParamMod` -- and a curve becomes events only there, or for a device that has
not opted in.
`docs/plans/archive/automation-curves/00-status.md`.

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
does not have (`resolve_plugin_curves` in `render.rs`, for a chain's devices
and for a channel's plugin instrument alike; MOO-195 generalizes the walk).
A knob or MIDI control on a plugin parameter sends the plugin's own id and
plain value in the command its device takes (`SetEffectParam` on a chain,
`SetChannelGeneratorParam` for the instrument), and a lane writing that
parameter holds it back.

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
hash of the cycle number rather than a running generator. Stopped, it
free-runs from where it was. Fade-in uses the same free/synced timing
vocabulary, begins when the source is installed, and restarts with a declared
note trigger. Output smoothing is a bounded one-pole slew at control rate;
square pulse width moves the high-to-low transition without changing the
route language. Note triggers are observed on the containing 32-frame control
subdivision, keeping the callback bounded and allocation-free.

An envelope's input names one channel's notes (`InputSource::ChannelNotes`),
and it stores ADSR values. Attack, decay,
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

True audio-rate FM **through a channel route** ("Control rate, not audio
rate") and true audio sidechain (below) are excluded. A control-rate envelope
follower exposed as an outlet is the correct first audio-derived-control form.

## Note-triggered effects

An insert never sees a note. Its node gets its own slot's queue
(`EffectSlot.events` in `render.rs`: a knob's value at the next block's first
frame, and Buffer gestures) and the block's lane and route curves through
`apply_curves`. The channel's event list, with its notes, goes to the
generator alone, and the generator never sees an insert's events. Notes also
reach every module whose input is that channel's notes, as per-tick note
gates. Keep that
isolation for parameter events, but **give effect slots access to the
channel's note stream as a separate input**.

This is what makes the rack an instrument rather than a chain of processors.
The rack's own modules already hear notes: an LFO resets its phase on
note-on, and a step module advances per note. What no insert can do is react
to one itself: a delay that flushes on a note, a stutter fired from a rack
step. The sample-accurate note pipe reaches every channel's generator and the
modules listening to it; it stops one node short of the inserts.

## Inter-device and inter-channel data

**Within the song:** the mod matrix covers it. A route crosses channels and
reaches tracks: a source on one channel may drive a parameter on any other
channel, on any track's inserts and strip, and on the master's, and every
source that reads a channel names it (see "Sources and source metadata").
Effects may expose outlet signals (a compressor's gain reduction, an envelope follower's output, a
gate's open state) as modulator sources. The dynamics effects already compute
exactly these internally; exposing them is a matter of publishing the value,
not of new DSP.

### Generator outlets

Generators also publish named, channel-rate outlets. This is how note-derived
data reaches an effect without pretending a shared channel effect can own
per-voice state: a generator reduces its voices to one musical control signal,
then a downstream effect consumes ordinary CV. ML-P8 and DS-01 publish them
(ML-P8's include Velocity, Gate and its LFO). The Modulation pane's OUTLETS
list shows every publishing channel's outlets under the channel's name, and a
route takes one as its source (`ModSourceRef::GeneratorOutlet`) to any legal
destination in the song.

An outlet source names its channel by `ChannelId` and an outlet id on that
channel's generator, plus its user-facing name. The first reduction is last-note; a later explicit outlet mode can add
highest or loudest note without changing routing. An outlet source is a
sibling of an LFO, not telemetry: its smoothing is part of its musical
contract, because an
unsmoothed velocity step can click a filter cutoff. Outlets are read one
block later, under the timing rule above.

Buffer outlets follow the same rule if and when the Buffer earns them;
`BUFFER_ENGINE.md` lists the candidates.

**Across channels and tracks: built** (0.1.7). `ParamAddr` already carried
a channel-or-bus scope, so it was a routing-policy change rather than a
retyping of every engine command: the session's
`Session::modulation_destination` takes any scope, and the engine files each
route under the chain it lands on (`CompiledModulation::chain_routes`).

**True audio sidechain: still deferred.** A sidechain is a dependency edge
in addition to ordinary audio routing: the source must be
scheduled before the consumer even though its signal is not summed into that
consumer's main input. `compile_bus_graph` currently models only each bus's one
audio destination, and `AudioNode::process` currently accepts only one in-place
stereo bus. Extend both through the process-buffer and typed-edge design in
`AUDIO_ARCHITECTURE.md`; do not retain a borrowed source bus inside an effect.

Control-rate ducking does not need any of this: it publishes modulator outputs
into the per-channel table read on the *following* block. That remains the
cheaper and more musical first move.

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

### The Modulation pane and common frame

The song's modules live in the **Modulation** pane, a view of its own
(`PaneViews.modulation`; **Show Modulation**, `view.pane-modulation`,
Ctrl+6), not in the device rack. It lists the song's modules as chips, the
outlets of every channel that publishes some, and an **Add** list. Selecting
a chip opens its editor without arming assignment. The editor contains
source-owned controls (for an LFO: waveform, free/synced rate, free/synced
fade-in, phase, depth, smoothing, square pulse width, and retrigger; for an
envelope: gate input, free/synced attack/decay/release, sustain, and amount).
Beside it are the selected source's routes, from the whole song, each naming
the chain, device and parameter it reaches. There is one pane for the song,
not a `MOD` page copied into Mono, Poly, Buffer, and every effect.

A common device frame shows the routes that terminate there, and may show
source pills where more legible. Activating that summary reveals the
Modulation pane focused on a destination-first route inspector. This is the
UI entry point where signal order is visible without falsely making sources
device-owned. **Not built yet:** the faces draw route-count dots under each
modulated parameter, but clicking one raises nothing, so it cannot select
its module or reveal the pane (open:
`docs/plans/archive/song-modulation/00-status.md`).

Source tiles are iconified summaries. Selecting one expands its source-owned
control surface without arming the rest of the pane. The expanded surface has
a separate **Assign** switch and the module's one input picker
(`InputSource`): none, then every channel's notes for the four kinds that
hear notes, or every other module in the song for Math. An LFO's
`Free | Note On` reset reads that input. A later picker may add named
generator, effect and Buffer outlets such as `Kick / Gate`. The picker binds
a declared control signal to a declared source inlet. It does not create a device-local
modulator or infer control data from telemetry.

Rate and fade-in place a clickable sync LED directly beside the knob. A dark
LED leaves the knob continuous; a lit LED turns that same gesture and readout
into the shared `4/1` through `1/64T` musical-division range. This compact
`O.` affordance is used consistently for source timing controls rather than
adding a second selector row for each one.

Today `Kick notes → Envelope / Gate → Sampler / Position` is readable and
playable, with the kick and the sampler on different channels if need be.
Generators publish typed outlets through the control table, and a route can
take one as its source, but a module's input cannot: it is a channel's notes
or another module. `Kick / Gate → LFO / Reset → Sampler / Position` uses the
same input and route concepts once an input can bind a declared generator
outlet.

### Direct assignment

1. Open the Modulation pane and select a module or outlet to edit it.
2. Activate **Assign** for that source. Legal controls on any channel's
   source device and inserts, and on any track's inserts, acquire a subtle
   assignable state; illegal controls do not.
3. Drag a normal control to create or adjust the armed source's route depth.
   Preserve the ordinary control's base value.
4. Keep the base readout; add a marker and modulation arc/range overlay for
   resolved excursion.
5. Clicking the marker opens a destination-first inspector listing incoming
   routes, source, polarity, depth, and a remove action.
6. Turning **Assign** off restores ordinary base-value editing while keeping
   the source selected for editing.

**A strip is a destination; its gesture is not built yet.** A channel's and
a track's volume and pan, the master's included, take routes: the session
accepts them (`Session::modulation_destination`) and the engine moves them.
Since a route names the channel or track it reaches, an assignable fader no
longer has to explain which channel it meant. But no fader or pan control
raises the assign gesture yet (the `strip-modulation-*` callbacks in
`main.slint` are declared and nothing raises them), so a route onto a strip
cannot be made by dragging one (open:
`docs/plans/archive/song-modulation/00-status.md`).

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

## Anti-aliasing policy

Hard and folding curves are **2x oversampled** (`Oversampler2x`), and the
Drive effect is the device that runs them. A waveshaper run at base rate folds
its harmonics back down as inharmonic fizz, which is the difference between a
usable saturator and a bad one.

Smooth `tanh`-family curves run at the **base rate**, by policy, because their
harmonics fall away fast enough that a voice's own filter and the 0.45 x sample
rate ceiling keep folded content inaudible. That is `apply_drive` (the
sampler, drum synth and both synths), `PreDrive`, the `tanh` and Tape shapers
inside the Ladder and Acid filters, and the Filter effect's drive. The rule
lives in `shaper.rs`.

Bitcrush is **deliberately not oversampled**. Its aliasing is the effect.

State this per-effect in the DSP module docs so the choice reads as
intentional rather than inconsistent.

## Scope boundaries and delivery order

This work includes the song-owned model, destination metadata,
base-plus-offset resolution, current LFO continuity, the pane/common-frame
interaction, and the direct-assignment inspector.

It excludes a general visual-programming environment (the Song Patch
proposal, `docs/plans/song-patch/README.md`, is where that would start);
copying the same general-purpose LFO into every device; true audio sidechain,
cross-device audio-rate FM, and control feedback cycles; and treating display
telemetry as control data. A device-specific LFO or per-voice modulation
system may remain local when it is an authored part of the instrument, works
without the song's modules, persists with the device, and publishes any
cross-device signal through the ordinary outlet contract. Transitional synth
LFOs that are merely generic modulators should still migrate instead
of growing a parallel system.

The song-owned model, direct assignment, durable references, the
step/random/math modules, generator outlets and the Modulation pane are built
(`docs/plans/archive/song-modulation/` records the move from per-channel
racks); macro and note-derived sources are not (`docs/plans/archive/modulator-modules/00-status.md`).
Next, declared effect and Buffer outlets through the one-block control table.
After typed auxiliary graph edges and compensation exist, evaluate true
sidechain and external routing. A graph UI, if useful, comes last.

## Acceptance criteria

- A source can target legal generator, insert, and strip parameters on any
  channel, and insert and strip parameters on any track, the master's
  included, without appearing as a device-owned LFO.
- Resolved values follow descriptor mapping, automation base, and summed route
  offsets at 32-frame resolution. Devices receive them as per-tick curves
  through `AudioNode::apply_curves`, whose default hands a device with no
  native curve path ordinary timed `ParamValue` events.
- Editing a modulated knob changes its base without deleting or fighting routes,
  and the UI communicates both base and excursion.
- Devices add parameters/outlets through metadata, not matrix special cases.
- Reordering the rack does not change durable destination identities; a future
  graph reconstructs every route from the same persisted data.
- The callback allocates nothing, locks nothing, performs no I/O, follows no
  implicit cross-channel edge, and never treats best-effort telemetry as
  modulation data.
