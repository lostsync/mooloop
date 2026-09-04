# ML-P8 plan status

**Steps 02, 03, 04 and 05 are in.** 06 and 07 are next, in order.

Step 05 finished the pool. A note is a *group* of physical slots: Unison at
1x/2x/4x/8x spends the eight rather than growing them, groups are allocated and
stolen whole, and a slot a smaller group steals leaves through the same short
de-click transition rather than stopping. Detune and Spread place a group's
members symmetrically about the note; at 1x Spread places notes by their stable
slot positions. Drift is stable per-slot character over pitch, cutoff, the
envelope times and the oscillator start phases, with no runtime entropy at any
setting. The finisher is four fixed policies over the rack's own
`ModulationEffect`, running on ML-P8's scratch buses.

Six things that step turned up:

- **A group needs an identity, and `age` already was one.** Every member of a
  group is stamped with the same age when it is allocated, so "the oldest age
  still on the board" names a whole group and stealing needs no second table.
  The event id could not do it: it identifies the *note*, and a slot being
  retired has stopped being part of one.
- **Stealing can be asymmetric.** A patch that was at 8x when a note started
  and is at 2x now steals eight slots to fill two, and the six left over have
  nothing to adopt. Dropping them is the one sample of silence in the middle of
  a sound that stealing exists to avoid, so they retire through the release
  instead, with their age set to zero so they are first in line to be reused.
- **"Drift 0 is exactly authored" is an identity, not a tolerance, and the
  code has to earn it.** Every multiplier Drift introduces is written so it is
  exactly `1.0` at zero — `exp2(0.0)` is one, and `x * 1.0` is `x` bit for bit
  — rather than being skipped by a branch. That is why the drifted pitch is
  folded into the oscillator ratio the sub already reads, instead of being a
  second multiply the sub would have needed its own copy of.
- **The finisher is nearly free, and the worst case has headroom.** Measured
  on the build box on a release build, 4.3 seconds of audio at 48 kHz in
  512-frame blocks: the step's worst case — eight physical voices as one 8x
  group, three pulse oscillators with every XMOD, self-feedback and noise
  amount at 100%, all three sync pairs live, a resonant LP24 with drive and
  voice feedback, Drift, Detune and Spread at their tops, and sixteen internal
  routes — costs **12.2% of one core**. Adding the Ensemble chorus takes it to
  13.2%, so the finisher is one point for a whole instrument's worth of it. The
  figure is a measurement rather than a test: a wall-clock assertion in the
  suite would be flaky on a laptop and would fail for reasons that are not
  this device's.
- **The face had no room, and three rack units was the boundary.** The VOICE
  region fitted its 664px exactly, with no slack at all — a sketch at 664
  shows every control and nothing to spare. Allocation and character had to go
  somewhere, and the only alternatives to widening the face were taking width
  from the network grid or height the region did not have. Four units, on the
  same argument DS-01's five already stand on. The controls sit rightmost,
  which is also where the plan wants the chorus to feel.
- **The finisher's buffers were the expensive part, and the fix was to render
  in chunks.** The chorus may not read the channel bus, so it needs two of its
  own; at `MAX_BLOCK_SIZE` that is 128 KB on every materialized channel for a
  control that is off by default. Rendering `render_range` in 512-frame chunks
  makes it 8 KB, and costs one `Prepared` per chunk. The engine's footprint
  test is what asked the question — `size_of::<MlP8>()` grew by 984 bytes and
  the test's job is to make that a decision rather than a drift.

In more detail, from the earlier steps: the device exists and plays — a new
`MlP8` generator kind beside the v1 poly synth, the three-oscillator network
with all six directed XMOD routes, self-feedback, noise into every phase
input, hard
sync with a band-limited reset, a derived sub, deterministic coloured noise,
eight fixed voices, and — from 03 — two envelopes, four filter modes,
keytracking, both velocity depths, and a feedback loop around the filter with
the drive inside it. From 04 it has its own modulation: an audio-rate LFO with
six waves, Warp, Slew and three retrigger policies; six per-voice sources
reaching thirty-one continuous destinations through authored routes; and an
ML-P8 MOD page to author them on. A complete moving patch needs nothing from
the channel modulation shelf. From 05 it allocates in groups, drifts by slot,
detunes and spreads them, and finishes with a chorus that is off by default.

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
