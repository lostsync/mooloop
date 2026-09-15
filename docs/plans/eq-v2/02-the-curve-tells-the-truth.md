# 02 — The curve tells the truth

**Landed 2026-09-14 on `feat/eq-curve-truth`.** `00-status.md` records what it
did; what is below is the step as it was written, with the notes added on the
way past. Two of its own conclusions did not survive being built, and both are
worth reading against the result:

- **The fidelity question had a third answer.** It offered two -- evaluate the
  real response in Slint, or state a weaker standard -- and the answer was
  neither: Rust samples the curve from the coefficients the audio path
  designs, which is what the channel strip's compressor plot already did.
- **The band half did not "stay as it is".** Once the curve arrived from
  Rust, two of its five floats had nothing reading them.


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

~~**Clicking a point selects a band and the controls lag it.**~~ **Closed
2026-09-14**, and this note is why it was found. Step 01 made the band
selector emit no engine command, and the caller read "no command" as "nothing
happened" and skipped the republish -- so the lag became absolute: not late,
never. It is a publish-ordering bug, exactly as this paragraph guessed, and
the fix was `EffectParamWrite` rather than a redesign. Nothing was confirmed
under `scripts/mooloop-mcp`; reading this note against the code step 01 had
just landed was enough, which is the argument for writing a symptom down
precisely.

## The fix for the layout, from the strip

Do not renumber the flat array. Take the strip's answer: hand the markup the
descriptor table and the values, and let the display read them by id.
`install_strip_spec` does this once at startup and `tests/strip_face.rs` fails
if a bound is ever spelled in the markup -- which also removes the last two
numbers `dupe-audit unchecked-face` still reports.

**Read this against what step 01 found, 2026-09-14.** Two things in the
paragraph above do not survive contact:

- **There are fifty ids, not forty-three** -- seven bands of six fields and
  two pass filters of four. The step file's 43 assumed five fields a band.
- **`EqResponseDisplay` is shared with the channel strip**, which has been
  drawing its own four bands through it since 2026-09-11 precisely so there is
  not a second answer to what a bell of a given Q looks like. The strip's ids
  are `STRIP_*`; the EQ's are its own. "Let the display read them by id" has
  no single id space to mean, so it cannot be done literally without either
  giving the display two id vocabularies or un-sharing it -- and un-sharing it
  reintroduces the thing sharing it fixed.

A shape that survives both: the **pass filters get their own property**
(`pass-data`, four floats each) rather than being appended after the bands
inside `band-data`. That removes the implicit split the fault above is really
about -- one array, two strides, produced in one file and consumed in another
-- without touching the band half the strip relies on, and the strip simply
sends nothing for it. The band half's five-floats-a-band stays, and what it
wants is not an id table but a *test*, of the `slint_face_agreement.rs` shape:
the publisher and the reader agreeing on stride and field order.

## The shelf curve has an anchor now, and a fidelity question

Step 03 gave a shelf a real slope on 2026-09-14, so there is something for the
plot to follow. Two things to know before drawing it:

- **Today's fixed `1.6` is not arbitrary.** In the display's
  `1 / (1 + pow(f / fc, k))` form, `k = 2S` reproduces the present curve at
  `S = 0.8` -- just above the 0.707 both shelves rest at. So following the
  knob is a one-token change with a defensible anchor rather than a new
  approximation.
- **"Matches what the DSP runs" cannot be met by that form**, and the
  acceptance line should say which standard it means. The DSP runs an RBJ
  biquad; the plot runs a rational approximation, for both the bell and the
  shelf, and always has. `strip_face.rs` holds the strip's plot to its
  *descriptors*, not to its coefficients. Either this step evaluates the real
  magnitude response (which is a per-sample `atan2`-free but non-trivial
  complex magnitude, times 140 samples, in Slint) or it says plainly that the
  plot is a shape and the agreement being tested is of bounds and direction.
  That is a decision, not an implementation detail.

## Acceptance

- The high-pass and low-pass appear on the curve, at their slope, and their
  points are grabbable.
- A shelf's drawn curve changes when its Q changes, and agrees with what the
  DSP runs to a standard this step states -- see the fidelity question above;
  `strip_face.rs` holds the strip's plot to its descriptors rather than to its
  coefficients, so "the same standard" needs saying out loud before it can be
  met.
- ~~Clicking any point selects that band and the controls show *that band's*
  values before a drag can write them.~~ Done 2026-09-14.
- No range or curve constant is spelled in `eq-device.slint`.
