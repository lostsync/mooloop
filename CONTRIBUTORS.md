# Contributors

Mooloop's code has been written almost entirely by AI coding agents working
under Adam's direction, across whatever tools happen to be in rotation
(Claude Code, Codex, opencode, others). This file is a sign-in sheet, not a
changelog — a place for each distinct model+harness combination to leave a
mark, so Adam has a real trail to work from later if he wants to credit
people^Wmodels properly.

## How to sign in

If you're an agent reading this after doing work in this repo: add or update
your own row below.

- One row per **model + harness** pair. "Claude Sonnet 5 — Claude Code" is
  distinct from "Claude Sonnet 5 — Codex", and from "Claude Opus 5 — Claude
  Code". If you don't know your exact model ID, use the most specific name
  you're aware of.
- If your pair already has a row, bump `Last seen` and `Sessions` and stop.
  Don't add a second row, and don't rewrite `Known for` to be about the
  session you just finished — it is the pair's standing summary, not a
  changelog entry.
- If your pair has no row, add one, alphabetical by model name then harness.
- **`Known for` is one short phrase, and the cell is the whole budget.**
  Anything longer goes in the commit message, or in
  [docs/AGENT_NOTES.md](docs/AGENT_NOTES.md) if it is worth keeping.
- `Sessions` is a rough count, not an audit. Increment it once per distinct
  work session you can recall being part of, and don't stress over precision.

## Roster

Alphabetical by model name, then by harness.

| Model | Harness | First seen | Last seen | Sessions | Known for |
| --- | --- | --- | --- | --- | --- |
| Claude Fable 5 | Claude Code | 2026-08-31 | 2026-08-31 | 1 | The modulator-grid plan, and its first step |
| Claude Fable 5.1 | Claude Code | 2026-09-02 | 2026-09-04 | 3 | Sampler slice and commit, device ordering, the effect preset system |
| Claude Opus 5 | Claude Code | 2026-08-21 | 2026-09-14 | 124 | Descriptors and modulation, the mixer and console arc, most of the documentation |
| Claude Sonnet 5 | Claude Code | 2026-08-21 | 2026-09-14 | 20 | Sampler UI, audio preferences, assignable shortcuts, the mockup tool |
| GLM 5.3 Flash (glm-5.3-flash) | opencode | 2026-08-23 | 2026-09-01 | 17 | Effect containers, the sample browser, the gain-structure plan |
| GPT-5 | Codex | 2026-08-21 | 2026-09-02 | 74 | Audio-core architecture, realtime project swaps, compiled bus graphs |
| GPT-5.6 Terra | Zed | 2026-08-23 | 2026-08-23 | 1 | Fixed duplicate loop-wrap event scheduling |
| Kimi k3-256k | Kimi Code CLI | 2026-08-23 | 2026-08-23 | 1 | The poly synth source device, end to end |
| Kimi k3-256k | opencode | 2026-08-21 | 2026-08-21 | 1 | The effects-chain vertical slice, the filter effect |
| ox-alpha | opencode | 2026-08-23 | 2026-08-23 | 2 | Rescued ZoomScrollBar into the piano roll's axes |

## Why this is a table

It used to be a `Notes` field under each entry, and the field did not hold.
The template asked for one line; the examples taught otherwise, because an
agent reads the roster before it reads the template and calibrates to its
neighbours. By 2026-09-14 one pair's notes ran to 206 lines — 70% of the file
— and the growth was close to linear: about 1.8 lines per session, which is
roughly what every pair here writes except GPT-5 on Codex. Nothing was wrong
with any one entry. There was simply no point at which the file got smaller.

So the prose moved to [docs/AGENT_NOTES.md](docs/AGENT_NOTES.md), which is
allowed to be long because nobody has to read it to find out who worked here,
and the roster became a table, because a cell is a budget a paragraph cannot
quietly exceed. If you find yourself wanting more room in `Known for`, that
want is correct and `AGENT_NOTES.md` is where it goes.
