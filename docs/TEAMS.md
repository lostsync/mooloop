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
| A hosted plugin instrument as a channel's source: the rack's processor swapped into the channel's `HostedSource` by `HostSourceProcessor`, keyed by the plugin's slot | Engine | Document (`ChannelSource::Plugin`, `DeviceKind::Plugin`), Instruments (`GeneratorParams::Plugin`) | MOO-84. `build_source` builds the source silent from the song; the rack's one rule puts the processor in, and a pull-back takes it out (silence, not a placeholder). A strip is carried across an install only onto the same slot. A plugin channel is made by `Session::set_plugin_source`; the window's path is Interface's (MOO-83): the add-channel menu's own "Add Plugin…" row and the browser's PLUGINS tab, and `SOURCE_KINDS_IN_PICKER_ORDER` still leaves `Plugin` (8) out, since a row there would make a channel with nothing in it |
| A hosted plugin's processor in a chain: built by the rack and swapped into its device's placeholder by `ReplaceEffect` keyed by the plugin's slot | Engine, with Effects owning the `EffectChain` it lands in, unchanged | nobody | MOO-81. The chain hosts it like any node (`replace_if_kind`, `build_effect`'s `PluginPlaceholder`). A plugin inside a container sizes its container's rings (MOO-212): Effects' `Session::resize_plugin_containers`, called from `service_plugins` on install and on a latency change, and `EffectChain::resize_container_rings` from `host_plugins` for an export, both from `mooloop_core::container_rings_with` |
| A hosted plugin's parameter driven by a lane or a route: which ids are driven, their conversion to the plugin's units, and the offset a route sends | Control, with Engine owning the adapter that turns `ParamValue`/`ParamMod` into CLAP events | nobody | MOO-82. `EffectChain::plugin_curves` (in Effects' `EffectChain`) walks the routes and lanes naming the device and resolves each id with `AudioNode::hosted_param`; a lane is a value, a route an offset (`Event::ParamMod`), because the plugin holds the base. A plugin's own edits are undo steps, recorded by the pump around `Session::capture_plugin_edits` |
| The Bus Comp as an insert: `EffectKind::BusComp`, `BusCompEffect`, its face and bank | Effects, running Mixer's `strip::bus_comp::BusComp` and drawing Mixer's `BusCompPanel` (`master-comp.slint`) unchanged | nothing | MOO-216. One compressor for the master's section and the insert: the insert's ids are the master's moved down (`bus_comp_master_id`), its descriptor table is derived from `MasterSectionParams::descriptors()`, and a change to the law or the panel is Mixer's and reaches both |
| Output-stage declicking: fader, mute, solo, polarity | Mixer | between Engine and Mixer | M2 fell between the two |
| The session reconcilers, the channel output verbs, `STRIP_DESCRIPTORS`, `pan_gains`/`balance_gains`, `ui/src/meter.rs` | Mixer | Session, core, dsp, Interface | Mixer state kept outside the mixer |
| The browser preview voice | Instruments | nobody | It plays files at the wrong pitch (I2) |
| `interpolate.rs`, `heldnotes.rs`, `synth_voice.rs` | Instruments | Foundations | Only instruments use them |
| The saturation stages; one voice-filter block and one glide block | Foundations | device-local copies | Five and four copies today (F7) |
| The equal-power crossfade law, `smooth::equal_power` | Foundations, called by Effects (Buffer's jumps, the effect host's wet/dry and Mix), `DelayLine`'s read head and Instruments (the sampler's loop seam) | four inline copies | MOO-43. One law, so a change to how a crossfade sounds is made once |
| A sampler's stretch pool size: `mooloop_core::sampler::stretch_pool_voices` (twice Voices, every voice with a lane on Voices), built by `ChannelStrip::load_source` on an install and by `Session::sync_sampler_stretch` on every pump tick, installed through `StructuralCommand::SetSamplerStretch` | Instruments (the rule and the reconciler), with Engine owning `load_source` and the install, Document the `sampler_stretch_sent` mirror, and Interface the pump line | the face's three sends, each building sixteen | MOO-7. One rule in two builders, so an undo and a Voices edit agree about the size. `Sampler::install_stretch` swaps the displaced pool's shared readers into the new one, so a resize keeps sounding voices |
| A sampler's key-zone audio: `ChannelAudioSnapshot::for_sampler` (the one builder), the session's `ZoneAudioTable` (`Session::zone_audio`, by path, replaced on open and new, never by undo), and `render_job_with_plugins`' per-channel `zones` | Instruments (the builder, the table type, the verbs), with Document owning `ChannelState::zones`, the `Session` field, `resolve_document`'s decode and the save write-back, and Engine the offline input and its refusal | nobody | MOO-14. Zone audio travels explicitly beside the base sample. An export that was not given a zone's audio is refused rather than rendered silent, and a zone restored by undo finds its buffer in the table without decoding. A copied channel carries its zones' buffers by path in `ChannelClipboard::zones` (Document), and `Session::paste_channel` admits them into the table before the install, because the clipboard outlives the table across New and Open (MOO-242) |
| A pattern written from a sampler's slices (`Session::write_slice_pattern`) | Instruments, writing Sequencing's pattern through its public paths only (`UpsertNote`/`RemoveNote`, `set_pattern_length`, the channel's note ids) | nobody | MOO-46. Instruments knows what the slices are; Sequencing owns what a pattern is, and nothing of it changed |
| Who writes a control's value: `controlled` on every shared value control (`controls.slint`, `toolbar.slint`) | Interface, with each face's team owning the republish its controls wait for | each face, knob by knob | MOO-220. A control reports and never writes, unless its caller says `controlled: false` over a plain property Rust sets directly. A face that binds a control to a model row or an expression must republish that row on the edit, and must not write the property itself. `controlled_faces_tests.rs` turns every slider in the window and holds each to the document |
| A hosted plugin in the window: the browser's PLUGINS tab, the insert menu's "Plugin…", and the face of a plugin with no GUI (`ui/src/plugin_ui.rs`, `ui/ui/plugin-device.slint`) | Interface, with Engine owning the session verbs it calls (`insert_plugin_effect`, `plugin_problem`, the rack's values and `value_text`) and Control owning `set_plugin_param` | nobody | MOO-83. The window reads only the host's neutral types (`PluginCache`, `ScannedPlugin`, `HostError`; `ui/tests/plugin_formats_stay_out.rs`). A face parameter crosses into Slint as its dense index, never its id, and so does a plugin route's `ModulationRouteRow.param` and the face's modulation overlays (MOO-228). The lane picker's list is `plugin_ui::lane_destinations`: the session's native `automation_destinations` with Control's `Session::plugin_destinations` placed before the strip; its index handler and the Automate request both read it and open by address. A knob's value is the plugin's, republished every pump tick into the slot's own model in place (a new model would rebuild a knob mid-drag); its undo step is the pump's "Plugin Edit", held while a gesture is open. A plugin device's preset (MOO-222) is Effects' (`load_plugin_effect_preset`, `plugin_preset_state` in `session/src/effects.rs`), opening its new slot through Engine's `host_new_plugin`, the half of `insert_plugin_effect` both share, and retiring the old one only through `PluginRack::remove` |
| Recorded MIDI's latency compensation: the driver's playback latency (`playback_latency_frames` in `jack_driver.rs`/`coreaudio_driver.rs`), carried in `SharedCells::capture_latency` and subtracted in `capture_note_on` | Sequencing (the subtraction and its wrap/clamp rule), Platform (what each driver reports), Engine (the cell, attached in `SharedCells::attach` with the rest, and `start` setting it from the driver) | nobody | MOO-209. An atomic, not a command: it follows the driver. The engine stores it on every start; the pump refreshes it once a second with the input latency (`EngineHandle::set_capture_latency`). The null driver and export report 0 |
| A MIDI take's Replace: which notes the playhead crossed (`Session::record_position`, `apply_replace`, `RecordTake` in `session/src/transport.rs`) and where a tick falls in the pattern (`mooloop_core::playlist::recording_offset`, the one rule the engine's `Sequencer::recording_tick` also calls) | Sequencing, with Interface owning the pump's call (`replace_crossed_notes` in `ui/src/lib.rs`) and the OVR/REPL button | nobody | MOO-234. The pump hands the session every `Position` in order, and applies a plan *before* the same drain's recorded notes, inside the take's one `Stream::Recording` undo entry. Only continuous advance crosses: a locate the session made says so (`RecordTake::locate`), and a stop, a pattern or mode change, a jump back other than the song loop's, or a jump forward of more than a bar crosses nothing. The recording channel is where the input is routed when the take starts (channels with their own input, else the selected one), fixed for the take. A note this take recorded is removed only by a later pass over its start |
| An export: the dialog's settings as one value, `RenderSettings` (`session/src/render_settings.rs`), built into the engine's `RenderJob` (`engine/src/offline.rs`) by `RenderSettings::job` and `Session::export_request`, and run by `document::run_export` | Engine for the job and its render (`RenderJob`/`RenderPass`/`RenderOutput`/`RenderTap`, `render_job_with_plugins`), Document for `RenderSettings` and the request, Interface for the card (`export-dialog.slint` layout, `ui/src/export_ui.rs`), Platform for the folder chooser and `music_dir` | nobody | MOO-180. The window never builds a job: it reads the card into `RenderSettings`, and everything a later Rendering issue adds (a range, a format, a source, naming) is a field of that value mapped into the job in `RenderSettings::job`. The range is the one field resolved outside `job`: `RenderRange::scope` against `Session::export_timeline` (transport, current pattern, loop points, song length) gives the pass's `RenderScope`, and a range the song cannot hold is a `SettingsProblem` on the card, never an engine error (MOO-181). Saved settings load tolerantly: an unknown range is the whole song (`RenderRange::loaded` for one the song has outgrown). A pass is one render of one timeline handed to every output; a hosted plugin's processors go to one pass, asked for by pass index. A name is numbered by default (MOO-188): Document resolves one number for the job in `RenderSettings::job` (`number_outputs`, `mooloop_core::file_names`) and marks each output `ExistingFile::Number { base }`; Engine places it with `rename_no_replace` and, if a file appeared since, on the next free number of `base`, reporting where it landed in `RenderedFile.path`. Only an `ExistingFile::Replace` output counts in `existing_targets`, and a job that would replace files asks once, with the count. The card's app-wide half (format, depth, dither, bitrate, mono, tail, numbering) is remembered by Interface in `settings.rs` (`ExportSettings`, the `[export]` table, loaded entry by entry), saved from `export_ui::remembered_export` when a render starts and shown by `show_export_memory` each time the card opens (MOO-190); the song's half (folder, name, source, range) is Document's to save in the song (MOO-194), and wins where both hold a value |
| A control naming its parameter to Rust: `ControlRequest` (`controls.slint`) and `name_if_asked` in the three `*_modulation_edit_started` handlers | Interface, with Control owning what learn and automate do with the address | nobody | MOO-143. A control sets `ControlRequest.naming`, fires its own `modulation-edit-started`, and clears the flag **in the same Slint function**; Rust notes the address in `UiState.named_param` and returns before learn or a gesture; the menu request that follows takes it. Rust never sets or clears `naming`. A face handler that does more than forward the callback must ignore it while `naming` is set |
| A device face's and a mixer strip's per-tick state: meters, dynamics readouts, the Buffer's marks, the EQ analyzer's, Preamp's and Buffer's traces, the strips' levels and lamps | Interface (the models and `ui/src/rack_displays.rs`), with Effects and Mixer owning the bindings in their faces | the rows (`EffectSlotRow`, `MixerStripRow`) | MOO-261. Nothing that moves every pump tick lives on a row that carries text: a whole-row write re-measures every text on the face. `effect-slot-meters` and `effect-slot-traces` are indexed like `effect-slots` and realigned (to rest) in `sync_effects`, the one place the rows are rebuilt; a face reads `root.slot-meters(index)` and `root.effect-slot-traces[index]`. A strip reads `StripMeters.level(track)`. The pump writes an entry only where it would be drawn differently, and a trace in place |
| Where a slow callback went: per-site lap timing in the block loop (`engine/src/site_times.rs`), the `HotSpot` record and its seqlock slot in `LoadMeters` (`engine/src/load.rs`), and the status bar's `audio-hot-spot` | Engine (the timing, the record, the threshold), with Mixer owning the bus walk its one lap per bus sits in, and Interface the pump line that names the sites and the Text that shows them | nobody | MOO-236. A lap is one `Instant::now()` as a site's turn begins and changes nothing the walk does; a new early exit in the bus walk needs no second read. The record is published only by a callback past `HOT_SPOT_SHARE_PERCENT` of its budget, after its work is measured, and never allocates or waits (`soak_tests.rs` publishes on every block). The pump names a `Site` by its index into `Session::channels` or `Session::buses` and sets the property at most once a second; the Text is a fixed-width box beside the readout, so a new string never re-measures the bar |

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
| `engine/src/render.rs` | Engine: `RenderState`, `process_block_inner`, `apply_command`, install, takes, the discontinuity fan-out, the tests | **Effects:** `EffectChain`, `EffectSlot`, `HostRamps`, `LeafPath`, `ContainerScratch`, `PendingEffectParams`, `ReclaimedEffect`. **Mixer:** `SendBank`, `OutputStage`, `BusStrip`, `AudioTapBank`, `mix_into`, the bus walk. **Sequencing:** `release_all_voices`, `inject_choke_events`, `HeldKeys`, `Audition`, `RecordingNote`, the transport and sequencer arms. **Control:** `apply_midi`, `AutomationBlock`, `AutomationCurve`, `AutomationPosition`, `ModulationBlock`, the modulator ticks, and `EffectChain::plugin_curves` (a hosted plugin's lanes and routes, MOO-82). **Instruments:** `PreviewVoice`, `RetiredPreviews`, `render_preview`, and each source's arm in `build_source` (the one match over the native kinds since MOO-56; `ChannelStrip` and its one `source` slot are Engine's). |
| `ui/src/lib.rs` | Interface: `AppUi::new` as a shell, the pump, the `wire_*!` macros as a framework | **Each feature team:** the wiring for its own faces and views inside `AppUi::new`. **Document:** the document lifecycle, and tempo, swing and embed. **Control:** the pump's control drain. **Engine:** hosted plugins' wiring (MOO-81): the opener set in `AppUi::new`, the export's plugin processors, and `AppUi::retire_plugins` with the pump's branch that waits for them at quit. |
| `ui/ui/main.slint` | Interface | **Sequencing:** the step grid and the playlist, both inline. **Each device team:** its entries in the DEVICES block. |
| `ui/src/settings.rs` | Interface | **Platform:** the XDG paths and the settings-load policy, and `PluginSettings`/`plugin_cache_path` (the scanner's, MOO-80). |
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
  MOO-113; a failure in a device it drives is that device's team's),
  `source_slot_tests.rs` (the one boxed source slot, MOO-56),
  `plugin_source_tests.rs` (a hosted source through the executor and the
  export, MOO-84),
  `plugin_host_tests.rs` (a hosted CLAP effect through the executor and the
  export, MOO-81), `live_check.rs` (the executor with no driver, behind the
  `test-support` feature: test support, not API, MOO-82), and
  `continuity_tests.rs` (the "control changes are
  continuous" family, MOO-104, whose cases each team adds for its own
  transitions)
- `crates/mooloop-plugin-host` (the plugin host, MOO-11; no plugin format's
  types leave it) and `crates/mooloop-test-plugin` (the in-repo CLAP test
  double every plugin-hosting step runs against)
- The plugin plan's neutral contract: `core/src/plugin.rs` (what a song says
  about a hosted plugin; its persisted shape is Document & Session's to
  review, as any saved field is), `core/tests/plugin_formats_stay_out.rs`,
  and `dsp/src/effects/plugin_placeholder.rs` (a plugin device with no
  plugin running in it), and `dsp/src/hosted_source.rs` (a channel source
  that runs a hosted plugin's processor, silent without one, MOO-84)
- `session/src/plugin_rack.rs`: the control thread's hosted plugins
  (`PluginRack`, `Session::service_plugins`, `Session::device_latency`,
  `insert_plugin_effect`, `export_plugin_processors`, `close_plugins`),
  in a Document-owned directory by agreement with Document & Session; and
  step 06's case, `session/tests/clap_effect.rs` and
  `session/examples/clap_effect_case.rs`
- `plugin-host/src/clap.rs`: the CLAP adapter (`ClapInstance`,
  `ClapProcessor`, `ClapOpener`), MOO-81
- `dsp/src/`: `node.rs` (*shared*), `event.rs`, `bus.rs` (*shared*), `taps.rs`
  — the `AudioNode` contract and what flows through it
- `core/src/bridge.rs`
- `session/src/engine.rs` (*shared*); `session/tests/ordering.rs`,
  `session/tests/delivery.rs`, and `session/tests/song_block_cost.rs` (what a
  real song's callbacks cost and which device kind or channel carries it; a
  finding in a device is that device's team's)
- `ui/ui/export-dialog.slint`
- The plugin-host crate, once it exists (`docs/plans/plugin-hosting/`, MOO-11):
  hosted code runs inside the callback. Scanning, plugin paths and packaging
  are Platform & Release's.

### 2. Sequencing & Time

- `engine/src/transport.rs`, `engine/src/sequencer.rs` (*shared*)
- `engine/src/voices.rs`: the table of voices the sequencer started, per
  channel strip, and every release rule that reads it (MOO-99)
- `engine/src/record_check.rs` (the MIDI capture path with no driver,
  behind `test-support`: test support, not API, MOO-234) and
  `session/tests/midi_record_loop.rs` (three passes through it, both modes,
  Overdub and Replace)
- `core/src/`: `pattern.rs`, `playlist.rs`, `time.rs`
- `session/src/`: `roll.rs`, `steps.rs`, `transport.rs`, `notes.rs`
- `ui/ui/`: `piano-grid.slint`, `channel-rack.slint`
- `ui/tests/`: `piano_drag.rs`, `piano_snapshot.rs`, `piano_tools.rs`,
  `roll_metrics.rs`, `playlist_snapshot.rs`, `rack_tools.rs`,
  `channel_reorder.rs`, `add_source_menu.rs`, `dock_resize.rs`

### 3. Mixer & Routing

- `core/src/`: `mixer.rs`, `gain.rs`, `strip.rs`, `outlet.rs`
- `dsp/src/`: `strip.rs` and `strip/` (`strip/bus_comp.rs`, the master bus
  compressor's laws, MOO-13), `console.rs`, `preamp.rs` (the strip's input stage;
  the Preamp insert is Effects'), `output_guard.rs` (the master's non-finite
  scrub and safety limiter)
- `engine/src/`: `meters.rs`, and the tests `console_tests.rs`,
  `strip_tests.rs`, `gain_structure_tests.rs`, `output_guard_tests.rs`, and the
  master bus compressor's acceptance render, `engine/examples/master_bus_comp.rs`
- `session/src/mixer.rs`
- `ui/src/meter.rs`; `ui/ui/`: `mixer.slint`, `strip.slint`, `meters.slint`,
  `bus-device.slint`, `gain.slint`, `master-comp.slint` (the master bus compressor's face)
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
- `session/src/`: `audio_file.rs`, `sample.rs`, `sampler.rs`, and the listening
  renders `session/examples/slice_pattern_case.rs` (MOO-46),
  `session/examples/sampler_key_zones.rs` (MOO-14) and
  `session/examples/mlp8_unison_levels.rs` (MOO-244)
- `engine/src/ds01_tests.rs`, `engine/src/sampler_tests.rs` (MOO-39), and the listening renders
  `engine/examples/mlp8_volume_pump.rs` (MOO-214),
  `engine/examples/sampler_loop_seam.rs` (MOO-43),
  `engine/examples/sampler_fit_freeze.rs` (MOO-39),
  `engine/examples/sampler_loop_grid.rs` (MOO-47),
  `engine/examples/sampler_slice_detect.rs` (MOO-44) and
  `engine/examples/sampler_legato_glide.rs` (MOO-45)
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
- `engine/src/`: the tests `buffer_workflow_tests.rs`, `container_tests.rs`,
  `settled_host_tests.rs` (a settled insert against the per-sample blend, MOO-260),
  `bus_comp_tests.rs`, and the Bus Comp insert's acceptance render
  `engine/examples/bus_comp_insert.rs` (MOO-216), and the inserts' listening
  renders `engine/examples/phaser_sweep.rs` (MOO-235),
  `engine/examples/drive_curves.rs` (MOO-250),
  `engine/examples/chorus_width.rs` (MOO-245) and
  `engine/examples/modulation_feedback.rs` (MOO-200)
- `ui/ui/`: `bitcrush-device.slint`, `buffer-device.slint`, `bus-comp-device.slint`,
  `compressor-device.slint`, `container-device.slint`, `delay-device.slint`,
  `drive-device.slint`, `eq-device.slint`, `filter-device.slint`,
  `gate-device.slint`, `layer-device.slint`, `limiter-device.slint`,
  `plate-device.slint`, `preamp-device.slint`, `reverb-device.slint`,
  `modulation-device.slint` (the Modulation insert's face; filed under Control
  until MOO-245)
- `ui/src/layer_view.rs`: what the rack draws of a chain holding layers
  (which rows a layer hides, where each box closes, the branch list and the
  bracket), the container drawing's derivation (`containers/09`)
- `ui/tests/`: `eq_face.rs`, `eq_reverb_drag.rs`, `effect_preset_menu.rs`,
  `layer_face.rs`, `rack_reorder.rs`

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
  `values.rs`, `plugin_params.rs` (a hosted plugin's parameters by address
  and by dense index, and the one conversion between them, MOO-82)
- `engine/src/plugin_automation_tests.rs` (lanes and routes on a hosted
  plugin's parameters, offline and live) and `session/tests/plugin_params.rs`
- `session/tests/source_param_kind.rs` (a binding made on one device is inert
  on another and comes back, MOO-135)
- `ui/ui/`: `modulation-shelf.slint`
- `ui/tests/`: `shelf_agreement.rs`, `midi_learn_gesture.rs`,
  `slint_face_agreement.rs`, `knob_value_text.rs`

### 8. Document & Session

- `project/src/` (all of `mooloop-project`)
- `session/src/`: `session.rs`, `document.rs`, `autosave.rs`, `history.rs`, `command.rs`,
  `project.rs`, `channel.rs`, `rack.rs` (*shared*), `recordings.rs`,
  `render_settings.rs` (the export dialog's settings, MOO-180),
  `take.rs`, `browser.rs`, `edit_cost.rs`, `lib.rs`
- `core/src/`: `project.rs`, `structure.rs`, `channel.rs`, `color.rs`,
  `file_names.rs` (a free name, and a write that never replaces a file:
  takes, assets and exports, MOO-241, MOO-188), `lib.rs`
- `session/tests/`: `gesture_undo.rs`, `structure.rs`, `source_switch.rs`,
  `document_fixtures.rs`
- `project/tests/source_param_kind.rs` (a song saved before generator
  addresses carried a kind loads with each one intact, MOO-135)
- `ui/ui/save-preset-dialog.slint`

### 9. Interface

- `ui/src/`: `lib.rs` (*shared*), `actions.rs`, `gestures.rs`, `settings.rs`
  (*shared*), `status_bar.rs`, `channel_colors.rs`, `mockup.rs`, `theme/`,
  `typed_value.rs`, `pump_profile.rs` (`MOOLOOP_PROFILE_UI`), `rack_displays.rs`
  and its tests (the pump's per-tick display models: rack meters and traces,
  strip meters, lamps and playheads, MOO-261)
- `ui/build.rs`, `ui/examples/`, `ui/tests/common/`
- `ui/ui/`: `main.slint` (*shared*), `controls.slint`, `toolbar.slint`,
  `menubar.slint`, `channel-sidebar.slint`, `appearance-dialog.slint`,
  `about-dialog.slint`, `color-picker.slint`, `theme.slint`, `reorder.slint`,
  `device-rack.slint` (*shared*), `device-displays.slint`,
  `device-concepts.slint`, `device-drag-harness.slint`,
  `save-error-dialog.slint`, `question-dialog.slint`, `takes-dialog.slint`,
  `mockup.slint`, `mockup-catalog.slint`,
  `mockup-tool.slint`, `plugin-device.slint` (the face of a hosted plugin
  with no GUI, MOO-83)
- `ui/src/plugin_ui.rs` and its tests (the plugin browser, menu row and face)
- `ui/src/export_ui.rs` and its tests (the export card's wiring, MOO-180)
- `ui/tests/`: `menubar.rs`, `question_dialog.rs`, `panes.rs`, `pane_drag.rs`, `first_click.rs`,
  `name_field.rs`, `picker_chip.rs`, `color_picker.rs`, `sidebar.rs`,
  `browser.rs`, `gesture_bracket.rs`, `rack_keyboard.rs`,
  `save_error_snapshot.rs`, `status_notice.rs`, `knob_typed_entry.rs`,
  `preferences_appearance_snapshot.rs`,
  `preferences_shortcuts_snapshot.rs`, `preferences_developer_snapshot.rs`,
  `plugin_formats_stay_out.rs`

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
  (`docs/plans/plugin-hosting/` step 05, MOO-80): `plugin-host/src/scan.rs`,
  `plugin-host/src/bin/mooloop-scan-child.rs` and `plugin-host/tests/scan.rs`,
  in Engine's crate by agreement, and `app/tests/scan_child.rs`. The test
  plugin's two misbehaving names (`CRASHES_ON_SCAN`, `HANGS_ON_SCAN` in
  `mooloop-test-plugin`) are there for these tests. In `ui/src/settings.rs`,
  `PluginSettings` and `plugin_cache_path`

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
