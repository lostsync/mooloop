# 02 — Step drawn as bars, and table (MOO-534)

The Step box's face becomes bars drawn with the mouse across the box, like
Reason's Matrix in curve mode, with the playing step lit. Length, rate, glide
and trigger stay knobs. A **polarity** setting: bipolar (`-1..1`, bars from
the middle) or unipolar (`0..1`, from the bottom), typed `step 8 uni`.

**`table n`**: `index`, `value`, `write` → `out`. n stored values, read at
`index` (`0..1` across n); `write` stores `value` at `index`. Same bar face.
Its values are saved.

## Done when

Bars can be drawn, polarity changes the output range, a table can be written
by wire and read back, and songs without the new fields open unchanged.
