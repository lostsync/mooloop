# Modulation and parameters

Status: the approved design and its implementation contract, August–October
2026. **Modulation is song-wide since 2026-10-06, and a patch since
2026-10-13** (0.1.7): the song owns every box, tag, wire and route, a route
reaches any channel or track, and the patch is drawn as one canvas in a pane
of its own. Adam, 2026-10-05: *"channels wont have modulators"*; asked how
many modulators a song can have, *"all of them"*; *"it will just go in its
own pane"*; and on what the rack was meant to be, 2026-09-23: *"if you wanna
do node based processing, maybe just draw some damn nodes like a grownup."*
Built: twenty box names typed as text with no count limit, control and note
wires, song inlet and notes tags, patch presets, durable identity, and
direct assignment on ordinary controls and on a box's own knobs. The plans
that built it are `docs/plans/archive/song-modulation/` and
`docs/plans/archive/song-patch/`.

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
├── the patch                                                 control and notes
│     boxes · tags (the song's sources in, notes out) · wires
│
└── explicit routes (assignments)                             control path
      outlet → transform → ParamAddr destination, on any channel or track
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
- The song's modulation is a graph, and the product draws it as one: the
  Modulation pane is a patching canvas of boxes and wires. The audio path is
  not: the device rack stays an ordered chain, and nothing on the canvas
  carries audio.

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

### The song's patch

The song holds one modulation set: `Project.modulation`, a `SongModulation`
(`crates/mooloop-core/src/modulation.rs`, the patch half in
`crates/mooloop-core/src/patch.rs`). It is a patch: **boxes** (`modules`,
each a `SongModule`), **tags** (`tags`, each a `SongTag`), the **wires**
between their jacks (`wires`), and the **routes** from an outlet to a
parameter (`routes`). A channel holds none. Boxes and tags are named by a
`ModSourceId` from one mint, so a wire names either the same way.

**A box** is typed as text and spelled back from its settings
(`mooloop_core::box_text`: `parse`, `spell`, and `VOCABULARY`, the thirty
names in the order the completion list offers them). A box keeps its
settings as parameters (`ModulatorParams`), never as text, so `*   -.5`
typed reads `* -0.5`, and a knob turned on its face changes what the box
says. The control boxes are `lfo`, `env`, `step`, `random`, `counter`,
`select`, `switch`, `gate`, `slew`, `num`, `hold` and `table`, and the arithmetic boxes `+ - * / min max clip` (one
kind, Math, with an operator). The note boxes are `chord`, `modal`,
`scale`, `transpose`, `chance`, `held`, `pick`, `unpack` and `merge`
(and a `switch` or a `gate` on notes), `note` makes notes from control,
and `wait` has a lane of each; their pitch, chord and mode tables
are `crates/mooloop-core/src/harmony.rs`. A name the vocabulary lacks makes
an **unknown** box that keeps its text and does nothing; a song from a later
build with a kind this one does not know opens with that box unknown.

**Jacks.** Each kind declares its inlets and outlets in port order
(`ModulatorKind::ports`, and `ModulatorParams::ports` where the settings
change them: a `select 6` has `index` and `a` to `f`, a `switch 3` and an
`unpack 3` have outlets `a` to `c`, a `merge 3` note inlets `a` to `c`,
`clip` has no operand), and each jack is one of two sorts (`JackSort`): **control**, a
value every control tick, or **note**, notes with each NoteOff following
its NoteOn. A wire (`Wire`) runs from one outlet to one inlet of the same
sort. **An inlet takes one wire**: two signals into one inlet are summed by
a `+` box, so the patch reads the way it runs. A wire may carry a hand-made
`bend`, and `late` (below).

**Tags** are boxes with no face, at the patch's edges. They are made from
the canvas's menu, not typed (`TagKind`):

- An **inlet** brings a control source from the song (`InletSource`): a
  channel's notes as a **gate** (1 while a note is held, firing on each
  NoteOn), one of a channel's generator **outlets**, a channel's mod wheel or
  aftertouch (**performance**), or the transport: **Beat** and **Bar** (a
  ramp 0 to 1 that fires on each, counted from the pattern's start in
  Pattern mode and the song's in Song mode), **Pattern position** (a ramp
  across the playing pattern, firing as it starts) and **Pattern** (the
  playing pattern's number, 0 to 1 across the song's patterns, firing when
  it changes; in Song mode the lowest-numbered pattern playing).
- A **notes in** tag brings a channel's notes into the patch, copied, or
  **taken** so that only the patch's output reaches the channel.
- A **notes out** tag plays notes on a channel.

A tag bound to nothing is an empty `[ ]` slot and sends nothing. One tag per
source: loading a song, landing a preset or pasting a channel finds the
song's tag for a source before making one.

**The patch ticks once per control tick, before anything renders**, in a
compiled order (`CompiledModulation::compile`, in
`crates/mooloop-core/src/modulation_plan.rs`): topological over the wires
(Kahn's), tags first, ties in list order. The audio thread runs the order
and never sees the graph. A box is named by its id, never by its place in
the list or on the canvas, so the list order is a tie-break and nothing
else.

**What a control wire carries.** A value, and the events of the tick it was
sent: a gate tag's channel NoteOns, NoteOffs and choke, the counts an
Envelope's gate has always taken, and a transport tag's fire, which a
trigger inlet takes as a NoteOn.

- A **trigger inlet** (the LFO's `retrigger`, the Step's and the counter's
  `advance` and `reset`, the Random's `trigger`) fires once a tick, on a
  NoteOn or when its wire rises through 0.5. A box made while its wire is
  already high does not fire for that. The LFO's retrigger, the Step's
  advance and the Random's trigger still act only in the modes that follow
  notes.
- The Envelope's **gate** counts a gate tag's notes (so overlapping notes
  and chokes behave as they always did), and treats any other wire as a
  gate by its level: held while at or above 0.5.
- The LFO's **rate** adds its wire in octaves: ±1 moves the rate two
  octaves either way.
- An arithmetic box's **in** is what it works on, and a wire into its
  **operand** replaces the typed operand.
- A **counter** sends its count as `0..1` across its steps, so a `select`
  of as many inputs reads it directly; a select reads its **index** as
  `0..1` across its inputs. A **slew** follows its **in**.
- A **num** box shows the value it sends, beside its spelling. With nothing
  wired in it sends its own value, set by its knob or typed (`num 0.25`);
  with a wire in, it passes the wire's value. Either way the value is kept
  inside its range (`num 0 16`, `-1..1` untyped, `0..1` for `num note`) and
  rounded for `num int`. Its knob is a position across the range, so a
  route onto it sweeps the range. A num box may send past `-1..1` on its
  wires (`num int 0 16` into a `/ 16`); **a route reads every source
  inside `-1..1`**, so an assignment never sees more.
- A **hold** box keeps its **in** as of its last **trigger**, 0 before the
  first.
- A **switch** of n (`switch 3`) sends its **in** to the outlet its
  **which** picks (`0..1` across n, as a select's index); the others send
  0. A route from a switch reads its first outlet. A **gate** passes its
  **in**, value and events, while its **open** is at or above 0.5, and
  sends 0 otherwise; with nothing wired into **open** it is closed. **A
  switch and a gate take the sort of the first wire into `in`**: a note
  wire there makes `in` and the outlets note jacks (and drops the wires
  out of it that no longer fit), a control wire makes them control again,
  and retyping keeps the sort (`ModSwitchParams::notes`,
  `ModGateParams::notes`). On notes, see the note boxes below.
- A **Step** box's steps, and a **table**'s values, are bars drawn with the
  mouse on its open face, the playing step or the slot read lit. A Step is
  bipolar (`-1..1`) or, typed `step 8 uni`, unipolar (`0..1`, a step held
  below zero playing as zero). A **table** of n values (`table 8`) sends
  the value at its **index** (`0..1` across n), and on each **write** stores
  its **value** wire there. What is drawn is saved; what a wire writes lasts
  until the set is rebuilt, and a value drawn again replaces what was
  written there.
- A **held** box sends what a gate tag does from the notes that reach it,
  on its first outlet; its **pitch** and **velocity** outlets carry the
  latest NoteOn's, each `0..1`, and no events.

**A loop runs a tick late.** A wire that closes a loop reads its outlet as of
the previous control tick, which keeps every patch bounded and identical
realtime and offline (`AUDIO_ARCHITECTURE.md`'s rule that feedback is an
explicit delayed edge). The loop is broken at the first box in list order
still waiting, and the canvas marks that wire. A wire saved `late` reads the
previous tick too: a song from before the patch ran its modules in list
order, and a wire from a module listed at or after its reader is saved late
on conversion, so the song plays as it did. A wire made on the canvas is
never late by itself.

**What reads a block late.** A generator's outlet is published as its
channel renders, after the patch has ticked, so an inlet tag bound to one
reads it as of the end of the block before (`InletSource::late`), as a
route from an outlet always has; its tag says **late**. A gate tag, a
performance tag and the transport tags are read in the tick they happen.

**Note wires.** The note pass (`crates/mooloop-engine/src/note_patch.rs`)
runs once a block, after every channel's event list is complete (sequencer
notes, owed releases, chokes, the keyboard and auditions) and before
anything renders, so a note sent from one channel to another arrives in the
same block at the same offset, live and in a render. A notes-in tag reads the
channel's own part, the list as it stands before the pass, so a channel that
takes its notes and plays them back to itself does not hear its output
again. A taken channel is taken whether or not its tag is wired: take means
only the patch's output reaches it. Its chokes, bends and parameters still
reach it.

- **Every note the patch plays has an id of its own** (from
  `0xC000_0000_0000_0000`), and each box and each link to a notes-out tag
  keeps a table from the note it heard to what it played, so a NoteOff
  releases exactly that whatever the box's settings did since. Two notes
  that land on one pitch play it once and release with the last. A Choke
  into a box releases everything it holds.
- **The release rules.** A NoteOff, a choke, a seek, a panic, a stop, a
  mute, a pattern switch or a loop fold releases what the patch played, as
  they release any sequenced note. A set that drops a link (a wire deleted,
  a tag rebound, a box on the path retyped to another kind) owes its held
  notes a NoteOff at the next block, on whichever seat the channel has moved
  to; past 256 owed, a Choke. A box kept across an edit on the same path
  from its tag keeps its notes.
- **A wire holds 64 notes at once**, as many as a channel's sequenced
  voices. One more is refused, never half-played, and the notes-out tag goes
  red for about a second; its NoteOff then finds nothing to release.
- **A note wire cannot close a loop** (`WireRefusal::NoteLoop`).
- **Note boxes read a control inlet once a block.** The note pass runs
  before the patch ticks, so a `transpose`'s semitones and a `chance`'s
  probability inlet are read as the patch ticked them last, at the end of
  the block before. A wire into transpose adds `round(value * 12)`
  semitones, so ±1 is an octave; into chance it adds to the probability,
  clamped to `0..1`. `chance` is seeded and restarts with the transport, so
  a bounce plays what playback did. A note switch's `which` and a note
  gate's `open` are read the same way.
- **A chord is the notes starting together**: the NoteOns at one offset
  in a block. **`pick`** keeps one note of each (`pick low`, `pick high`,
  `pick 2` the second from the bottom, `pick -2` the second from the top;
  none when the chord is smaller). **`unpack n`** sends each chord's kth
  note from the bottom out of outlet k, dropping those past n. **`merge
  n`** plays what reaches any of its n inlets on one outlet, in offset
  order; a Choke into one inlet releases only what came in there.
- **A note switch** sends each NoteOn out of the outlet `which` picks, and
  its NoteOff follows it there whatever `which` does since. **A note gate**
  passes NoteOns while `open` is high and, when it closes, releases what it
  let through at the start of the block.
- **`wait`** (`wait 1/8`, `wait 2b` for the division two beats long,
  `wait 250ms`) delays what reaches it: each NoteOn and its NoteOff on its
  `notes` lane by the same time, at the tempo as the note arrived, and its
  control `in` on its `out`. It never waits past 8 seconds. It tracks 64
  notes at once; a NoteOn past that is dropped with its NoteOff. Stopping,
  a seek, a loop fold, a panic and a Choke into it drop what it holds back
  and release what it played. The control lane keeps 2048 readings across
  its longest wait, so past the shortest waits it reads between them.
- **`note`** plays its `pitch` (as a `held` box sends it, `0..1` for notes
  0 to 127; middle C unwired) at its `velocity` (100 unwired) each time its
  `trigger` fires, and lets go when the next one fires or when a level
  trigger falls back below 0.5. It reads its inlets each control tick and
  plays what they struck a block later, at the tick each struck. Stopping, a
  seek, a loop fold and a panic release what it plays. With `unpack`,
  `held`, a `counter` and a `select` it builds an arp from parts.
- The boxes make a graph with no loops, compiled so each box runs after
  every box feeding it (`CompiledNoteBox::inputs`, `NoteFeed`); a box keeps
  its notes across an edit while it and everything feeding it are the same
  path.

**An assignment is a route.** A route (`ModRoute`) runs from a box or a tag
(`ModSourceRef::Id`), reading its first outlet (a held box's gate), to one
`ParamAddr` anywhere in the song: any channel's generator, inserts and
strip, any track's inserts and strip, the master's included, and a box's
own knobs (`ParamOwner::Modulator { module }`, under `MODULATOR_SCOPE`, a
scope no channel or track uses). The canvas draws each route as an
assignment tag wired from its box, placed through
`SongModulation::route_places`. A route onto a box's knob moves it in its
normalized range, summed per knob and clamped, before the box ticks; a
stepped knob refuses modulation, and a box cannot route onto its own knobs.
A route from a tag bound to an outlet or a keyboard compiles to the same
read a route from the outlet itself made, so a song converted to tags reads
the same values at the same block. `ModSourceRef::GeneratorOutlet` and
`Performance` remain for what is carried outside the song (a copied
channel's routes), and inside it are transient: the route reads the tag
for its source (`SongModulation::adopt_channel_routes`).

**A preset names nothing outside itself.** A patch preset is a piece of the
patch (`SongModulation::fragment`, landed by `land`, in
`crates/mooloop-core/src/patch_preset.rs`): its tags are saved empty with a
hint of what they were bound to (`SongModulation::hints`), and an
assignment onto anything outside it is saved loose, its destination as text
(`SongModulation::loose`), driving nothing until it is bound by arming its
box and dragging a knob. A channel preset and each kit entry carry the piece
of the patch that plays on their channel (`SongModulation::channel_fragment`,
landed by `land_on_channel`): a one-channel song in which "this channel" is
`PRESET_CHANNEL` at seat 0. A preset saved before the patch carries a
`ModRack` and is lifted where it lands, as a 0.1.6 song's racks are; the
`MAX_MODULATORS_PER_CHANNEL` (8) and `MAX_MOD_ROUTES_PER_CHANNEL` (16)
constants bound what such a rack holds, not what a song holds.

**There is no count a user meets.** The set is sized from the song, not
reserved: any edit that changes its shape (a box, tag, wire or route added
or removed, a box retyped, a route onto a box's knob retuned, a channel or
track moved) installs a whole new set built off the audio thread
(`StructuralCommand::SetModulation`), and the callback carries each
surviving box's state into it by id and hands the old set back to be
dropped off the audio thread. Only a box's params and a route's depth and
polarity are retuned in place. This is `CAPACITY_POLICY.md`'s
preallocate-and-grow-by-replacement rule. The canvas holds what the song
has; nothing draws a fixed row of empty bays. The control pass costs about
3% of a 128-frame block at a thousand boxes
(`docs/plans/archive/song-patch/00-status.md`, step 02).

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
saved before the id existed decode from their slot number. A wire names its
ends by id too (`Jack { node, port }`), so neither the list order nor a
place on the canvas is something a saved song depends on.

**A source names its channel.** A tag that reads a channel names it by
`ChannelId` (`InletSource::channel`, `TagKind::channel`), and so does a
generator outlet or performance source carried outside the song
(`ModSourceRef::GeneratorOutlet`, `ModSourceRef::Performance`). Nothing
means "my channel" implicitly; a preset's "this channel" is
`PRESET_CHANNEL`, bound to the receiving channel where it lands.

| Kind | Meaning | Status |
| --- | --- | --- |
| LFO | Free-running or note-restarted periodic movement. | Implemented. |
| Envelope | Gate-driven attack, decay, sustain, and release contour. | Implemented. Its gate is any control wire: a gate tag's notes, a held box, a generator's gate outlet through an inlet tag, or any value read by its level. |
| Step / random generator | Clocked patterns, probability, and controlled variation. | Implemented as the Step and Random boxes, with `counter`, `select` and `slew` beside them. |
| Arithmetic | Scaling, inverting, offsetting and limiting another signal. | Implemented as the arithmetic boxes, `+ - * / min max clip`. |
| Transport | Beat, bar, pattern position and the playing pattern. | Implemented as inlet tags. |
| Note value | Velocity, pitch and gate from a channel's notes, reduced to one control signal. | Implemented as the `held` box on a note wire: the latest NoteOn's pitch and velocity. |
| Macro | A user macro knob. | Planned. |
| Generator outlet | Generator-reduced values such as last-note velocity, gate, envelope, or Buffer state. | Implemented for the ML-P8's seven and DS-01's six, through the one-block control table, and read on the canvas through an inlet tag bound to the outlet: the tag is armed and wired like a box's outlet. Only the control run is offered; an audio outlet is refused by domain. Note the range convention below. |
| Device outlet | Named effect signals such as gain reduction, envelope-following level, or gate state. | Planned. The vocabulary is shared with generator outlets; no effect declares one. |
| Audio-derived control | Explicit envelope follower, transient detector, or another control extractor. | Deferred until it has an outlet contract. |
| Another channel's control | Any channel's notes, generator outlets, mod wheel and aftertouch, each through a tag naming its channel. | Implemented. |
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

### The patch, not a rack

The song owns one patch and one routing matrix (see "Decisions" and "The
song's patch"). It does not strip an authored instrument of endemic
modulation: per-voice envelopes, velocity/key/gate relationships, audio-rate
oscillator routing, and a device-specific LFO with saved internal routes
cannot in general be reproduced after a chord has been reduced to one control
value.

Song-wide, not per channel. Adam, 2026-10-05: *"channels wont have
modulators"*. A copied or pasted channel brings the routes that point into
it, aimed at the copy, from the same boxes (*"assignments would
copy/paste"*); a channel preset brings the piece of the patch that plays on
its channel and adds it to the song.

A control source is something that produces a normalized bounded control
signal over time, conventionally `-1..1` before route transformation. The
first was an LFO; the boxes, the transport and the channels' notes, outlets
and keyboards are sources now. The taxonomy is intentionally broader still:
macros, named device outlets, and eventually external control or
audio-derived signals can all participate if they declare their timing and
value semantics. **The vocabulary is curated, not an open language** (Adam,
2026-09-27): small boxes that combine, so the LFO sequence is `counter` plus
`select` plus `lfo`s rather than one `lfo-seq` box, and a saved patch preset
is how a bigger one is kept. A Pd box that runs a `.pd` file may come later
as the one escape hatch.

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

### The rack is a chain; the patch is a graph

The ordered device rack remains the presentation and workflow of the audio
path. The patch is the graph of the control and note paths: cross-device
sources, destinations, routes, timing and latency are explicit data, drawn
on one canvas. Authored device-local modulation also has persisted,
inspectable source and destination identities, but may execute inside a
voice where channel-rate routing cannot preserve its semantics; it stays in
its device and does not appear on the canvas. Control on the canvas is
monophonic.

The canvas carries no audio. It must not introduce a second cross-device
modulation engine beside the compiled set, or redefine the rack model.
Assigning stays a direct-manipulation gesture on ordinary controls (arm an
outlet, drag a knob), not a matrix to fill in: the assignment tags on the
canvas show the routes, and are not a second way to make them.

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

An envelope's gate is the wire into its `gate` inlet (a gate tag's notes, or
any control wire by its level), and it stores ADSR values. Attack, decay,
and release use the same free/synced timing vocabulary as the LFO. Note On
restarts attack from the current value; the final held Note Off begins release,
so overlapping piano-roll notes keep the gate high. Runtime output stays in the
patch's signed convention and a new envelope route defaults to unipolar
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

**An outlet does not use the patch's signed convention, and a route's
polarity is about that convention.** A box always emits `-1..1`, which is
why `Unipolar` lifts it — `(v + 1) / 2` — so a one-way box contributes no
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
reach the patch, through gate tags as per-tick gates and through notes-in
tags as notes.

**The patch is one answer, for anything a parameter can do.** A gate tag or
a notes-in tag into a `held` box drives an Envelope, a Step or a Random, and
its assignment moves any insert's knob: a filter that opens on each kick, a
delay whose feedback swells while a chord is held, a send that steps per
note, all at control rate, with the notes from any channel. What the patch
cannot do is make an insert *act* on a note rather than move a knob: a delay
that flushes its buffer on a note, a stutter that captures on one, the
Buffer jumping its playhead. Those need the note at the sample it lands on
and an action the device takes, not a parameter. **So the request stands:
give effect slots access to the channel's note stream as a separate input**,
and keep the isolation for parameter events. It is the same device change
the algorithmic Buffer (Adam's patch 5) needs.

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
(ML-P8's include Velocity, Gate and its LFO). An inlet tag bound to one
(`InletSource::Outlet`) brings it onto the canvas, where it feeds a box's
inlet or is assigned straight to any legal destination in the song.

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

The song's patch lives in the **Modulation** pane, a view of its own
(`PaneViews.modulation`; **Show Modulation**, `view.pane-modulation`,
Ctrl+6), not in the device rack. It is one canvas for the song
(`ui/patch-canvas.slint`, laid out and hit tested in
`crates/mooloop-ui/src/patch_canvas.rs`), larger than the pane and
scrolling: boxes where the song put them, tags at the patch's edges, an
assignment tag for each route from a box, and the wires. Under it, the
selected box's name or what the selected tag reads, its **Assign** switch,
and the selected source's routes from the whole song, each naming the
chain, device and parameter it reaches. There is one pane for the song, not
a `MOD` page copied into Mono, Poly, Buffer, and every effect.
`UI_DESIGN.md` has the canvas's gestures.

**A box's settings are on its face**, opened in place by the arrow at the
box's right: a small knob with a label and a readout per setting
(`crates/mooloop-ui/src/patch_face.rs` picks them). A box shows the settings
that change what it does now: an LFO in sync shows its division and not its
rate, so one knob reads as the box's speed whichever way it is set, and the
sync is a stepped knob of its own. A note box's arguments are stepped knobs
that read by name (`min7`, `1st`, `dorian`). The face's knobs are
destinations like any other, so one box can move another's rate or depth.

**A box's inlets are its inputs.** What drives a box is the wires into it:
a channel's notes through a gate tag, another box, a transport tag, a
generator's outlet. `Kick gate → env → Sampler / Position` and
`Kick / Gate outlet → lfo retrigger → Sampler / Position` are both a tag, a
box and an assignment, with the kick and the sampler on different channels
if need be. Wiring an inlet binds a declared control signal to a declared
inlet; it does not create a device-local modulator or infer control data
from telemetry.

A common device frame shows the routes that terminate there, and may show
source pills where more legible. Activating that summary reveals the
Modulation pane focused on a destination-first route inspector. This is the
UI entry point where signal order is visible without falsely making sources
device-owned. **Not built yet:** the faces draw route-count dots under each
modulated parameter, but clicking one raises nothing, so it cannot select
its box or reveal the pane (MOO-531).

### Direct assignment

1. Open the Modulation pane and select a box or tag.
2. Arm it: click its outlet on the canvas, or switch on **Assign** under
   the canvas. Legal controls on any channel's source device and inserts,
   on any track's inserts, and on every open box's face but its own,
   acquire a subtle assignable state; illegal controls do not.
3. Drag a normal control to create or adjust the armed source's route depth.
   Preserve the ordinary control's base value.
4. Keep the base readout; add a marker and modulation arc/range overlay for
   resolved excursion.
5. Clicking the marker opens a destination-first inspector listing incoming
   routes, source, polarity, depth, and a remove action.
6. Turning **Assign** off, or Escape on the canvas, restores ordinary
   base-value editing while keeping the source selected.

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

**Depth is set by the drag; math boxes do the scaling** (Adam, 2026-09-27).
A route has a depth and a polarity and nothing else; a curve, an offset or
an inversion shared by several destinations is a box before the outlet
(`* -1`, `+ 0.5`, `clip 0 1`), so it is drawn where it acts.

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

This work includes the song-owned patch (boxes, tags, control and note
wires, the compiled order and its one-tick loops), destination metadata,
base-plus-offset resolution, current LFO continuity, the canvas and the
common-frame interaction, the direct-assignment inspector, and patch and
channel presets that carry a piece of the patch.

It excludes an open patching language (the vocabulary is curated; a Pd box
is the agreed escape hatch, later); audio on the canvas, audio-rate wires and
per-voice control; copying the same general-purpose LFO into every device;
true audio sidechain and cross-device audio-rate FM; note loops; MIDI out (a
notes-out tag reaches a channel, not a port); and treating display telemetry
as control data. Control loops are in, a tick late. A device-specific LFO or
per-voice modulation system may remain local when it is an authored part of
the instrument, works without the song's patch, persists with the device,
and publishes any cross-device signal through the ordinary outlet contract.
Transitional synth LFOs that are merely generic modulators should still
migrate instead of growing a parallel system.

Built: the song-owned patch, direct assignment, durable references, twenty
box names, transport, gate, outlet and keyboard inlets, note wires and note
boxes, patch presets, and channel presets carrying their patch
(`docs/plans/archive/song-modulation/` records the move from per-channel
racks, `docs/plans/archive/song-patch/` the patch). Not built: macros,
declared effect and Buffer outlets, inserts that hear notes ("Note-triggered
effects"), and the route dots' reveal. Next, declared effect and Buffer
outlets through the one-block control table. After typed auxiliary graph
edges and compensation exist, evaluate true sidechain and external routing.
`IDEAS.md` keeps what the song patch plan left out, each with its reason.

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
- Reordering the rack does not change durable destination identities, and
  neither the list order nor a place on the canvas changes what any box,
  wire or route means.
- The patch runs in its compiled order: a box reads what feeds it in the
  same tick, a loop reads a tick late and is marked, an outlet tag reads a
  block late and says so, and a note box reads its control inlets as of the
  block before.
- Notes sent through the patch arrive in the same block at the same offset,
  realtime and offline, and every note the patch plays is released by
  whatever releases the note it came from, or at the next block when its
  link goes.
- A song saved by an earlier build opens as a patch and plays exactly as it
  did (`crates/mooloop-engine/tests/modulation_null.rs`,
  `factory_patch_null.rs`).
- The callback allocates nothing, locks nothing, performs no I/O, follows no
  implicit cross-channel edge, and never treats best-effort telemetry as
  modulation data.
