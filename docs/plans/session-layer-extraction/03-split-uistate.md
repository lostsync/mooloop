# 03 — Split `UiState` into a session and its view models

Read `00-status.md` first. Steps 01 and 02 must be in.

This is where the plan stops being mechanical. `UiState` (`lib.rs:2175`) is two
things wearing one name: the application's live model, and a bag of Slint
models that mirror it. This step separates them.

## The shape today

Roughly ninety percent of `UiState`'s fields are plain Rust —

```
channels: Vec<ChannelState>          buses: Vec<BusSetup>
pattern_lengths: Vec<usize>          pattern_names: Vec<String>
playlist: Vec<PatternPlacement>      song_mode: bool
current_pattern: usize               selected: usize
effect_target: EffectTarget          selected_note_ids: HashSet<NoteId>
marquee_base: Option<(i32, HashSet<NoteId>)>
scale_base: Option<ScaleBase>        browser_locations: Vec<PathBuf>
browser_expanded: HashSet<PathBuf>   bundle_path: Option<PathBuf>
dirty: bool                          revision / source_revision: u64
generator_presets / channel_presets: Vec<PresetSummary>
automation_target: Cell<Option<ParamAddr>>
modulation_outputs: Cell<[f32; MAX_MODULATORS_PER_CHANNEL]>
...
```

— and about a dozen are `Rc<VecModel<...>>`: `rows`, `step_models`,
`note_model`, `automation_point_model`, `automation_target_model`,
`playlist_model`, `waveform_model`, `slice_model`, `playhead_model`,
`effect_slot_model`, `modulation_source_model`, `modulation_route_model`,
`mixer_strip_model`, `browser_rows`.

The models are not state. They are a *projection* of the plain fields, rebuilt
by the `sync_*` and `refresh_*` methods. Nothing reads a model to answer a
question about the project — which is exactly why this split is available.

## What to do

Define `Session` in `mooloop-session`, holding every plain field. Leave a
`ViewModels` struct in `mooloop-ui` holding the `Rc<VecModel<...>>` fields, and
have `AppUi` own both.

Then divide the 58 methods on the line they already fall on:

**Moves to `Session`** — model logic, no toolkit:

```
reset_channel_source     project_snapshot        sample_snapshots
replace_project          song_length_ticks       placement_covering
automation_destinations  automation_lanes        automation_lane
automation_lane_mut      automation_descriptor   effect_chain
effect_chain_mut         retarget_effect_slots   update_tempo_synced_delay_times
destination_depths       destination_offsets     modulation_depth_for
modulation_envelope_mut  channel_modulation_destination
select_note              toggle_note_selection   select_all_notes
remove_note_from_selection                       prune_note_selection
bus_feed_count           allowed_destinations
begin_modulation_edit    finish_modulation_edit  set_armed_modulation_depth
```

**Stays in `mooloop-ui`** — projection, takes `&Session` and `&MainWindow`:

```
show_pattern             refresh_rack_cell       refresh_rack_row
sync_row_flags           sync_pattern_menu       sync_playlist
sync_generator_preset_menu                       sync_channel_preset_menu
refresh_note_editor      refresh_automation      refresh_automation_points
refresh_selection_bounds refresh_selected_note_controls
sync_effects             refresh_modulation      refresh_modulation_offsets
sync_mixer               mixer_strip_row         sync_mixer_strip
sync_mixer_selection     sync_bus_editor         refresh_editor
update_document_title
```

`send_modulation` and `send_modulator_slot` are ambiguous — they emit engine
commands, which is session work, but are currently reached from view code. Move
them, and let step 05 tidy how they are reached.

## The judgement call in this step

Some methods do both: they compute something *and* push it into a model. The
temptation is to leave those alone as "not worth splitting."

Split them anyway, and split them the same way every time: the computation
becomes a `Session` method returning plain data, and the projection becomes a
view function that calls it and fills the model. It is more edits than leaving
them, but a half-split `UiState` is worse than either end state and the
remaining pairs are what step 04 has to work through.

`refresh_editor` (`lib.rs:4070`) is the hardest instance and the best test of
whether the split is real. Do it last in this step.

## Definition of done

- `Session` lives in `mooloop-session` with the listed methods and no `slint`.
- `ViewModels` holds only models, and every model is written by a projection
  function that takes `&Session`.
- No projection function is called from inside a `Session` method.

## Verification

Full build and a UI snapshot of each affected pane, per
`docs/AGENT_OPERATIONS.md`. This step can change what is drawn if a projection
is missed, so the snapshots are the point rather than a formality.
