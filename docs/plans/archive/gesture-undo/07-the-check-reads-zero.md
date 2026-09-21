# 07 · The check reads zero

The step that makes the plan finishable, and the one most likely to be
skipped. `AGENTS.md`'s standing complaint about gap lists applies to this
plan's own paperwork: *a gaps list is only worth reading if closing a gap
includes striking it.*

## What to do

**Run the check.** `scripts/dupe-audit unrecorded-edit` reports zero, or
reports a short exemption list whose every entry has a reason. If it reports
anything else, the plan is not finished, whatever the steps say.

**Rewrite `CURRENT.md`'s undo paragraph.** It currently says "**Undo is not
universal**" and lists what does not record: device and generator parameters,
step-grid edits, pattern length, add-pattern, playlist placements and the two
renames — plus the sentence about an unrecorded edit being discarded by undo.
That whole passage becomes a description of what undo now covers, and the
destruction sentence goes, because the thing it describes will no longer
happen.

**Strike the `LOOSE_ENDS.md` entries**, do not annotate them:

- "An undo of one edit silently destroys every unrecorded edit made after
  it" — the entry, its list, and its three options.
- The 400 ms timer paragraph and the gated-pair correction under it, both of
  which describe a stand-in that step 02 deleted.
- The gesture-pair decision paragraph, which becomes this plan's history.

Keep exactly what is still true. The caret-eats-spacebar entry survives; so
does anything step 06 exempted.

**Close MOO-50 and its steps**, and move `docs/plans/gesture-undo/` to
`docs/plans/archive/`. The move is what marks a plan finished
(`docs/plans/README.md`).

**Write the journal entry.** The interesting thing to record is not that undo
now works. It is that the feared cost — a contract change across every device
face — was not what it cost, because a global inverted the direction and the
faces never had to know. That is a reusable lesson about this codebase's
markup boundary, and it belongs beside the ones already in `JOURNAL.md`.

## Done when

- [ ] `scripts/dupe-audit unrecorded-edit` is clean.
- [ ] `CURRENT.md` describes undo as it then is, with no stale gap list.
- [ ] The closed `LOOSE_ENDS.md` entries are gone rather than annotated.
- [ ] Rung 4 green, the plan directory is in `archive/`, and MOO-50 is Done
      with the thing it asked for actually built.
