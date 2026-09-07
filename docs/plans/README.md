# Plans

A plan is a directory of dated, numbered step documents. `00-status.md` records
what has landed and what the doing changed about the plan; the numbered files
are the steps, worked in order. **When every step is done, the whole directory
moves to `archive/`** — that move is what marks a plan finished, so an active
directory should always contain live work.

`docs/FOCUS.md` decides which of these is next. This file only says what state
each one is in.

Last swept 2026-09-06, when `containers/` step 01 landed: a rack device is now
an identity rather than a position, and `SlotRemap` is gone. Before that,
2026-09-05, seven times, the last when `auto-offline-idle-devices/`
finished: `AudioNode` can say it has nothing to do, and a thirty-two channel
project with one channel playing now costs a tenth of what it did per block.
Before that, when `typed-audio-edges/` finished
and took `poly-synth-v2/` and `drum-synth-v2/` into `archive/` with it — three
directories closing on one mechanism, which is why it was worth building once
rather than twice. Earlier the same day, five times: when the plan was
written, when DS-01's step 09 closed, when `FOCUS.md` was rewritten around the
debt those two instruments left, when ML-P8's step 07 listening pass closed,
and when `latency-compensation/` finished and archived.

`docs/ARCHITECTURE_REVIEW.md` is where the two newest plans came from, and it
is worth reading before either.

## Active

| Plan | State |
| --- | --- |
| `buffer-implementation/` | **Stage 1 done, Stage 2 open.** Stage 1's acceptance test 8 (RT allocation hygiene) is still unverified. |
| `mono-synth-v2/` | **Complete and played**, one finding deliberately left open (Acid's cutoff corner). Kept out of the archive only because that finding needs Adam's ear, not because a step is unbuilt. |
| `session-layer-extraction/` | **Done, 2026-09-03.** Lifted `mooloop-session` -- the model, the edits, undo, and engine command emission -- out of `mooloop-ui/src/lib.rs`, which is down from 14,157 lines to 9,797. `cargo test -p mooloop-session` is 87 tests in under a second, and is the first coverage the edit logic has ever had. Two departures are recorded in `00-status.md`: `UiState::new` is still long (callback *registration*, no longer decisions), and the pump's meter polling stayed in the view on purpose. |
| `edit-loop/` | **Steps 01 and 02 landed, 03 closed unstarted, 04 waiting on one measurement.** Six of every ten working hours went on `cargo`. `scripts/antibox` now picks incremental compilation for dev builds and sccache for release builds (64% off `cargo test --workspace`), `AGENTS.md` carries a verification ladder, the mockup tool is behind a Cargo feature, and `scripts/mooloop-run` is one command from edit to running application. Splitting device faces was measured and rejected: 79% of face commits also edit `main.slint`. What is left is `main.slint` itself, which no Slint arrangement reaches -- read `04-decide.md` before `egui-view-layer/`. |
| `egui-view-layer/` | **Written, not decided; argument 4 tested and upheld; `edit-loop/` now points at it.** `edit-loop/04-decide.md` fixed the Rust half of the loop and found the UI half unreachable from inside Slint, which is the argument this plan was waiting for; one post-change `scripts/loop-profile` run closes it. No longer blocked: `session-layer-extraction/` is done, so a view layer would inherit a session rather than reproduce one. Still gated on step 01's spike. `00-status.md` states the case both ways. Compile cost was assumed to be the argument against and measured as an argument for: `build.rs` expands `ui/main.slint` into a single 39 MB Rust module, which is where the four minutes and the 3.4 GB go. What is left to decide is frame time and interaction feel. |
| `containers/` | **Step 01 landed 2026-09-06; 02-06 open.** A container is a device that holds a run of devices. Its `README.md` answers the brief's four source questions and records three places `reference/CONTAINERS.md` was wrong -- the largest being that per-device wet/dry already exists and is already latency compensated, so what is missing is wet/dry across a *run*. Step 01 gave rack devices durable `DeviceId`s and deleted `SlotRemap` outright: reordering, inserting and deleting rack rows now rewrite no address anywhere, and the saved addresses are asserted byte-identical across a reorder. |

## Queued, not started

These have steps but no `00-status.md`, because nothing has landed to record.

| Plan | Why it is queued |
| --- | --- |
| `poly-v1-mono-mode/` | One step. The only thing blocking deletion of `DeviceKind::MonoSynth`, which is what lets `MlM1` take the plain name. The held-note stack it needs already exists. |
| `preset-system/` | **Done: steps 01-04 ran 2026-09-04 and landed on `main` after Adam confirmed the interface.** A preset's unit is a device, with relative addressing. The effect-level preset exists end to end: one rack row, no routes, no absolute addressing, `contains = ["effect_params"]` in the manifest so a later fragment format can supersede it cleanly, `presets/effects/<kind>/` on disk, an undoable load through the session, and the rack row's rail buttons wired. `PresetSummary` names three preset classes. Every effect kind ships a factory bank, seeded like the ML-M1 one. A second pass fixed the load path — an effect preset is a rack edit, not a document load — and put the preset's name in the device header. `00-status.md` records what the run found. A second entry, 2026-09-05, moves the *generator* half onto the device rail beside the effect half and gives the source device a preset label in its header. The browser, the taxonomy surface, and an updatable factory mechanism are unblocked now that DS-01's bank ships. As of 2026-09-05 the browser has a home: Adam wants preset browsing in the sample browser panel, and `FOCUS.md` step 3 carries it. |
| `adopt-shared-biquad-in-eq/` | `effects/eq.rs:30` still declares its own `Biquad` after the shared one was promoted out of it. |
| `extract-mid-level-dsp-blocks/` | The primitives-to-devices ladder has no middle rung on the DSP side, and `device-displays.slint` holds eight visualizers with no shared canvas. |

## Archived

`archive/` holds finished plans with their status audits intact. Each is worth
reading before reopening the area it covers, because several record *why* a
tempting change was rejected:

`auto-offline-idle-devices/` (all three, closed 2026-09-05; read its status
before touching anything a device runs on the clock rather than on its input,
and before assuming a frozen delay line reads the same as a running one) ·
`typed-audio-edges/` (all five, closed 2026-09-05; the first half of
`AUDIO_ARCHITECTURE.md`'s migration step 6, and the plan that closed the two
below. Read it before building parallel sends or a sidechain input: the
compiled order and the refusal table are what they hang off, and its status
records why same-block delivery was forced rather than chosen) ·
`poly-synth-v2/` (all six, closed 2026-09-05; the ML-P8, its own modulation,
its voice pool, its five-page face and its eight-patch bank) ·
`drum-synth-v2/` (all nine, closed 2026-09-05; the DS-01, its six-page face
and its seventeen-patch kit) ·
`latency-compensation/` (all five, closed 2026-09-05; it is
`AUDIO_ARCHITECTURE.md`'s migration step 5, and the reason typed audio edges
could be built against an aligned tree) ·
`effects-feedback/` (all fourteen, closed 2026-09-02) ·
`gain-structure/` (all eight; `docs/GAIN_STRUCTURE.md` is the standing
reference) · `modulator-modules/` · `modulator-capacity/` ·
`share-dsp-primitives/` · `amortize-reverb-partition-cost/` ·
`reduce-ui-pump-overhead/` · `reduce-audio-jack-buffer-size/` ·
`skip-empty-effect-slots/`
