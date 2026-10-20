# 03 — switch and gate; gate becomes held (MOO-535)

- **`switch n`**: `in`, `which` → n outlets. Sends `in` to the outlet
  `which` picks (`0..1` across n); the others hold 0. `select` stays n-in-to-1.
- **`gate`**: `in`, `open` → `out`. Passes `in` while `open` is at or above
  0.5, else 0.
- **Today's `gate` is renamed `held`.** It is saved by kind, so a saved song
  opens with it reading `held`.

Control wires only here; step 04 lets both take notes.

## Done when

Both work on control wires, typing `gate` makes the new box, and a song
saved with the old box opens with `held`.
