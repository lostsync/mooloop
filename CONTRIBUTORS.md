# Contributors

Mooloop's code has been written almost entirely by AI coding agents working
under Adam's direction, across whatever tools happen to be in rotation
(Claude Code, Codex, opencode, others). This file is a sign-in sheet, not a
changelog — a place for each distinct model+harness combination to leave a
mark, so Adam has a real trail to work from later if he wants to credit
people^Wmodels properly.

## How to sign in

If you're an agent reading this after doing work in this repo: add your own
row below, once.

- One row per **model + harness** pair. "Claude Sonnet 5 — Claude Code" is
  distinct from "Claude Sonnet 5 — Codex", and from "Claude Opus 5 — Claude
  Code". If you don't know your exact model ID, use the most specific name
  you're aware of.
- If your pair already has a row, you are already signed in. Leave it alone
  and stop. Don't add a second row, and don't rewrite `Known for` to be about
  the session you just finished — it is the pair's standing summary, not a
  changelog entry.
- If your pair has no row, add one, alphabetical by model name then harness.
- **`Known for` is one short phrase, and the cell is the whole budget.**
  Anything longer goes in the commit message, or in
  [docs/JOURNAL.md](docs/JOURNAL.md) if it is worth keeping.

## Roster

Alphabetical by model name, then by harness.

| Model | Harness | First seen | Known for |
| --- | --- | --- | --- |
| Claude Fable 5 | Claude Code | 2026-08-31 | The modulator-grid plan, and its first step |
| Claude Fable 5.1 | Claude Code | 2026-09-02 | Sampler slice and commit, device ordering, the effect preset system |
| Claude Opus 4.8 | Claude Code | 2026-09-22 | The natural-space reverb rewrite (in-loop FDN diffusion) |
| Claude Opus 5 | Claude Code | 2026-08-21 | Descriptors and modulation, the mixer and console arc, most of the documentation |
| Claude Sonnet 5 | Claude Code | 2026-08-21 | Sampler UI, audio preferences, assignable shortcuts, the mockup tool |
| GLM 5.3 Flash (glm-5.3-flash) | opencode | 2026-08-23 | Effect containers, the sample browser, the gain-structure plan |
| GPT-5 | Codex | 2026-08-21 | Audio-core architecture, realtime project swaps, compiled bus graphs |
| GPT-5.6 Terra | Zed | 2026-08-23 | Fixed duplicate loop-wrap event scheduling |
| Kimi k3-256k | Kimi Code CLI | 2026-08-23 | The poly synth source device, end to end |
| Kimi k3-256k | opencode | 2026-08-21 | The effects-chain vertical slice, the filter effect |
| ox-alpha | opencode | 2026-08-23 | Rescued ZoomScrollBar into the piano roll's axes |

## Why this is a table

It used to be a `Notes` field under each entry, and the field did not hold.
The template asked for one line; the examples taught otherwise, because an
agent reads the roster before it reads the template and calibrates to its
neighbours. By 2026-09-14 one pair's notes ran to 206 lines — 70% of the file
— and the growth was close to linear: about 1.8 lines per session, which is
roughly what every pair here writes except GPT-5 on Codex. Nothing was wrong
with any one entry. There was simply no point at which the file got smaller.

So the prose moved out and the roster became a table, because a cell is a
budget a paragraph cannot quietly exceed. The overflow file it moved to,
`docs/AGENT_NOTES.md`, was itself deleted on 2026-09-14 in the documentation
trim: 393 lines of what each pair said it did, which nobody read and which was
not authoritative about the code. If you find yourself wanting more room in
`Known for`, that want is correct, and the commit message is where it goes —
`docs/JOURNAL.md` if it is a story worth keeping.

## Why there is no session count

There were `Last seen` and `Sessions` columns until 2026-09-22, and they were
the only fields that made an existing row need rewriting. Every session
touched the same line, so every pair of concurrent branches collided on it:
four rounds of merges in four days conflicted here and nowhere else, each time
on two rows of a table that no build reads. A count that has to be
hand-incremented is also not a count — it drifts by however many sessions
forgot.

What the columns were for is still here. The roster says who worked on this
and what they are known for; the dates a pair actually worked are in the git
history, which does not need maintaining and does not conflict. So sign in
once and leave your row alone. The sign-in was always the point, not the
tally.
