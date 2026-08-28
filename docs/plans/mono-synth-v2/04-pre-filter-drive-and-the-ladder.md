# Pre-filter drive and the ladder

This is the Moog-ish half of Mono's identity. Step 05 is the other half; they
share the dispatch structure this step builds, so build it here properly.

## What is wrong

Two things, and they are the same thing.

**Drive is in the wrong place.** `apply_drive(filtered, drive)`
(`crates/mooloop-dsp/src/monosynth.rs:284`) saturates the filter's *output*.
Nothing an oscillator does can change how the filter behaves — level is
purely a gain. On the instruments Mono is aiming at, pushing the mixer into
the filter is the main tone control.

**The filter has no character.** `Svf` (`crates/mooloop-dsp/src/filter.rs:22`)
is a clean, linear, topology-preserving 2-pole SVF. It is a good filter and
exactly right for Poly. It is not the star of anything.

## Do this

### 1. Filter model dispatch

Add the enum and the state, but expect to add ACID's variant in step 05:

```rust
pub enum MonoFilterModel { Ladder, Acid }  // default Ladder
```

The real-time rule from the spec: **enum-based state with static storage, no
heap, no trait objects.** `MonoVoice` owns the state both models need as
concrete fields — a 4-element ladder state array covers Ladder and is likely
reusable for Acid — and `render_range` matches on the model once per block,
not per sample. Do not add a `Box<dyn Filter>`.

Switching model mid-note must not click. Reset the model's state only when the
voice is silent; a live switch keeps whatever state carries over and lets the
5 ms parameter smoothing cover the discontinuity. If a listening test shows an
audible pop, cross-fade over the smoothing window rather than adding a
mute-and-reset.

### 2. The ladder model

A nonlinear 4-pole ladder: four cascaded one-pole low-pass stages with a
resonance feedback path from the fourth stage back to the input, and a
saturating nonlinearity inside the loop. Zero-delay-feedback or a
one-sample-delay form with the usual cutoff compensation are both acceptable;
circuit accuracy is explicitly not required.

Three things it has to get right, in priority order:

1. **24 dB/oct-ish slope.** This is the audible difference from `Svf` and the
   whole point.
2. **Resonance to self-oscillation, stable there.** The existing SVF reaches
   near-self-oscillation via its `damping` term; the ladder gets it from
   feedback gain. It must stay finite at maximum resonance under maximum
   drive at any cutoff, including cutoff swept every sample by the filter
   envelope.
3. **Sensible low end as resonance rises.** Classic ladders thin out — the
   feedback path cancels bass. Some of that is the character; total bass loss
   is not. Add a bass-compensation term mixing a fraction of the input back
   in, and voice it by ear on the Round Bass patch (step 08).

Cutoff mapping stays `hz_from_normalized(cutoff, max_hz)` so the Cutoff knob
means the same thing across models, and the envelope/keytrack octave offsets
from step 02 apply unchanged. Channel modulation reaches Cutoff through its
descriptor path rather than a Mono-owned LFO term.

Put it in `crates/mooloop-dsp/src/filter.rs` next to `Svf`, not in
`monosynth.rs` — it is a primitive, and `filter.rs` is where the next person
looks.

### 3. Move drive ahead of the filter

The mix feeds a saturating pre-drive stage, whose output feeds the filter, and
the filter's output goes straight to the VCA:

```rust
let driven = pre_drive(mix, drive);
let filtered = /* model dispatch */;
let sample = filtered * amp_env.level() * velocity * tremolo;
```

The gain-staging problem this creates is real and is the reason to be careful:
three oscillators at full level sum to roughly 3× a single one, so pre-drive
sees wildly different input depending on the OSC page. The
`docs/plans/gain-structure/` work is the eventual answer; until it lands,
compensate inside the drive stage — normalize by a cheap running estimate of
the mix level, or apply makeup gain as a function of `drive` — so that turning
Drive up changes character much more than it changes loudness. Record whatever
you choose here in this file.

`apply_drive` is still used by the sampler and the drum synth
(`crates/mooloop-dsp/src/filter.rs`); leave it alone and add the pre-drive
stage beside it. Poly keeps post-filter drive for now — step 03 of the Poly
plan revisits placement there and the answer may legitimately differ.

### 4. Parameters and UI

`drive` keeps ID 8 and its 0-1 range. One new field:

| Field          | Kind                      | Default  | ID |
|----------------|---------------------------|----------|----|
| `filter_model` | `MonoFilterModel` enum    | `Ladder` | 28 |

`Ladder` as the migration default: it is the musically conservative choice for
an unmarked old patch, and old patches were 2-pole clean, so neither model is
a null-test match anyway.

UI: a `SelectorBank` in the FILTER panel header on the AMP/FILTER page —
`LADDER` / `ACID`, the slot step 02 left for it. `FilterResponseDisplay`
(`crates/mooloop-ui/ui/device-displays.slint`) takes a `mode` property that
mono currently hardcodes to 0; drive it from the model so the curve shows a
4-pole slope.

## Done when

- Ladder is audibly a 4-pole: measured stopband rolloff is around 24 dB/oct
  against `Svf`'s 12 at the same cutoff. Assert on a rendered sweep, with a
  loose tolerance — this is a nonlinear filter and the number will not be
  exact.
- Maximum resonance × maximum drive × envelope-swept cutoff stays finite and
  ≤ 1.0 peak. Extend `resonant_filter_and_drive_stay_bounded` to run per
  model.
- Raising oscillator 2's level with Drive up changes the timbre, not only the
  level. Assert as a harmonic-content change at roughly matched RMS.
- Turning Drive from 0 to 100% at a fixed patch changes output level by less
  than the character change — pick a concrete dB bound when the compensation
  scheme is chosen and write it into the test.
- Switching model on a sounding voice produces no step: reuse the `max_step`
  helper from the existing smoothing tests.
- Cutoff at a given knob position lands at a comparable frequency in both
  models.
