# Put the meters on a real standard

## Problem

Adam: "i usually expect yellow to start at -10dB. let's use a real metering
standard if we aren't."

We are close but not on one.

- `SegmentedMeter` (`crates/mooloop-ui/ui/meters.slint:74`) has
  `warning-db: -12` (`:78`) and `hot-db: -3` (`:79`). Yellow starts 2 dB
  lower than expected.
- `MeterBallistics` (`crates/mooloop-ui/src/meter.rs:34`) attacks
  instantaneously and falls at `DECAY_DB_PER_SECOND = 20.0` (`:4`), with
  `HOLD_SECONDS = 1.0` (`:5`). Instantaneous attack and a 1 s hold are both
  standard for a digital peak meter; the fall rate is arbitrary.
- The scale is linear in dB from -60 (`meters.slint:20`), so a 12-segment
  meter spends 5 dB per segment and gives the top 20 dB — where all the
  decisions happen — only four segments.

## What to do

1. Move `warning-db` to -10 (`meters.slint:78`). Keep `hot-db` at -3. Check
   every override at the call sites; `MasterMeter` and `ChannelMeter`
   (`meters.slint:162`, `:203`) may pass their own.

2. Adopt IEC 60268-18 digital peak ballistics: instantaneous attack (already
   correct), **20 dB fall in 1.7 s**, 1 s peak hold (already correct). That
   makes `DECAY_DB_PER_SECOND` about 11.8. Update the test at
   `crates/mooloop-ui/src/meter.rs:107` —
   `attacks_immediately_and_decays_at_twenty_db_per_second` — including its
   name.

3. Move the meter's dB constants into `mooloop_core::gain` alongside
   `MIN_DB` from step 03, and mirror the colour thresholds into the
   `GainMath` global so the meter scale and the fader scale cannot drift
   apart.

4. Optional, and worth doing if the meters still read poorly after the
   above: give the meter scale more resolution near the top, the way
   hardware meters do. `MeterScaleMath.normalize`
   (`meters.slint:13-15`) is the single place the mapping lives, and the
   tick list at `:21` already anticipates uneven spacing
   (`[0, -6, -12, -18, -24, -36, -48]` — note those are not evenly spaced).
   A piecewise mapping giving the top 20 dB half the meter's length would
   match that tick list's intent. `segment-color` (`:97`) derives its
   threshold from the segment index and would need to follow the same
   mapping.

## Constraints

- Change the ballistics and the colours in separate commits. Both alter
  every snapshot, and a snapshot diff that mixes them is unreadable.
- The clip latch (`CLIP_LATCH_SECONDS = 2.0`, `meter.rs:6`, triggered at
  `linear_peak >= 1.0`, `:57`) is a full-scale detector and is unrelated to
  the warning threshold. Leave it alone.
- Step 05 makes everything quieter by design. Do not compensate for that by
  moving the meter thresholds — the whole point is that meters now read in
  the range they were always meant to.

## Verification

`cargo test -p mooloop-ui`. Software-rendered snapshots of the master meter
and a mixer strip at a few known levels, checking the colour transition
lands at -10. Play the kick-and-snare case from step 02 and confirm the
master meter reads near -9 in green, which after step 05 is what a healthy
default project should look like.
