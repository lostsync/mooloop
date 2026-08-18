## Taste / design context

`reference/ADAM.md` is a taste-and-aesthetic brief about Adam (sound,
mixing, UI, and workflow priors) — not a spec, and it's long, so don't load
it wholesale into every task. Read it when a decision has more than one
reasonable implementation and the "right" one depends on his taste: UI/visual
design, sound-design/DSP defaults, sequencer/groove/microtiming behavior, or
overall product feel. Skip it for mechanical work (build fixes, refactors,
plumbing) where taste doesn't come into play.

# Working agreement for AI coding agents (Claude Code, Codex, opencode)

Adam works alone and is not disciplined about git by default — he leans on
Ctrl+Z. That's fine for him, but it's not fine for us. When multiple agents
touch this repo, sloppy git hygiene causes silent data loss and merge
disasters. These rules exist to make git the safety net, not a landmine.
Every agent working in this repo follows this file, no exceptions.

## The core rule: never switch gears with a dirty tree

Before starting *any* new task, run `git status`. If the working tree is
dirty and the changes are not part of the task you're about to start:

- Do NOT just start editing on top of it.
- Do NOT stash it and forget about it.
- Either finish and commit what's there, or stop and ask Adam what he wants
  done with it (commit / discard / put in its own branch).

Leaving uncommitted work behind while moving on to something else is the
exact failure mode this file exists to prevent.

## One task = one branch = one worktree

Never do a task's work directly on `main`, and never do two unrelated
things on the same branch. For every non-trivial task (bug fix, feature,
refactor):

1. Create a worktree off `main`, in a sibling directory, with a new branch:

   ```
   git worktree add ../mooloop-worktrees/<branch-name> -b <type>/<slug> main
   ```

   Branch prefixes: `feat/`, `fix/`, `refactor/`, `chore/`, `spike/`.
   Example: `git worktree add ../mooloop-worktrees/fix-sampler-clip -b fix/sampler-clip main`

2. Do all editing, building, and testing inside that worktree's directory.
   Never edit files in one worktree while `cd`'d into another.

3. This is what makes parallelism safe: Claude, Codex, and opencode can each
   own a different worktree/branch at the same time without stepping on each
   other's uncommitted state. If you're about to start work and another
   agent's worktree already exists for a related branch, say so instead of
   silently working around it.

Claude Code specifically: prefer the Agent tool's `isolation: "worktree"`
option for delegated sub-tasks instead of hand-rolling `git worktree`
commands, when spawning a subagent for isolated work.

## Commit discipline

- Commit early and often within a branch — small, atomic, buildable
  commits, not one giant diff at the end.
- Write commit messages in the imperative mood that explain *why*, not just
  *what* (the diff already shows what).
- Never commit secrets, generated artifacts, or `target/` output.
- Never use `--no-verify`, `--amend` on already-shared commits, or rewrite
  history on a branch other agents might be using, without asking first.

## Finishing a task

- Working tree must be clean before you consider a task done. Run
  `git status` as a final check.
- Run tests/build before merging.
- Merge the branch back into `main` with a fast-forward (`git merge
  --ff-only <branch>` from a `main` checkout) so history stays a single
  straight line. If `main` has moved on and a fast-forward isn't possible,
  rebase the branch onto `main` first, then fast-forward. Then clean up:

  ```
  git worktree remove ../mooloop-worktrees/<branch-name>
  git branch -d <type>/<slug>
  ```

- Don't leave stale worktrees or merged branches lying around. `git
  worktree list` and `git branch --merged main` should stay tidy.

## Never do without explicit confirmation

`push --force`, `reset --hard`, rewriting published history, deleting a
branch/worktree that has unmerged or uncommitted work, or merging into
`main` without the tests passing.

## If Adam hands you a dirty tree right now

Don't lecture him — just apply the rule above: figure out what the
uncommitted changes are, get them into a proper branch/commit (asking if
their intent is unclear), and only then start new work in its own
worktree.
