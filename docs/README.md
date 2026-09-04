# Mooloop documentation

Every document here has one job. If two disagree, the order below decides:
**source and tests** describe what the code does, **`CURRENT.md`** describes
what the application does, **`PRODUCT.md`** decides what it is for, and the
design contracts decide how to build the next thing. Adam's current direct
feedback outranks all of them.

`AGENTS.md` at the repository root is the workflow contract and carries a
table of which document to read for which task. Start there, not here.

## What exists now

| Document | Job |
| --- | --- |
| [CURRENT.md](CURRENT.md) | The implemented surface and its known gaps. The one to update when behaviour changes. |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crates, components, and the data crossing between them, in one diagram. |
| [PROJECT_FORMAT.md](PROJECT_FORMAT.md) | The on-disk song, kit, and channel documents, and what each defaulted field is for. |
| [GAIN_STRUCTURE.md](GAIN_STRUCTURE.md) | Operating level, summing, taper, wet/dry, metering. `gain.rs` is the authority. |
| [REVERB.md](REVERB.md) | The FDN reverb's realtime contract, its parameters, and why it is not a convolver. |
| [ACTIONS.md](ACTIONS.md) | The action registry: how a shortcut, menu row, or future console command is added. |
| [WIDGET_INVENTORY.md](WIDGET_INVENTORY.md) | UI patterns duplicated in `.slint` with no component behind them. Read before writing a new widget. |
| [JOURNAL.md](JOURNAL.md) | The narrative: what was built, what broke, and what it taught. |
| [ARCHITECTURE_REVIEW.md](ARCHITECTURE_REVIEW.md) | The engine graded against an external reference architecture, and where the one real gap is. |

## What it is for

| Document | Job |
| --- | --- |
| [PRODUCT.md](PRODUCT.md) | Scope, pillars, non-goals, and the decisions firm enough to build against. |
| [FOCUS.md](FOCUS.md) | The active working sequence and what must not interrupt it. Rewritten when the sequence is exhausted. |
| [ROADMAP.md](ROADMAP.md) | The whole product ordered by dependency. `FOCUS.md` outranks it on what is next. |
| [VERSIONS.md](VERSIONS.md) | Outcome-based release targets, and which of their milestones are met. |
| [CAPACITY_POLICY.md](CAPACITY_POLICY.md) | Why user-facing collections do not get small caps, and why a ceiling is not a reservation. |
| [ENHANCEMENTS.md](ENHANCEMENTS.md) | Adam's standing wish list, in his words, annotated with what has landed. |
| [IDEAS.md](IDEAS.md) | Loose notes and design conversation. Nothing scheduled. |

## How to build it

| Document | Job |
| --- | --- |
| [AUDIO_ARCHITECTURE.md](AUDIO_ARCHITECTURE.md) | The boundary between editable musical state and audio execution: control plane, graph compiler, executor, time, latency. |
| [MODULATION_PLAN.md](MODULATION_PLAN.md) | The approved parameter and modulation design. Descriptors, `ParamAddr`, base-plus-offset, control rate. |
| [MODULATOR_SYSTEM_SPEC.md](MODULATOR_SYSTEM_SPEC.md) | The implementation spec that expands it: source metadata, destination policy, the shelf, the assign gesture. |
| [BUFFER_ENGINE.md](BUFFER_ENGINE.md) | The retained-audio thesis, what shipped against it, and the product test still outstanding. |
| [MIXER_PLAN.md](MIXER_PLAN.md) | The signal-slot mixer that replaces the fixed master-plus-16-bus bank. Not built. |
| [COMPOSABLE_DEVICE_UNITS.md](COMPOSABLE_DEVICE_UNITS.md) | How a reusable DSP unit presents itself. Mostly a target; its three load-bearing habits are not. |
| [UI_DESIGN.md](UI_DESIGN.md) | The interface composition language, the rack layout contract, and the acceptance checklist. |
| [NODE_MODEL.md](NODE_MODEL.md) | A recorded direction for node-based patching. Explicitly not a plan and not scheduled. |
| [OPERATIONS.md](OPERATIONS.md) | Worktrees, Cargo, the integration suite, releases, and cleaning up. |
| [AGENT_OPERATIONS.md](AGENT_OPERATIONS.md) | The agent-specific half: memory limits, the remote build box, headless rendering, the live app. |

## Directories

- [plans/](plans/) — numbered work orders. `00-status.md` in each says what has
  landed; work the files in order. Completed directories move to
  [plans/archive/](plans/archive/). [plans/README.md](plans/README.md) is the
  one place that says which state every plan is in.
- [reference/](reference/) — outside material worth keeping whole, read but not
  written here. [ANATOMY_OF_A_DAW.md](reference/ANATOMY_OF_A_DAW.md) is what
  `ARCHITECTURE_REVIEW.md` grades against.
- [archive/](archive/) — documents fully consumed by the work they described.
  Kept for the reasoning, not as contracts.
