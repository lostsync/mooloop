# Teams

mooloop is divided into ten teams. They are areas of ownership, not groups of
people: Adam is on all ten, and agents come and go. What a team provides is an
owner. When a defect turns up, a feature is planned, or a boundary is unclear,
this file says whose it is.

The division was drawn for a review, in `reports/teams-2026-09-22.md`. Its §1
draws the lines, and §1.1 moves the ones its reviewers found in the wrong
place. This file is that map with the moves applied, extended to every file in
the tree, and it is the copy to keep current. The report stays as written.

**The seams matter more than the file lists.** The review found the components
well built and the joins between them fragile. Almost every High finding sat on
a seam that no team owned:

- hung notes, between the sequencer and seven devices;
- clicks, between the block loop and the output stage;
- the silent dialog failure, between Session and Platform.

[Seams that have an owner now](#seams-that-have-an-owner-now) is the section to
read when a finding does not obviously belong anywhere.

## The rules

- **Every file has one owning team.** So does every symbol in
  [a file two teams share](#files-two-teams-share). A test belongs to the team
  whose code it tests.
- **Every Linear issue carries exactly one `Team` label**: the team that owns
  the code where the fix lands. The label group is single-select, so Linear
  enforces the "one". Name any other team the work touches in the description,
  or give it a sub-issue of its own.
- **A seam with no row here has no owner.** Finding one is a result: add the
  row, in the same commit as whatever found it.
- **Keep this file true.** A commit that adds, moves or renames a file updates
  its entry. A new file inside a directory listed below belongs to that team
  without an edit.
- `docs/`, `reports/` and the root Markdown files belong to no team. A document
  follows its subject.

Paths are from the repository root, except that `engine/…` means
`crates/mooloop-engine/…`, and likewise `core`, `dsp`, `session`, `project`,
`ui` and `app`.

## The ten

| # | Team | Owns | Talks to the others through |
| --- | --- | --- | --- |
| 1 | **Realtime Engine** | The audio callback and its contract, the block loop that orchestrates it, command application, install and carry, offline export | `EngineCommand`/`EngineEvent`, `StructuralCommand`, `AudioNode`/`EventList`, `Discontinuity`, the meter and load atomics |
| 2 | **Sequencing & Time** | The musical clock, note scheduling, patterns, the playlist, and every note from NoteOn to release | `BlockSpan`, `Event::{NoteOn, NoteOff, Choke}`, `send_deferred` |
| 3 | **Mixer & Routing** | The signal from a channel's output stage to the driver buffer: strips, sends, buses, summing, latency, solo, meters | `CompiledBusGraph`/`CompiledLatency`, `MixerStripRow`, `STRIP_DESCRIPTORS` |
| 4 | **Instruments** | The eight sources end to end (DSP, descriptors, faces, factory patches) and sample import | `SourceNode`, `GeneratorParams`, audio taps |
| 5 | **Effects** | The inserts end to end, containers, effect presets, and the host that runs them | `ParamDescriptor`, `EffectSlotRow`, the `AudioNode` contract |
| 6 | **DSP Foundations** | The primitives every device is built from | the primitive APIs; `mooloop_core::gain` |
| 7 | **Parameters & Control** | Parameter identity, and the three things that address it: automation, modulation, MIDI learn and mapping | `Event::ParamValue`, `EngineEvent::ControlInput`, `set_param_normalized` |
| 8 | **Document & Session** | The live document, undo, the persisted format, and everything that touches disk | `Project`, `ProjectSnapshot`/`ProjectEdit`, `DocumentResult`, `edits_document` |
| 9 | **Interface** | The application shell, the 8 ms pump, and the UI every team shares: actions, shortcuts, shared controls, themes, panes | Slint models and callbacks, the pump |
| 10 | **Platform & Release** | The process boundary: drivers, MIDI port I/O, OS integration, startup and shutdown, packaging, CI, the toolchain | `AudioConfig`, `DriverStatus`, `OutputTarget`, `EngineHandle::open` |

## Seams that have an owner now

The moves from the report's §1.1: work that belonged to nobody, or to the wrong
team. **Ownership moved when this file was written; most of the code has not.**
Where it still sits in another team's file, the next section says where. Finding
ids (C2, P4, …) are the report's.

| Seam | Owner | Before | Why |
| --- | --- | --- | --- |
| The note lifecycle: which voices are sounding, all-notes-off, panic | Sequencing | `release_all_voices` plus seven per-device play→stop checks | Only the scheduler knows what it started (S1). `engine/src/voices.rs` holds that record since MOO-99; the per-device play→stop checks remain beside it |
| `RenderState::apply_midi`, which decides whether a message is played or mapped | Control | nobody | Pads are broken there (C2) |
| Automation lookup and lane editing in the sequencer | Control | Sequencing | It addresses the parameter system |
| Descriptor range and curve stability, not only ids | Control | each device team | Lanes are saved normalized, so widening a range silently changes playback (C11) |
| Driver lifecycle: shutdown, sample-rate change, reconnect | Platform, with Engine owning the reinstall path | nobody | Two reviewers found it unowned (P4, R1) |
| OS dialogs, XDG paths, the settings-load policy | Platform | Session, Interface | OS integration, and the worst defect in the review sits on that line (P1) |
| The document lifecycle: save/open/new/quit sequencing, `DocumentResult` handling, history record and commit | Document | `ui/src/lib.rs` | No document test can reach it there (D3, D4, D8) |
| Tempo, swing and the embed flag | Document | Slint window properties | Every snapshot reads `window.get_bpm()`, so the document had two owners |
| `core/src/structure.rs` | Document | Engine | Rack and container editing, with nothing realtime in it |
| The effect host: `EffectChain`/`EffectSlot` (bypass, wet/dry, trims, sleep, container scheduling) | Effects | Engine | It decides most of what is heard at a transition (E2) |
| Output-stage declicking: fader, mute, solo, polarity | Mixer | between Engine and Mixer | M2 fell between the two |
| The session reconcilers, the channel output verbs, `STRIP_DESCRIPTORS`, `pan_gains`/`balance_gains`, `ui/src/meter.rs` | Mixer | Session, core, dsp, Interface | Mixer state kept outside the mixer |
| The browser preview voice | Instruments | nobody | It plays files at the wrong pitch (I2) |
| `interpolate.rs`, `heldnotes.rs`, `synth_voice.rs` | Instruments | Foundations | Only instruments use them |
| The saturation stages; one voice-filter block and one glide block | Foundations | device-local copies | Five and four copies today (F7) |

## Work that crosses teams

**Audio recording** is one feature spread across four teams, run as a strike
team led by Engine rather than as an eleventh team:

- the capture ring (Engine);
- the drain thread and WAV writing (Document);
- the RECORD page (Instruments);
- the input ports (Platform).

**MIDI is not a team.** Port I/O is Platform's, notes recorded into patterns
are Sequencing's, and learn and mapping are Control's. The one join between
them, `apply_midi`, is Control's (above).

## Files two teams share

The default owner holds everything the exceptions do not name. The report's
§4.1 says how to cut the first three files so that the file lines match these
lines. Until then, this table is the boundary.

| File | Default owner | Except |
| --- | --- | --- |
| `engine/src/render.rs` | Engine: `RenderState`, `process_block_inner`, `apply_command`, install, takes, the discontinuity fan-out, the tests | **Effects:** `EffectChain`, `EffectSlot`, `ContainerScratch`, `PendingEffectParams`, `ReclaimedEffect`. **Mixer:** `SendBank`, `OutputStage`, `BusStrip`, `AudioTapBank`, `mix_into`, the bus walk. **Sequencing:** `release_all_voices`, `inject_choke_events`, `HeldKeys`, `Audition`, `RecordingNote`, the transport and sequencer arms. **Control:** `apply_midi`, `AutomationBlock`, `AutomationCurve`, `AutomationPosition`, `ModulationBlock`, the modulator ticks. **Instruments:** `PreviewVoice`, `RetiredPreviews`, `render_preview`. |
| `ui/src/lib.rs` | Interface: `AppUi::new` as a shell, the pump, the `wire_*!` macros as a framework | **Each feature team:** the wiring for its own faces and views inside `AppUi::new`. **Document:** the document lifecycle, and tempo, swing and embed. **Control:** the pump's control drain. |
| `ui/ui/main.slint` | Interface | **Sequencing:** the step grid and the playlist, both inline. **Each device team:** its entries in the DEVICES block. |
| `ui/src/settings.rs` | Interface | **Platform:** the XDG paths and the settings-load policy. |
| `ui/ui/device-rack.slint` | Interface: the shell every face is drawn in (`DeviceFrame`, `DeviceHeader`, `EffectDeviceShell`, the rails) | **Effects:** `ContainerEnclosure` and the container drawing, which the layer device's branches extend. |
| `engine/src/sequencer.rs` | Sequencing | **Control:** the automation functions (`automation_lane_at`, `has_automation_at`, `open_automation_lane` and the rest). |
| `core/src/modulation.rs` | Control | **Mixer:** `STRIP_DESCRIPTORS`. |
| `session/src/engine.rs` | Engine | **Mixer:** `sync_compensation`, `sync_console_sums`, `sync_solo`, `sync_channel_solo`. |
| `session/src/rack.rs` | Document | **Mixer:** the channel output verbs (`toggle_channel_mute`, `toggle_channel_solo`, `channel_solo_silenced`, `set_channel_volume`, `set_channel_pan`, `set_channel_bus`). |
| `session/src/midi.rs` | Control | **Sequencing:** `record_note`. |
| `dsp/src/bus.rs` | Engine | **Mixer:** `pan_gains`, `balance_gains`. |
| `dsp/src/node.rs` | Engine | **Foundations:** the math helpers (`feedback_tail_frames`). |

## Who owns which file

Files marked *shared* are split in the table above.

### 1. Realtime Engine

- `engine/src/`: `executor.rs`, `lib.rs` (`EngineHandle`), `load.rs`,
  `offline.rs`, `take.rs`, `block_cost.rs`, `render.rs` (*shared*), and the
  tests `render_test_support.rs`, `idle_skip_tests.rs`, `audio_edge_tests.rs`,
  `take_tests.rs`, `soak_tests.rs` (every device kind through the executor,
  MOO-113; a failure in a device it drives is that device's team's), and
  `continuity_tests.rs` (the "control changes are
  continuous" family, MOO-104, whose cases each team adds for its own
  transitions)
- `dsp/src/`: `node.rs` (*shared*), `event.rs`, `bus.rs` (*shared*), `taps.rs`
  — the `AudioNode` contract and what flows through it
- `core/src/bridge.rs`
- `session/src/engine.rs` (*shared*); `session/tests/ordering.rs`,
  `session/tests/delivery.rs`
- `ui/ui/export-dialog.slint`
- The plugin-host crate, once it exists (`docs/plans/plugin-hosting/`, MOO-11):
  hosted code runs inside the callback. Scanning, plugin paths and packaging
  are Platform & Release's.

### 2. Sequencing & Time

- `engine/src/transport.rs`, `engine/src/sequencer.rs` (*shared*)
- `engine/src/voices.rs`: the table of voices the sequencer started, per
  channel strip, and every release rule that reads it (MOO-99)
- `core/src/`: `pattern.rs`, `playlist.rs`, `time.rs`
- `session/src/`: `roll.rs`, `steps.rs`, `transport.rs`, `notes.rs`
- `ui/ui/`: `piano-grid.slint`, `channel-rack.slint`
- `ui/tests/`: `piano_drag.rs`, `piano_snapshot.rs`, `piano_tools.rs`,
  `roll_metrics.rs`, `playlist_snapshot.rs`, `rack_tools.rs`,
  `channel_reorder.rs`, `add_source_menu.rs`, `dock_resize.rs`

### 3. Mixer & Routing

- `core/src/`: `mixer.rs`, `gain.rs`, `strip.rs`, `outlet.rs`
- `dsp/src/`: `strip.rs`, `console.rs`, `preamp.rs` (the strip's input stage;
  the Preamp insert is Effects'), `output_guard.rs` (the master's non-finite
  scrub and safety limiter)
- `engine/src/`: `meters.rs`, and the tests `console_tests.rs`,
  `strip_tests.rs`, `gain_structure_tests.rs`, `output_guard_tests.rs`
- `session/src/mixer.rs`
- `ui/src/meter.rs`; `ui/ui/`: `mixer.slint`, `strip.slint`, `meters.slint`,
  `bus-device.slint`, `gain.slint`
- `ui/tests/`: `mixer_snapshot.rs`, `strip_face.rs`, `fader_taper.rs`,
  `gain_slint_agreement.rs`, `meter_threshold.rs`, `track_reorder.rs`
- `spikes/preamp-measure/`

### 4. Instruments

- `dsp/src/`: `sampler.rs`, `stretch.rs`, `stretch_cost.rs`, `commit.rs`,
  `sample_analysis.rs`, `drumsynth.rs`, `ds01.rs`, `monosynth.rs`,
  `polysynth.rs`, `mlm1.rs`, `mlp8.rs`, `aux_in.rs`, `interpolate.rs`,
  `heldnotes.rs`, `synth_voice.rs`
- `core/src/`: `sampler.rs`, `synth.rs`, `generator.rs`, `ds01.rs`,
  `ds01_factory.rs`, `mlm1.rs`, `mlm1_factory.rs`, `mlp8.rs`,
  `mlp8_factory.rs`, `aux_in.rs`, `input.rs`
- `session/src/`: `audio_file.rs`, `sample.rs`, `sampler.rs`
- `engine/src/ds01_tests.rs`
- `ui/ui/`: `sampler-device.slint`, `drum-device.slint`, `ds01-device.slint`,
  `mono-device.slint`, `poly-device.slint`, `mlm1-device.slint`,
  `mlp8-device.slint`, `aux-in-device.slint`, `device-oscillator.slint`,
  `envelope.slint`
- `ui/tests/`: `ds01_face.rs`, `mlp8_face.rs`, `source_snapshot.rs`,
  `source_kind_menu.rs`, `record_page.rs`

### 5. Effects

- `dsp/src/effects/`, `dsp/src/buffer_device.rs`
- `core/src/`: `effect.rs`, `effect_factory.rs`, `buffer.rs`
- `session/src/effects.rs`
- `engine/src/`: the tests `buffer_workflow_tests.rs`, `container_tests.rs`
- `ui/ui/`: `bitcrush-device.slint`, `buffer-device.slint`,
  `compressor-device.slint`, `container-device.slint`, `delay-device.slint`,
  `drive-device.slint`, `eq-device.slint`, `filter-device.slint`,
  `gate-device.slint`, `limiter-device.slint`, `plate-device.slint`,
  `preamp-device.slint`, `reverb-device.slint`
- `ui/tests/`: `eq_face.rs`, `eq_reverb_drag.rs`, `effect_preset_menu.rs`,
  `rack_reorder.rs`

### 6. DSP Foundations

- `dsp/src/`: `filter.rs`, `biquad.rs`, `osc.rs`, `env.rs`, `lfo.rs`,
  `smooth.rs`, `scale.rs`, `delayline.rs`, `align.rs`, `shaper.rs`,
  `harmonics.rs`, `dynamics.rs`, `analysis.rs`, `lib.rs`, and `testkit.rs`,
  the measurement kit every DSP test measures through at 44.1, 48, 96 and
  192 kHz (MOO-117), and the voice blocks `voice_filter.rs` and `glide.rs`
  (MOO-144)
- `spikes/time-stretch/`

### 7. Parameters & Control

- `core/src/`: `modulation.rs` (*shared*), `automation.rs`, `control.rs`,
  `midi.rs`, `mod_metadata.rs`, and the tests `param_id_freeze_tests.rs`,
  `stepped_round_trip_tests.rs`
- `dsp/src/modulator.rs`
- `session/src/`: `automation.rs`, `modulation.rs`, `midi.rs` (*shared*),
  `values.rs`
- `ui/ui/`: `modulation-shelf.slint`, `modulation-device.slint`
- `ui/tests/`: `shelf_agreement.rs`, `midi_learn_gesture.rs`,
  `slint_face_agreement.rs`, `knob_value_text.rs`

### 8. Document & Session

- `project/src/` (all of `mooloop-project`)
- `session/src/`: `session.rs`, `document.rs`, `history.rs`, `command.rs`,
  `project.rs`, `channel.rs`, `rack.rs` (*shared*), `recordings.rs`,
  `take.rs`, `browser.rs`, `edit_cost.rs`, `lib.rs`
- `core/src/`: `project.rs`, `structure.rs`, `channel.rs`, `color.rs`, `lib.rs`
- `session/tests/`: `gesture_undo.rs`, `structure.rs`
- `ui/ui/save-preset-dialog.slint`

### 9. Interface

- `ui/src/`: `lib.rs` (*shared*), `actions.rs`, `gestures.rs`, `settings.rs`
  (*shared*), `channel_colors.rs`, `mockup.rs`, `theme/`
- `ui/build.rs`, `ui/examples/`, `ui/tests/common/`
- `ui/ui/`: `main.slint` (*shared*), `controls.slint`, `toolbar.slint`,
  `menubar.slint`, `channel-sidebar.slint`, `appearance-dialog.slint`,
  `about-dialog.slint`, `color-picker.slint`, `theme.slint`, `reorder.slint`,
  `device-rack.slint` (*shared*), `device-displays.slint`,
  `device-concepts.slint`, `device-drag-harness.slint`,
  `save-error-dialog.slint`, `question-dialog.slint`, `takes-dialog.slint`,
  `mockup.slint`, `mockup-catalog.slint`,
  `mockup-tool.slint`
- `ui/tests/`: `menubar.rs`, `question_dialog.rs`, `panes.rs`, `pane_drag.rs`, `first_click.rs`,
  `name_field.rs`, `picker_chip.rs`, `color_picker.rs`, `sidebar.rs`,
  `browser.rs`, `gesture_bracket.rs`, `rack_keyboard.rs`,
  `save_error_snapshot.rs`, `preferences_appearance_snapshot.rs`,
  `preferences_shortcuts_snapshot.rs`, `preferences_developer_snapshot.rs`

`device-rack.slint` and `device-displays.slint` are the shell and the display
canvas every device face is drawn in. They are Interface's; the faces inside
them belong to the device teams, and the container drawing in
`device-rack.slint` is Effects'.

### 10. Platform & Release

- `app/src/` (the binary and `engine-selftest`)
- `engine/src/`: `jack_driver.rs`, `coreaudio_driver.rs`, `driver.rs`,
  `null_driver.rs` — the driver adapters, MIDI port I/O included, and the
  driver for no device
- `core/src/log.rs` and its test `core/tests/crash_report.rs`,
  `session/src/dialogs.rs`
- `ui/src/signals.rs` (quit signals routed to the pump)
- `ui/ui/audio-preferences.slint`, `ui/tests/preferences_audio_snapshot.rs`
- `packaging/`, `.github/`, `scripts/`, `.githooks/`, `.cargo/`,
  `rust-toolchain.toml`, `Cargo.toml`, `Cargo.lock`, and every crate's
  `Cargo.toml`
- The tooling configuration: `.mcp.json`, `.codegraph/`, `.cursor/`,
  `.claude/`, `package.json`, `package-lock.json`, `.gitignore`
- Plugin scanning, plugin paths, and packaging the plugin host
  (`docs/plans/plugin-hosting/` step 05, MOO-80)

## In Claude Code

Each team has a subagent in `.claude/agents/`, named `team-<slug>`:
`team-engine`, `team-sequencing`, `team-mixer`, `team-instruments`,
`team-effects`, `team-foundations`, `team-control`, `team-document`,
`team-interface` and `team-platform`, in the order of [the ten](#the-ten).
Each definition points back here for what its team owns, and preloads
`.claude/skills/team-brief/`, the instructions every team agent shares: stay
inside the team, file what lands elsewhere, and report back in a fixed shape.
`.claude/agents/orchestrator.md` routes work to them by `Team` label. A team
that is renamed, added or merged here changes its agent in the same commit.

## In Linear

The workspace has one Linear team, **Mooloop** (`MOO`), and it stays one. A
Linear team is for a group of people with their own statuses, triage queue and
cycles; these ten have one member and one workflow between them. Ten Linear
teams would mean ten backlogs to check, a new identifier for every moved
issue (the tree cites `MOO-n` over a hundred times), and a paid plan.

Instead, a workspace label group, **`Team`**, holds one label per team, named
exactly as in [the ten](#the-ten). It is single-select. A board per team is an
issue view filtered or grouped by `Team`. Each label's description is a few
words and a pointer back here; this file is the definition.

The rest of how Linear is used -- projects, statuses, the type labels, and the
`Question` and `Triage` labels -- is in `AGENTS.md`, *Tracking work: Linear*.
