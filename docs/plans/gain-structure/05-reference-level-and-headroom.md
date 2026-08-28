# Calibrate sources to a -12 dBFS operating level

## Problem

This is the step that fixes "everything is hot as hell by default".

Nothing in the codebase defines an operating level, so every generator was
tuned in isolation against full scale. The drum synth's kick body gains run
0.72 to 1.18 depending on character
(`crates/mooloop-dsp/src/drumsynth.rs:261-265`), multiplied by an envelope
that reaches 1.0. `Channel::new` then defaults to `volume: 0.8`
(`crates/mooloop-core/src/channel.rs:57`) — a hidden -1.9 dB that is not a
design decision so much as an attempt to leave a little room — and buses
sum at unity (`crates/mooloop-core/src/mixer.rs:74`).

The result is that a single default drum channel peaks a couple of dB below
clipping, which is why a second channel feels like it has nowhere to go, and
why the master fader is the only control that appears to do anything.

## What to do

1. Add the operating level to `crates/mooloop-core/src/gain.rs` from step
   03:

   ```rust
   /// Peak level a generator's default patch produces at default velocity
   /// with its channel at unity. The headroom between this and 0 dBFS is
   /// what lets sources sum without pulling the master down first.
   pub const REFERENCE_PEAK_DBFS: f32 = -12.0;
   ```

2. Calibrate each generator so step 02's measurement 1 lands within about
   1 dB of it. Prefer adjusting the device's own default parameters and
   internal gain staging over bolting a correction constant onto its
   output — a `* 0.25` at the end of `process` is a fudge that the next
   person will not understand. Where a per-device output reference genuinely
   is the clearest expression, name it and document it against this
   constant.

   `DrumSynth` is the loudest offender and the one Adam measured; start
   there. The character tables at `drumsynth.rs:261-265`, `:286-288`, and
   `:315-319` are relative balances between characters and should keep their
   *ratios* — scale the set, do not retune each entry by ear.

3. Set `Channel::new`'s default `volume` to 1.0 (`channel.rs:57`). A fresh
   channel should be genuinely at unity, with the headroom coming from the
   source calibration rather than from a quiet default fader. Update
   `MixerFader`'s `default-value` to match if step 04 has not already.

4. Write `docs/GAIN_STRUCTURE.md`, summarising `01-the-gain-contract.md` as
   standing documentation rather than plan state: the operating level, the
   honest-summing rule, the fader taper, the control ranges, and a pointer
   to `mooloop_core::gain` as the only place these are implemented. Add it
   to the task-context table in `AGENTS.md` under audio-engine work.

## Constraints

- Do not compensate by lowering bus or master defaults. Buses are summing
  points and must stay at unity, for the reason already documented at
  `mixer.rs:71-73` — assigning a channel to a bus must never quietly
  attenuate it.
- Do not add a limiter or any automatic gain control to the master to
  "protect" the new headroom. The headroom is the mechanism; a safety net
  would hide whether it is working.
- Velocity response is part of this. If a generator's default velocity does
  not map to its calibrated peak, fix the mapping rather than the peak.
- Sampler content is user-supplied and cannot be calibrated. `Sampler`'s
  measurement should be taken with a known test asset, and the honest answer
  for arbitrary samples is that the channel trim is where the user matches
  them to the reference.

## Verification

Re-run every measurement from step 02. Expected: each generator's default
patch within ~1 dB of -12 dBFS; the kick-and-snare case moving from about
-4.2 to somewhere near -9; the summing test crossing 0 dBFS at a much
higher channel count than before. Update the recorded values in
`00-status.md`.

Then listen. This step changes how the application sounds more than any
other, and the test suite can only confirm the numbers moved, not that the
defaults are still musical.
