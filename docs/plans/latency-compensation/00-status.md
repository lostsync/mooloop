# Latency compensation plan status

**Steps 02 and 03 are in. 04 and 05 are the engine and the measurements.**

Nothing is compensated yet. What exists is the two halves that had to be
built before anything could be: a device can say what it costs without being
built, and the tree can be turned into a per-producer delay. Neither touches
the audio path, which is why they are one branch and the engine is another.

## Step 02 landed: latency is declared

`EffectKind::latency_frames()` answers in `mooloop-core`, so the control
thread can size a compensation delay before any node exists to ask — which is
the whole reason the plan is compiled off the audio thread at all.

`OVERSAMPLER_LATENCY_FRAMES` moved to `mooloop-core::effect` and
`mooloop-dsp::shaper` re-exports it. It was a private property of the
oversampler; it is an interface number now.

Two things worth recording:

- **The declaration and the node are two numbers for one fact.** That is the
  trap this step creates, and a disagreement would be silent: the delay would
  be built to the wrong length and the misalignment would *move* rather than
  go away. `every_effect_kinds_declared_latency_matches_its_node` builds one
  node per kind at two sample rates and asserts they agree. Two rates, because
  a device whose latency depended on the rate could not be declared statically
  at all, and this is where that would be found out.
- **The constant stopped being derived, and that is fine.** It was
  `HALF_TAPS - 1`; it is a literal `15` in core now, because core cannot see
  the filter. What keeps it honest is not arithmetic but
  `identity_path_has_the_declared_latency`, which drives an impulse through
  the real path and asserts where the peak lands — a better guard than the
  derivation was, since it would survive a change of kernel shape that
  `HALF_TAPS - 1` would not.

## Step 03 landed: the tree compiles to a compensation

`compile_latency` in `mooloop-core::mixer`, beside `compile_bus_graph`, which
already owns the topology. Pure, `Copy`, fixed-capacity, no allocation.

- **It reuses the render order rather than sorting again.** `compile_bus_graph`
  already emits buses sources-first, so one descending pass has every feeder's
  arrival ready by the time its destination is read. No recursion, no second
  sort.
- **`clamp_bus` moved from the engine to core.** The engine held the only copy,
  and the plan has to agree with the executor about which bus a channel
  actually feeds — two answers to that would compensate a channel against a
  summing point it does not sum into.
- **The correction happens once, at the deepest summing point that needs it.**
  Two buses of different depths into the master delay the *shallow bus*, not
  its inputs again; the tests spell that out, because compensating a channel
  and then its bus for the same deficit is the obvious way to get this wrong.

The interesting cases are graph shapes rather than sounds — a bus feeding a
bus, a channel whose sibling is longer, a bank whose routes were repaired —
which is exactly why this is a pure function in core and not a method on
`RenderState`.

## What 04 has to be careful about

The plan is **global**. Installing a Drive on channel 3 changes the
compensation required on channels 1, 2 and 4, because it moves the master's
arrival. So every structural edit that can change a chain's latency has to
recompile the whole plan and reissue the delays that changed, and the list of
such edits must not grow silently. `04-preallocated-delays.md` names them.

## Reading order

`01-what-this-is.md` for why this is ahead of typed audio edges at all, then
02 through 05 in order.
