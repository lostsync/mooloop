# ML-P8 plan status

**Every step is in, and the plan closed 2026-09-05.** Steps 02 through 05 and
07 landed first; 06's control half followed, and its audio half was the last
thing open in this directory.

**06 closed 2026-09-05**, by `typed-audio-edges/`. ML-P8's seven audio outlets
are connectable: a producer fills a tap only for an outlet somebody has
subscribed to, and an `Aux In` channel is what subscribes. The step's own
acceptance fixture is a test —
`an_aux_in_hears_an_oscillator_its_producer_does_not_carry` — and it holds both
halves, that `Osc 3` at level zero is audible through the edge *and* that
ML-P8's own output does not carry it. Read
`docs/plans/archive/typed-audio-edges/00-status.md` for what the doing changed;
the one thing worth knowing here is that a subscriber had to become a reason
to *run* an oscillator, since `Prepared` skips a source at level zero that
nothing else needs — which is exactly the source this step exists to publish.

## The face was rebuilt on 2026-09-04

Not a step: Adam played the shipped face on a 14" laptop and the verdict was
that the controls are "small and hard to see", with the cutoff knob a smudge.
He is right, and the reason is measurable. ML-P8 has sixty-nine parameters and
one 884x240 face, and the only way that ever fit was a 20px dial with a 9px
caption. **A face that fits by shrinking its controls has not fit.**

It is five pages now -- OSC, NETWORK, FILTER, AMP, ML-P8 MOD -- the way the v1
mono and poly faces are, and every control is a 34px dial. The width did not
change; the density did. `mockups/` holds the concept and the three rounds of
Adam's notes that shaped it.

Three things the rewrite turned up:

- **The knob it needed did not exist.** `ParameterKnob` stacks a caption and a
  value around a large dial but the value is read-only; `KnobField` makes the
  value typed into but lays the parts in a row, 130px wide at a legible dial
  size. ML-P8 has typed entry on every control, so swapping to `ParameterKnob`
  would have quietly removed it. `KnobStack` is both, and its dial is a real
  `ParameterKnob` with its captions off so there is still one implementation of
  what a drag or an armed route does to a gesture.
- **The grid did not need redrawing.** It is absolutely placed off four
  constants, so giving it a page to itself was `cell-w: 46px` becoming 176px.
  The concept reimplemented it and got the fill directions wrong on the first
  try; the shipped one was already right.
- **The output stage earns its place.** Adam asked for a master volume and pan.
  They are new ids 69 and 70, and they are not a second channel fader:
  `VcaLevel` and `Pan` were already per-voice modulation destinations resolving
  from *hardcoded* unity and centre, so a Velocity route on Pan swung around
  dead centre whatever the patch wanted. They are the authored base now, and
  Spread widens around wherever the device sits rather than around the middle.
  A test pins that the render is bit-identical at their defaults, so the gain
  contract is untouched.

## Step 06, so far

What a device publishes now has a vocabulary, and ML-P8 has filled it in. What
does *not* exist yet is the route mechanism that would let anything consume it,
and the split is deliberate rather than a stopping point chosen at random —
see "What 06 still needs" below.

**Landed:** `mooloop_core::outlet` is the generic declaration — `OutletDomain`,
`OutletTap`, `OutletDescriptor`, and the rule that a control outlet declares
[`ControlLatency::OUTLET`] whether or not its author remembers to. ML-P8's
fourteen outlets are declared with frozen ids: seven control (`LFO`, both
envelopes, `Velocity`, `Note`, `Gate`, `Trigger`) and seven audio (`Osc 1-3`,
`Sub`, `Noise`, `Pre-Filter Mix`, `Filter`) with their tap points. The device
computes and publishes the seven control values, reduced through the focus
group.

Three things that half turned up:

- **`Gate` and `Trigger` had to be built from different state, not the same
  state read twice.** `Gate` is "any note is held", which is the channel-level
  fact and does not fall when the newest note of a chord is released while an
  older one is down; `Trigger` is "a note started since you last looked". The
  second one has no width in samples — its width *is* the publication cadence
  — so publishing is what clears it, and a device cannot define it alone.
- **`Velocity` publishes the event, not the ramp.** The voice's smoothed
  velocity gain is what the VCA uses, and on a stolen slot it is still sliding
  from the previous note. The outlet is a fact about the note, so the voice
  now carries the velocity it was played at beside the smoother that plays it.
  Eight bytes a voice, and the test is that the two are measurably different
  on a stolen slot.
- **The published rate is `PerBlock`, and saying so is the point.** A local
  modulator runs on the 32-frame tick; a device reduces per-voice state into
  one number at the end of its block. Publishing per tick would mean rendering
  the device in 32-frame pieces so the reduction had somewhere to happen, and
  storing a tick's worth of every outlet against the chance somebody reads it.
  Declaring the coarser rate honestly is what leaves that upgrade open — a
  consumer told `PerBlock` cannot come to depend on more.

**Outlets are routable.** A route names its source through `ModSourceRef`
rather than a bare `ModSourceId`, because a generator outlet is not a rack
module and has no identity the rack could mint for it. It resolves into a flat
control address space — the rack's eight slots, then the generator's eight
outlets — so `offset_for` never learns there are two kinds of source, and the
persisted form carries an outlet id where a module route carries an identity.
An engine test drives a filter cutoff from ML-P8's `Gate` and asserts the
value is still at its base in the block the note lands in and moved in the
block after: the one block of declared latency, observed rather than assumed.

Two things that came out of wiring it:

- **The one-block latency is an ordering fact, not a delay.** The control
  table is filled before the strips render, so what a route reads is
  necessarily what the generator published at the end of the previous block.
  Nothing schedules it and nothing can forget to, which is what makes an
  offline render agree with a live take.
- **The two halves are captured at different rates, and pretending otherwise
  cost 8 KB a live channel.** The first shape put the outlet band in the
  per-tick control table, which stored eight block-constant values in all 256
  tick rows. `ControlSources` keeps the flat address space that routes and
  projects depend on while storing each half at the rate it is actually
  captured; the whole cost of routable outlets is now 32 bytes a live channel
  and 16 KiB of the reserved figure. The engine's footprint test is what asked
  the question, for the second time in two steps.

**The picker offers them.** A generator that publishes control outlets gives
the modulation shelf an OUTLETS pane beside its module grid: one named chip per
outlet, with the live meter a module tile already carries, selectable and
armable on exactly the terms a module is. The assign gesture is unchanged --
arm, then drag a knob -- because selection and arming were already carried as a
slot in the flat control address space, and an outlet's slot is in the upper
half of it. Three things it took:

- **`DeviceKind` had to answer what it publishes.** `PublishesOutlets` had no
  implementor: the table existed and nothing could be asked for it. It answers
  with the control prefix, so an audio tap cannot reach a control destination
  by being in the same table as one -- the refusal is a domain check rather
  than a rule the picker has to remember.
- **An outlet route's polarity is the destination's default, and the obvious
  guess is wrong.** `ModPolarity` describes how a route reads a *rack module*,
  which always emits `-1..1`: `Unipolar` lifts it into `0..1` so a one-way
  module rests at the base. An outlet publishes in the range its `SignalShape`
  declares, where a unipolar one is *already* `0..1`, so `Bipolar` is what
  passes it through. This was got backwards on the first pass -- a `Gate`
  route was defaulted to `Unipolar`, which sat it half a depth *above* the
  base at idle and gave it half the swing -- and corrected the same day. The
  two conventions are now written down in `MODULATOR_SYSTEM_SPEC.md`, which is
  where the trap belongs: they share one address space and disagree about
  rest.
- **An outlet does not move, and a reorder must not disturb it.** Selection and
  arming follow a module's durable id across a drag; an outlet has no rack
  identity to follow, so it keeps its slot. Re-deriving it through the rack
  would have silently disarmed a `Trigger` every time two LFOs were swapped.

An outlet has no editor pane, because the device that publishes it owns its
behaviour; the pane shows the declaration -- shape, rate, and block latency --
and the arm button, which is the reason it is in the picker.

**What 06 needed was the audio outlets, and they landed 2026-09-05**, in
`docs/plans/archive/typed-audio-edges/`. Nothing in the control half above
changed to accommodate them: an audio outlet is still never offered as a
control source, and `OutletDomain` is still what refuses it.

## Step 07 is closed: the bank shipped and was played

Eight patches, in `mlp8_factory`, seeded once into `presets/generators/mlp8/`
as generator presets -- generator rather than channel, because an ML-P8 patch's
modulation is its own routes and its own LFO and has no channel rack to
re-scope. That is the DS-01 bank's shape rather than the ML-M1's, and it is
the same argument: the plan's rule is that a patch reaches its sound with no
channel routes at all, so a bank that needed one would have contradicted the
step it belongs to.

The plan's constraint is the interesting part, and it is a test rather than a
note: **seven of the eight run at Unison 1x with the chorus off, and five
leave Drift at 0.** A patch that needed a duplicator to be interesting would
not have proved the network, so `the_bank_makes_its_case_before_the_finishers`
counts them, and also checks that Wide Machine -- the one patch that is
*about* the finishers -- actually uses all four, so the count cannot be
satisfied by a bank that simply never turns them on.

Three things authoring it turned up:

- **Init Saw has to be the device default, byte for byte.** The gain contract
  is calibrated against one saw at the reference level, so a reference patch
  that differed from `MlP8Params::default()` would be measuring something the
  contract does not describe. It is asserted rather than assumed.
- **Unison sums, and the patch pays for it.** Wide Machine at 4x peaked at
  1.46 on a single note: four voices at full level, because the device does
  not normalise by voice count and the plan says it must not. The fix is the
  patch's own Volume, not a normaliser, and the bank test asserts headroom
  (peak <= 0.95) rather than merely "did not clip" -- which is what turned
  this up.
- **Distinctness needs a measure that is not loudness.** Eight copies at eight
  volumes would pass a sample-for-sample inequality. The acceptance test spans
  *brightness* -- high-frequency energy over total energy, which does not move
  with level -- so the bank has to differ in timbre and not in gain.

**Adam played the bank and closed the step on 2026-09-05.** The range
decisions it lists -- XMOD curve, self-feedback scaling, Sub balance, LP24
resonance distribution, Voice Feedback bounds, LFO Warp/Slew, Detune maximum,
whether Chorus needs a Mix -- were the point of the pass, and **none of them
moved**: the values in steps 02 and 03 are the answer rather than a placeholder
for one, and the documents that called them provisional now say so. The bank
made the pass possible in one sitting, which is what it was for: open a
channel, switch it to ML-P8, and the eight patches are in its preset rail.

The step's own "done when" list carries two items the pass did not settle,
and both are on the audio half rather than the ear. Its third published-
interface fixture -- muted Osc 3 feeding a consumer through its pre-Level
outlet -- says outright that it waits for typed audio edges, and the automation
abuse pass has been run over the control surface the device actually exposes.
Neither holds the step open; both belong to 06.

The audio outlets stayed declared and unconnectable until 2026-09-05, which is
the split the step itself offers, and the reason was cost: materialising seven
stereo taps with nothing able to read them would be 56 KB a channel and
buffers in the callback for no consumer, which the same step forbids in its
last line. `typed-audio-edges/` paid it the other way round — a tap is
allocated when it is subscribed and not before — and its footprint test
records that a project with no edges holds no buffers at all.

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

Step 02's own document records the ids, ranges, and curves as built. They were
provisional until step 07's listening pass; that pass happened on 2026-09-05
and moved none of them, so they are simply the ranges now.

The separate filter ADSR and keytracking that used to be an unnamed
prerequisite are now part of step 03. The device's own modulation is part of
step 04. The channel modulation rack remains useful for reaching other devices
and for adding channel-level sources, but ML-P8 must make complete patches with
no channel routes at all.
