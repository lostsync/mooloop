# EQ v2

The seven-band EQ works and is the weakest device in the program. This is what
is wrong with it, what the channel strip already solved, and what five measured
plugins say about the part neither of them does yet.

## The argument: our devices should reach automation the way a plugin will

`docs/archive/ARCHITECTURE_REVIEW.md` records that `AudioNode` "is already shaped to take
CLAP one-to-one", and `PRODUCT.md` lists plugin hosting as a non-goal *before*
the instrument model is coherent -- deferred, not refused. A CLAP plugin hands
the host N independent parameters, each with a stable id, each individually
automatable, and no context. There is no way to say "the selected band's
frequency" to a plugin, so a host cannot be built around that idea.

The EQ is built around that idea. Six parameters cover seven bands and two pass
filters, and `get`/`set` resolve each one through `selected_target`:

```rust
EQ_PARAM_FREQUENCY_HZ => Some(match p.selected_target() {
    0..EQ_MAX_BANDS => p.bands[p.selected_target()].frequency_hz,
    EqParams::HIGH_PASS_TARGET => p.high_pass.frequency_hz,
    _ => p.low_pass.frequency_hz,
}),
```

**The model is not closed under its own automation system.** `AutomationLane`
is `{ target: ParamAddr, points }` with no filter on what a lane may address, so
`EQ_PARAM_TARGET` -- the band selector -- is automatable. A lane on it changes,
over time, which band every other EQ lane refers to. Nothing prevents that
today and nothing could make it mean anything.

**And the codebase already does it the other way, four times.** The channel
strip's EQ gives every band its own ids (`strip_band_param(band, field)` =
`STRIP_BAND_BASE + band * STRIDE + field`). DS-01 has 39 discrete parameters
across its pages, ML-P8 has 55, and the modulator modules are per-module. The
effect EQ is the outlier, not the pattern.

`ParamDescriptor { id, name, unit, min, max, curve, default }` is already
close to `clap_param_info`. Converging the EQ onto it removes a special case
rather than inventing a mechanism.

### The part that is bigger than the EQ

Worth writing down here because it will be discovered anyway:
`EffectKind::descriptors()` returns `&'static [ParamDescriptor]` -- a table per
*kind*. A plugin's parameters belong to an *instance*, are discovered at load,
and carry arbitrary `u32` ids rather than a dense run. So hosting CLAP means the
descriptor lookup becomes instance-scoped, and several things keyed on "the
kind's table" follow it: the modulation arrays sized by `descriptor_slots`
(max id + 1, unworkable for arbitrary ids), the faces' `modulation-allowed[N]`
indexing, and `param_id_freeze_tests`, which pins parameters by position in a
static table.

None of that is this plan. Step 01 is a small instance of the same shape, which
is the argument for doing it first.

## What is wrong with the EQ today

Four faults, each confirmed against the source rather than reported from use.

**The pass filters are invisible on the curve, and the data is already there.**
`eq_band_data` in `mooloop-ui/src/lib.rs:1266` sends seven bands at five floats
each and then appends the high-pass and low-pass at **four** floats each.
`EqResponseDisplay.band-value(index, field)` reads `index * 5 + field`. The
producer and the consumer disagree about the layout of a flat float array, so
the display cannot see state it is already being handed, and nothing checks that
the two agree.

**A shelf ignores its Q.** `effects/eq.rs:88` calls
`filter.shelf(frequency_hz, gain_db, low, sr)` -- no Q argument, so the knob
does nothing on a shelf. The strip calls `shelf_slope(frequency_hz, gain_db,
band.q, low, sample_rate)`, where Q *is* the shelf's slope.
`Biquad::shelf_slope` already exists and is already tested.

**The display draws a shelf with a fixed exponent.** `band-shape` uses
`1 / (1 + pow(frequency / center, 1.6))`, so even once the DSP honours Q the
plot would not follow it. The bell path does use Q.

**Clicking a curve point selects a band but the controls keep the old value.**
`point-grabbed` does call `target-changed`, so this is not a missing callback --
it is that the knobs are bound to `slot.pN`, which arrives from Rust on the next
publish, so the face shows the previous band's values for a frame and the drag
that follows writes them to the newly selected band. Worth confirming under the
MCP server before designing the fix; it may be a publish-ordering bug rather
than a design one.

## What the strip already solved

Four borrowings, all of them already written and tested:

- **Per-band ids**, `strip_band_param` / `strip_band_of`.
- **`Biquad::shelf_slope`**, so a band's Q knob is its slope when it is a shelf
  and its Q when it is a bell -- which is what makes the same knob honest in
  both positions.
- **`eq_effective_q(q, gain_db, profile)`**, the proportional-Q law, shared by
  the DSP and every display that plots the curve that is *running*.
- **`install_strip_spec`**: the markup is handed the whole descriptor table once
  at startup, so no control on a strip face declares a range and
  `tests/strip_face.rs` fails if one ever does. That is the stronger form of
  what `slint_face_agreement.rs` does, and it is the answer to the flat-float
  -array problem above.

## What the measurements say

Five EQs were driven offline and are committed under
`spikes/preamp-measure/out/eq/`: Massive Passive, Puigtec EQP1A, SlickEQ, SSL
Native, and the VEQ4 (a Neve 1081 inductor EQ). 73 curve points per setting, and
the harmonic battery at several band settings.

Two things in that data are not textbook, and both are things this EQ could have.

**Bandwidth moves with gain.** The VEQ4's LMF bell measures Q ~= 0.28 at +4 dB
and Q ~= 0.61 at +8 dB -- it narrows as it is pushed. That is the law
`eq_effective_q` already implements for the strip and the effect EQ does not
apply.

**Distortion is band-dependent.** Flat, the VEQ4 is ~0.003% THD evenly across
the spectrum. With the LMF boosted it is **0.050% at 630 Hz**, the band's own
centre, 0.028% at 440, 0.021% at 880, and 0.007% by 320 -- roughly seventeen
times the flat figure, at the boosted band and nowhere else.
`REFERENCE_MEASUREMENTS.md` says why: an inductor EQ's nonlinearity sits
*inside* the filter network, and that "is a large part of why those EQs are
described as musical".

mooloop already has the structure this needs. `mooloop-dsp/src/preamp.rs` is
built as tilt, shape, untilt, because a transformer saturates from the bottom up
-- a filter sandwich that makes saturation frequency-dependent. A band-dependent
EQ nonlinearity is the same structure with the band's own response as the tilt.
That is step 04, and it is the one step that is a new sound rather than a
correction.

## Steps

- `01-per-band-parameters.md` -- the parameter model. The only step the CLAP
  argument forces.
- `02-the-curve-tells-the-truth.md` -- the pass filters on the plot, shelves
  drawn at their real slope, and selection by clicking a point.
- `03-a-shelf-has-a-slope.md` -- `shelf_slope` and `eq_effective_q` in the
  effect EQ, from the strip.
- `04-band-dependent-saturation.md` -- the measured character. Optional, and
  last, because it is the only one that changes how the device sounds when
  nobody asked it to.
