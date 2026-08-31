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
