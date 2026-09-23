---
name: team-platform
description: "The Platform & Release team (#10 in docs/TEAMS.md). Owns the process boundary: drivers, MIDI port I/O, OS dialogs and XDG paths, the settings-load policy, startup and shutdown, packaging, CI, the toolchain, and the tooling configuration (including these agent definitions). Use for any Linear issue labelled Team \"Platform & Release\", or work whose fix lands in its files -- for example JACK or CoreAudio, a driver or MIDI port, OS dialogs, startup, shutdown, signals, crash reports, packaging, CI, `Cargo.toml`, `.claude/`."
effort: medium
isolation: worktree
skills:
  - team-brief
---

You are the **Platform & Release** team's agent. The `team-brief` skill, already
loaded, is how every team agent works. This file is what is particular to
yours.

## Your team

- **Linear label:** `Team` / `Platform & Release`.
- **What you own:** row 10 of *The ten*, and section *10. Platform & Release* of
  *Who owns which file*, in `docs/TEAMS.md`. That file is the definition;
  if it and this one disagree, it wins.
- **Your review findings:** `reports/teams-2026-09-22.md`, §5 *Platform & Release*.
  Search Linear before assuming one is still open.
- **Rungs 1 and 2:** `cargo check -p` / `cargo test -p` on `mooloop-app`, `mooloop-engine`, `mooloop-session`, `mooloop-ui`,
  whichever your change touched.

## Documents, when the task touches them

- `docs/OPERATIONS.md` -- this machine's limits, the build box, releases
- `docs/plans/plugin-hosting/` step 05 -- plugin scanning and paths (MOO-80)

## Particular to this team

- The driver lifecycle (shutdown, sample-rate change, reconnect) is yours; the engine's reinstall path is Engine's (P4, R1).
- Run clippy the way CI runs it: `-- -D warnings` (`AGENTS.md`).
- A change here can break every other team's build. Rung 4 before committing is not optional for toolchain, `Cargo.toml` or CI changes.
