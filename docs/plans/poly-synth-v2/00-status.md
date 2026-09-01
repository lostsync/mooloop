# ML-P8 plan status

**Step 02 is in.** The device exists and plays: a new `MlP8` generator kind
beside the v1 poly synth, the three-oscillator network with all six directed
XMOD routes, self-feedback, noise into every phase input, hard sync with a
band-limited reset, a derived sub, deterministic coloured noise, eight fixed
voices, an amplitude envelope, and a three-page face. 03 through 07 are next,
in order.

Two things step 02 turned up that the plan could not have known:

- **The sync BLEP made aliasing worse until two mistakes were fixed** — the
  step height has to be measured on the *naive* waveform, and the oscillator's
  own cycle-boundary residual has to stand down for the sample after a reset.
  Neither is visible without building it, and neither shows up in a test that
  looks for energy in a high band, because a hard-synced oscillator is exactly
  periodic at its master's rate and every alias product folds back onto the
  master's own harmonic grid. The test compares harmonic magnitudes against an
  eight-times-oversampled render instead.
- **"Skip an oscillator nothing reads" needed a caveat.** The skip is decided
  once per block from the *target* levels, but levels are smoothed — so a
  level knob reaching zero un-needs an oscillator while its smoother is still
  milliseconds from silence, and skipping it there replaces the ramp with a
  step.

One decision worth knowing about: **ML-P8's parameter ids are their own
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
