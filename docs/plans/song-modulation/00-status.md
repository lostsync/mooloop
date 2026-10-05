# song-modulation — status

Linear: project [Song modulation](https://linear.app/mooloop/project/song-modulation-3e9d248073e2).
Release label `0.1.7`.

| Step | Issue | Milestone |
| --- | --- | --- |
| 01 | MOO-510 | One set for the song |
| 02 | MOO-511 | One set for the song |
| 03 | MOO-512 | Reach anything |
| 04 | MOO-513 | Reach anything |
| 05 | MOO-514 | Reach anything |

Each step is blocked by the one before it. Step 02 closes MOO-497, MOO-487,
MOO-373 and MOO-170 on the way.

Planned 2026-10-05. Nothing is built.

## Adam's rulings

**2026-10-05**, in the 0.1.7 planning thread (quoted in full in
`README.md`):

| Question | Answer |
| --- | --- |
| Does a channel keep modulators? | **No.** *"channels wont have modulators"* |
| What does a module listen to for notes? | *"you pick the input from a list of outlets sending compatible events"* |
| Does a copied channel bring its routes? | **Yes.** *"assignments would copy/paste"* |
| How many modulators can a song have? | *"all of them"*: no cap a user meets |
| Where do they live? | *"it will just go in its own pane"* |
| How big is this push? | *"let's start by moving what we have and making it work document-wide. we'll see how that went"* |

## Defaults the plan picked, open to Adam

- **Paste shares modules.** A pasted channel's routes come from the same
  modules as the original's, so one LFO drives both. The alternative is
  copying the modules too.
- **A channel preset brings its modules.** Loading one adds the modules its
  routes use to the song, as new modules.
- **0.1.6 opens a new song without its modulation.** It can't be made to
  refuse (step 01 says why). Recorded under MOO-379.

## Open

- **Step 04:** Adam sees the pane's mock-up before its markup is built.

## Measurements

Step 02 records MOO-170's three `block_cost.rs` cases here, before and
after.
