# 09 — one lane editor: the piano roll's lane joins

Adam, on whether the playlist's lanes should reuse the roll's: *"not sure
its worth it to try bc there's a lot i'd like to update there eventually…so
maybe this new one is the updated version."* This step makes it so. The
roll's automation lane (`main.slint:7062`) is replaced by the lane editor the
panel uses:
- the same component;
- the same gestures (06);
- the same curves (08);
- the same cascade (05);
- resizable heights (07).

The difference is that it edits the selected channel's **pattern** lanes, on
the pattern's own tick axis.

This is also what keeps `IDEAS.md`'s warning from coming true: two editors
that nearly agree. After this step there is one.

## What changes for the roll

- **Several lanes at once.** The roll shows the selected channel's open
  pattern lanes stacked, as the panel does, instead of one lane behind a
  picker. The 8-lane-per-pattern cap stays: it is the pattern store's
  preallocated shape, not this plan's to change (`CAPACITY_POLICY.md`
  records the pattern bank as its own problem).
- **The lane area resizes** against the roll with a divider, like the panel
  does against the playlist rows (03).
- **The roll's picker** becomes the cascade, restricted to what a pattern
  lane may name. A pattern lane may name its own channel or any track, which
  `lane_allowed` already says.
- **MOO-467**, if it is still open, is fixed by construction: the line test
  from step 03 now covers the roll.

## Done when

- **One component:** the roll's lane and the panel's lanes are the same Slint
  component, fed by one Rust model builder. No duplicated gesture code exists
  in `main.slint`. A `scripts/dupe-audit` lead, if one of its checks reports
  the old lane's literals, is gone.
- **Every gesture test from step 06** passes against a roll lane as well as
  a panel lane.
- **Every existing pattern-lane test** still passes: `tests/piano_drag.rs`
  and `tests/piano_snapshot.rs`, updated only where a gesture deliberately
  changed. Each change is listed in `00-status.md`.
- **`docs/CURRENT.md` and `docs/UI_DESIGN.md`** describe one lane editor.
