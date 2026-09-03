# 01 — Capacity is a constant

Step 03 of the modulator-modules plan claimed capacity was "a protocol
edit, not a UI edit", and made the grid's slot names and column count read
from `MAX_MODULATORS_PER_CHANNEL` to prove it. That claim is currently
half true. Raising the constant to sixteen and rendering it shows three
places that still assume a small number.

## What actually caps the number today

- **`ModuleGrid.rows: 2` is a literal.** The grid pane's height, and the
  add-menu overlay drawn over it, are both two rows tall. A ninth module
  has nowhere to appear. This is the hard cap: the constant can move but
  the modules become invisible.
- **The grid pane cannot scroll.** Even with rows derived, the shelf is a
  fixed 240px inside a rack whose height is derived from it. Rows cannot
  grow without either pushing the rack around or clipping.
- **The math module's input jack is a `SelectorBank`.** One segment per
  slot, laid out in a row. At eight it is comfortable, at sixteen it fills
  the strip edge to edge, and past that it overflows.

None of these are protocol. All three are the layout quietly encoding a
number it was told not to encode.

## The work

- Derive the grid's row count from `max-sources` and the column count,
  and put the tiles in a scroll view so a taller grid scrolls inside a
  fixed shelf instead of resizing the rack. The shelf's height stays a
  layout decision; capacity stops being one.
- Replace the math input jack's `SelectorBank` with the same `ComboBox`
  affordance the envelope's gate input already uses, which scales to any
  slot count for free.
- Make that jack list **module names rather than slot numbers**. The
  picker currently says "3"; it should say "STEP 3", and an empty slot
  should say so. This is worth doing on its own merits — a numbered
  picker into a grid of named modules is a puzzle — and it happens to be
  what makes the control scale.
- `MAX_MOD_ROUTES_PER_CHANNEL` stays 16 here and moves in its own step.
  It is a real constraint at eight modules (two routes each) but it is a
  separate number with a separate price, and bundling them would hide
  which one paid for what.

## Explicitly not in this step

Nothing about memory. This step does not change what a capacity costs,
only what it takes to change it. Steps 02 and 03 are where the price
moves.

## Done when

- Setting `MAX_MODULATORS_PER_CHANNEL` to sixteen and rendering shows
  sixteen reachable module cells, with no layout edit anywhere.
- The math input jack names its modules and is unbothered by the count.
- The ring-cost test fails with a number to argue with, which is the only
  thing a capacity change should have to touch.
- `cargo test --workspace` on the build box, plus a software-rendered
  shelf snapshot at eight and at sixteen.
