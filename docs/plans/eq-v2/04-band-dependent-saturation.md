# 04 — Band-dependent saturation

The one step here that changes how the device sounds rather than correcting
something. Last, and optional.

## What the measurements found

An inductor EQ's nonlinearity sits *inside* the filter network, so its
distortion appears where a band is boosted and nowhere else.
`REFERENCE_MEASUREMENTS.md` predicted it and the VEQ4 data confirms it:

| setting | worst THD | where |
| --- | --- | --- |
| flat | 0.003% | evenly, across the whole battery |
| LMF +12 | **0.050%** | 630 Hz, the boosted band's own centre |
| LMF +12 | 0.028% | 440 Hz |
| LMF +12 | 0.021% | 880 Hz |
| LMF +12 | 0.007% | 320 Hz |

Seventeen times the flat figure, at the band, falling away either side. A
frequency-response plot cannot show it, which the reference note says is "a
large part of why those EQs are described as musical".

## mooloop already has the structure

`mooloop-dsp/src/preamp.rs` is tilt, shape, untilt -- a filter sandwich -- and
its header says why: a transformer's core flux goes as `V / f`, so it saturates
from the bottom up, which is why iron is obvious on a kick and nearly clean on a
hat. Frequency-dependent saturation out of a memoryless shaper plus filters.

A band-dependent EQ nonlinearity is the same structure with the *band's own
response* as the tilt: shape where the band is boosted, leave the rest. The parts
are `harmonics.rs` for an authorable profile and `preamp.rs` for the sandwich.

## Do not start here

Three reasons, in order.

It is a sound nobody asked for yet. Steps 01 to 03 are corrections -- things the
device claims and does not do. This adds a claim.

It wants the display the preamp wants and does not have.
`LOOSE_ENDS.md` records that the interesting question for the preamp is *where
on the spectrum* it is distorting, that `SpectrumAnalyzer` already produces the
right thing, and that the preamp is unusual in having dry and wet in hand at the
same sample. An EQ doing band-dependent saturation has exactly the same need, and
building the display once for both is cheaper than twice.

And it costs CPU on a device that is currently cheap and on every channel. Price
it with `block_cost.rs` before committing to it, and make it opt-in -- a switch,
or a voicing, not a law.

## Acceptance

- A band boosted hard measures materially more THD at its own centre than
  either side of it, in the same shape the VEQ4 data shows.
- Flat, the device measures as clean as it does today.
- Off by default, and the cost of it being off is not measurable.
