# Session layer extraction — plan status

Steps 01 to 03 done, 2026-09-03. Written 2026-09-02, out of
`docs/ARCHITECTURE_REVIEW.md`.

`crates/mooloop-session` exists and `crates/mooloop-ui/src/lib.rs` is down from
14,157 lines to 11,920. The line counts quoted below are the ones the plan was
written against and are left as written; `UiState::new` has not been touched.

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
| `04` | Break up `UiState::new`; hoist closure bodies onto `Session` | High — the bulk of the work |
| `05` | Move engine command emission behind a session-owned interface | Medium |
| `06` | Test the session layer | None; this is the payoff |

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
