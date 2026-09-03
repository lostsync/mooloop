# ML-P8 plan status

**Steps 02, 03 and 04 are in.** The device exists and plays: a new `MlP8`
generator kind beside the v1 poly synth, the three-oscillator network with all
six directed XMOD routes, self-feedback, noise into every phase input, hard
sync with a band-limited reset, a derived sub, deterministic coloured noise,
eight fixed voices, and — from 03 — two envelopes, four filter modes,
keytracking, both velocity depths, and a feedback loop around the filter with
the drive inside it. From 04 it has its own modulation: an audio-rate LFO with
six waves, Warp, Slew and three retrigger policies; six per-voice sources
reaching thirty-one continuous destinations through authored routes; and an
ML-P8 MOD page to author them on. A complete moving patch needs nothing from
the channel modulation shelf. 05 through 07 are next, in order.

The face is one screen, not pages: SOURCE, NETWORK, VOICE, divided by rules
along the signal path. Its centre is a grid of every route in the voice —
rows are sources, columns the oscillators they reach, the diagonal is an
oscillator on itself, and a MIX column carries the levels, because a level is
a route to the output. The left column's five tabs are that grid's five rows.

Five things the steps turned up that the plan could not have known:

- **The sync BLEP made aliasing worse until two mistakes were fixed** — the
  step height has to be measured on the *naive* waveform, and the oscillator's
  own cycle-boundary residual has to stand down for the sample after a reset.
  Neither is visible without building it, and neither shows up in a test that
  looks for energy in a high band, because a hard-synced oscillator is exactly
  periodic at its master's rate and every alias product folds back onto the
  master's own harmonic grid. The test compares harmonic magnitudes against an
  eight-times-oversampled render instead.
- **Clearing the feedback loop on `restart()` was not enough.** That only
  runs for a *fresh* slot, and stealing a sounding voice deliberately keeps
  its oscillator phases — restarting them under a running envelope is a
  click. It was keeping the loop with them, which is exactly the tail step 03
  says a reassigned slot must not emit.
- **"Skip an oscillator nothing reads" needed a caveat.** The skip is decided
  once per block from the *target* levels, but levels are smoothed — so a
  level knob reaching zero un-needs an oscillator while its smoother is still
  milliseconds from silence, and skipping it there replaces the ramp with a
  step. Step 04 needed the same caveat again for a different reason: a route
  aimed at a level, an XMOD amount or the filter is one more thing that can
  move it, so every one of those skips now stands down when a route exists.
  Otherwise a route would have been silenced by the optimisation it
  invalidated.
- **A route offset has to be applied before the curve, not after.** The map
  from authored percent to phase deviation squares the magnitude. Adding an
  offset to the already-curved value would mean an amount of 20% moving a
  different distance at every point on the knob, so `Prepared` keeps the
  authored percent beside the prepared cycles for exactly the amounts routes
  can reach. The same reasoning splits an oscillator's semitones from its
  cents: a route reaching pitch is clamped through the semitone control's own
  range, and cents stay the fine offset the patch authored.
- **A route at zero amount has to keep its compiled row.** Dropping it is the
  obvious optimisation and it quietly breaks the step's own promise that
  automating an amount never rebuilds the topology: a lane sweeping up from
  silence needs a row that is not there. For the same reason the node compares
  route *topology* rather than equality when a parameter block arrives — once
  an amount has been automated away from what the block still carries, every
  later knob change would otherwise look like a structural edit.

Three decisions worth knowing about.

**A route's amount is authored in percent, and addressed by the route's own
identity.** Percent because every other signed depth on this device is, and
because the amount needs a descriptor for its automation lane — a `[-1, 1]`
fraction would have put the authored number, the lane's normalization and the
readout in disagreement at three boundaries. Addressed through a new
`ParamOwner::SourceRoute { route }` rather than a block of generator parameter
ids: sixteen routes' worth of ids would be a permanent carve-out of the
device's own id space, spent on a capacity number this plan calls provisional,
and every route would then have to keep the slot it was authored in forever.
The cost is four bytes on `ParamAddr` and it is recorded in the footprint test.

**Polarity is shown on a route row, not chosen.** The step's UI sentence lists
it beside the signed amount. It is a property of the *source* — the LFO and Key
swing both ways, an envelope and velocity rest at zero — and the sign of the
amount already decides which direction a unipolar source travels. A per-route
polarity switch would be a second answer to a question the source table has
already settled.

**ML-P8's parameter ids are their own
namespace starting at zero**, and its descriptor table lives in `mlp8.rs`
rather than `generator.rs`. The shared `SYNTH_PARAM_*` ids and `100 + n * 10`
oscillator blocks exist because Mono and Poly are the same voice with a
different count; this device is not. Its serialized tag is `mlp8`, chosen
explicitly rather than taking the `rename_all` default `ml_p8` — the ML-M1's
frozen `ml1` is the reason to pick an on-disk name on purpose the first time.

This plan defines **ML-P8**, a new eight-voice polysynth. It replaces the
earlier Poly v2 design, whose identity depended too heavily on three stacked
oscillators, per-voice drift, unison, and chorus. Those can all make a sound
wider; none makes the oscillator section more programmable.

The original Poly synth remains as its own device. ML-P8 is not a rename or an
in-place migration of it, and old Poly projects continue to load unchanged.

The face is two pages from step 04: the instrument on one, ML-P8 MOD on the
other. A page bar costs a full face twenty-four pixels, which came out of the
VOICE region's knob diameters and its three displays rather than out of the
network grid.

Read 01 first, then work 02 through 07 in order:

1. `01-what-poly-is.md` is the product and DSP contract.
2. `02-per-voice-drift.md` builds the oscillator network, sub, and noise.
3. `03-the-multimode-filter.md` adds the two envelopes, multimode filter, and
   per-voice feedback loop.
4. `04-unison-groups.md` adds ML-P8's native LFO and internal modulation
   routes.
5. `05-internal-chorus.md` finishes allocation, optional drift, unison, and
   chorus without making duplication the instrument's identity.
6. `06-oscillator-sync.md` publishes the instrument's typed control and audio
   outlets.
7. `07-poly-factory-patches.md` is the listening, range-tuning, and identity
   pass.

The filenames are retained so existing references to this plan do not break;
their headings describe their new scope.

Step 02's own document records the ids, ranges, and curves as built, and what
is still provisional until step 07's listening pass.

The separate filter ADSR and keytracking that used to be an unnamed
prerequisite are now part of step 03. The device's own modulation is part of
step 04. The channel modulation rack remains useful for reaching other devices
and for adding channel-level sources, but ML-P8 must make complete patches with
no channel routes at all.
