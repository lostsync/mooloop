# The oscillator network, sub, and noise

This is the first defining implementation step. It turns the existing three
oscillators from parallel layers into a voice-level signal network.

## Three complete oscillators

Keep the existing waveform, coarse tune, fine tune, level, and pulse-width
controls on all three oscillators. Extend the oscillator primitive only where
the network requires a phase-modulation input, a visible wrap event for hard
sync, and a reset-to-phase operation.

Each oscillator exposes two internal taps:

- **modulation tap:** waveform output before its Level control;
- **mixer tap:** the same waveform multiplied by Level.

Level at `-inf` therefore mutes an oscillator from the source mix without
turning off its XMOD or sync duty. Do not skip an oscillator's DSP merely
because its audible level is zero when another active route consumes its tap.
The prepared route topology may still skip a genuinely unused oscillator.

## Six-way XMOD

Add one bipolar amount for every directed pair:

```text
1 -> 2    1 -> 3
2 -> 1    2 -> 3
3 -> 1    3 -> 2
```

The implementation is audio-rate phase modulation. The panel calls the
section **XMOD**, and status text says `phase modulation`; do not label the DSP
as analog exponential FM. Phase modulation keeps the carrier's center pitch
stable and supports deep, automatable harmonic movement without a separate
tuning-compensation system.

Every route reads the source oscillator's previous sample. All six routes are
therefore causal, can be active simultaneously, and sound the same regardless
of the order in which oscillator structs happen to be stored. Amount is
bipolar so one automation lane can pass through zero and invert the modulation
phase.

Tune the maximum phase deviation by ear in step 07. It must travel from subtle
animation through metallic spectra before its final quarter becomes hostile;
the useful range may not be compressed into the first few percent.

## Noise modulation and oscillator self-feedback

Noise has three bipolar audio-rate routes, one to each oscillator's phase-
modulation sum. This enables roughened pitch, unstable attacks, and broadband
spectra without making Noise Level audible in the source mixer.

Each oscillator also has a bipolar **Feedback** amount. It phase-modulates the
oscillator from its own previous sample. Self-feedback uses exactly the same
one-sample state and bounded scaling as cross-modulation; do not introduce a
special recursion path.

All modulation inputs are summed, then limited by a documented smooth bounded
function before they move phase. The bound prevents numeric failure; it must
not flatten the musically useful range.

## Hard sync is part of the network

Each oscillator gets a Sync Source selector: `OFF`, `1`, `2`, `3`, excluding
itself in the UI. When the selected source wraps, the destination resets its
phase at the sample's fractional wrap position.

A naive reset aliases. Apply a BLEP correction for the introduced waveform
discontinuity and verify it on high notes. Sync and XMOD may be active at the
same time: XMOD shapes the slave between master wraps. Changing sync source on
a sounding voice crossfades or applies another proven click-free transition.

## Sub oscillator

Sub is a derived per-voice source, not a fourth independently tuned oscillator.
It has:

- Source: Osc 1 / 2 / 3;
- Octave: -1 / -2;
- Wave: sine / square;
- Level: `-inf` to 0 dB.

It follows the selected oscillator's base pitch and hard-sync phase reference,
but not that oscillator's XMOD distortion. This keeps Sub a dependable
fundamental underneath a mangled carrier. Changing source or octave on a
sounding voice must not produce a DC step.

## Noise source

Noise is generated independently in each physical voice from a fixed seed
derived from the voice-slot index. It has:

- Level: `-inf` to 0 dB;
- Color: continuously dark through white to bright.

Color must be a stable, inexpensive filter with no allocation and no shared
global random state. A voice reset restores the same sequence, so a project
renders identically twice. If repeated notes become objectionably identical,
advance a deterministic per-slot note counter rather than introducing runtime
entropy.

## Reserved parameter IDs

ML-P8 is a new generator kind, so these IDs form its own append-only namespace.
The existing oscillator controls occupy the first reviewed block. Reserve the
next block in this step for:

| IDs | Parameters |
| --- | --- |
| 20-24 | Sub level, octave, wave, source, reserved expansion |
| 25-26 | Noise level and color |
| 27-32 | Six directed oscillator XMOD amounts |
| 33-35 | Noise-to-oscillator amounts |
| 36-38 | Three oscillator self-feedback amounts |
| 39-41 | Three oscillator sync-source selectors |

Record the exact constants beside the descriptor table when implemented. Do
not expose XMOD, self-feedback, or noise modulation in raw radians; their
natural UI value is signed percent mapped through one documented musical
curve.

## Done when

- A muted oscillator can audibly modulate another oscillator but contributes
  no direct mixer signal.
- Every one of the six XMOD directions works, and activating a reverse route
  creates a stable causal loop rather than order-dependent output.
- Noise can modulate each oscillator while remaining inaudible in the source
  mix.
- Oscillator self-feedback travels from subtle reshaping to aggressive spectra
  and stays finite at both polarities.
- All legal hard-sync pairs work with XMOD active; high-note alias energy stays
  within the chosen oscillator quality bound.
- Sub tracks the intended fundamental under deep XMOD and contributes exactly
  zero when its level is `-inf`.
- Noise sequences and the complete oscillator network render bit-identically
  across fresh processes.
- One oscillator at 0 dB still hits the generator reference level; adding
  sources sums honestly and does not normalize existing sources.
- Full eight-note chords at worst-case XMOD/self-feedback remain finite and
  complete within the realtime budget.

## What landed

The device exists: `MlP8Params` in `crates/mooloop-core/src/mlp8.rs`, the
`MlP8` node in `crates/mooloop-dsp/src/mlp8.rs`, an `MlP8` channel source and
generator kind, and a three-page face. The v1 poly synth is untouched.

### Parameter ids, as built

ML-P8's ids are **its own namespace starting at zero**, and its descriptor
table lives in `mlp8.rs` beside the struct rather than in `generator.rs`. The
shared `SYNTH_PARAM_*` ids and the `100 + n * 10` oscillator blocks exist
because Mono and Poly are the same voice with a different count; this device
is not, and two unrelated numbering schemes in one file is a collision neither
of them can see. Two tests hold the reservation: ids are unique, and none
reaches outside 0-41 or lands on the reserved 24.

| IDs | Parameters |
| --- | --- |
| 0-14 | Three oscillator blocks, five wide: wave, semis, cents, level, width |
| 15-19 | Amp attack, decay, sustain, release, glide |
| 20-23 | Sub level, octave, wave, source (24 reserved) |
| 25-26 | Noise level, colour |
| 27-32 | XMOD `1>2, 1>3, 2>1, 2>3, 3>1, 3>2` |
| 33-35 | Noise into oscillators 1-3 |
| 36-38 | Oscillator self-feedback 1-3 |
| 39-41 | Sync source 1-3 |

### Ranges and curves, as built

All provisional until step 07's listening pass; the numbers are here so that
pass has something specific to move.

- **Modulation amounts are signed percent**, `-100..100`, linear in the
  descriptor. The musical curve is in the DSP: `route_depth` squares the
  magnitude and keeps the sign, so the low half of the knob buys fine
  animation and an automation lane through zero inverts the modulation phase
  rather than folding.
- **`ROUTE_MAX_CYCLES = 2.0`** — phase deviation one route reaches at 100%.
- **`PHASE_BOUND_CYCLES = 4.0`** — where the summed inputs asymptote. The
  bound is `x / (1 + |x| / B)`: unity slope at zero, differentiable
  everywhere, so it cannot flatten the useful range or put a corner where an
  automation sweep would hear one. This is the safety mechanism, inside the
  sound; there is no limiter after the voice sum.
- **Noise Color is bipolar percent**, dark at `-100` and bright at `+100`,
  because white is a centre to tune away from in two directions rather than a
  half-way point. Two one-pole states, `NOISE_DARK_HZ = 700` and
  `NOISE_BRIGHT_HZ = 4000`; the RMS compensation is derived from the
  coefficient rather than tabulated, so the colour does not change level with
  the sample rate. Measured spread across dark/white/bright is under 3 dB.

- **`VOICE_OUTPUT_REFERENCE = 0.51`**, measured rather than derived, and the
  same figure the v1 poly synth uses: the default patch is one band-limited
  saw into a VCA with nothing in between, which is that synth's default path
  too. `gain_structure_tests` now puts ML-P8 at -12.0 dBFS beside every other
  generator. Adding a source never turns the first one down; an eight-note
  worst case sums honestly and is bounded but not normalized.

### What it costs

A `ChannelStrip` holds one node of every generator kind, so a new device is
paid for on every live channel whether or not anything uses it. `MlP8` is
2,016 bytes, the strip grew 2,656 (adding it also moved the padding between
its neighbours), and a sixteen-channel project went from 1,157 KiB to 1,198.
`MlP8Params` is also now the widest block a command carries, so
`EngineCommand` went from 136 to 152 bytes. Both numbers are pinned by the
tests `docs/plans/archive/modulator-capacity/` left behind, and both were updated
rather than worked around.

### The oscillator primitive

`Osc` gained exactly three things, as the step allows: a phase-modulation
offset applied at the read (not to the accumulator, so centre pitch does not
move — pinned by a test), a sub-sample wrap position, and a sync reset that
reports the step height it introduced.

Two things about the BLEP were only findable by building it:

1. **The step height has to be measured on the naive waveform.** Reading it
   through `wave_value`'s PolyBLEP means measuring a value that has already
   been corrected once, and correcting it again.
2. **The oscillator's own cycle-boundary residual has to stand down for the
   sample after a reset.** A sync reset lands the phase inside that residual's
   window, where the waveform would otherwise correct a wrap that did not
   happen — and correct it for the wrong height, since a natural wrap steps by
   the full range and a sync reset steps by wherever the slave had reached.

With both wrong the correction made aliasing *worse*, which is why the test
compares harmonic magnitudes against an eight-times-oversampled render rather
than looking for energy in a band: a hard-synced oscillator is exactly
periodic at its master's rate, so every alias product folds back onto the
master's own harmonic grid and no band of the spectrum is alias-only. Against
that reference the correction cuts harmonic error by about a third.

### Sub

A phase accumulator at `source_ratio / divisor`, hard-sync-reset with the
same band-limited correction whenever its source is. It never sees the
source's cross-modulation, which is what leaves a fundamental standing under
a carrier that has been taken apart. Changing octave or source moves the
frequency and leaves the phase alone, so neither steps.

### One thing the plan did not anticipate

The step's rule is that an oscillator is skipped only when *nothing* reads it,
and `Prepared` decides that once per block from the parameters. But it reads
the **target** level, and levels are smoothed — so the moment a level knob
reaches zero the oscillator stops being needed while its smoother is still a
few milliseconds from silence, and skipping it there replaces the ramp with a
step. Liveness is therefore `needed || smoothed level still above epsilon`,
with a test that turning a source down does not step the output and does
still reach silence.
