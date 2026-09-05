# Latency compensation plan status

**Steps 02, 03 and 04 are in, and most of 05's measurements came with 04.**

The mixer is time aligned. Two channels hitting on the same tick, one of them
through a device that costs fifteen frames, now land in the same frame — and
the assertion that proves it (`together[..15]` is silent) fails on the commit
before this one.

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

## Step 04 landed: the delays are installed and reconciled

`StructuralCommand::SetCompensation` carries a preallocated ring to a channel
or a bus, on the same ownership round trip `SetSamplerStretch` already makes.
The producer waits immediately before it sums: after its output stage, before
`add_from`.

**The global-plan hazard was solved by deriving rather than tracking.** The
worry in the plan was that installing a Drive on channel 3 changes what
channels 1, 2 and 4 owe, so every edit path would need a call site and
forgetting one would produce a misalignment nothing reports.
`Session::sync_compensation` runs from the pump instead: it derives the plan
from the model, diffs it against what was last sent, and sends only the
difference. It cannot be forgotten, it costs a comparison when nothing changed,
and it converges within one frame of any edit — and a structural edit already
interrupts the thing being edited, so that frame is not a cost anyone hears.
`RenderState::load_project` installs the plan directly as well, because an
**offline render** builds its own state and never runs a pump.

Three things the doing turned up:

- **Bypass was not time transparent, and the plan exposed it.** A bypassed
  slot passed audio through untouched while `chain_latency` counted its
  declared fifteen frames, so every other channel would have been
  over-compensated against it. The container now pushes the signal through the
  slot's own ring when bypassed, which is what "bypass retains latency" means
  in every host that says it — and it subsumes what that branch did before,
  since the ring sees the same samples either way. Web search confirmed the
  convention: bypassing does not remove latency, *removing the device* does.
- **A muted producer had to empty its ring.** A muted channel renders nothing,
  so its compensation ring would hold pre-mute audio and emit it on unmute.
  Fifteen writes, and it is the honest state: a silent producer's pipeline is
  silent. A muted *bus* instead keeps advancing, because it still processes.
- **`DryAlign` became `IntegerDelay`.** The dry-path aligner and the
  compensation delay are the same stereo integer ring doing the same job one
  level out, so it is one type. The old name would have been a lie at the new
  call site.

## What 05 still owes

Most of its measurements landed with 04: impulse alignment through a channel
and through a bus, the block-size null, bypass not moving the channel, and the
footprint. What remains is the explicit offline-versus-live comparison (the
same `from_project` path is exercised, but not against a live render), and
`AUDIO_ARCHITECTURE.md`'s migration sequence still lists step 5 as pending.

## Reading order

`01-what-this-is.md` for why this is ahead of typed audio edges at all, then
02 through 05 in order.
