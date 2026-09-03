# Plans

A plan is a directory of dated, numbered step documents. `00-status.md` records
what has landed and what the doing changed about the plan; the numbered files
are the steps, worked in order. **When every step is done, the whole directory
moves to `archive/`** — that move is what marks a plan finished, so an active
directory should always contain live work.

`docs/FOCUS.md` decides which of these is next. This file only says what state
each one is in.

Last swept 2026-09-02.

`docs/ARCHITECTURE_REVIEW.md` is where the two newest plans came from, and it
is worth reading before either.

## Active

| Plan | State |
| --- | --- |
| `poly-synth-v2/` | **In progress.** ML-P8 steps 02-04 in; 05-07 next. `origin/feat/mlp8-mod` is stale: it carried step 04's WIP, which has landed. |
| `drum-synth-v2/` | **Approved, no code.** DS-01, nine steps, three rendered face concepts in `mockups/`. |
| `buffer-implementation/` | **Stage 1 done, Stage 2 open.** Stage 1's acceptance test 8 (RT allocation hygiene) is still unverified. |
| `mono-synth-v2/` | **Complete and played**, one finding deliberately left open (Acid's cutoff corner). Kept out of the archive only because that finding needs Adam's ear, not because a step is unbuilt. |
| `session-layer-extraction/` | **Written, no code.** Six steps. Lifts a toolkit-free session crate out of `mooloop-ui/src/lib.rs` (13,411 lines, of which `UiState::new` is 7,700). Worth doing whether or not the egui plan ever runs; step 06 is the first test coverage the edit logic has ever had. |
| `egui-view-layer/` | **Written, not decided.** Blocked on `session-layer-extraction/` finishing, and on step 01's spike. `00-status.md` states the case both ways. Compile cost was assumed to be the argument against and measured as an argument for: `build.rs` expands `ui/main.slint` into a single 39 MB Rust module, which is where the four minutes and the 3.4 GB go. What is left to decide is frame time and interaction feel. |

## Queued, not started

These have steps but no `00-status.md`, because nothing has landed to record.

| Plan | Why it is queued |
| --- | --- |
| `poly-v1-mono-mode/` | One step. The only thing blocking deletion of `DeviceKind::MonoSynth`, which is what lets `MlM1` take the plain name. The held-note stack it needs already exists. |
| `preset-system/` | Device-level presets were asked for and never delivered. Waiting on DS-01's factory bank so the design answers to two banks rather than one. |
| `adopt-shared-biquad-in-eq/` | `effects/eq.rs:30` still declares its own `Biquad` after the shared one was promoted out of it. |
| `auto-offline-idle-devices/` | `AudioNode` has no rest/tail contract, so every occupied slot and every channel strip runs every block regardless of whether it is doing anything. |
| `extract-mid-level-dsp-blocks/` | The primitives-to-devices ladder has no middle rung on the DSP side, and `device-displays.slint` holds eight visualizers with no shared canvas. |

## Archived

`archive/` holds finished plans with their status audits intact. Each is worth
reading before reopening the area it covers, because several record *why* a
tempting change was rejected:

`effects-feedback/` (all fourteen, closed 2026-09-02) ·
`gain-structure/` (all eight; `docs/GAIN_STRUCTURE.md` is the standing
reference) · `modulator-modules/` · `modulator-capacity/` ·
`share-dsp-primitives/` · `amortize-reverb-partition-cost/` ·
`reduce-ui-pump-overhead/` · `reduce-audio-jack-buffer-size/` ·
`skip-empty-effect-slots/`
