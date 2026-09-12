# 02 — The curve tells the truth

The response plot is the EQ's main instrument and it is currently drawing
something other than what the device does.

## Three faults

**The pass filters are not on it, and their data already arrives.**
`eq_band_data` appends the high-pass and low-pass at four floats each after
seven bands at five, and `EqResponseDisplay.band-value` reads `index * 5 +
field`. A flat float array with an implicit layout, produced in one file and
consumed in another, with nothing asserting they agree.

**A shelf is drawn at a fixed exponent.** `1 / (1 + pow(frequency / center,
1.6))`, so the plot cannot follow a shelf's slope even after step 03 gives it
one.

**Clicking a point selects a band and the controls lag it.** `point-grabbed`
does fire `target-changed`; the knobs are bound to `slot.pN`, which arrives on
the next publish. Confirm the sequence under `scripts/mooloop-mcp` before
designing anything -- if it is a publish-ordering bug it is a much smaller fix
than a redesign, and step 01 changes the binding anyway.

## The fix for the layout, from the strip

Do not renumber the flat array. Take the strip's answer: hand the markup the
descriptor table and the values, and let the display read them by id.
`install_strip_spec` does this once at startup and `tests/strip_face.rs` fails
if a bound is ever spelled in the markup -- which also removes the last two
numbers `dupe-audit unchecked-face` still reports.

After step 01 there are 43 ids for one EQ, so an array indexed by id is the
natural shape and the stride problem cannot recur.

## Acceptance

- The high-pass and low-pass appear on the curve, at their slope, and their
  points are grabbable.
- A shelf's drawn curve changes when its Q changes, and matches what the DSP
  runs -- the same standard `strip_face.rs` holds the strip's plot to.
- Clicking any point selects that band and the controls show *that band's*
  values before a drag can write them.
- No range or curve constant is spelled in `eq-device.slint`.
