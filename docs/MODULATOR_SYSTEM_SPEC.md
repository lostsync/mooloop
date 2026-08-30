# Mooloop Modulator System

Status: implementation specification, August 2026. This expands the approved
decisions in [MODULATION_PLAN.md](MODULATION_PLAN.md). It formalizes the
source/outlet metadata and the rack interaction while retaining the existing
parameter and realtime contracts.

## Purpose

Mooloop modulation is a **channel-owned control-signal system**. It makes a
channel one playable instrument: an LFO can move a synth cutoff, a delay
feedback control, and the channel strip without becoming a feature of any one
of those devices.

The normal workflow should be immediate: select a source, touch an ordinary
control, and set the amount by direct manipulation. Its representation must
still be explicit enough to admit device outlets, Buffer signals, and a future
zoomed-out graph view without replacing the engine or creating a second routing
language.

This is not a proposal to turn Mooloop into Max/MSP or to clone Bitwig. The
useful lesson is a coherent model for arbitrary signals and destinations.
Mooloop's normal presentation remains the ordered device rack.

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
  property of Mono, Buffer, an insert, or the strip.
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

## Existing foundation: retain it

The repository already has the critical engine boundary. This specification
extends it; it does not replace it.

| Surface | Contract retained |
| --- | --- |
| `ParamAddr` | Stable destination address: scope, owner, and a per-kind never-renumbered descriptor ID. It already models sources, effect slots, modulator slots, and strips. |
| `ParamDescriptor` | Single source of truth for natural range, curve, default, and normalized conversion. Events carry natural values; routes operate in normalized destination space. |
| `ModRack` | Persisted per-channel source slots and routes. Its current realtime storage is four local source slots and sixteen route rows. |
| `ModRoute` | Current source slot, destination, signed full-range depth, and polarity. Reassigning the same source/destination retunes instead of duplicating it. |
| Renderer | Sources tick at `CONTROL_RATE_FRAMES = 32`; offsets are summed, clamped, converted through the descriptor, then sent as ordinary timed `Event::ParamValue` events. |
| Base state | The renderer/chain retains the knob base separately from a device's last resolved value. A lane supplies the base when active; modulation is added after it. |
| DSP | `AudioNode::process` has an in-place stereo bus and timed events. Effects already split at parameter-event offsets; they need no modulation-specific branch. |

The local LFO and gate-driven ADSR envelope are implemented today. The LFO is
a bipolar `-1..1` source with sine, triangle, saw, square, and sample-and-hold
random waves. The envelope is unipolar and currently binds its gate inlet to
the scheduled Note On/Off stream of an explicitly selected piano-roll channel.
That note stream is the first adapter for a future typed generator `Gate`
outlet; envelope destinations already use ordinary routes.

Do not replace `ParamAddr`, descriptor IDs, natural-unit events, or the
timed-event path. They are the seam shared by knobs, automation, and
modulation.

## Ownership and data model

### Channel collection

`ChannelSetup` continues to contain one modulation collection. A channel owns
the sources it can play and all routes it makes. Project-global modulation is
not implied; a later global source is a new explicit scope/source kind.

The realtime representation may remain bounded and allocation-free. Capacity is
an engine protocol boundary, not a UI layout: the UI shows existing sources
plus **Add source**, never four permanent empty bays. A larger capacity must
not alter persisted destination or route meaning.

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

`ModSourceId` is the durable source identity. The existing `source_slot: u8`
is a good initial runtime locator, but must not become the long-term persisted
vocabulary for an extensible source collection. Resolve a durable source ID to
a bounded runtime slot off the audio thread. When added, legacy routes decode
as their corresponding local-slot source and preserve their current behavior.

| Kind | Meaning | Status |
| --- | --- | --- |
| LFO | Free-running or note-restarted periodic movement. Random is initially a sample-and-hold LFO wave. | Implemented as local slots. |
| Envelope | Gate-driven attack, decay, sustain, and release contour. | Implemented with an explicit channel-note gate adapter; typed device gate outlets are planned. |
| Step / random generator | Clocked patterns, probability, and controlled variation. | Planned. |
| Macro / internal value | User macro, transport phase, velocity, key track, pressure, or another declared channel value. | Planned. |
| Generator outlet | Generator-reduced values such as last-note velocity, gate, envelope, or Buffer state. | Planned. |
| Device outlet | Named effect signals such as gain reduction, envelope-following level, or gate state. | Planned. |
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

A bipolar route swings source `-1..1` about the base. A unipolar route maps
that output to `0..1`, making the base the floor. Signed depth inverts either
form without inventing another source. Clamp only after all offsets sum.

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
setting or resetting phase. Fade-in uses the same free/synced timing
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
last-note, with alternatives added as explicit outlet modes.

True audio-rate FM and true audio sidechain are excluded. `AudioNode`
currently has one in-place stereo bus; true sidechain requires prepared typed
auxiliary edges/process buffers and graph latency compensation. Do not retain a
borrowed source bus inside an effect. A control-rate envelope follower exposed
as an outlet is the correct first audio-derived-control form.

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

## Scope boundaries and delivery order

This work includes the channel-owned model, destination metadata,
base-plus-offset resolution, current LFO continuity, the shelf/common-frame
interaction, and the direct-assignment inspector.

It excludes a general visual-programming environment; device-local general LFO
pages; general cross-channel/global routing beyond the explicit channel-note
gate adapter; true audio sidechain, audio-rate FM, and control feedback cycles;
and treating display telemetry as control data. Existing transitional synth
LFO pages should migrate into channel sources rather than grow a parallel
system.

1. Preserve `ModRack`/`ParamAddr`; add destination metadata and expose LFO
   routes in the channel shelf.
2. Complete direct assignment, base/excursion feedback, destination inspector,
   and undo as the normal workflow.
3. Add durable source references with a legacy local-slot adapter, then step,
   random, macro, and note-derived sources.
4. Add declared generator/effect/Buffer outlets through the one-block control
   table.
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
