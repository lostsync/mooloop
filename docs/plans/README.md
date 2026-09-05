# Plans

A plan is a directory of dated, numbered step documents. `00-status.md` records
what has landed and what the doing changed about the plan; the numbered files
are the steps, worked in order. **When every step is done, the whole directory
moves to `archive/`** — that move is what marks a plan finished, so an active
directory should always contain live work.

`docs/FOCUS.md` decides which of these is next. This file only says what state
each one is in.

Last swept 2026-09-03.

`docs/ARCHITECTURE_REVIEW.md` is where the two newest plans came from, and it
is worth reading before either.

## Active

| Plan | State |
| --- | --- |
| `poly-synth-v2/` | **In progress.** ML-P8 steps 02-05 in; 06 mostly in (outlets declared, published and routable; the picker does not offer them and the audio outlets wait on typed edges); 07 after it. `origin/feat/mlp8-mod` is stale: it carried step 04's WIP, which has landed. |
| `drum-synth-v2/` | **Steps 02-08 in; step 09's bank ships, its listening does not.** DS-01 plays, is descriptor-addressed throughout, has a six-page face and a seventeen-patch factory bank. Step 07's published outlets are blocked on the shared device-outlet mechanism ML-P8's step 06 also waits for. `mockups/` holds four rendered concepts: the shipped pages, and the three one-screen layouts they replaced. |
| `buffer-implementation/` | **Stage 1 done, Stage 2 open.** Stage 1's acceptance test 8 (RT allocation hygiene) is still unverified. |
| `mono-synth-v2/` | **Complete and played**, one finding deliberately left open (Acid's cutoff corner). Kept out of the archive only because that finding needs Adam's ear, not because a step is unbuilt. |
| `session-layer-extraction/` | **Done, 2026-09-03.** Lifted `mooloop-session` -- the model, the edits, undo, and engine command emission -- out of `mooloop-ui/src/lib.rs`, which is down from 14,157 lines to 9,797. `cargo test -p mooloop-session` is 87 tests in under a second, and is the first coverage the edit logic has ever had. Two departures are recorded in `00-status.md`: `UiState::new` is still long (callback *registration*, no longer decisions), and the pump's meter polling stayed in the view on purpose. |
| `edit-loop/` | **Steps 01 and 02 landed, 03 closed unstarted, 04 waiting on one measurement.** Six of every ten working hours went on `cargo`. `scripts/antibox` now picks incremental compilation for dev builds and sccache for release builds (64% off `cargo test --workspace`), `AGENTS.md` carries a verification ladder, the mockup tool is behind a Cargo feature, and `scripts/mooloop-run` is one command from edit to running application. Splitting device faces was measured and rejected: 79% of face commits also edit `main.slint`. What is left is `main.slint` itself, which no Slint arrangement reaches -- read `04-decide.md` before `egui-view-layer/`. |
| `egui-view-layer/` | **Written, not decided; argument 4 tested and upheld; `edit-loop/` now points at it.** `edit-loop/04-decide.md` fixed the Rust half of the loop and found the UI half unreachable from inside Slint, which is the argument this plan was waiting for; one post-change `scripts/loop-profile` run closes it. No longer blocked: `session-layer-extraction/` is done, so a view layer would inherit a session rather than reproduce one. Still gated on step 01's spike. `00-status.md` states the case both ways. Compile cost was assumed to be the argument against and measured as an argument for: `build.rs` expands `ui/main.slint` into a single 39 MB Rust module, which is where the four minutes and the 3.4 GB go. What is left to decide is frame time and interaction feel. |

## Queued, not started

These have steps but no `00-status.md`, because nothing has landed to record.

| Plan | Why it is queued |
| --- | --- |
| `poly-v1-mono-mode/` | One step. The only thing blocking deletion of `DeviceKind::MonoSynth`, which is what lets `MlM1` take the plain name. The held-note stack it needs already exists. |
| `preset-system/` | **Done: steps 01-04 ran 2026-09-04 and landed on `main` after Adam confirmed the interface.** A preset's unit is a device, with relative addressing. The effect-level preset exists end to end: one rack row, no routes, no absolute addressing, `contains = ["effect_params"]` in the manifest so a later fragment format can supersede it cleanly, `presets/effects/<kind>/` on disk, an undoable load through the session, and the rack row's rail buttons wired. `PresetSummary` names three preset classes. Every effect kind ships a factory bank, seeded like the ML-M1 one. A second pass fixed the load path — an effect preset is a rack edit, not a document load — and put the preset's name in the device header. `00-status.md` records what the run found. The browser, the taxonomy surface, and an updatable factory mechanism stay queued behind DS-01's second bank. |
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
