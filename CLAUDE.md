# mooloop

Git/worktree workflow rules for this repo are in [AGENTS.md](./AGENTS.md) —
read it and follow it exactly. It's shared with Codex and opencode, which
also work in this repo, so it's written tool-agnostic; nothing here
overrides it.

Claude Code specific note: when delegating isolated sub-tasks via the Agent
tool, use `isolation: "worktree"` rather than hand-rolling `git worktree`
commands yourself.
