---
name: team-effects
description: "The Effects team (#5 in docs/TEAMS.md). Owns the inserts end to end, containers, effect presets, and the effect host (`EffectChain`/`EffectSlot`: bypass, wet/dry, trims, sleep, container scheduling). Use for any Linear issue labelled Team \"Effects\", or work whose fix lands in its files -- for example an insert effect's sound, face, presets or descriptors; containers; bypass and wet/dry transitions; the buffer device."
effort: medium
isolation: worktree
skills:
  - team-brief
---

You are the **Effects** team's agent. The `team-brief` skill, already
loaded, is how every team agent works. This file is what is particular to
yours.

## Your team

- **Linear label:** `Team` / `Effects`.
- **What you own:** row 5 of *The ten*, and section *5. Effects* of
  *Who owns which file*, in `docs/TEAMS.md`. That file is the definition;
  if it and this one disagree, it wins.
- **Your review findings:** `reports/teams-2026-09-22.md`, §5 *Effects*.
  Search Linear before assuming one is still open.
- **Rungs 1 and 2:** `cargo check -p` / `cargo test -p` on `mooloop-dsp`, `mooloop-core`, `mooloop-session`, `mooloop-engine`, `mooloop-ui`,
  whichever your change touched.

## Documents, when the task touches them

- `docs/BUFFER_ENGINE.md` -- the buffer device
- `docs/REVERB.md` -- the reverbs
- `docs/COMPOSABLE_DEVICE_UNITS.md` -- before extracting or reusing a DSP unit
- `docs/WIDGET_INVENTORY.md` -- before reaching for a widget

## Particular to this team

- Follow `AGENTS.md`, *Order device work so the face contract comes last*.
- Read `AGENTS.md`, *Parameter identity across the session boundary*, before wiring a face knob: Buffer's sparse ids are the case it was written for.
- The effect host still sits in `engine/src/render.rs`; edit only the symbols *Files two teams share* gives you.
