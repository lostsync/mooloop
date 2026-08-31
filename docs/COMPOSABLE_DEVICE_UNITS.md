# Composable Device Units

Status: target design contract, August 2026.

This document defines how reusable DSP pieces present themselves for
composition. It complements `AUDIO_ARCHITECTURE.md`, which owns preparation,
execution, graph timing, and realtime lifecycle, and
`MODULATOR_SYSTEM_SPEC.md`, which owns the current channel control-routing
model.

It is a design rule, not a claim that mooloop currently has a general node
editor or dynamically instantiates every internal primitive. `PRODUCT.md`
continues to rule out making a Max/MSP-scale patching environment the ordinary
workflow.

## Goal

Any discrete DSP unit in mooloop should be designed so that, conceptually, it
could exist as a Max/MSP object.

An oscillator, envelope, filter, LFO, noise source, gain stage, buffer
operator, or other reusable DSP primitive should have a clear set of inputs,
outputs, parameters, and runtime behavior.

Units constructed from smaller units follow the same rule. A complete
instrument may internally contain several oscillators, envelopes, filters,
modulators, and utility stages, but from the outside it still presents itself
as another composable unit with a defined interface.

The basic idea is recursive:

```text
primitive -> unit
units -> larger unit
larger units -> device
devices -> larger graphs
```

There is no conceptual boundary where composition suddenly stops. There may
still be deliberate implementation boundaries: a private oscillator can remain
an ordinary Rust field, a performance-critical voice can stay monolithic, and
a whole device can implement the current in-place `AudioNode` adapter. The
contract requires an intentional interface at a useful composition boundary;
it does not require trait objects, heap allocation, runtime dispatch, or public
metadata for every private helper.

## Max-object mental model

A useful design test is:

> If this were a Max object, what would its inlets and outlets be?

For an oscillator, that might be:

```text
Oscillator

in:
    frequency
    pulse width
    phase/reset

out:
    audio
```

For an envelope:

```text
Envelope

in:
    gate / trigger
    attack
    decay
    sustain
    release

out:
    envelope value
```

For a filter:

```text
Filter

in:
    audio
    cutoff
    resonance
    drive

out:
    audio
```

The implementation does not need to resemble Max internally. The test asks
whether the unit has a defined interface independent of the larger device that
currently owns it.

## Composite units follow the same rule

A unit assembled from other units can itself expose inlets and outlets.

For example:

```text
KickVoice

internally:

pitch env ---> oscillator frequency
                  |
                  v
              oscillator ---> amp
                              ^
                              |
                           amp env
```

Internally, this may be ordinary Rust code. Externally, `KickVoice` might
expose:

```text
in:
    trigger
    pitch
    sweep amount
    decay

out:
    audio
    pitch envelope
    amplitude envelope
```

The caller does not need to know how many lower-level components are inside
it. A `KickVoice`, `ThreeOscVoice`, `MonoSynth`, or future buffer-processing
construction can itself become a building block.

This recursive shape does not mean every layer must be dynamically
rewireable. A composite may have a fixed internal topology and still be a
well-formed unit. Runtime graph construction is one possible consumer of the
contract, not the definition of composability.

## Inlets and outlets are designed, not inferred

Inlets and outlets must not be inferred accidentally from implementation
details after the fact. When a reusable or publicly routable unit is designed,
its external interface is defined alongside its DSP behavior.

An interface descriptor states at least:

```text
name
stable identifier
direction
signal/value type
range or units where applicable
rate/domain
declared latency where applicable
```

For example:

```text
id: pitch_env
name: Pitch Envelope
direction: output
type: control
range: 0.0 .. 1.0
rate: control
latency: one block
```

or:

```text
id: cutoff
name: Cutoff
direction: input
type: control
unit: Hz
range: 20 .. 20000
rate: control
```

The exact Rust representation can evolve. Published identifiers cannot be
casually renumbered once projects, presets, automation, or routes persist
them.

Signal type and rate are semantic, not decorative metadata. Audio, control,
gate/trigger, note/event, and display telemetry are different domains. A
connection is legal only when their declared types and timing contracts are
compatible or an explicit adapter exists.

## Ports are published and discoverable

A publicly composable unit reports its available inputs, outputs, and
parameters without requiring the caller to know its concrete implementation
type. Conceptually:

```rust
unit.inputs()
unit.outputs()
unit.parameters()
```

Metadata lives on the control side and does not require inspecting or mutating
realtime DSP state. A host can ask:

```text
What can receive modulation?
What can provide modulation?
What audio inputs and outputs exist?
What events can this unit receive or emit?
What rate and latency does each connection carry?
```

`AudioNode` remains the narrow realtime processing interface. Unit metadata,
editable graph state, and prepared runtime state are separate layers; one
large trait must not accumulate all three responsibilities.

## Published and private ports

Not every internal connection becomes part of the public graph. A device can
contain:

```text
oscillator
pitch envelope
amp envelope
filter
drive stage
```

while exposing only:

```text
inputs:
    trigger
    pitch
    decay
    drive

outputs:
    audio
    amp envelope
```

The remaining connections stay private. This lets device authors construct
opinionated instruments without giving up composability.

Publishing a port is therefore a product and compatibility decision. A public
port needs a useful musical purpose, stable identity, declared type/rate, and
testable behavior. Internal state does not become public merely because it is
easy to expose.

## Parameters and ports

A parameter and an inlet are related but not identical. For example,
oscillator frequency may have:

```text
parameter:
    base frequency = 440 Hz

inlet:
    frequency modulation
```

The final DSP value derives from both. Likewise, a UI knob manipulates an
authored parameter value but is not the parameter itself.

Keeping these concepts distinct allows:

```text
UI control
automation
modulation
node connection
preset value
```

to influence one DSP property without coupling the primitive to the UI. The
existing modulation rule remains authoritative: automation or the knob
provides the base, routed control signals add bounded offsets, descriptor
mapping resolves the natural-unit value, and the device receives an ordinary
sample-timed parameter event.

An inlet may eventually accept a typed signal directly where that is the
correct DSP contract. That does not make every parameter an inlet or every
inlet a persisted parameter.

## First practical use: modulation outlets

The first reason to establish this interface is modulation. Suitable published
control outputs become discoverable modulation sources, including:

```text
LFO output
envelope output
velocity
note gate
note pitch
internal sequencer value
random/noise control signal
follower output
buffer position
device-specific internal values
```

A device containing an amplitude envelope may choose to publish:

```text
MonoSynth / Amp Envelope
```

The channel modulation system can then route it without special-case knowledge
of `MonoSynth`. A composite can publish a selected value from a deeply nested
unit while keeping the rest private.

Published device outlets use the timing contract in
`MODULATOR_SYSTEM_SPEC.md`: they enter a prepared per-channel control table,
and cross-device consumers currently read them with one declared block of
latency. Display telemetry is never silently promoted into a control source.

Audio-rate FM, sidechain audio, and feedback **between public units** require
typed graph edges, buffer ownership, cycle policy, and latency compensation
from `AUDIO_ARCHITECTURE.md`. A composite may still implement an explicit
fixed/delayed audio-rate network internally. The conceptual public interface
does not bypass the graph work for an external connection.

## UI independence

DSP units do not know about Slint or a future node-editor implementation.
Prefer this separation:

```text
DSP primitive
    |
unit/component interface
    |
UI representation
```

For example:

```text
Osc
    |
Osc unit metadata
    |
Osc node UI
```

The same oscillator can be used:

- inside a fixed synth voice;
- inside a dynamically constructed device;
- as a source in a node graph;
- with no visible UI.

The UI represents the unit; it is not part of the unit.

## Realtime and preparation contract

Composition does not relax the realtime rules. Editable units and connections
are validated and compiled on the control side. Runtime storage, schedules,
adapters, latency, and bounded capacities are prepared before activation.

The audio callback does not:

- enumerate metadata or discover ports;
- allocate a connection or resize storage;
- validate types or repair a graph;
- take locks or perform I/O;
- traverse an open-ended editable object graph.

It executes a prepared representation. Private fixed composition may compile
away entirely; public dynamic composition may become a bounded schedule. Both
can satisfy the same conceptual unit contract.

## Design checklist

When adding or extracting a reusable unit, answer:

1. What does it own, and what does it borrow?
2. What are its parameters, inlets, and outlets?
3. Which ports are public, and what musical workflow justifies each one?
4. What are their stable identifiers, types, ranges, units, rates, and latency?
5. How are reset, transport discontinuity, tail, and failure defined?
6. What is prepared off-thread, and what is the bounded per-sample/per-block
   cost?
7. Does extracting the unit make at least two real compositions clearer, or
   merely add an abstraction layer?

If the last answer is only theoretical, keep the boundary conceptual until a
second real use makes the shared unit honest.

## Long-term consequence

Maintaining this contract lets mooloop eventually support a node editor
without redesigning every DSP primitive around it. A node editor becomes a way
to create and display connections between interfaces that already exist.

That remains a long-term consequence, not the immediate product goal. The
ordered device rack and direct channel-modulation workflow stay primary; a
future graph view edits the same units, ports, and routes rather than creating
a parallel engine.
