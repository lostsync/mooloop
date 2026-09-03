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
    compile_bus_graph, sanitize_route, would_create_cycle, MASTER_BUS, MAX_BUSES,
    retarget_lanes, strip_descriptor, AutomationLane, BusSetup, Channel, ChannelSetup,
    ChannelSource, DeviceKind, DrumSynthParams, DrumSynthState, Ds01Params, Ds01State,
    EffectParams, EffectSlotState, EffectTarget, MlM1Params, MlM1State, MlP8Params, MlP8State,
    ModDestinationDescriptor, ModEnvelopeParams, ModPolarity, ModRoute, ModulatorParams,
    MonoSynthParams, MonoSynthState, NoteId, ParamAddr,
    ParamDescriptor, ParamOwner, PatternPlacement, PlaybackMode, PointId, PolySynthParams,
    PolySynthState, Project, ProjectChannel, SampleReference, SamplerParams, SamplerState,
    SlotRemap, MAX_MODULATORS_PER_CHANNEL, MAX_SWING_PERCENT, MIN_SWING_PERCENT, TICKS_PER_BAR,
    TICKS_PER_STEP,
};
use mooloop_dsp::SampleData;
use mooloop_project::PresetSummary;
use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Which preset kind a save dialog in flight is for.
#[derive(Clone, Copy)]
pub enum PresetSaveTarget {
    Generator,
    Channel,
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
    pub modulation_armed_slot: Cell<Option<u8>>,
    /// The selected channel's latest modulator outputs, refreshed from the
    /// engine on the pump tick. Held here rather than recomputed per knob
    /// so one read of the audio thread's cells feeds every destination.
    pub modulation_outputs: Cell<[f32; MAX_MODULATORS_PER_CHANNEL]>,
    /// Channel that owns the transient selection/assignment state. Changing
    /// channels clears both even when the new channel happens to occupy the
    /// same runtime slot.
    pub modulation_ui_channel: Cell<Option<usize>>,
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
    /// Mirror of the project's bus bank, master first. Always `MAX_BUSES`
    /// long, matching the engine's preallocated bank.
    pub buses: Vec<BusSetup>,
    pub pattern_lengths: Vec<usize>,
    pub pattern_names: Vec<String>,
    pub playlist: Vec<PatternPlacement>,
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
    pub pending_preset_save: Option<PresetSaveTarget>,
}

/// Bins the stored channel waveform is reduced to. A fixed overview; the
/// editor re-derives real detail for whatever range it is zoomed to.
pub const WAVEFORM_BINS: usize = 256;

/// Coerce a loaded bus bank to the fixed size the engine preallocates,
/// padding a short one and repairing any routing an older or hand-edited file
/// left illegal. Everything downstream can then index the bank directly.
///
/// Per-edge nonsense is fixed first, then the graph as a whole: a file whose
/// routing contains a loop is flattened to everything-to-master rather than
/// rejected, matching what the engine does with the same file.
fn normalized_buses(buses: &[BusSetup]) -> Vec<BusSetup> {
    let mut normalized: Vec<BusSetup> = (0..MAX_BUSES)
        .map(|index| match buses.get(index) {
            Some(setup) => {
                let mut setup = setup.clone();
                setup.bus.output = sanitize_route(index as u8, setup.bus.output);
                setup
            }
            None => BusSetup::new(index),
        })
        .collect();
    if compile_bus_graph(&normalized).is_none() {
        for setup in &mut normalized {
            setup.bus.output = MASTER_BUS;
        }
    }
    normalized
}

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
        channel.kind = kind;
        channel.name = match kind {
            DeviceKind::Sampler => format!("Sampler {}", index + 1),
            DeviceKind::DrumSynth => format!("Drum {}", index + 1),
            DeviceKind::MonoSynth => format!("Mono {}", index + 1),
            DeviceKind::PolySynth => format!("Poly {}", index + 1),
            DeviceKind::MlM1 => format!("ML-M1 {}", index + 1),
            DeviceKind::MlP8 => format!("ML-P8 {}", index + 1),
            DeviceKind::Ds01 => format!("DS-01 {}", index + 1),
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
                        ParamAddr::effect(channel, slot as u8, descriptor.id),
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
                            slot as u8,
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
            ParamOwner::Effect { slot } => {
                let effects = match target.scope {
                    EffectTarget::Channel(channel) => &self.channels.get(channel as usize)?.effects,
                    EffectTarget::Bus(bus) => &self.buses.get(bus as usize)?.effects,
                };
                effects.get(slot as usize)?.kind().descriptor(target.param)
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
        match self.effect_target {
            EffectTarget::Channel(index) => self.channels.get(index as usize).map(|c| &c.effects),
            EffectTarget::Bus(index) => self.buses.get(index as usize).map(|b| &b.effects),
        }
    }

    pub fn effect_chain_mut(&mut self) -> Option<&mut Vec<EffectSlotState>> {
        match self.effect_target {
            EffectTarget::Channel(index) => self
                .channels
                .get_mut(index as usize)
                .map(|c| &mut c.effects),
            EffectTarget::Bus(index) => self.buses.get_mut(index as usize).map(|b| &mut b.effects),
        }
    }

    /// Run one chain edit's permutation over everything on this side that
    /// names a slot in `target`'s chain: the channel's routes, every lane in
    /// every pattern, and the lane the editor is showing. The engine runs the
    /// same table for the same command, which is what keeps a route meaning
    /// the same knob on both sides after the rack is reordered.
    pub fn retarget_effect_slots(&mut self, target: EffectTarget, remap: &SlotRemap) {
        let channels: &mut [ChannelState] = match target {
            EffectTarget::Channel(channel) => match self.channels.get_mut(channel as usize) {
                Some(channel) => std::slice::from_mut(channel),
                None => &mut [],
            },
            // A bus chain can be automated from any channel's clip.
            EffectTarget::Bus(_) => &mut self.channels,
        };
        for channel in channels {
            channel.modulation.retarget_effect_slots(target, remap);
            for lanes in &mut channel.automation {
                retarget_lanes(lanes, target, remap);
            }
        }
        self.automation_target.set(
            self.automation_target
                .get()
                .and_then(|shown| remap.address(target, shown)),
        );
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
            ParamOwner::Effect { slot } => state
                .effects
                .get(slot as usize)
                .and_then(|effect| effect.kind().descriptor(address.param))
                .map(|descriptor| {
                    (
                        format!(
                            "{} {}",
                            state.effects[slot as usize].kind().label(),
                            slot + 1
                        ),
                        descriptor,
                    )
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

    /// Retunes every tempo-synced delay to `bpm`, returning the changes the
    /// engine has to be told about.
    pub fn update_tempo_synced_delay_times(&mut self, bpm: f64) -> Vec<(EffectTarget, u8, f32)> {
        let mut changes = Vec::new();
        for (channel, state) in self.channels.iter_mut().enumerate() {
            for (slot, effect) in state.effects.iter_mut().enumerate() {
                let EffectParams::Delay(params) = &mut effect.params else {
                    continue;
                };
                if params.tempo_sync {
                    params.time_ms = params.time_division.time_ms(bpm);
                    changes.push((
                        EffectTarget::Channel(channel as u8),
                        slot as u8,
                        params.time_ms,
                    ));
                }
            }
        }
        for (bus, state) in self.buses.iter_mut().enumerate() {
            for (slot, effect) in state.effects.iter_mut().enumerate() {
                let EffectParams::Delay(params) = &mut effect.params else {
                    continue;
                };
                if params.tempo_sync {
                    params.time_ms = params.time_division.time_ms(bpm);
                    changes.push((EffectTarget::Bus(bus as u8), slot as u8, params.time_ms));
                }
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
                        let held = self.channels.get(index).filter(|held| {
                            held.commit.as_ref() == Some(commit)
                                && held
                                    .sample_data
                                    .as_ref()
                                    .is_some_and(|held| Arc::ptr_eq(held, source))
                        });
                        held.and_then(|held| held.committed_sample.clone())
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
                    modulation: setup.modulation,
                    bus: setup.channel.bus,
                }
            })
            .collect::<Vec<_>>();

        self.buses = normalized_buses(&project.buses);
        self.pattern_lengths = project
            .pattern_lengths
            .iter()
            .map(|length| *length as usize)
            .collect();
        self.pattern_names = vec![String::new(); self.pattern_lengths.len()];
        self.playlist = project.playlist.clone();
        self.song_mode = project.playback_mode == PlaybackMode::Song;
        self.current_pattern = project.current_pattern as usize;
        self.selected = project.selected_channel as usize;
        // Modulation source selection and assignment are session gestures,
        // never document state. A newly loaded project must start unarmed
        // even if it selects the same channel index as the previous one.
        self.modulation_ui_channel.set(None);
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
        let Some(channel) = self.channels.get_mut(self.selected) else {
            return ArmedRoute::Unchanged;
        };
        let default_polarity = match channel.modulation.params(source_slot as usize) {
            // Sources that only ever swing one way default to a unipolar
            // route, so their resting value is the destination's base.
            Some(ModulatorParams::Envelope(_)) => ModPolarity::Unipolar,
            Some(ModulatorParams::Random(random)) if !random.bipolar => ModPolarity::Unipolar,
            _ => policy.default_polarity,
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
        let Some(index) = channel.modulation.add_route(ModRoute::to_slot(
            source_slot,
            destination,
            depth,
            default_polarity,
        )) else {
            // The armed slot was checked above, so the only way the rack
            // refuses is a full matrix.
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
        let outputs = self.modulation_outputs.get();
        for descriptor in descriptors {
            let policy = ModDestinationDescriptor::for_param(descriptor);
            offsets[descriptor.id as usize] =
                channel
                    .modulation
                    .offset_for(address(descriptor.id), &outputs, &policy);
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
}
