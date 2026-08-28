# Give the faders a console taper and dB readouts

## Problem

`MixerFader` (`crates/mooloop-ui/ui/controls.slint:1080`) maps travel to
value linearly: `set-normalized` at `:1105` computes
`minimum + n * (maximum - minimum)`, and `normalized` at `:1092` is the
inverse. With `minimum: 0` and `maximum: 1`, travel *is* linear amplitude.
`crates/mooloop-ui/ui/mixer.slint:174` then labels it
`round(strip.volume * 100) + "%"`.

So three-quarter travel is 0.75 linear, which is -2.5 dB. That is exactly
the 2.6 dB Adam measured moving the master from the top of its throw to
"75%", and it is why every fader reads "100%" at rest: `MixerBus::new`
starts buses at `volume: 1.0` (`crates/mooloop-core/src/mixer.rs:74`).

`crates/mooloop-ui/ui/bus-device.slint:59-63` has the same control and the
same percent label. Meanwhile channel volume in the sequencer already uses
a `TrimKnob` in dB (`main.slint:1478`), so the two halves of the same
signal path currently disagree about what a gain control is.

Oscillator level knobs (`mono-device.slint:64-66`,
`poly-device.slint:65-67`) are also percent, and are gains.

## What to do

1. Give `MixerFader` a taper. Add an `in property <bool> db-taper` (or a
   `ValueScale`-style enum member — `controls.slint:3` already has
   `ValueScale`, and a third `fader` variant may fit better than a bool).
   When set, `normalized` and `set-normalized` route through
   `GainMath.fader-db-to-position` / `fader-position-to-db` from step 03
   rather than interpolating the raw value.

   The stored `value` stays linear gain throughout. Only the mapping from
   travel changes. This means no project-format change and no migration.

2. `default-value` becomes 1.0 (0 dB, three-quarter travel) rather than the
   current 0.8, so double-click returns a fader to unity. Check every
   `MixerFader` instantiation for a `default-value` override.

3. Replace the percent labels with `GainMath.format-db(...)`:
   `mixer.slint:174` and `bus-device.slint:62`. The travel ticks drawn at
   `controls.slint:1121-1129` are currently five evenly spaced marks; with
   a dB taper they should land on meaningful dB values instead. 0 and the
   -12/-24/-40 breakpoints are the natural choices, and `MeterScaleMath` in
   `meters.slint` already demonstrates the pattern for positioning marks
   from a value list.

4. Convert the oscillator level knobs to `TrimKnob` with `maximum: 0`
   (`mono-device.slint:64`, `poly-device.slint:65`). This is a UI change
   only in this step — the stored `OscParams::level`
   (`crates/mooloop-core/src/synth.rs:252`) stays linear in `[0, 1]` and the
   `.slint` boundary converts. Step 06 owns what those levels should
   actually be.

5. Sweep for any remaining gain control still reading in percent. The
   candidates are in `gallery.slint:261` (the `MiniKnob` volume demo) and
   `mockup.slint:293`. Blend controls — `mix` on drive, bitcrush, delay, the
   frame's wet/dry at `device-rack.slint:224` and `main.slint:2358` — stay
   in percent per the contract; do not convert those.

## Constraints

- Do not change what any stored value means. A bus at `volume: 1.0` must
  still render identically; it simply now displays "±0.0 dB" with its cap at
  three-quarter travel instead of "100%" at the top.
- `MixerFader`'s drag is deliberately relative, never absolute
  (`controls.slint:1179-1186` and the comment above it). Preserve that: with
  a taper, a relative drag moves *position*, which is what makes the throw
  feel even in dB. Do not convert the drag to operate on the value.
- The fine-drag and scroll divisors at `:1185` and `:1193` are in position
  units already, so they carry over unchanged.
- `01-the-gain-contract.md` fixes the top of travel at +6 dB. The fader's
  `maximum` must therefore be `db_to_linear(6.0)`, roughly 1.995 — not
  `MAX_LINEAR_GAIN`. The clamp in `OutputStage::set_volume`
  (`crates/mooloop-engine/src/render.rs:660`) stays at +12 dB, since
  automation and hand-edited files may legitimately go higher than the
  fader's own throw.

## Verification

`cargo test -p mooloop-ui`. Software-rendered snapshots of the mixer and of
`bus-device.slint` showing dB readouts and the cap at three-quarter travel
for a default project. Re-run step 02's fader-travel assertion: 0.75 travel
must now be 0 dB, and the kick-and-snare master peak must be *unchanged*,
since this step does not touch audio.
