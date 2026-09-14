# Composable Device Units

Status: design rule, September 2026. **Mostly a target; the three habits in
"What we actually do now" are not** — those are load-bearing today.

> Trimmed 2026-09-14 from 469 lines. What went was the long-form target model:
> the Max-object mental model, composite units, designed inlets and outlets,
> published versus private ports, parameters-versus-ports, and the
> first-practical-use walkthrough — roughly 350 lines describing infrastructure
> that does not exist and is deliberately not being built ahead of a
> demonstrated workflow. It is summarised below and preserved in git. The
> habits, the checklist and the consequence are kept whole, because those are
> the parts anyone acts on.

`AUDIO_ARCHITECTURE.md` owns preparation, execution, graph timing and realtime
lifecycle. `MODULATION.md` owns the channel control-routing model. This owns
how a reusable DSP piece presents itself.

It is a design rule, not a claim that mooloop has a general node editor or
dynamically instantiates every internal primitive. `PRODUCT.md` continues to
rule out a Max/MSP-scale patching environment as the ordinary workflow.

## Goal

Any discrete DSP unit should be designed so that, conceptually, it *could*
exist as a Max/MSP object: a clear set of inputs, outputs, parameters and
runtime behaviour. Units built from smaller units follow the same rule, so the
idea is recursive — `primitive -> unit -> larger unit -> device -> graph` —
and there is no point where composition suddenly stops.

Deliberate implementation boundaries are still fine. A private oscillator can
be an ordinary Rust field, a performance-critical voice can stay monolithic,
and a device can implement the in-place `AudioNode` adapter. The contract asks
for an intentional interface **at a useful composition boundary**; it does not
ask for trait objects, heap allocation, runtime dispatch, or public metadata
around every private helper.

## The target model, in brief

The trimmed sections described a port contract that does not exist yet. In
summary, so the rule is still legible:

- A unit declares **inlets and outlets** deliberately, rather than having them
  inferred from whatever its fields happen to be.
- Ports are **published and discoverable**, carrying stable identifiers, types,
  ranges, units, rates and latency — the same metadata a parameter descriptor
  carries, because a port and a parameter are the same kind of promise.
- **Published is not the same as private.** Publishing a port is a commitment
  to a musical workflow, not a side effect of a value existing; private
  internals stay private.
- The first practical use was always going to be **modulation outlets**, and
  that is the part that has since shipped — devices publish typed control and
  audio outlets, and `compile_audio_graph` resolves them.

The full version is in git (`docs/COMPOSABLE_DEVICE_UNITS.md` before
2026-09-14) if the port table is ever built for real.

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

## What we actually do now


Everything above is a target. Most of it is unimplemented: there is no port
table, no `unit.inputs()`, and no published outlet. `AudioNode` is an
in-place stereo process call, two declared latencies, and two best-effort
telemetry readers (`buffer_collisions`, `dynamics_frame`) — and those two are
observation, deliberately not the outlet contract this document describes.
That gap is deliberate: the contract is a design rule, and building its
infrastructure ahead of a demonstrated workflow is how it would turn into
ceremony.

Three of its habits are load-bearing today, though, and they are cheap. They
are the difference between a contract that stays reachable and one that has to
be retrofitted against fused code. Follow them whether or not the port
metadata ever exists:

**1. A value that matters gets a name and a stable identity, not a local.**
If a signal has musical meaning — a phase, an envelope output, an oscillator
tap, a read head position — it must be reachable by something other than the
expression that computed it. Naming a value later is cheap when it is already
a field with a defined meaning, and impossible when it only ever existed
halfway through a line of arithmetic.

`Osc` used to be the live counter-example: its `phase` was private with no
reset, no wrap event, and no way to read it, which is exactly why hard sync
could not be built on it. Building the ML-P8 forced the change this habit
predicted — `Osc` now exposes `phase()`, `reset_to`, a reported cycle wrap,
and a `sync_reset` that corrects the step with a BLEP — and the change was
mechanical because the value already existed as a field. That is the whole
argument for the habit: the retrofit cost one commit rather than a rewrite of
every consumer.

**2. Do not fuse topology that costs nothing to keep separable.**
A voice may hold oscillator, filter, and amplifier in a fixed internal order
and still be a well-formed unit. It stops being one when those stages are
inlined into a single unsplittable expression for no measured reason. Fixed
topology is fine; fused topology is not.

**3. Address things relatively, never absolutely.**
Anything a fragment might be saved and reloaded elsewhere must not name its
neighbours by index. `ModRoute` named its destination channel absolutely, so a
channel preset saved from channel 3 modulated channel 3 wherever it was
loaded; `rescope_modulation` exists to undo that. Assume any unit may be moved.

None of the three is a bet on a node editor. They are ordinary hygiene, they
make the code better if no graph view is ever built, and they are the reason
the option stays open at close to zero cost.

## Long-term consequence


Maintaining this contract lets mooloop eventually support a node editor
without redesigning every DSP primitive around it. A node editor becomes a way
to create and display connections between interfaces that already exist.

That remains a long-term consequence, not the immediate product goal. The
ordered device rack and direct channel-modulation workflow stay primary; a
future graph view edits the same units, ports, and routes rather than creating
a parallel engine.
