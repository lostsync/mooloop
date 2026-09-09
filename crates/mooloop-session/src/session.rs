//! The live application model.
//!
//! `Session` is the project as the application is editing it: channels,
//! patterns, playlist, buses, selection, and the transient state a gesture
//! carries. Nothing here is a widget, and nothing here knows one exists --
//! `mooloop-ui` projects this into Slint models and never the other way
//! round.

use crate::channel::ChannelState;
use crate::sample::{sample_description, sample_duration, sample_files_in_directory, sample_index, waveform_peaks};
use crate::notes::ScaleBase;
use crate::project::ProjectSnapshot;
use crate::values::descriptor_slots;
use mooloop_core::{
    default_buses, sanitize_bank, would_create_cycle, DEFAULT_STEPS,
    MAX_BUSES, MAX_PLAYLIST_PLACEMENTS,
    drop_lanes_for_device, strip_descriptor, AutomationLane, BusSetup, Channel, ChannelSetup,
    DeviceId,
    AuxInParams, AuxInState, ChannelSource, DeviceKind, DrumSynthParams, DrumSynthState, Ds01Params, Ds01State,
    EffectParams, EffectSlotState, EffectTarget, MlM1Params, MlM1State, MlP8Params, MlP8State,
    LoopRange, ModDestinationDescriptor, ModEnvelopeParams, ModPolarity, ModRoute, ModulatorParams,
    MonoSynthParams, MonoSynthState, NoteId, ParamAddr,
    ParamDescriptor, ParamOwner, PatternPlacement, PlaybackMode, PointId, PolySynthParams,
    PolySynthState, Project, ProjectChannel, SampleReference, SamplerParams, SamplerState,
    modulation::CONTROL_SOURCE_SLOTS,
    MAX_MODULATORS_PER_CHANNEL, MAX_SWING_PERCENT, MIN_SWING_PERCENT, TICKS_PER_BAR,
    TICKS_PER_STEP, DELAY_PARAM_TIME_MS, MODULATION_PARAM_RATE_HZ,
};
use mooloop_dsp::SampleData;
use mooloop_project::PresetSummary;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// Which preset kind a save dialog in flight is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresetSaveTarget {
    Generator,
    Channel,
    /// One rack row of `target`'s chain, by the device's durable id.
    ///
    /// This used to carry the slot number and be rewritten whenever the rack
    /// was reordered under an open dialog, because a save that landed on
    /// whichever device now sits in position 3 would be a bug that is very
    /// hard to find later. An id cannot land on the wrong device: it either
    /// resolves to the one the dialog was opened from or to nothing at all.
    Effect { target: EffectTarget, device: DeviceId },
}

pub struct Session {
    pub channels: Vec<ChannelState>,
    /// Destination shown in the piano roll's variable lane. `None` means the
    /// lane is open but empty-handed, which is the state a fresh project is
    /// in; it is not the same as the lane being hidden.
    /// A `Cell` so `refresh_automation` can run from the shared `&self`
    /// editor refresh: reconciling a destination whose device was removed is
    /// part of drawing the lane, not a separate edit.
    pub automation_target: Cell<Option<ParamAddr>>,
    /// Point last created or dragged. Drives the highlight and the header
    /// readout; a drag re-reads it by id, so reordering the model underneath
    /// an in-flight drag is harmless.
    pub automation_selected_point: Cell<Option<PointId>>,
    /// The channel and note of the slice a handle is currently holding down,
    /// so its release goes to exactly the note that was struck. Kept rather
    /// than re-derived from the handle's index on the way up: a drag past a
    /// neighbour reorders the map underneath the handle, and the index it
    /// releases with is then a different slice's.
    pub slice_audition: Option<(u8, u8)>,
    pub modulation_shelf_open: bool,
    /// Source whose editor is open in the shelf. Selection is intentionally
    /// separate from assignment: looking at an LFO must not hijack knob
    /// gestures throughout the rack.
    pub modulation_selected_slot: Cell<Option<u8>>,
    /// The rack device the keyboard acts on, as an identity rather than a
    /// position.
    ///
    /// This is the first thing to *use* what `containers/` step 01 built.
    /// A selection kept as a slot number would have to be rewritten by every
    /// reorder -- which is exactly the class of bug `SlotRemap` existed for
    /// and was deleted for -- so the selection names the device and the slot
    /// is derived when it is needed. Dragging a row therefore does not change
    /// what is selected, and neither does inserting one above it.
    ///
    /// Scoped to `effect_target` like the chain itself: pointing the rack at
    /// a different channel or bus clears it, because a device id is only
    /// unique within one chain.
    pub selected_device: Option<(EffectTarget, DeviceId)>,
    /// The generator at the head of a channel's chain, when *it* is what the
    /// selection names.
    ///
    /// A separate field rather than a variant of `selected_device`, because a
    /// generator has no `DeviceId`: it is not a row of the effect chain, it
    /// is the thing the chain runs after. Holding it here keeps every
    /// existing reader of `selected_device` -- copy, cut, duplicate, the
    /// rack's selected border -- meaning exactly what it meant, and the two
    /// are kept mutually exclusive by `select_device` and `select_source`,
    /// which is the invariant that makes "what is selected" answerable.
    ///
    /// Scoped to an `EffectTarget` for the same reason the device selection
    /// is, and only ever a `Channel`: a bus has no generator.
    pub selected_source: Option<EffectTarget>,
    pub modulation_armed_slot: Cell<Option<u8>>,
    /// The selected channel's latest modulator outputs, refreshed from the
    /// engine on the pump tick. Held here rather than recomputed per knob
    /// so one read of the audio thread's cells feeds every destination.
    pub modulation_outputs: Cell<[f32; CONTROL_SOURCE_SLOTS]>,
    /// Channel that owns the transient selection/assignment state. Changing
    /// channels clears both even when the new channel happens to occupy the
    /// same runtime slot.
    pub modulation_ui_channel: Cell<Option<usize>>,
    /// The compensation plan the engine has been told about, so the pump's
    /// reconcile can send only what changed. Not document state: it is a
    /// record of what has been said to the audio thread, and a fresh session
    /// has said nothing.
    pub compensation_sent: mooloop_core::CompiledLatency,
    /// Which buses the engine has been given a console accumulator for, so
    /// the pump's reconcile sends only what changed. Same status as
    /// [`Self::compensation_sent`]: a record of what has been said to the
    /// audio thread, not document state.
    pub console_sums_sent: [bool; MAX_BUSES],
    /// The audio-edge plan the engine has been told about, so the pump's
    /// reconcile sends only what changed. Same status as
    /// [`Self::compensation_sent`]: a record of what has been said to the
    /// audio thread, not document state.
    pub audio_graph_sent: mooloop_core::CompiledAudioGraph,
    /// Snapshot captured at the start of a direct knob gesture. Intermediate
    /// control updates still reach audio immediately, while one release
    /// becomes one undoable route edit.
    pub modulation_edit_before: Option<ProjectSnapshot>,
    pub modulation_edit_changed: bool,
    /// Sample-browser folders in display order, mirroring the persisted
    /// settings for this session.
    pub browser_locations: Vec<PathBuf>,
    /// Folders currently expanded, by path, so a refresh survives reordering.
    pub browser_expanded: HashSet<PathBuf>,
    pub default_waveform: Vec<f32>,
    pub default_sample_description: String,
    pub default_sample_duration: f32,
    /// Mirror of the project's track bank, master first.
    ///
    /// As long as the song says, not a fixed seventeen: a track exists
    /// because somebody made it. `mooloop_core::sanitize_bank` guarantees the
    /// master and repairs routing on the way in.
    pub buses: Vec<BusSetup>,
    pub pattern_lengths: Vec<usize>,
    pub pattern_names: Vec<String>,
    pub playlist: Vec<PatternPlacement>,
    /// The section of the arrangement the transport repeats. Document state,
    /// not a session gesture: a loop is set around the part being worked on
    /// and is worth reopening the song to.
    pub loop_range: LoopRange,
    pub song_mode: bool,
    pub current_pattern: usize,
    pub selected: usize,
    /// Which effect chain the device rack edits. Selecting a channel in the
    /// step grid points it at that channel; selecting a strip in the mixer
    /// points it at a bus. `selected` stays put either way, because the piano
    /// roll, the step grid, and the sampler all still mean a channel.
    pub effect_target: EffectTarget,
    pub selected_note_id: Option<NoteId>,
    /// The full multi-selection, driving highlight and bulk delete. Always a
    /// superset of `selected_note_id` when non-empty; the precision editor
    /// (`refresh_selected_note_controls`) only shows fields when this has
    /// settled on exactly one member, since it edits one note, not a group.
    pub selected_note_ids: HashSet<NoteId>,
    /// The selection a marquee started from, plus how it should combine with
    /// what the band catches. `None` when no band is in flight.
    pub marquee_base: Option<(i32, HashSet<NoteId>)>,
    /// The selection's geometry when a scale drag started, plus the tick it
    /// scales about. Every frame is applied to this rather than to the live
    /// notes, so repeated scaling does not compound its own rounding.
    pub scale_base: Option<ScaleBase>,
    pub bundle_path: Option<PathBuf>,
    pub dirty: bool,
    pub revision: u64,
    pub source_revision: u64,
    pub generator_presets: Vec<PresetSummary>,
    pub channel_presets: Vec<PresetSummary>,
    /// Every effect preset on disk, across every kind. A rack row offers the
    /// entries whose [`mooloop_project::PresetKind`] matches its own device;
    /// one flat list here means one scan per refresh rather than one per row.
    pub effect_presets: Vec<PresetSummary>,
    /// The preset each rack row was last loaded from or saved as, so the
    /// device header can say which one it is.
    ///
    /// Kept here rather than inside [`EffectSlotState`], which is `Copy` and
    /// is exactly what a preset bundle stores: the label is how the row got
    /// to these settings, not part of what the settings are. It follows the
    /// row through a reorder and is dropped with a removal, like every other
    /// thing on this side that names a slot.
    pub effect_preset_names: HashMap<(EffectTarget, DeviceId), String>,
    /// The preset each channel's generator was last loaded from or saved as,
    /// for the source device's header. The generator half of
    /// `effect_preset_names`, keyed by channel because a channel has exactly
    /// one generator and it is not a slot in any chain.
    pub source_preset_names: HashMap<u8, String>,
    pub pending_preset_save: Option<PresetSaveTarget>,
}


impl Default for Session {
    /// The session the application holds before any document is installed:
    /// one empty sampler channel, one pattern, the default bus bank.
    fn default() -> Self {
        Self {
            channels: vec![ChannelState::new(0)],
            automation_target: Cell::new(None),
            automation_selected_point: Cell::new(None),
            slice_audition: None,
            modulation_shelf_open: false,
            modulation_selected_slot: Cell::new(None),
            selected_device: None,
            selected_source: None,
            modulation_armed_slot: Cell::new(None),
            modulation_outputs: Cell::new([0.0; CONTROL_SOURCE_SLOTS]),
            modulation_ui_channel: Cell::new(None),
            compensation_sent: mooloop_core::CompiledLatency::default(),
            console_sums_sent: [false; MAX_BUSES],
            audio_graph_sent: mooloop_core::CompiledAudioGraph::default(),
            modulation_edit_before: None,
            modulation_edit_changed: false,
            browser_locations: Vec::new(),
            browser_expanded: HashSet::new(),
            default_waveform: Vec::new(),
            default_sample_description: String::new(),
            default_sample_duration: 0.0,
            buses: default_buses(),
            pattern_lengths: vec![DEFAULT_STEPS as usize],
            pattern_names: vec![String::new()],
            playlist: Vec::with_capacity(MAX_PLAYLIST_PLACEMENTS),
            loop_range: LoopRange::default(),
            song_mode: false,
            current_pattern: 0,
            selected: 0,
            effect_target: EffectTarget::Channel(0),
            selected_note_id: None,
            selected_note_ids: HashSet::new(),
            marquee_base: None,
            scale_base: None,
            bundle_path: None,
            dirty: false,
            revision: 0,
            source_revision: 0,
            generator_presets: Vec::new(),
            channel_presets: Vec::new(),
            effect_presets: Vec::new(),
            effect_preset_names: HashMap::new(),
            source_preset_names: HashMap::new(),
            pending_preset_save: None,
        }
    }
}

/// Bins the stored channel waveform is reduced to. A fixed overview; the
/// editor re-derives real detail for whatever range it is zoomed to.
pub const WAVEFORM_BINS: usize = 256;

/// What `Session::arm_modulation_route` did.
pub enum ArmedRoute {
    /// Nothing armed, the destination refuses modulation, or it already sits
    /// at the depth asked for.
    Unchanged,
    /// The channel's modulation matrix is full; no route was added.
    Full,
    Added(ModRoute),
}

impl Session {
    pub fn reset_channel_source(&mut self, index: usize, kind: DeviceKind) {
        let Some(channel) = self.channels.get_mut(index) else {
            return;
        };
        self.source_revision = self.source_revision.wrapping_add(1);
        // A fresh device did not come from whatever preset the last one wore.
        self.source_preset_names.remove(&(index as u8));
        channel.kind = kind;
        channel.name = match kind {
            DeviceKind::Sampler => format!("Sampler {}", index + 1),
            DeviceKind::DrumSynth => format!("Drum {}", index + 1),
            DeviceKind::MonoSynth => format!("Mono {}", index + 1),
            DeviceKind::PolySynth => format!("Poly {}", index + 1),
            DeviceKind::MlM1 => format!("ML-M1 {}", index + 1),
            DeviceKind::MlP8 => format!("ML-P8 {}", index + 1),
            DeviceKind::Ds01 => format!("DS-01 {}", index + 1),
            DeviceKind::AuxIn => format!("Aux {}", index + 1),
        };
        match kind {
            DeviceKind::Sampler => {
                channel.params = SamplerParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::DrumSynth => {
                channel.drum_params = DrumSynthParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::MlM1 => {
                channel.mlm1_params = MlM1Params::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::Ds01 => {
                channel.ds01_params = Ds01Params::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::AuxIn => {
                channel.aux_in_params = AuxInParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::MlP8 => {
                channel.mlp8_params = MlP8Params::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::MonoSynth => {
                channel.mono_params = MonoSynthParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
            DeviceKind::PolySynth => {
                channel.poly_params = PolySynthParams::default();
                channel.sample_name.clear();
                channel.sample_description.clear();
                channel.sample_duration = 0.0;
                channel.sample_path = None;
                channel.sample_embedded = false;
                channel.sample_data = None;
                channel.committed_sample = None;
                channel.commit = None;
                channel.slices.clear();
                channel.waveform.clear();
                channel.can_previous_sample = false;
                channel.can_next_sample = false;
            }
        }
    }

    pub fn project_snapshot(&self, bpm: i32, swing_percent: i32) -> Project {
        let channels = self
            .channels
            .iter()
            .map(|channel| {
                let source = match channel.kind {
                    DeviceKind::Sampler => {
                        let sample = channel
                            .sample_path
                            .as_ref()
                            .map(|path| SampleReference::File {
                                path: path.clone(),
                                embedded: channel.sample_embedded,
                            })
                            .unwrap_or_default();
                        ChannelSource::Sampler(SamplerState {
                            params: channel.params,
                            sample,
                            slices: channel.slices.clone(),
                            commit: channel.commit.clone(),
                        })
                    }
                    DeviceKind::DrumSynth => ChannelSource::DrumSynth(DrumSynthState {
                        params: channel.drum_params,
                    }),
                    DeviceKind::MonoSynth => ChannelSource::MonoSynth(MonoSynthState {
                        params: channel.mono_params,
                    }),
                    DeviceKind::PolySynth => ChannelSource::PolySynth(PolySynthState {
                        params: channel.poly_params,
                    }),
                    DeviceKind::MlM1 => ChannelSource::MlM1(MlM1State {
                        params: channel.mlm1_params,
                    }),
                    DeviceKind::MlP8 => ChannelSource::MlP8(MlP8State {
                        params: channel.mlp8_params,
                    }),
                    DeviceKind::Ds01 => ChannelSource::Ds01(Ds01State {
                        params: channel.ds01_params,
                    }),
                    DeviceKind::AuxIn => ChannelSource::AuxIn(AuxInState {
                        params: channel.aux_in_params,
                    }),
                };
                ProjectChannel {
                    setup: ChannelSetup {
                        channel: Channel {
                            name: channel.name.clone(),
                            kind: channel.kind,
                            muted: channel.muted,
                            volume: channel.volume,
                            pan: channel.pan,
                            bus: channel.bus,
                        },
                        source,
                        effects: channel.effects.clone(),
                        modulation: channel.modulation,
                        next_device_id: channel.next_device_id,
                    },
                    notes: channel.notes.clone(),
                    automation: channel.automation.clone(),
                    next_note_id: channel.next_note_id,
                }
            })
            .collect();
        Project {
            bpm: bpm.clamp(1, 999) as u16,
            swing_percent: swing_percent.clamp(MIN_SWING_PERCENT.into(), MAX_SWING_PERCENT.into())
                as u8,
            ppq: 96,
            beats_per_bar: 4,
            playback_mode: if self.song_mode {
                PlaybackMode::Song
            } else {
                PlaybackMode::Pattern
            },
            current_pattern: self.current_pattern as u16,
            selected_channel: self.selected as u8,
            channels,
            buses: self.buses.clone(),
            pattern_lengths: self
                .pattern_lengths
                .iter()
                .map(|length| *length as u16)
                .collect(),
            playlist: self.playlist.clone(),
            loop_range: self.loop_range,
        }
    }

    pub fn sample_snapshots(&self) -> Vec<Option<Arc<SampleData>>> {
        self.channels
            .iter()
            .map(|channel| {
                (channel.kind == DeviceKind::Sampler)
                    .then(|| channel.sample_data.clone())
                    .flatten()
            })
            .collect()
    }

    pub fn song_length_ticks(&self) -> u32 {
        let content_end = self
            .playlist
            .iter()
            .filter_map(|placement| {
                self.pattern_lengths
                    .get(placement.pattern as usize)
                    .map(|steps| {
                        placement
                            .start_tick
                            .saturating_add(*steps as u32 * TICKS_PER_STEP)
                    })
            })
            .max()
            .unwrap_or(TICKS_PER_BAR)
            .max(TICKS_PER_BAR);
        content_end.div_ceil(TICKS_PER_BAR) * TICKS_PER_BAR
    }

    pub fn placement_covering(&self, pattern: usize, tick: u32) -> Option<PatternPlacement> {
        self.playlist.iter().copied().find(|placement| {
            if placement.pattern as usize != pattern {
                return false;
            }
            let length = self.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
            tick >= placement.start_tick && tick < placement.start_tick.saturating_add(length)
        })
    }

    /// Every destination the selected clip can address: the channel's own
    /// effect chain plus every bus's, because a clip's automation is allowed
    /// to reach the buses its channel feeds into.
    ///
    /// Generators are deliberately absent. They ship whole parameter structs
    /// rather than descriptor-addressed params, so there is nothing to name
    /// yet (`docs/plans/buffer-implementation/02-control-and-modulation.md`,
    /// build order step 2).
    pub fn automation_destinations(&self) -> Vec<(ParamAddr, String, &'static ParamDescriptor)> {
        let mut rows = Vec::new();
        let channel = EffectTarget::Channel(self.selected as u8);
        if let Some(state) = self.channels.get(self.selected) {
            // The generator first: it is the top of the signal path, and it is
            // what most channels have instead of an effect chain.
            let generator = state.generator_params();
            let device = state.name.clone();
            for descriptor in generator.kind().descriptors() {
                rows.push((
                    ParamAddr {
                        scope: channel,
                        owner: ParamOwner::Source,
                        param: descriptor.id,
                    },
                    device.clone(),
                    descriptor,
                ));
            }
            for (slot, effect) in state.effects.iter().enumerate() {
                let kind = effect.kind();
                let device = format!("{} {}", kind.label(), slot + 1);
                for descriptor in kind.descriptors() {
                    rows.push((
                        ParamAddr::effect(channel, effect.id, descriptor.id),
                        device.clone(),
                        descriptor,
                    ));
                }
            }
        }
        for (index, bus) in self.buses.iter().enumerate() {
            for (slot, effect) in bus.effects.iter().enumerate() {
                let kind = effect.kind();
                let device = format!("{} · {} {}", bus.bus.name, kind.label(), slot + 1);
                for descriptor in kind.descriptors() {
                    rows.push((
                        ParamAddr::effect(
                            EffectTarget::Bus(index as u8),
                            effect.id,
                            descriptor.id,
                        ),
                        device.clone(),
                        descriptor,
                    ));
                }
            }
        }
        rows
    }

    pub fn automation_lanes(&self) -> Option<&Vec<AutomationLane>> {
        self.channels
            .get(self.selected)?
            .automation
            .get(self.current_pattern)
    }

    pub fn automation_lane(&self) -> Option<&AutomationLane> {
        let target = self.automation_target.get()?;
        self.automation_lanes()?
            .iter()
            .find(|lane| lane.target == target)
    }

    pub fn automation_lane_mut(&mut self) -> Option<&mut AutomationLane> {
        let target = self.automation_target.get()?;
        let pattern = self.current_pattern;
        self.channels
            .get_mut(self.selected)?
            .automation
            .get_mut(pattern)?
            .iter_mut()
            .find(|lane| lane.target == target)
    }

    /// Descriptor for the currently shown lane, used to turn normalized
    /// breakpoints back into the natural units the readout displays.
    pub fn automation_descriptor(&self) -> Option<&'static ParamDescriptor> {
        let target = self.automation_target.get()?;
        match target.owner {
            ParamOwner::Source => {
                let EffectTarget::Channel(channel) = target.scope else {
                    return None;
                };
                self.channels
                    .get(channel as usize)?
                    .generator_params()
                    .kind()
                    .descriptor(target.param)
            }
            // A route amount is not in the device's table; its descriptor
            // belongs to the route.
            ParamOwner::SourceRoute { .. } => {
                let EffectTarget::Channel(channel) = target.scope else {
                    return None;
                };
                self.channels
                    .get(channel as usize)?
                    .generator_params()
                    .kind()
                    .route_descriptor(target.param)
            }
            ParamOwner::Effect { device } => {
                let effects = match target.scope {
                    EffectTarget::Channel(channel) => &self.channels.get(channel as usize)?.effects,
                    EffectTarget::Bus(bus) => &self.buses.get(bus as usize)?.effects,
                };
                let slot = mooloop_core::device_slot(effects, device)?;
                effects[slot].kind().descriptor(target.param)
            }
            ParamOwner::Modulator { .. } | ParamOwner::Strip => None,
        }
    }

    /// Replaces the whole note selection with exactly one note (or clears it
    /// when `id` is `None`). Every single-note interaction -- rack step
    /// edits, a plain piano-roll click, create/move/resize/velocity -- goes
    /// through this, so a Shift-click or Select All selection never lingers
    /// once the user touches a single note through any other gesture.
    pub fn select_note(&mut self, id: Option<NoteId>) {
        self.selected_note_id = id;
        self.selected_note_ids.clear();
        self.selected_note_ids.extend(id);
    }

    /// Adds or removes one note from the selection (Shift/Ctrl-click).
    pub fn toggle_note_selection(&mut self, id: NoteId) {
        if !self.selected_note_ids.remove(&id) {
            self.selected_note_ids.insert(id);
        }
        self.selected_note_id = (self.selected_note_ids.len() == 1)
            .then(|| *self.selected_note_ids.iter().next().unwrap());
    }

    /// Selects every note in `channel`'s current pattern (Ctrl+A).
    pub fn select_all_notes(&mut self, channel: usize) {
        let pattern = self.current_pattern;
        let length_ticks = self.pattern_lengths[pattern] as u32 * TICKS_PER_STEP;
        self.selected_note_ids = self.channels[channel].notes[pattern]
            .iter()
            .filter(|note| note.start_tick < length_ticks)
            .map(|note| note.id)
            .collect();
        self.selected_note_id = (self.selected_note_ids.len() == 1)
            .then(|| *self.selected_note_ids.iter().next().unwrap());
    }

    /// Drops ids that no longer exist from the selection, e.g. after a batch
    /// removal elsewhere in the rack or piano roll.
    /// Drops one note from the selection, leaving the rest alone. The
    /// subtract-from-selection role needs this to be idempotent: dragging a
    /// remove-marquee back and forth over a note must not re-add it, which a
    /// toggle would.
    pub fn remove_note_from_selection(&mut self, id: NoteId) {
        self.selected_note_ids.remove(&id);
        self.selected_note_id = (self.selected_note_ids.len() == 1)
            .then(|| *self.selected_note_ids.iter().next().unwrap());
    }

    pub fn prune_note_selection(&mut self, removed: &[NoteId]) {
        self.selected_note_ids.retain(|id| !removed.contains(id));
        if self
            .selected_note_id
            .is_some_and(|id| removed.contains(&id))
        {
            self.selected_note_id = None;
        }
    }

    /// The chain the device rack is currently editing, channel or bus.
    pub fn effect_chain(&self) -> Option<&Vec<EffectSlotState>> {
        self.effect_chain_of(self.effect_target)
    }

    /// The chain of `target`, whether or not the rack is pointed at it.
    pub fn effect_chain_of(&self, target: EffectTarget) -> Option<&Vec<EffectSlotState>> {
        match target {
            EffectTarget::Channel(index) => self.channels.get(index as usize).map(|c| &c.effects),
            EffectTarget::Bus(index) => self.buses.get(index as usize).map(|b| &b.effects),
        }
    }


    pub fn effect_chain_mut(&mut self) -> Option<&mut Vec<EffectSlotState>> {
        Some(self.effect_chain_parts_mut()?.0)
    }

    /// The chain the rack is pointed at, together with its identity mint.
    ///
    /// The pair rather than the chain alone, because inserting a device is
    /// two facts -- where it goes and which device it is -- and they live in
    /// two fields of the same owner.
    pub fn effect_chain_parts_mut(
        &mut self,
    ) -> Option<(&mut Vec<EffectSlotState>, &mut u32)> {
        match self.effect_target {
            EffectTarget::Channel(index) => self
                .channels
                .get_mut(index as usize)
                .map(|c| (&mut c.effects, &mut c.next_device_id)),
            EffectTarget::Bus(index) => self
                .buses
                .get_mut(index as usize)
                .map(|b| (&mut b.effects, &mut b.next_device_id)),
        }
    }

    /// Follow a channel edit through everything on *this* side that names a
    /// channel by position.
    ///
    /// `Project::rescope_after` covers the durable half -- routes, lanes and
    /// Aux In subscriptions, all of which are saved with the song. This is
    /// the session half, which is not saved and so was never in that walk:
    /// six things here are keyed by a channel index, or by an `EffectTarget`
    /// holding one.
    ///
    /// It is not a bug the reorder introduced. An insert or a delete moves
    /// every channel past it too, and has silently mis-keyed all six since
    /// they were written; the reorder is just the first edit that makes it
    /// visible, because it is the first one a user performs *while looking
    /// at* the device the labels belong to.
    ///
    /// [`Self::effect_target`] is deliberately not here: `replace_project`
    /// re-points it at the project's own selected channel on every install,
    /// and the edit sets that to wherever the moved channel landed.
    pub fn rescope_after(&mut self, edit: mooloop_core::ChannelEdit) {
        fn moved(edit: mooloop_core::ChannelEdit, target: EffectTarget) -> Option<EffectTarget> {
            match target {
                EffectTarget::Channel(channel) => {
                    edit.channel(channel).map(EffectTarget::Channel)
                }
                // A bus exists independently of which channels feed it, and
                // is untouched for the same reason `ChannelEdit::address`
                // leaves a bus scope alone.
                EffectTarget::Bus(_) => Some(target),
            }
        }

        self.selected_device = self
            .selected_device
            .and_then(|(target, device)| Some((moved(edit, target)?, device)));
        self.selected_source = self.selected_source.and_then(|target| moved(edit, target));
        self.automation_target
            .set(self.automation_target.get().and_then(|addr| edit.address(addr)));
        self.pending_preset_save = match self.pending_preset_save {
            Some(PresetSaveTarget::Effect { target, device }) => {
                moved(edit, target).map(|target| PresetSaveTarget::Effect { target, device })
            }
            other => other,
        };
        self.effect_preset_names = self
            .effect_preset_names
            .drain()
            .filter_map(|((target, device), name)| {
                Some(((moved(edit, target)?, device), name))
            })
            .collect();
        self.source_preset_names = self
            .source_preset_names
            .drain()
            .filter_map(|(channel, name)| Some((edit.channel(channel)?, name)))
            .collect();
    }

    /// Let go of `device` everywhere on this side that could still be naming
    /// it: the channel's routes, every lane in every pattern, the lane the
    /// editor is showing, a save dialog left open on it, and the preset label
    /// its row was wearing.
    ///
    /// Called on removal and on nothing else. A reorder or an insert used to
    /// run a permutation over all five; none of them can observe one now,
    /// because none of them holds a position.
    pub fn forget_device(&mut self, target: EffectTarget, device: DeviceId) {
        let channels: &mut [ChannelState] = match target {
            EffectTarget::Channel(channel) => match self.channels.get_mut(channel as usize) {
                Some(channel) => std::slice::from_mut(channel),
                None => &mut [],
            },
            // A bus chain can be automated from any channel's clip.
            EffectTarget::Bus(_) => &mut self.channels,
        };
        for channel in channels {
            channel.modulation.forget_device(target, device);
            for lanes in &mut channel.automation {
                drop_lanes_for_device(lanes, target, device);
            }
        }
        if self.automation_target.get().is_some_and(|shown| {
            shown.scope == target && shown.device() == Some(device)
        }) {
            self.automation_target.set(None);
        }
        if self.pending_preset_save
            == Some(PresetSaveTarget::Effect { target, device })
        {
            self.pending_preset_save = None;
        }
        self.effect_preset_names.remove(&(target, device));
        if self.selected_device == Some((target, device)) {
            self.selected_device = None;
        }
    }

    /// The preset `device` of `target` was last loaded from or saved as.
    ///
    /// Deliberately kept through a knob move: the label says where these
    /// settings came from, which stays true after they are adjusted. It does
    /// not survive undo, which restores the project but not this map.
    ///
    /// Keyed by identity rather than by position, so a reorder no longer has
    /// to rebuild the whole map to keep a row wearing its own label.
    pub fn effect_preset_name(&self, target: EffectTarget, device: DeviceId) -> Option<&str> {
        self.effect_preset_names
            .get(&(target, device))
            .map(String::as_str)
    }

    /// Names `device` of `target` after a preset, or clears it when `name` is
    /// empty.
    pub fn set_effect_preset_name(&mut self, target: EffectTarget, device: DeviceId, name: &str) {
        if name.is_empty() {
            self.effect_preset_names.remove(&(target, device));
        } else {
            self.effect_preset_names
                .insert((target, device), name.to_string());
        }
    }

    /// The preset `channel`'s generator was last loaded from or saved as.
    ///
    /// Kept through a knob move for the same reason the effect label is: it
    /// says where these settings came from, which stays true after they are
    /// adjusted. Dropped when the channel changes source, since the device
    /// wearing it is then gone.
    pub fn source_preset_name(&self, channel: u8) -> Option<&str> {
        self.source_preset_names.get(&channel).map(String::as_str)
    }

    /// Names `channel`'s generator after a preset, or clears it when `name`
    /// is empty.
    pub fn set_source_preset_name(&mut self, channel: u8, name: &str) {
        if name.is_empty() {
            self.source_preset_names.remove(&channel);
        } else {
            self.source_preset_names.insert(channel, name.to_string());
        }
    }

    pub fn modulation_depth_for(&self, source_slot: u8, destination: ParamAddr) -> f32 {
        self.channels
            .get(self.selected)
            .and_then(|channel| {
                channel.modulation.routes.iter().flatten().find(|route| {
                    route.source_slot == source_slot && route.destination == destination
                })
            })
            .map_or(0.0, |route| route.depth)
    }

    pub fn modulation_envelope_mut(&mut self, slot: usize) -> Option<&mut ModEnvelopeParams> {
        let selected = self.selected;
        let params = self
            .channels
            .get_mut(selected)?
            .modulation
            .params_mut(slot)?;
        match params {
            ModulatorParams::Envelope(envelope) => Some(envelope),
            _ => None,
        }
    }

    /// The modulation shelf may address only the selected channel's own
    /// generator, inserts, and strip. Buses and another channel's controls
    /// stay deliberately outside this pass even though `ParamAddr` can name
    /// them, matching the per-channel routing policy.
    pub fn channel_modulation_destination(
        &self,
        address: ParamAddr,
    ) -> Option<(String, &'static ParamDescriptor)> {
        let EffectTarget::Channel(channel) = address.scope else {
            return None;
        };
        if channel as usize != self.selected {
            return None;
        }
        let state = self.channels.get(self.selected)?;
        match address.owner {
            ParamOwner::Source => state
                .generator_params()
                .kind()
                .descriptor(address.param)
                .map(|descriptor| (state.name.clone(), descriptor)),
            ParamOwner::Effect { device } => mooloop_core::device_slot(&state.effects, device)
                .and_then(|slot| {
                    let effect = &state.effects[slot];
                    effect
                        .kind()
                        .descriptor(address.param)
                        .map(|descriptor| (slot, effect.kind(), descriptor))
                })
                .map(|(slot, kind, descriptor)| {
                    (format!("{} {}", kind.label(), slot + 1), descriptor)
                }),
            ParamOwner::Strip => strip_descriptor(address.param)
                .map(|descriptor| ("Channel strip".to_string(), descriptor)),
            // Modulators are sources in this first UI pass, not destinations.
            // An instrument's own routes are not channel destinations either:
            // the shelf reaches a device's controls, and a route amount
            // belongs to the patch's internal modulation rather than to the
            // device's control surface.
            ParamOwner::Modulator { .. } | ParamOwner::SourceRoute { .. } => None,
        }
    }

    pub fn finish_modulation_edit(&mut self) -> Option<ProjectSnapshot> {
        let before = self.modulation_edit_before.take();
        let changed = std::mem::replace(&mut self.modulation_edit_changed, false);
        if changed {
            before
        } else {
            None
        }
    }

    /// Sequencer channels feeding `bus` directly. Buses routed into it are not
    /// counted: the number answers "what lands here", not "what reaches here".
    pub fn bus_feed_count(&self, bus: usize) -> usize {
        self.channels
            .iter()
            .filter(|channel| channel.bus as usize == bus)
            .count()
    }

    /// Retunes every tempo-synced effect to `bpm`, returning the parameter
    /// changes the engine has to be told about.
    ///
    /// The engine only ever receives resolved values -- a delay time in
    /// milliseconds, an LFO rate in hertz -- so nothing below this layer
    /// knows that a division exists. Which is why the return type names the
    /// parameter: two kinds of effect answer to a tempo change now, and a
    /// caller that assumed one id was writing a rate into a delay time.
    pub fn update_tempo_synced_effects(&mut self, bpm: f64) -> Vec<(EffectTarget, u8, u32, f32)> {
        let mut changes = Vec::new();
        for (channel, state) in self.channels.iter_mut().enumerate() {
            let target = EffectTarget::Channel(channel as u8);
            for (slot, effect) in state.effects.iter_mut().enumerate() {
                retune_effect(&mut effect.params, bpm, target, slot as u8, &mut changes);
            }
        }
        for (bus, state) in self.buses.iter_mut().enumerate() {
            let target = EffectTarget::Bus(bus as u8);
            for (slot, effect) in state.effects.iter_mut().enumerate() {
                retune_effect(&mut effect.params, bpm, target, slot as u8, &mut changes);
            }
        }
        changes
    }

    /// Installs a loaded or restored document as the live model.
    ///
    /// Decoding is the caller's: `samples` arrives already decoded so this
    /// never blocks on audio. A committed stretch is re-rendered rather than
    /// carried in the document, except where the identical buffer is already
    /// in hand.
    pub fn replace_project(&mut self, project: &Project, samples: &[Option<Arc<SampleData>>]) {
        self.source_revision = self.source_revision.wrapping_add(1);
        // Every baked commit currently in hand, found by what it was baked
        // from rather than by which channel is holding it.
        //
        // Re-rendering one is a couple of hundred milliseconds on the UI
        // thread, and this used to look only at `self.channels[index]` -- the
        // channel sitting in the same seat. That holds for an edit that
        // leaves the channel list alone and fails for every edit that does
        // not: undoing an insert or a delete moves everything past it along
        // by one, so each of those channels found a stranger in its seat and
        // re-rendered. Two channels of one-second audio measured 0.08 ms
        // aligned and 55 ms shifted, and it scales with both the channel
        // count and the sample length.
        //
        // A `Vec` rather than a map because `SampleCommit` holds floats and
        // cannot be hashed, and because the scan is a pointer comparison
        // against at most a few hundred entries -- which is nothing beside
        // the render it avoids.
        let baked: Vec<(&Arc<SampleData>, &mooloop_core::SampleCommit, Arc<SampleData>)> = self
            .channels
            .iter()
            .filter_map(|held| {
                Some((
                    held.sample_data.as_ref()?,
                    held.commit.as_deref()?,
                    held.committed_sample.clone()?,
                ))
            })
            .collect();
        let channels = project
            .channels
            .iter()
            .enumerate()
            .map(|(index, project_channel)| {
                let setup = &project_channel.setup;
                // One accessor a kind rather than one tuple arm a kind: the
                // shape was a five-tuple whose every arm restated the four
                // defaults it was not, which is a line of edit per synth per
                // synth added.
                let source = &setup.source;
                let sampler = source.sampler_state();
                let drum_params = source.drum_synth_state().map(|s| s.params).unwrap_or_default();
                let mono_params = source.mono_synth_state().map(|s| s.params).unwrap_or_default();
                let poly_params = source.poly_synth_state().map(|s| s.params).unwrap_or_default();
                let mlm1_params = source.mlm1_state().map(|s| s.params).unwrap_or_default();
                let mlp8_params = source.mlp8_state().map(|s| s.params).unwrap_or_default();
                let ds01_params = source.ds01_state().map(|s| s.params).unwrap_or_default();
                let aux_in_params = source.aux_in_state().map(|s| s.params).unwrap_or_default();
                let sample = sampler
                    .is_some()
                    .then(|| samples.get(index).cloned().flatten())
                    .flatten();
                let (sample_path, embedded) = match sampler.map(|state| &state.sample) {
                    Some(SampleReference::File { path, embedded }) => {
                        (Some(path.clone()), *embedded)
                    }
                    Some(SampleReference::Builtin { .. } | SampleReference::Empty) | None => {
                        (None, false)
                    }
                };
                // Only a legacy `Builtin` reference (a project saved before
                // the sampler stopped auto-loading a kick) substitutes the
                // cached default sample; `Empty` means genuinely no sample.
                let is_builtin = matches!(
                    sampler.map(|state| &state.sample),
                    Some(SampleReference::Builtin { .. })
                );
                let missing = sample_path.is_some() && sample.is_none();
                let sample_name = if sampler.is_some() {
                    sample_path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            if is_builtin {
                                "default kick".to_string()
                            } else {
                                String::new()
                            }
                        })
                } else {
                    String::new()
                };
                // A committed stretch is re-rendered rather than reloaded:
                // the spec is length-determined, so the buffer that comes
                // back is the one that was baked, and the project never had
                // to carry the audio.
                //
                // Only when it has to, though. Undo and every other project
                // edit reinstall the whole document through here, and a
                // commit is a couple of hundred milliseconds of rendering per
                // channel -- paid on the UI thread, so it is a visible stall.
                // A buffer already in hand, baked from the same source under
                // the same spec, is the same buffer.
                let commit = sampler.and_then(|state| state.commit.clone());
                let committed = commit.as_ref().zip(sample.as_ref()).and_then(
                    |(commit, source)| {
                        baked
                            .iter()
                            .find(|(held_source, held_commit, _)| {
                                Arc::ptr_eq(held_source, source)
                                    && *held_commit == commit.as_ref()
                            })
                            .map(|(_, _, buffer)| buffer.clone())
                            .or_else(|| mooloop_dsp::commit::rerender_commit(source, commit))
                    },
                );
                let published = committed.as_ref().or(sample.as_ref());
                let waveform = published
                    .map(|sample| waveform_peaks(sample, WAVEFORM_BINS))
                    .unwrap_or_else(|| {
                        if is_builtin {
                            self.default_waveform.clone()
                        } else {
                            Vec::new()
                        }
                    });
                let description = published
                    .map(|sample| sample_description(sample))
                    .unwrap_or_else(|| {
                        if missing {
                            "Missing sample - load an audio file to relink".into()
                        } else if is_builtin {
                            self.default_sample_description.clone()
                        } else {
                            String::new()
                        }
                    });
                let duration = published
                    .map(|sample| sample_duration(sample))
                    .unwrap_or_else(|| {
                        if missing {
                            0.0
                        } else if is_builtin {
                            self.default_sample_duration
                        } else {
                            0.0
                        }
                    });
                let (can_previous, can_next) = sample_path
                    .as_ref()
                    .and_then(|path| {
                        sample_files_in_directory(path)
                            .ok()
                            .map(|files| (path, files))
                    })
                    .map(|(path, files)| {
                        let index = sample_index(path, &files);
                        (
                            index.is_some_and(|index| index > 0),
                            index.is_some_and(|index| index + 1 < files.len()),
                        )
                    })
                    .unwrap_or((false, false));
                ChannelState {
                    name: setup.channel.name.clone(),
                    kind: setup.channel.kind,
                    muted: setup.channel.muted,
                    volume: setup.channel.volume,
                    pan: setup.channel.pan,
                    params: sampler.map(|state| state.params).unwrap_or_default(),
                    drum_params,
                    mono_params,
                    poly_params,
                    mlm1_params,
                    mlp8_params,
                    ds01_params,
                    aux_in_params,
                    sample_name,
                    sample_description: description,
                    sample_duration: duration,
                    sample_path,
                    sample_embedded: embedded,
                    sample_data: sample,
                    committed_sample: committed,
                    commit,
                    slices: sampler.map(|state| state.slices.clone()).unwrap_or_default(),
                    waveform,
                    can_previous_sample: can_previous,
                    can_next_sample: can_next,
                    notes: project_channel.notes.clone(),
                    automation: project_channel.automation.clone(),
                    next_note_id: project_channel.next_note_id,
                    effects: setup.effects.clone(),
                    next_device_id: setup.next_device_id,
                    modulation: setup.modulation,
                    bus: setup.channel.bus,
                }
            })
            .collect::<Vec<_>>();

        self.buses = sanitize_bank(&project.buses);
        self.pattern_lengths = project
            .pattern_lengths
            .iter()
            .map(|length| *length as usize)
            .collect();
        self.pattern_names = vec![String::new(); self.pattern_lengths.len()];
        self.playlist = project.playlist.clone();
        self.loop_range = project.loop_range;
        self.song_mode = project.playback_mode == PlaybackMode::Song;
        self.current_pattern = project.current_pattern as usize;
        self.selected = project.selected_channel as usize;
        // Modulation source selection and assignment are session gestures,
        // never document state. A newly loaded project must start unarmed
        // even if it selects the same channel index as the previous one.
        self.modulation_ui_channel.set(None);
        // The engine's state is replaced wholesale by a load, and
        // `RenderState::load_project` installs its own compensation. Forget
        // what this side thinks was sent so the next reconcile re-derives
        // against the new project rather than trusting a plan for the old one.
        self.compensation_sent = mooloop_core::CompiledLatency::default();
        // Same for the console accumulators: `RenderState::load_project`
        // installs its own through `install_console`, so this side must
        // re-derive rather than trust a plan for the document that just left.
        self.console_sums_sent = [false; MAX_BUSES];
        // Same for the audio edges: `RenderState::load_project` compiles and
        // allocates its own, so this side must re-derive rather than trust a
        // plan for the document that just left.
        self.audio_graph_sent = mooloop_core::CompiledAudioGraph::default();
        // A load points the device rack back at a channel; the bus the
        // previous document had open means nothing in this one.
        self.effect_target = EffectTarget::Channel(project.selected_channel);
        self.selected_note_id = None;
        self.selected_note_ids.clear();
        self.channels = channels;
    }

    /// Points the armed modulation source at `destination` at `depth`.
    ///
    /// Returns what the rack did rather than deciding how to report it: a
    /// full matrix is a refusal the user has to be told about, and telling
    /// them is the view's job.
    pub fn arm_modulation_route(&mut self, destination: ParamAddr, depth: f32) -> ArmedRoute {
        let Some(source_slot) = self.modulation_armed_slot.get() else {
            return ArmedRoute::Unchanged;
        };
        let Some((_, descriptor)) = self.channel_modulation_destination(destination) else {
            return ArmedRoute::Unchanged;
        };
        let policy = ModDestinationDescriptor::for_param(descriptor);
        if !policy.allowed {
            return ArmedRoute::Unchanged;
        }
        let depth = policy.clamp_depth(depth);
        // Which kind of source is armed decides how the route is authored.
        let outlet = self.selected_channel_outlet(source_slot);
        // A slot in the outlet band that resolves to no outlet names nothing:
        // the generator was swapped while the gesture was armed. Refusing
        // here rather than falling through matters because the rack half
        // would refuse it too, and would call it a full matrix.
        if outlet.is_none() && mooloop_core::modulation::outlet_of_slot(source_slot).is_some() {
            return ArmedRoute::Unchanged;
        }
        let Some(channel) = self.channels.get_mut(self.selected) else {
            return ArmedRoute::Unchanged;
        };
        let default_polarity = match outlet {
            // An outlet takes the destination's own default, whatever shape
            // it declares, and the reason is that the two kinds of source
            // publish in different ranges.
            //
            // `ModPolarity` describes how a route reads a **rack module**,
            // which always emits `-1..1`: `Unipolar` lifts that to `0..1` so
            // a one-way module rests at the destination's base rather than
            // at its midpoint. An outlet publishes in its *declared* range,
            // and a unipolar one is already `0..1` -- so `Bipolar` is what
            // passes it through, resting at the base at zero and reaching
            // full depth at one. Lifting it again would make a Gate sit half
            // a depth above the base with nothing playing, and give it only
            // half the swing when something did.
            Some(_) => policy.default_polarity,
            None => match channel.modulation.params(source_slot as usize) {
                // Sources that only ever swing one way default to a unipolar
                // route, so their resting value is the destination's base.
                Some(ModulatorParams::Envelope(_)) => ModPolarity::Unipolar,
                Some(ModulatorParams::Random(random)) if !random.bipolar => ModPolarity::Unipolar,
                _ => policy.default_polarity,
            },
        };
        let current = channel
            .modulation
            .routes
            .iter()
            .flatten()
            .find(|route| route.source_slot == source_slot && route.destination == destination)
            .map(|route| route.depth);
        if current.is_some_and(|current| (current - depth).abs() < f32::EPSILON) {
            return ArmedRoute::Unchanged;
        }
        // An outlet route is authored complete: its id is already durable, so
        // there is no slot to stamp an identity out of.
        let authored = match outlet {
            Some(outlet) => {
                ModRoute::from_outlet(outlet.id, destination, depth, default_polarity)
            }
            None => ModRoute::to_slot(source_slot, destination, depth, default_polarity),
        };
        let Some(index) = channel.modulation.add_route(authored) else {
            // Both authored forms resolve: the module slot was checked above
            // and an outlet's locator is bounded arithmetic on an id this
            // generator publishes. So the only way the rack refuses is a full
            // matrix.
            return ArmedRoute::Full;
        };
        // The rack stamped the durable source id on the way in; that stamped
        // row is what travels, so the engine resolves the route against the
        // module the gesture meant rather than against a slot number.
        let Some(route) = channel.modulation.routes[index] else {
            return ArmedRoute::Unchanged;
        };
        self.modulation_edit_changed = true;
        ArmedRoute::Added(route)
    }

    /// Depth the armed source drives each destination in `descriptors` at.
    ///
    /// Indexed by descriptor id, so the result is as long as the table's
    /// highest id rather than its length: the view hands the whole array to
    /// one control row.
    pub fn destination_depths(
        &self,
        armed: Option<u8>,
        descriptors: &[ParamDescriptor],
        address: impl Fn(u32) -> ParamAddr,
    ) -> Vec<f32> {
        let mut depths = vec![0.0; descriptor_slots(descriptors)];
        for descriptor in descriptors {
            depths[descriptor.id as usize] = armed.map_or(0.0, |slot| {
                self.modulation_depth_for(slot, address(descriptor.id))
            });
        }
        depths
    }

    /// Live modulation offset currently applied to each destination in
    /// `descriptors`, from the last outputs read off the engine.
    pub fn destination_offsets(
        &self,
        descriptors: &[ParamDescriptor],
        address: impl Fn(u32) -> ParamAddr,
    ) -> Vec<f32> {
        let mut offsets = vec![0.0; descriptor_slots(descriptors)];
        let Some(channel) = self.channels.get(self.selected) else {
            return offsets;
        };
        // The engine publishes one flat row a block; a route reads it as two
        // halves, because they are captured at different rates. Split once
        // here rather than per descriptor.
        let outputs = self.modulation_outputs.get();
        let (modulators, outlets) = outputs.split_at(MAX_MODULATORS_PER_CHANNEL);
        let sources = mooloop_core::modulation::ControlSources {
            modulators: modulators.try_into().expect("the rack's half"),
            outlets: outlets.try_into().expect("the outlet band"),
        };
        for descriptor in descriptors {
            let policy = ModDestinationDescriptor::for_param(descriptor);
            offsets[descriptor.id as usize] =
                channel
                    .modulation
                    .offset_for(address(descriptor.id), sources, &policy);
        }
        offsets
    }

    /// Which buses `bus` may be routed to without closing a loop.
    pub fn allowed_destinations(&self, bus: usize) -> Vec<bool> {
        (0..self.buses.len())
            .map(|candidate| {
                candidate != bus && !would_create_cycle(&self.buses, bus as u8, candidate as u8)
            })
            .collect()
    }

    /// Opens a direct modulation-knob gesture against `snapshot`.
    ///
    /// Intermediate control updates still reach audio immediately; one
    /// release becomes one undoable route edit.
    pub fn begin_modulation_edit(&mut self, snapshot: ProjectSnapshot) {
        if self.modulation_edit_before.is_none() {
            self.modulation_edit_before = Some(snapshot);
            self.modulation_edit_changed = false;
        }
    }

    /// A channel and its decoded audio, for the clipboard.
    ///
    /// The sample travels with it so a paste never has to decode on the UI
    /// thread.
    pub fn channel_clipboard(
        &self,
        index: usize,
        bpm: i32,
        swing_percent: i32,
    ) -> Option<crate::channel::ChannelClipboard> {
        let mut project = self.project_snapshot(bpm, swing_percent);
        crate::project::normalize_project_pattern_banks(&mut project);
        Some(crate::channel::ChannelClipboard {
            channel: project.channels.get(index)?.clone(),
            sample: self.sample_snapshots().get(index)?.clone(),
        })
    }
}

/// One effect's answer to a tempo change, if it has one.
///
/// Free with the `EffectParams` match rather than a trait, because there are
/// two and the third would want to be visible here rather than opted into
/// somewhere else.
fn retune_effect(
    params: &mut EffectParams,
    bpm: f64,
    target: EffectTarget,
    slot: u8,
    changes: &mut Vec<(EffectTarget, u8, u32, f32)>,
) {
    match params {
        EffectParams::Delay(delay) if delay.tempo_sync => {
            delay.time_ms = delay.time_division.time_ms(bpm);
            changes.push((target, slot, DELAY_PARAM_TIME_MS, delay.time_ms));
        }
        EffectParams::Modulation(modulation) if modulation.tempo_sync => {
            modulation.rate_hz = modulation.synced_rate_hz(bpm);
            changes.push((target, slot, MODULATION_PARAM_RATE_HZ, modulation.rate_hz));
        }
        _ => {}
    }
}

#[cfg(test)]
mod commit_reuse_tests {
    use std::sync::Arc;

    use mooloop_core::{Project, ProjectChannel, SampleCommit, StretchMode};
    use mooloop_dsp::SampleData;

    use super::Session;

    fn sample(seed: f32) -> Arc<SampleData> {
        Arc::new(SampleData {
            frames: (0..24_000)
                .map(|index| {
                    let phase = index as f32 / 48_000.0 * (220.0 + seed) * std::f32::consts::TAU;
                    [phase.sin() * 0.8, (phase * 1.5).sin() * 0.8]
                })
                .collect(),
            sample_rate: 48_000,
            root_note: 60,
        })
    }

    fn committed(channels: usize) -> (Project, Vec<Option<Arc<SampleData>>>) {
        let mut project = Project {
            pattern_lengths: vec![64],
            ..Project::default()
        };
        project.channels.clear();
        let mut samples = Vec::new();
        for index in 0..channels {
            let mut channel = ProjectChannel::sampler(index, 1);
            if let Some(state) = channel.setup.source.sampler_state_mut() {
                state.commit = Some(Box::new(SampleCommit {
                    mode: StretchMode::Music,
                    ratio: 1.5,
                    grain: 40,
                    source_markers: Vec::new(),
                    source_start: 0.0,
                    source_end: 1.0,
                    source_loop_start: 0.0,
                    source_loop_end: 1.0,
                }));
            }
            project.channels.push(channel);
            samples.push(Some(sample(index as f32)));
        }
        (project, samples)
    }

    /// A baked commit is found by what it was baked from, not by which seat
    /// its channel is sitting in.
    ///
    /// Re-rendering one costs a couple of hundred milliseconds on the UI
    /// thread, so the buffer already in hand is reused -- and every edit that
    /// changes the channel list moves channels into different indices. This
    /// installs the same channels one seat along, the way undoing an insert
    /// does, and asserts the buffers are the same allocations rather than
    /// equal ones: a fresh render would be a new `Arc`, and would have cost
    /// the stall this test exists to prevent.
    #[test]
    fn a_baked_commit_survives_the_channels_moving_along_by_one() {
        let (project, samples) = committed(3);
        let mut session = Session::default();
        session.replace_project(&project, &samples);

        let before: Vec<Arc<SampleData>> = session
            .channels
            .iter()
            .map(|channel| {
                channel
                    .committed_sample
                    .clone()
                    .expect("every channel was committed")
            })
            .collect();

        let mut shifted = project.clone();
        let mut shifted_samples = samples.clone();
        shifted.channels.insert(0, ProjectChannel::sampler(99, 1));
        shifted_samples.insert(0, None);
        session.replace_project(&shifted, &shifted_samples);

        for (index, original) in before.iter().enumerate() {
            let moved = session.channels[index + 1]
                .committed_sample
                .as_ref()
                .expect("the committed buffer moved with its channel");
            assert!(
                Arc::ptr_eq(original, moved),
                "channel {index} re-rendered its commit after moving one seat along"
            );
        }
    }
}
