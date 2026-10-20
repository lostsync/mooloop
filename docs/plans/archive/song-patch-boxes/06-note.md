# 06 — note: control to notes (MOO-538)

**`note`**: `pitch`, `velocity`, `trigger` → notes. Plays `pitch` on each
trigger and releases it on the next or when the trigger falls. Pitch reads as
`held` sends it (`0..1` = note/127). Its control inlets are read a block
late, as `transpose`'s are.

## Done when

unpack 4 → four `held` → `select 4` (indexed by a `counter 4` on a Beat tag)
→ note → notes out plays an arp built from parts.
