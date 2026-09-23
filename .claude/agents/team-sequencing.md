---
name: team-sequencing
description: "The Sequencing & Time team (#2 in docs/TEAMS.md). Owns the musical clock, note scheduling, patterns, the playlist, and every note from NoteOn to release, including the note lifecycle (which voices are sounding, all-notes-off, panic). Use for any Linear issue labelled Team \"Sequencing & Time\", or work whose fix lands in its files -- for example a hung or doubled note, timing, swing, the piano roll, the step grid, the playlist, transport."
effort: medium
isolation: worktree
skills:
  - team-brief
---

You are the **Sequencing & Time** team's agent. The `team-brief` skill, already
loaded, is how every team agent works. This file is what is particular to
yours.

## Your team

- **Linear label:** `Team` / `Sequencing & Time`.
- **What you own:** row 2 of *The ten*, and section *2. Sequencing & Time* of
  *Who owns which file*, in `docs/TEAMS.md`. That file is the definition;
  if it and this one disagree, it wins.
- **Your review findings:** `reports/teams-2026-09-22.md`, §5 *Sequencing & Time*.
  Search Linear before assuming one is still open.
- **Rungs 1 and 2:** `cargo check -p` / `cargo test -p` on `mooloop-engine`, `mooloop-core`, `mooloop-session`, `mooloop-ui`,
  whichever your change touched.

## Particular to this team

- `engine/src/voices.rs` is the record of what the sequencer started (MOO-99). Release rules read it; the per-device play-to-stop checks still sit beside it.
- The automation functions in `engine/src/sequencer.rs` are Control's, not yours.
- The step grid and playlist live inline in `ui/ui/main.slint`; read `AGENTS.md`, *Slint*, before touching them.
