# Make it safe to bring in a second oscillator

## Problem

Adam: "it should be easier to try bringing in another osc without it feeling
like the synth is going to blow itself apart."

The summing itself is right. `crates/mooloop-dsp/src/monosynth.rs:247-262`
accumulates `mix += osc_level * osc.next_sample(...)` across the three
oscillators, and `crates/mooloop-dsp/src/polysynth.rs:324-328` does the
same. That is what gear does and `01-the-gain-contract.md` keeps it.

Three problems sit around it:

1. Each oscillator at full level is full scale on its own, so the *first*
   oscillator already uses the entire budget. Step 05 fixes this for the
   synth's default patch, but only for the default — a patch with one
   oscillator raised to full still clips before a second is added.
2. `OscParams::level` is linear in `[0, 1]`
   (`crates/mooloop-core/src/synth.rs:252`) and its knob reads in percent,
   so the useful range is crowded into the top of the control. Step 04
   converts the readout; this step decides what the value means.
3. The signal after the oscillator mix goes through a filter and
   `apply_drive` before the envelope (`monosynth.rs:276-284`). Drive is a
   gain stage with no compensation on this path, so raising it raises level
   as well as harmonics.

## What to do

1. Decide and document the per-oscillator unity reference. With honest
   summing and three oscillators, the choices are:

   - Each oscillator's 0 dB is full scale, and three at 0 dB is +9.5 dB over
     one. Truthful, and after step 05 lands at roughly -2.4 dBFS rather than
     clipping. **Prefer this** — it is what the contract says and it keeps
     one oscillator sounding the same whether or not others are enabled.
   - Each oscillator's 0 dB is one-third of full scale, so all three at 0 dB
     is unity. Safer, but it makes a single oscillator quiet and its level
     dependent on a design decision about a feature the user is not using.

   Whichever is chosen, write it into `docs/GAIN_STRUCTURE.md` next to the
   summing rule. The first option needs no code change beyond step 05 — it
   needs step 05 to be *verified* against the three-oscillator case, not
   just the default single-oscillator patch.

2. Check `apply_drive` (`monosynth.rs:284`, and the `PolySynth` equivalent)
   for output compensation. `FilterParams::drive`
   (`crates/mooloop-core/src/effect.rs:616`) documents itself as
   "compensated soft-saturation", so a compensated shaper already exists in
   this codebase — `crates/mooloop-dsp/src/shaper.rs` is the place to look.
   If the synth's drive is uncompensated, compensate it: a drive control
   should change character, not level.

3. Give both synths a device output trim if they do not already have one.
   `DeviceFrame` supplies `output-trim-enabled` /
   `output-trim-changed` (`crates/mooloop-ui/ui/device-rack.slint:191`) and
   the comment there is explicit that a face opts in only when its DSP has a
   real parameter to receive the value. Check whether `mono-device` and
   `poly-device` opt in; if not, that is the control a user reaches for
   after stacking oscillators, and its absence is part of the complaint.

## Constraints

- Do not normalize by the number of enabled oscillators. Enabling a second
  oscillator must not change the first one's level — that is the behaviour
  that makes a synth feel like it is fighting you.
- The smoothing on `osc_level` (`monosynth.rs:214-215`) exists so knob turns
  do not step the output, and there are tests for it
  (`monosynth.rs:598-624`). Any change to the level mapping has to keep the
  smoothing on the *linear* value; smoothing in dB through -inf will not
  behave.
- `OscParams::default` has `level: 0.0` (`synth.rs:263`), so oscillators 2
  and 3 are silent by default. Keep that.

## Verification

Step 02's oscillator-summing measurement, now expected to show three
oscillators peaking below 0 dBFS rather than clipping. The existing
`monosynth.rs` and `polysynth.rs` test suites must still pass — in
particular `oscillator_levels_gate_their_contribution` (`monosynth.rs:454`)
and the step-continuity tests. Add a drive-compensation test if step 2
changes the shaper: sweeping drive at fixed input should move harmonic
content without moving peak level much.
