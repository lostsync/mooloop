# Measure the current gain structure

## Problem

Every claim in `01-the-gain-contract.md` is a measurement, and none of them
are currently pinned by a test. Adam's own measurement — a default kick and
snare peaking at -4.2 dBFS on the master, dropping only to -6.8 when the
master fader moves to "75%" — is the kind of number this plan needs to be
able to reproduce before and after.

Without these, later steps cannot tell a fix from a regression, and the
-12 dBFS operating level in step 05 is a guess.

## What to do

Add offline render tests. `crates/mooloop-engine/src/offline.rs` and the
existing `rendered_energy` helper in `render.rs`'s test module
(`crates/mooloop-engine/src/render.rs:2529` and its callers) are the
starting points; prefer peak measurements over energy here, since headroom
is a peak property.

Capture at minimum:

1. **Source peak at unity.** For each `DeviceKind` (`Sampler`, `DrumSynth`,
   `MonoSynth`, `PolySynth`), one default-patch note at default velocity,
   channel at 0 dB, bus at unity — record the master peak in dBFS. This is
   the number step 05 moves to -12.
2. **The kick-and-snare case.** Reproduce Adam's measurement: a default kick
   and a default snare on separate channels, master peak in dBFS. Assert it
   as a range, not a point.
3. **Summing.** N identical channels for N in 1, 2, 4, 8 — assert the peak
   grows the way honest summing predicts, and record where it crosses
   0 dBFS. This is the test that proves step 05 bought real headroom.
4. **Oscillator summing.** One oscillator at full level versus three, on
   both `MonoSynth` and `PolySynth`. Expect ~9.5 dB.
5. **Reverb wet/dry gain.** Render a signal through a default `Reverb` at
   100% wet and at 0% wet, and record the ratio. Do the same for `Plate`.
   This is the number step 07 exists to fix; expect it to be badly above
   unity, which is the confirmation that peak-normalizing the IR
   (`crates/mooloop-dsp/src/effects/reverb.rs:209`) is the cause.
6. **Fader travel.** Not an audio test — assert directly that the current
   `MixerFader` mapping turns 0.75 travel into -2.5 dB, so step 04 has a
   failing case to flip.

## Constraints

- These are characterization tests, not correctness tests. They lock in
  behaviour that is *about to change*. Name and comment them so the next
  agent knows to update the expected values deliberately rather than
  treating a diff as a bug. A `gain_structure` test module with a header
  comment pointing at this plan directory is enough.
- Use wide tolerances. The point is catching a 6 dB shift, not a 0.1 dB one.
- Do not fix anything in this step. If a measurement looks wrong, record it
  and note it — the fix belongs to the step that owns that stage.

## Verification

`cargo test -p mooloop-engine` and `cargo test -p mooloop-dsp`. Write the
measured values into `00-status.md` so later steps can compare without
re-running anything.
