# mooloop

Before editing a repository-tracked file, run `git status --short --branch`.
`main` is read/merge-only: create a task worktree before every edit unless
Adam explicitly directs otherwise.

Read and obey [AGENTS.md](./AGENTS.md) before starting work; it is the shared,
authoritative workflow contract for Claude Code, Codex, and opencode, and it
covers the rest of the git rules, which document to read for which task, and
verification. Nothing below overrides it.

When delegating an isolated Claude Code subtask, use the Agent tool with
`isolation: "worktree"`; that satisfies the one-task-one-worktree rule without
a manual `git worktree add`.
