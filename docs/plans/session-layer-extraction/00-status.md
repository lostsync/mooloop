# Session layer extraction — plan status

All six steps done, 2026-09-03, with two recorded departures (steps 04 and
05 below). Written 2026-09-02, out of
`docs/ARCHITECTURE_REVIEW.md`.

`crates/mooloop-session` is 23 modules and 87 tests. `crates/mooloop-ui/src/lib.rs`
is down from 14,157 lines to 9,797, and `UiState::new` from 8,008 to 6,281.
The line counts quoted below are the ones the plan was written against and are
left as written.

## The decision

**A new `mooloop-session` crate owns the live application model — session state,
edit logic, undo, and engine command emission — and knows nothing about any UI
toolkit. `mooloop-ui` keeps the window, the models, the callbacks, and the
projection into them, and nothing else.**

This is a refactor. No behaviour changes, no new features, and no new
capability at the end of it. What it produces is a seam.

## Why do it, independently of egui

The egui question is what surfaced this, but it is not what justifies it. Three
things justify it on their own terms:

- **`crates/mooloop-ui/src/lib.rs` is 13,411 lines in one file.** It is the
  largest file in the repository by a factor of two and a half, and it is the
  file every UI task has to open.
- **`UiState::new` runs from line 4370 to line 12078.** A 7,700-line constructor
  that registers 187 callbacks inline. Edit logic lives inside closure bodies,
  interleaved with `window.set_*` calls, where it cannot be called from anywhere
  else or tested by anything.
- **None of the edit logic is under test**, because reaching it requires a
  `MainWindow`. The engine has `gain_structure_tests.rs`; the sequencer has
  scheduling and drift tests; the session layer — which owns undo, structural
  retargeting, note selection, and every project mutation — has none.

There is a fourth, found while measuring the egui question. `mooloop-ui`'s build
is dominated by the single 39 MB module `slint_build` generates, but `lib.rs`
being one 13,411-line file is a second codegen unit that cannot be subdivided
either. Moving it into its own crate lets it rebuild independently of the
generated module and of every `.slint` edit — a compile-time win that arrives
with the extraction and does not depend on any toolkit decision. See
`docs/plans/egui-view-layer/00-status.md` for the measurements.

That third point is the real one. `mooloop_core::structure` exists because
positional addressing was a live bug: routes and automation lanes named their
destination by slot and their channel by index, so any structural edit silently
re-aimed them (`docs/FOCUS.md`). The code that *performs* those edits still sits
in closures nothing can call.

If egui never happens, this is still the right shape. If egui does happen, this
is what turns it from "rewrite the application" into "write a view layer."

## What makes this tractable

The seam is much cleaner than the file size suggests. Four facts, all verified:

1. **`ChannelState` (`lib.rs:586`) is already entirely toolkit-free.** It holds
   names, params, samples, notes, automation lanes, effects, and a `ModRack`.
   Nothing Slint touches it.
2. **`UiState`'s fields are roughly ninety percent plain Rust.** About a dozen
   `Rc<VecModel<...>>` fields are mixed in among plain `Vec`s, `Cell`s,
   `HashSet`s and `PathBuf`s. The models are separable rather than pervasive.
3. **Its 58 methods divide on an obvious line.** `sync_*`, `refresh_*` and
   `show_*` project into Slint. `automation_lane`, `effect_chain`,
   `retarget_effect_slots`, `select_note`, `prune_note_selection`,
   `placement_covering`, `song_length_ticks`, `allowed_destinations`,
   `destination_depths`, `modulation_depth_for` and their neighbours are model
   logic that never needed a toolkit.
4. **Most free functions below line 12289 are already portable.** Sample
   loading, waveform peaks, browser scanning, zenity dialogs, note-name
   formatting and sample inspection have no Slint in them. They move without
   being rewritten.

The genuinely coupled part is smaller than it looks: the `*_from_int` /
`*_to_int` converters around `lib.rs:1810-1970` exist because Slint has no Rust
enums, and they belong to whichever view layer is current.

## Scope boundary

**In scope:** creating `mooloop-session`; moving state, edit logic, undo, and
document handling into it; reducing `mooloop-ui` to window, models, callbacks
and projection; the tests that become possible.

**Out of scope, deliberately:**

- **Changing any behaviour.** A step that fixes a bug it uncovers should record
  the bug and fix it separately. Refactors that also change behaviour cannot be
  verified by "it does the same thing."
- **Redesigning the undo model.** Snapshots stay. `docs/ARCHITECTURE_REVIEW.md`
  explains why the reference's command-log argument does not bite at this scale.
- **Touching the engine, the DSP crates, or the project format.** They are on
  the correct side of the boundary already.
- **Anything egui.** That is `docs/plans/egui-view-layer/`, and it must not
  start until this plan is finished — the whole point is that it inherits a
  session layer rather than reproducing one.

## The steps

| Step | What it does | Risk |
| --- | --- | --- |
| `01` | Create the crate; move the toolkit-free free functions and `history.rs` | Very low — pure moves — **done** |
| `02` | Move the plain data types | Low — **done** |
| `03` | Split `UiState` into `Session` plus view-side models | Medium — **done** |
| `04` | Break up `UiState::new`; hoist closure bodies onto `Session` | High — the bulk of the work — **done, one departure** |
| `05` | Move engine command emission behind a session-owned interface | Medium — **done, one departure** |
| `06` | Test the session layer | None; this is the payoff — **done** |

Steps 01 and 02 are mechanical and should land quickly. Step 04 is most of the
plan and is divided by UI area inside its own document, because a single commit
that moves 7,700 lines is not reviewable.

## The rule for every step

**After each step the application builds, runs, and behaves identically, and
`mooloop-session` still does not depend on `slint`.** The second half is the one
that is easy to lose: a single convenience `impl From<X> for SharedString`
smuggled across the boundary undoes the plan quietly. Keep the dependency
absent from `crates/mooloop-session/Cargo.toml` and the compiler enforces the
rest.

## Cost

Best guess: **step 04 is the plan**, and everything else is a week's tail around
it. The line count moving is large but the transformation is repetitive —
closure body becomes named method, closure becomes two lines calling it. It
does not need to be finished in one sitting, and the intermediate states are
all shippable, which is the reason for this ordering.

## Progress

### 01 — done, 2026-09-03

`crates/mooloop-session` has no `slint` in its manifest or its dependency tree,
and carries five modules: `history`, `audio_file`, `sample`, `browser`, and
`dialogs`. Everything moved verbatim; the only edits were visibility, import
paths, and the module each function landed in.

Two departures from the step document, both small:

- **`LoadedSample` came across early.** It is listed in step 02's table, but
  `load_sample_at_path` returns it, so step 01 could not move that function
  without it. It lives in `session::sample` beside its constructor.
- **`is_playable_sample` went to `browser` rather than `sample`.** Its own
  doc comment says it decides what the tree shows; `sample_files_in_directory`
  calls `audio_file::is_supported_extension` directly and never wanted it.

Six tests moved with the code and now run under `cargo test -p
mooloop-session` (11 passing, 1 ignored behind ffmpeg). The two browser-tree
tests stayed in `mooloop-ui` because they assert on `BrowserRow`, which is a
Slint type.

`Cargo.lock` grew by 300 lines that have nothing to do with this change: adding
a workspace member forces a re-resolve, and `i-slint-backend-testing`'s
optional dependencies got recorded that had been absent. No version moved and
nothing new compiles — `cargo tree -i prost` finds no path to any of it.

### 02 — done, 2026-09-03

Every type in the step's table now lives in `mooloop-session`, split across
`channel`, `command`, `document`, `notes`, `project`, `sample`, and `values`.
Thirteen more tests came with them; `cargo test -p mooloop-session` is 24
passing and 1 ignored, and the workspace count is unchanged at 1,019, so
nothing was dropped on the way.

`DocumentResult` moved intact. The step document offered a fallback for the
`SharedString` in its failure arm; there is no longer one to convert, so it
was not needed.

Three departures:

- **`Pane`, `PANE_CYCLE` and `cycle_pane` moved after all.** The step says to
  leave them, but `CommandState` has a `pane: Pane` field, so `CommandState`
  could not move without the enum. `apply_pane` — the half that sets Slint
  properties — stayed behind, which is the split the rest of the plan uses
  anyway.
- **`quarantine_song` gained two parameters.** It reached for
  `settings::quarantine_dir()` and `build_description()`, and `settings.rs`
  still holds a `slint::Color`. It now takes `directory: &Path` and
  `build: &str`; the one caller passes them. This also makes it testable,
  which it was not.
- **`snapshot_channel_clipboard` did not move.** Its signature is
  `(&UiState, &MainWindow, usize)`, so it is blocked on step 03 rather than
  on anything in this step. `LoadTarget` moved even though it is not in the
  table, because `DocumentResult::Loaded` carries it.

### 03 — done, 2026-09-03

`Session` lives in `mooloop-session/src/session.rs` with 35 fields and 30
methods. `UiState` keeps the fourteen `Rc<VecModel<...>>` fields and the 32
projection methods, and holds the session in a field.

Nearly all of it was done by the compiler rather than by hand. The fields moved
first; every `st.channels` then became an error naming its own byte span, and a
script walked cargo's JSON diagnostics inserting `session.` at each one — 869
spans across four passes, with the compiler deciding what moved rather than a
regex over names. The methods went the same way.

Five methods did both halves and were split rather than moved, all the same
way: the computation became a `Session` method returning plain data and the
projection stayed behind calling it.

- `replace_project` — the session installs the document; the view rebuilds the
  step models, the rows, and the window properties, then re-syncs.
- `update_tempo_synced_delay_times` — the retune is the session's, the
  `sync_effects` after it is not.
- `destination_depths` / `destination_offsets` return `Vec<f32>` and the view
  makes the `ModelRc`. `allowed_destinations` likewise with `Vec<bool>`.
- `set_armed_modulation_depth` became `Session::arm_modulation_route`
  returning an `ArmedRoute`. A full matrix is a refusal the user has to be
  told about, and the telling is the view's.
- `begin_modulation_edit` takes the snapshot rather than building one, because
  building one needs the window's tempo.

Three departures:

- **There is no `ViewModels` struct.** `UiState` is that struct — models plus a
  `session` field — because it is what 187 callbacks already hold as
  `Rc<RefCell<UiState>>`. Splitting the ownership as well as the contents would
  have changed every one of those signatures for no gain the plan asks for.
- **`send_modulation` and `send_modulator_slot` stayed.** Both need the window,
  because both end in a projection. Step 05 is where they get an interface that
  does not; moving them now would only mean moving them twice.
- **`descriptor_slots`, `normalized_buses` and `WAVEFORM_BINS` moved** without
  being listed, because the methods that moved use them.

One stale comment was dropped rather than carried: `normalized_buses` had a
first doc line describing `ChannelState`, glued there on `main` long before
this plan. It says nothing true about the function it was attached to.

### 04 — done, 2026-09-03, in eleven commits

Worked by area, as the step document asks, each its own commit and each
leaving the application working:

| Area | Session module | Tests |
| --- | --- | --- |
| Transport, patterns, playlist | `transport` | 5 |
| Step grid | `steps` | 6 |
| Channel rack | `rack` | 5 |
| Piano roll | `roll` | 9 |
| Automation lanes | `automation` | 5 |
| Device rack | `effects` | 6 |
| Mixer and buses | `mixer` | 4 |
| Modulation shelf | `modulation` | 5 |
| Sampler: slices, commit, snapping | `sampler` | 6 |
| Document: export, presets | `document` | — |
| Browser navigation | `browser` | — |

The transformation was the one the step prescribes, every time: the closure
body becomes a named `Session` method taking plain arguments and returning
plain data or ordered engine commands, and the closure becomes an adapter that
reads the window, calls it, and projects the result. Roughly 1,600 lines of
decision-making left the closures.

What came out of it beyond the seam is the repetition it exposed. All six
step-grid callbacks opened with the same twenty lines of cell resolution
written six times with the bounds check subtly restated each time. Three of the
roll's drag gestures each carried their own copy of the group-clamping rule and
their own paragraph explaining it. The four effect trim knobs were
near-identical twenty-line blocks differing only in which field they set. Each
of those is now stated once, next to the test that pins it.

**Gesture tokens survived intact**, which the step document singles out as the
thing a build will not tell you about. `CommandState::gesture` still lives in
the view, `record_project_history` still reads it, and
`tests/piano_drag.rs::a_drag_opens_and_closes_exactly_one_gesture` still
passes. `tests/ordering.rs` now asserts the same property at the document
level.

**The departure: `UiState::new` is 6,281 lines, not "a few hundred".** What is
left in it is 5,228 lines of callback *registration* — 191 blocks of capture
preamble around adapters that contain no decisions — plus the pump and the
setup. Splitting the registration into `wire_*` functions needs the shared
handles gathered into a struct, and that rewrite cannot be done mechanically:
`window` appears 820 times in the region and `state` 252, and most of those are
closure-local rebindings (`let Some(window) = weak.upgrade()`,
`let mut state = st.borrow_mut()`) rather than the captured outer name. A
textual rewrite would silently capture the wrong one in some closure, and no
test in this repository would catch it. Every other move in this plan was
verified by the compiler naming the exact byte span; this one cannot be, so it
is left for someone doing it by hand, or for the view rewrite that would
replace the constructor anyway. The step's substantive criterion — every
extracted edit is a named `Session` method reachable from a menu, a shortcut,
or a test — is met.

### 05 — done, 2026-09-03, in two commits

`PendingEngineMessage`, the six typed senders, `TelemetryAction`,
`AudioAction`, `ChannelAudio` and `publish_channel_audio_to` are
`mooloop-session/src/engine.rs`. `Session::apply_engine_message` drains the
five queued messages that need nothing but the handle, and
`Session::transport_position` resolves the position readout's arithmetic.

**The departure: there is no `TickReport`, and the meter, playhead and
modulator-output polling stayed in the view.** That loop interleaves reading
the engine with per-row change detection, precisely so that it does *not*
write a Slint model when a value has not visibly moved;
`docs/plans/archive/reduce-ui-pump-overhead/` is what tuned it that way and
this step says in as many words that it must not be undone. Returning those
readings as plain data would allocate a vector per tick at 125 Hz to feed
change detection that would then run in the view instead. The step's own
framing is that the engine's side is already correct and this is only about
who calls the poll — and for the meters, the caller it already has is the
right one.

`ProjectEdit` and `Audio` stayed with the view for a smaller reason: both end
in something the user sees — an installed document, a JACK error in the
preferences pane — so both belong with the layer that can show it.

### 06 — done, 2026-09-03

`cargo test -p mooloop-session` is 87 tests in 0.27s. Fifty-two were written
alongside the extraction in steps 04 and 05, next to the behaviour they pin;
`tests/structure.rs` and `tests/ordering.rs` cover the rest of the step's
priority list. All five priorities are covered: structural retargeting,
undo at the document level, document round-trip, command ordering, and note
editing.

## Where this leaves the boundary

`mooloop-session` has no `slint` in its manifest or its dependency tree, and
the compiler has enforced that at every step. The session owns the model, the
edits, the undo unit, and the commands; the view owns the window, the models,
the callbacks, and the projection into them.

Two things are still on the view's side of the line and are worth naming
rather than leaving to be discovered:

- **`settings.rs` holds a `slint::Color`**, so the appearance and preferences
  store cannot move. `quarantine_song` and the preset save path both take the
  paths they need as arguments because of it.
- **The pump is a Slint `Timer`**, and the meter polling described above lives
  inside it.

Neither blocks a view swap; both would be part of one.
