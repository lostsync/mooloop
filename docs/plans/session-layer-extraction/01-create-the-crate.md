# 01 — Create the crate and move what is already portable

Read `00-status.md` first, particularly the scope boundary.

This step creates `mooloop-session` and fills it with code that is *already*
toolkit-free. Nothing is rewritten. If a move requires an edit beyond fixing an
import path, it belongs in a later step.

## What is wrong today

`crates/mooloop-ui/src/lib.rs` ends with roughly six hundred lines of functions
that have nothing to do with Slint — sample decoding, waveform peak reduction,
filesystem browsing, zenity dialog invocation, note-name formatting. They live
in the UI crate for no reason except that they were written there.

## What to do

Create `crates/mooloop-session` depending on `mooloop-core`, `mooloop-dsp`,
`mooloop-engine`, `mooloop-project`, and `symphonia`. **Not `slint`.** Add it to
the workspace and to `mooloop-ui`'s dependencies.

Move, unchanged except for visibility and imports:

- **`crates/mooloop-ui/src/history.rs` entire.** It is already documented as
  "UI-thread-only" rather than toolkit-coupled, and `History<T>` is generic.
- **`crates/mooloop-ui/src/audio_file.rs` entire** (227 lines), and the sample
  loading built on it: `load_sample_at_path`, `sample_files_in_directory`,
  `sample_index`, `adjacent_sample`, `is_playable_sample`, `inspect_sample`,
  `SampleInspection`.
- **Waveform reduction:** `waveform_peaks`, `waveform_peaks_windowed`,
  `peaks_from_frames`.
- **Sample description helpers:** `sample_description`, `sample_duration`,
  `midi_to_note_name`, `midi_to_frequency_hz`, `tune_label`.
- **Browser filesystem walking:** `has_playable_descendant`, `scan_browser_dir`,
  `browser_display_name`.
- **The zenity layer:** `zenity_path`, `pick_bundle_via_zenity`,
  `pick_song_via_zenity`, `pick_save_via_zenity`, `pick_export_via_zenity`,
  `pick_sample_via_zenity`, `confirm_via_zenity`, `normalize_song_selection`.

The dialogs are worth a note: they shell out to zenity rather than using a
Slint dialog, so they are portable as they stand. That is luck, but it is luck
worth banking — file and confirmation dialogs are usually the stickiest part of
a toolkit migration and here they are already free.

## What stays behind

- `build_browser_rows`, `push_browser_rows`, `refresh_browser` — they build
  `BrowserRow`, a Slint type. The *walking* moves; the *row building* does not.
- `waveform_peaks`' callers that push into `Rc<VecModel<f32>>`.
- Every `*_from_int` / `*_to_int` converter. These exist because Slint has no
  Rust enums and belong to the view.

## Definition of done

- `crates/mooloop-session/Cargo.toml` exists and does not name `slint`.
- `mooloop-ui/src/lib.rs` is meaningfully shorter and the application is
  unchanged.
- `cargo test -p mooloop-session` runs, even if it only carries the tests that
  came with `audio_file.rs`.

## Verification

`cargo check -p mooloop-session` then a full `cargo build`, on the build box per
`docs/AGENT_OPERATIONS.md`. No UI snapshot needed — nothing drawable changed.
