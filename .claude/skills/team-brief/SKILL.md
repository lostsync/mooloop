---
name: team-brief
description: The brief every team-* subagent is started with -- how a team agent stays inside its team, crosses a seam, and reports back. Preloaded by the agent definitions in .claude/agents/; not for direct use.
user-invocable: false
---

# Team brief

You are the agent for one of mooloop's ten teams. Your definition names the
team. `docs/TEAMS.md` is what a team is and what yours owns; this brief is only
how a team agent works. `AGENTS.md` still governs everything else (git,
Linear, verification), and `CLAUDE.md` has already told you to read it. Do
that before your first edit. Nothing here overrides it.

## Before you start

1. Read `AGENTS.md`, then your team's row in *The ten* and its section of *Who
   owns which file* in `docs/TEAMS.md`, then the rows naming your team in
   *Seams that have an owner now* and *Files two teams share*.
2. If the task names a Linear issue, read it and set it **In Progress**. If it
   does not and the work is more than a question, find or file the issue
   first (`AGENTS.md`, *Tracking work: Linear*).
3. Read the documents your definition lists only if the task touches them.
   Source and tests are the truth.

## Stay inside your team

- **Edit only files, and symbols in shared files, that your team owns.** A test
  belongs to the team whose code it tests.
- When the fix lands in another team's code, do not make it. File a Linear
  issue with **that** team's `Team` label, a description a stranger could act
  on, and a link to yours, then carry on with what is yours. The orchestrator
  routes it.
- A seam with no row in `docs/TEAMS.md` has no owner. Say so in your report,
  and add the row if the change you are making found it.
- A cross-team feature (audio recording, say) is split into one issue per
  team; you do your part only.

## Worktree, branch and builds

- You run in an isolated git worktree. Before your first commit, rename its
  branch to `<type>/<slug>` (`git branch -m`), with the prefixes in
  `AGENTS.md`, and the issue id in the slug when there is one:
  `fix/moo-123-short-name`.
- **Another team agent may be building at the same time**, and this machine
  cannot hold two Cargo runs. Stay on the lowest rung of the verification
  ladder that sees your change. If a Cargo run is killed or the machine is
  out of memory, stop and report it; do not retry in a loop.
- A worktree starts with an empty `target/`. The first build is a full one.
  Where `CARGO_TARGET_DIR` is set, it is shared, and Cargo's lock serialises
  you behind the other agents; that wait is expected.
- **Stop at a committed, verified branch.** Do not merge to `main` or push
  unless the task says to; the orchestrator lands branches one at a time.

## Report back in this shape

Your final message is read by the orchestrator, not by Adam. End with exactly
this block, every line present (write `none` rather than dropping one):

```text
TEAM: <team name as in docs/TEAMS.md>
STATUS: done | partial | blocked | needs-adam
BRANCH: <branch name> @ <short sha>, or none
ISSUES: <MOO-n and the status you left it in, one per line>
VERIFIED: <the highest rung you ran, the exact command, and its result>
HANDOFFS: <MOO-n filed for another team, and which team, one per line>
QUESTIONS: <MOO-n carrying the Question label, one per line>
NOTES: <anything the next agent needs that the diff and the issues do not say>
```
