# song-automation — status

Linear: project [Song automation](https://linear.app/mooloop/project/song-automation-c835d3892a99).

Steps:

| Step | Issue |
| --- | --- |
| 01 | MOO-470 |
| 02 | MOO-471 |
| 03 | MOO-472 |
| 04 | MOO-419 (the issue that started the plan, retitled) |
| 05 | MOO-473 |
| 06 | MOO-474 |
| 07 | MOO-475 |
| 08 | MOO-476 |
| 09 | MOO-477 |

Milestones:
- **A song lane you can hear** (01-03);
- **Tracks, picking and editing** (04-07);
- **Curves and one editor** (08-09).

Owed outside the steps: write modes, MOO-478. It is unscheduled.

Planned 2026-09-30. Nothing is built. It is in for 0.2.0 (`SCOPE.md` §4). It
has no Release label until Adam adds one, and isn't in `FOCUS.md`'s sequence
until he picks it.

## Adam's rulings

**2026-09-30, on MOO-419**, asked where a track's lane should live: the
playlist editor, as song automation, in a new lane editor that is *"maybe …
the updated version"* of the piano roll's. His eight requirements are quoted in
`README.md`, and each step file opens with the one it answers.

**2026-09-30, confirming the reversal of 2026-09-17's "no song-level
automation" (MOO-24):**

| Question | Answer |
| --- | --- |
| Do pattern lanes stay? | **Yes.** *"that's basically 'clip automation'"* |
| Before or after the 0.2.0 freeze? | **Before**, *"almost certainly"* |
| Write a plan? | **Yes**: this directory |

**2026-10-02, on MOO-471**, asked which lane wins when a pattern lane and a
song lane drive one parameter: *"i dont think there should be a winner.
teamwork makes the dream work."* They combine: each lane's distance from the
knob adds (the rule is in step 02).

## Open

- **Step 03 (MOO-472):** Adam sees the panel's mock-up before its markup is
  built.
- **The roll's lane moved first (2026-10-06, MOO-530).** Asked for in the project
  thread, ahead of this plan: the piano roll now stacks every open pattern
  lane, resizes each lane by its bottom edge (Shift for all) and the area by
  its top edge, snaps values to semitones and stepped positions, drags
  finely with Ctrl, and draws value lines. MOO-467 (the short curve) was
  `Path`'s default `contain` fit, fixed there. Step 09 still owes the one
  shared component; heights there are view state, not saved (step 07 is
  where saving them is decided).

## Steps

| Step | State |
| --- | --- |
| 01 — the song lane store | Backlog (MOO-470) |
| 02 — the engine plays song lanes | Backlog (MOO-471) |
| 03 — the automation panel | Backlog (MOO-472) |
| 04 — a track's fader and pan | Backlog (MOO-419) |
| 05 — choosing a parameter | Backlog (MOO-473) |
| 06 — editing points | Backlog (MOO-474) |
| 07 — lane heights | Backlog (MOO-475) |
| 08 — curves between points | Backlog (MOO-476) |
| 09 — one lane editor | Backlog (MOO-477) |
