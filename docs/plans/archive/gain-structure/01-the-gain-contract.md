# The gain contract

This is the reference document for the rest of this plan. It records the
decisions; the numbered steps implement them. Read this first even when
picking up a later step, and update it here — not in a step — if a decision
has to change.

## Why

Gain in mooloop is currently correct arithmetic with no shared policy. Each
site independently decided what a control means, so:

- The mixer fader is linear in amplitude and labelled with that ratio, so
  "75%" is -2.5 dB rather than the -6 to -10 dB a fader at three-quarter
  travel reads on any real console.
- Sources are built at close to full scale, so a single default drum channel
  peaks a couple of dB below clipping and nothing has room to sum.
- The convolution reverb's impulse response is peak-normalized, so its wet
  path is many dB louder than the dry path it is blended against.
- `linear_to_db` exists in three forms: `crates/mooloop-ui/src/meter.rs:71`,
  an inline `pow(10, v / 20)` at `crates/mooloop-ui/ui/main.slint:1479`, and
  the `-59.9` magic floor next to it.

The device-frame trims already do the right thing (`TrimKnob`,
`crates/mooloop-ui/ui/controls.slint:983`): dB from unity, -60 to +12,
unity at double-click, `+3.0 dB` readout. That control is the model. This
plan extends its convention to everything else and gives the audio a
reference level for it to sit against.

## Decisions

### Operating level

**-12 dBFS is unity operating level.** A generator's default patch, played
at default velocity with its channel at 0 dB, peaks at approximately
-12 dBFS. That leaves 12 dB of headroom, which is what lets sources sum
without the master needing to be pulled down first.

This is the single number that fixes "everything is already turned all the
way up". Everything else in this plan is either the controls that express
it or a specific stage that violates it.

Adam has explicitly waived backwards compatibility: existing project files
will get quieter, and that is fine. Do not add a migration, a compatibility
flag, or a version bump for level changes alone.

### Summing

**Sum honestly, like gear.** Do not normalize a summing point by its input
count and do not auto-attenuate as sources are added. Three oscillators at
equal level are ~9.5 dB louder than one, and should be. The reason adding
an oscillator currently feels dangerous is the absent headroom, not the
summing — with the operating level above, three oscillators at full land
near -2.4 dBFS instead of clipping.

This applies equally to channels summing into a bus and buses into the
master.

### Fader taper

Faders are **linear in dB over travel**, piecewise between these
breakpoints, interpolated in dB:

| Travel | dB    |
| ------ | ----- |
| 1.00   | +6    |
| 0.75   | 0     |
| 0.50   | -12   |
| 0.30   | -24   |
| 0.15   | -40   |
| 0.05   | -60   |
| 0.00   | -inf  |

Unity sits at three-quarter travel and the top of the throw is +6 dB. This
is the taper for `MixerFader`. Knobs do not use it — a knob's travel is
already linear in dB over its own range, which is the `TrimKnob`
convention.

### Ranges

- Trim/gain knobs: -60 dB to +12 dB, unity default, `-inf` at the floor.
  Already true of `TrimKnob`; keep it.
- Channel and bus volume: stored as **linear gain**, clamped to
  `MAX_LINEAR_GAIN` (4.0, +12 dB — `crates/mooloop-core/src/channel.rs:23`
  and its duplicate `MAX_TRIM_GAIN` at
  `crates/mooloop-engine/src/render.rs:60`). The dB is a presentation
  concern; the wire format and project format stay linear.
- Oscillator level: -inf to 0 dB. An oscillator never boosts.

### Readouts

Every gain, trim, level, and fader reads in dB, formatted as `TrimKnob`
already formats: `-inf`, `±0.0 dB`, `+3.0 dB`, `-12.4 dB`. Per the project's
tooltip convention, the tooltip carries the value only; explanatory text
belongs in the status bar.

Blend controls (wet/dry, and per-effect `mix`) stay in percent. They are
ratios, not gains, and a dB reading on them would be worse.

### Metering

Peak metering in dBFS, floor -60. Green below -10, yellow -10 to -3, red
above -3. Ballistics per IEC 60268-18 digital peak: instantaneous attack,
20 dB fall in 1.7 s, 1 s peak hold.

## The end state

When this plan is done there is one `mooloop-core` gain module and one
matching Slint global. Adding a gain control anywhere later means calling
them, not writing another `pow(10, v / 20)`. Step 03 builds them; every
later step is a caller.

A `docs/GAIN_STRUCTURE.md` summarising the contract for future work is a
deliverable of step 05, once the operating level is real rather than
aspirational.
