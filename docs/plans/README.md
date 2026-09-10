# Plans

A plan is a directory of dated, numbered step documents. `00-status.md` records
what has landed and what the doing changed about the plan; the numbered files
are the steps, worked in order. **When every step is done, the whole directory
moves to `archive/`** — that move is what marks a plan finished, so an active
directory should always contain live work.

`docs/FOCUS.md` decides which of these is next. This file only says what state
each one is in.

`pattern-bank-floor/` was added 2026-09-08 and is **not started, and not on
anyone's list**. It fell out of an audio-dropout investigation whose actual
cause was `rtkit` demoting the machine's realtime threads. What it records is
that every project reserves 1.00 GiB of pattern storage before it holds
anything, and that an ordinary edit rebuilds it at 20 ms a time. Whether that
is worth a step is a `FOCUS.md` question; the measurements to judge it by are
committed either way.

Last swept 2026-09-09, when `console/` steps 01, 02, 04 and 05 landed and step
05 was verified: the mixer became a list of tracks somebody made rather than a
fixed seventeen, a track routes a copy of itself to another track, and any
summing point can glue what feeds it. The plan's own `README.md` was rewritten
mid-plan, which is the thing worth carrying: its first pair of decisions
described a derived *view of strips* where grouping took a channel's strip
away, and a console does not do that -- the mixer is tracks, all of them,
always, with bus and send as roles rather than species. Before that,
2026-09-08, when `pane-layout/` steps 01 and 02 landed: the work
area is five views in three slots, the top pane splits, the dock stopped
carrying two toolbars where one would do, and `Ctrl+1`..`Ctrl+5` started
meaning "reveal this view wherever it lives" without their registry entries
changing at all. Earlier the same day, when that plan was written: Adam asked for
the top pane to split, and the four requirements he gave turned out to be one
requirement -- *this view, in that place, at that size* -- asked about four
surfaces, so the plan answers it once. Earlier the same day, when
`ui-consistency-pass/` finished: Adam's standing
list turned into an audit, and the audit found controls that were saying
things the engine was not doing -- a clip light nothing could clear, a sampler
envelope reading 2.5x what it played, a delay whose `1/2` was an eighth note,
and eighteen envelope stages laid across a linear range their own descriptors
call exponential. Before that, 2026-09-07, when `interface-iteration/` step 02
landed: a rack
device can be copied, cut, pasted and duplicated, and the selection that makes
that reachable from the keyboard is an identity rather than a slot. Earlier
the same day, step 01: the browser sidebar browses presets, and an effect
preset loaded from it adds a device rather than replacing one. Earlier the same day, when that plan was written
and the root focus scope was fixed -- it had been standing beside the
interface rather than around it, which is why a shortcut needed a click on the
background first.
Earlier the same day, when `containers/` steps 02-05 landed: a container is a
device that holds a run of devices, blends it, and saves as one preset. Before
that, 2026-09-06, when its step 01 landed: a rack device is now an identity
rather than a position, and `SlotRemap` is gone. Before that,
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
| `pane-layout/` | **Done 2026-09-08, including the two pieces first left out** -- the tab's right-click menu and the arrangement persisted into settings. Ready for `archive/`; read `00-status.md` first, which records four Slint constraints and one wrong claim worth not repeating. Adam's split-view request, answered as a set of views and three slots rather than a control per requirement. The work area computes each slot's rectangle instead of nesting layouts, so a view needs one instance wherever it is drawn, and the dock's two stacked toolbars are the one row per view that made possible. The top pane splits, any pane zooms to the window, a tab drags between panes, and `editor-page` is gone -- a page index of the lower dock cannot name a view that has moved out of it. Read its `README.md` before touching the work area, and `00-status.md` for the four Slint constraints step 01 found -- one of which, a nested model index that type-checks and does not evaluate, cost a wrong render. |
| `buffer-implementation/` | **Stage 1 done, Stage 2 open.** Stage 1's acceptance test 8 (RT allocation hygiene) is still unverified. |
| `mono-synth-v2/` | **Complete and played**, one finding deliberately left open (Acid's cutoff corner). Kept out of the archive only because that finding needs Adam's ear, not because a step is unbuilt. |
| `session-layer-extraction/` | **Done, 2026-09-03.** Lifted `mooloop-session` -- the model, the edits, undo, and engine command emission -- out of `mooloop-ui/src/lib.rs`, which is down from 14,157 lines to 9,797. `cargo test -p mooloop-session` is 87 tests in under a second, and is the first coverage the edit logic has ever had. Two departures are recorded in `00-status.md`: `UiState::new` is still long (callback *registration*, no longer decisions), and the pump's meter polling stayed in the view on purpose. |
| `edit-loop/` | **Steps 01 and 02 landed, 03 closed unstarted, 04 waiting on one measurement.** Six of every ten working hours went on `cargo`. `scripts/antibox` now picks incremental compilation for dev builds and sccache for release builds (64% off `cargo test --workspace`), `AGENTS.md` carries a verification ladder, the mockup tool is behind a Cargo feature, and `scripts/mooloop-run` is one command from edit to running application. Splitting device faces was measured and rejected: 79% of face commits also edit `main.slint`. What is left is `main.slint` itself, which no Slint arrangement reaches -- read `04-decide.md` before `egui-view-layer/`. |
| `egui-view-layer/` | **Written, not decided; argument 4 tested and upheld; `edit-loop/` now points at it.** `edit-loop/04-decide.md` fixed the Rust half of the loop and found the UI half unreachable from inside Slint, which is the argument this plan was waiting for; one post-change `scripts/loop-profile` run closes it. No longer blocked: `session-layer-extraction/` is done, so a view layer would inherit a session rather than reproduce one. Still gated on step 01's spike. `00-status.md` states the case both ways. Compile cost was assumed to be the argument against and measured as an argument for: `build.rs` expands `ui/main.slint` into a single 39 MB Rust module, which is where the four minutes and the 3.4 GB go. What is left to decide is frame time and interaction feel. |
| `interface-iteration/` | **Steps 01-02 landed 2026-09-07; 03-04 open.** What `FOCUS.md` step 3 became when Adam decided against building the 1.0 mockup as one push. The browser sidebar has a PRESETS tab. Its `00-status.md` records two places the plan was wrong: there is no "selected rack row" for an effect preset to land on, so the browser *appends* a device instead -- which is the better gesture and is why an effect preset is always loadable -- and category is not worth a tree level, because sixty-six of ninety-nine shipped presets are categorised "Factory". Step 02 built the selected-device concept step 01 found missing, as a `DeviceId` rather than a slot -- the first thing in the tree to spend `containers/` step 01. |
| `ui-consistency-pass/` | **All six steps landed 2026-09-08; ready to archive once Adam has played it.** Not a feature plan: it is the audit Adam's standing list asked for, and every finding is a control that disagreed with the table it is drawn against. Its `README.md` records the method so the sweep can be re-run. Three things came out of it that outlive the fixes: `slint_face_agreement.rs` now holds every envelope stage to its descriptor (the drum synth had been the only device with such a test), `StatusHint` gives any control at any depth a line in the status bar without a property threaded through its device face, and the toolbar rules in `UI_DESIGN.md` -- a switcher leads the toolbar it decides, a pane's setting lives in that pane's header once, and a button per member of a growing set does not survive the set growing. |
| `containers/` | **Steps 01-05 landed 2026-09-06/07; 06 is Adam's call and builds nothing.** A container is a device that holds a run of devices: it wraps, nests four deep, blends its whole run against the signal that went in, bypasses as a unit, and saves as one preset. Its `README.md` answers the brief's four source questions and records three places `reference/CONTAINERS.md` was wrong -- the largest being that per-device wet/dry already existed, so the gap was wet/dry across a *run*. Step 01 gave rack devices durable `DeviceId`s and deleted `SlotRemap` outright. **One thing is blocked rather than done:** a container preset cannot carry the modulation that drives it, because a route's source lives in the channel's rack, and fixing that means deciding whether a modulator can live in a container -- which the brief reserves. 06 prices layers and defers them. |

## Queued, not started

These have steps but no `00-status.md`, because nothing has landed to record.

| Plan | Why it is queued |
| --- | --- |
| `poly-v1-mono-mode/` | One step. The only thing blocking deletion of `DeviceKind::MonoSynth`, which is what lets `MlM1` take the plain name. The held-note stack it needs already exists. |
| `preset-system/` | **Done: steps 01-04 ran 2026-09-04 and landed on `main` after Adam confirmed the interface.** A preset's unit is a device, with relative addressing. The effect-level preset exists end to end: one rack row, no routes, no absolute addressing, `contains = ["effect_params"]` in the manifest so a later fragment format can supersede it cleanly, `presets/effects/<kind>/` on disk, an undoable load through the session, and the rack row's rail buttons wired. `PresetSummary` names three preset classes. Every effect kind ships a factory bank, seeded like the ML-M1 one. A second pass fixed the load path — an effect preset is a rack edit, not a document load — and put the preset's name in the device header. `00-status.md` records what the run found. A second entry, 2026-09-05, moves the *generator* half onto the device rail beside the effect half and gives the source device a preset label in its header. The browser, the taxonomy surface, and an updatable factory mechanism are unblocked now that DS-01's bank ships. As of 2026-09-05 the browser has a home: Adam wants preset browsing in the sample browser panel, and `FOCUS.md` step 3 carries it. |
| `adopt-shared-biquad-in-eq/` | `effects/eq.rs:30` still declares its own `Biquad` after the shared one was promoted out of it. |
| `console/` | **Written 2026-09-09, from Adam's morning list; steps 01, 02, 04 and 05 all landed the same day, leaving 03 (EQ + comp) and 06 (preamp) open.** Reorder, group, console mixer and sends turn out to be one design with a character layer on top. Its `README.md` carries the decisions that make it incremental, and **it was rewritten mid-plan because the first pair of them were a spreadsheet's idea of a console**: the mixer is *tracks*, all of them, always -- every channel has one permanently and keeps it when grouped -- with bus and send as roles a track is put in by what routes into it, rather than a derived view of strips in which grouping takes a channel's strip away. `EffectTarget` stays while the track list becomes dynamic, rather than doing `MIXER_PLAN.md`'s full `SignalSlotId` rewrite in the first move. `docs/TERMINOLOGY.md` is the vocabulary and is worth reading before the plan. The retired first draft -- including the policy *a mixer strip is created by a musical act, not an administrative one*, which answered a problem that does not arise once every channel simply has a track -- is kept at the bottom of that `README.md`. Steps 01-03 (channel reorder, Airwindows-style console summing, a channel-strip device) each end in something audible and need none of the routing work; 04-05 are the structural block. Note `adopt-shared-biquad-in-eq/` above is absorbed by step 03, and step 04 closes the bus-rename entry in `LOOSE_ENDS.md`. Step 01 (a channel can be dragged to another row) found that the *session's* six channel-keyed things -- the selected device, the open lane, the preset labels -- were never rescoped by any channel edit, so an insert or a delete has been mis-keying them all along; `00-status.md` records that and the one place the plan's premise was wrong about the engine. Step 02 (console summing) is in and off by default: every summing point decodes, so two channels glue with no bus created and nothing placed in a chain, and nesting needed no special case. Two of the plan's numbers were wrong and are corrected there -- the ceiling is +3.92 dBFS rather than 0, and normalizing the curve to move it was built and rejected because its round trip is worst exactly where music is loudest. Step 05 (sends) is in, and its own step doc was the thing that turned out to be wrong: it named `CompiledBusGraph::destinations` as the assumption a send breaks, when a track really does have one output and what breaks is one *edge* per node -- so the step scheduled last for forcing a rewrite of the realtime schedule did not touch it. `05-sends.md` is the rewrite and keeps the original at the bottom. Adam retired returns as a concept and the four-bar ceiling `THE-STRIP.md` had drawn; the drill-down tap points and channel-authored sends are stage 2, listed in `00-status.md`. A verification pass over step 05 the same day found the engine half complete against its own acceptance list and three gaps outside it: one component behind four controls still tooltipped every destination row `"Send to"`, two acceptance cases were true but untested, and four documents still said sends were absent -- `AUDIO_ARCHITECTURE.md`'s migration step 6 among them, which had parallel sends written as the thing its typed-edge model would grow into when they turn out not to extend it at all. |
| `extract-mid-level-dsp-blocks/` | The primitives-to-devices ladder has no middle rung on the DSP side, and `device-displays.slint` holds eight visualizers with no shared canvas. |
| `theming/` | Written 2026-09-09 from Adam's question about skins rather than color schemes. `Theme` in `ui/theme.slint` already is the stylesheet -- Slint has no cascade and does not need one -- and the survey in its `README.md` says which axes it is missing: color and radius and motion are tokenized, type and stroke are not at all (313 literal `font-size`, 81 literal `border-width`), and metrics are half-started in `toolbar.slint`'s `ToolbarMetrics`. Queued because it costs more with every device face added, not because it is urgent. **The reason to build it is accessibility rather than the homage**: the working type size is 7-11px and is not adjustable, and nothing checks that a chosen palette is readable. Step 02 (a relief primitive, because a Slint `Rectangle` has one border colour and a bevel needs four) is the only design problem in it; 01 and 03 are a token sweep and a file format, and are worth having on their own. |

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
