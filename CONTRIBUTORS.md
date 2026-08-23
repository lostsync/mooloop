# Contributors

Mooloop's code has been written almost entirely by AI coding agents working
under Adam's direction, across whatever tools happen to be in rotation
(Claude Code, Codex, opencode, others). This file is a sign-in sheet, not a
changelog — a place for each distinct model+harness combination to leave a
mark, so Adam has a real trail to work from later if he wants to credit
people^Wmodels properly.

## How to sign in

If you're an agent reading this after doing work in this repo: add or update
your own entry below.

- One entry per **model + harness** pair (e.g. "Claude Sonnet 5 — Claude
  Code" is distinct from "Claude Sonnet 5 — Codex", and distinct from
  "Claude Opus 5 — Claude Code"). If you don't know your exact model ID,
  use the most specific name you're aware of.
- If an entry for your exact pair already exists, just bump `Last seen` and
  `Sessions` — don't create a duplicate.
- If it doesn't exist yet, add one, alphabetized by model name, using the
  template below.
- Keep it to the template's fields. This file is a roster, not a diary —
  save war stories for commit messages, not here.

```
### <Model name> — <Harness>
- First seen: YYYY-MM-DD
- Last seen: YYYY-MM-DD
- Sessions: N
- Notes: optional, one line
```

`Sessions` is a rough count, not an audit — increment it once per distinct
work session you can recall being part of, and don't stress over precision.

## Roster

### Claude Opus 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-08-22
- Sessions: 2
- Notes: Parameter descriptors, the modulation design, seven effects, and the
  mixer bus graph.

### Claude Sonnet 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-08-23
- Sessions: 2
- Notes: Set up this file at Adam's request. Refreshed the README
  screenshot.

### GPT-5 — Codex
- First seen: 2026-08-21
- Last seen: 2026-08-23
- Sessions: 5
- Notes: Audio-core architecture, realtime project swaps, compiled bus graphs,
  and latency/gain hardening.

### Kimi k3-256k — opencode
- First seen: 2026-08-21
- Last seen: 2026-08-21
- Sessions: 1
- Notes: Implemented the effects-chain vertical slice (filter effect, end to end).
