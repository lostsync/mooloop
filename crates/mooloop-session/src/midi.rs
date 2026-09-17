//! The control thread's half of MIDI: what a forwarded control message does,
//! and where a recorded note lands.
//!
//! The engine routes notes and forwards everything else
//! (`mooloop_core::control`'s header says why). This module is the other end
//! of that: it holds the resolved routing the engine runs on, the pickup state
//! the bindings need, and the learn gesture, and it turns one forwarded
//! message into the same edits the interface would have made.
//!
//! Nothing here is MIDI-specific below [`Session::apply_control_input`]'s first
//! few lines. An OSC surface decodes to a `ControlValue`, and everything from
//! the binding onwards — takeover, transport, the parameter write — is already
//! shared.

use mooloop_core::{
    ChannelMidiInput, ControlBinding, ControlLearn, ControlMode, ControlOutcome, ControlTarget,
    EffectSlotState, EffectTarget, EngineCommand, MidiInputRoute, MidiKind, MidiMessage,
    MidiPortInfo, NoteEvent, ParamAddr, ParamDescriptor, ParamOwner, Project, Takeover,
    TransportControl, STRIP_PARAM_PAN, STRIP_PARAM_VOLUME,
};

use crate::roll::NoteEdit;
use crate::session::Session;

/// What one forwarded message asked for.
///
/// Returned rather than applied, because the caller owns the engine handle and
/// the undo history, and because a learn gesture ends in something the
/// interface has to redraw.
#[derive(Debug, Default)]
pub struct ControlEffects {
    /// Engine commands, in order.
    pub commands: Vec<EngineCommand>,
    /// Parameters that moved, so the interface can republish their controls.
    pub moved: Vec<ParamAddr>,
    /// A learn gesture completed and bound this control.
    pub learned: Option<ControlBinding>,
    /// Whether the document changed. A transport gesture does not dirty a
    /// document; a parameter move and a new binding both do.
    pub edits: bool,
}

/// One binding as a mapping list draws it: what it listens to, what it moves,
/// and the two things about it that are worth switching from a list.
///
/// A view rather than a reference into the map, because every field here is
/// *resolved* -- a target's name comes from the project, and whether a port is
/// missing comes from the ports that exist right now. Handing the interface a
/// `&ControlBinding` would mean resolving both again on the other side of the
/// boundary, in a layer that has neither.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlBindingView {
    /// Position in the map, and the handle every mutation takes.
    pub index: usize,
    pub source: String,
    pub target: String,
    pub mode: String,
    /// The takeover this binding runs, or `None` for a mode that has none.
    pub takeover: Option<Takeover>,
    pub inverted: bool,
    /// The named port is not plugged in, so this binding is inert this run.
    pub unresolved: bool,
    /// A transport gesture rather than a parameter.
    pub transport: bool,
}

impl ControlEffects {
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.moved.is_empty() && self.learned.is_none()
    }
}

impl Session {
    /// What each channel's MIDI input resolves to against the ports that exist
    /// now, in channel order, for [`EngineHandle::set_midi_routing`].
    ///
    /// [`EngineHandle::set_midi_routing`]: mooloop_engine::EngineHandle::set_midi_routing
    pub fn midi_routing(&self, ports: &[MidiPortInfo]) -> Vec<MidiInputRoute> {
        self.channels
            .iter()
            .map(|channel| channel.midi_input.resolve(ports))
            .collect()
    }

    /// The same resolution over a document, for the routing an install
    /// carries: at that point the session still holds the outgoing project's
    /// channels, and the routing has to be in the incoming one's order.
    pub fn project_midi_routing(project: &Project, ports: &[MidiPortInfo]) -> Vec<MidiInputRoute> {
        project
            .channels
            .iter()
            .map(|channel| channel.setup.channel.midi_input.resolve(ports))
            .collect()
    }

    /// One channel's stored MIDI input.
    pub fn channel_midi_input(&self, channel: usize) -> ChannelMidiInput {
        self.channels
            .get(channel)
            .map(|channel| channel.midi_input.clone())
            .unwrap_or_default()
    }

    /// Point a channel at an input. Returns whether anything changed, so a
    /// picker that republishes its own value does not dirty the document.
    pub fn set_channel_midi_input(&mut self, channel: usize, input: ChannelMidiInput) -> bool {
        let Some(state) = self.channels.get_mut(channel) else {
            return false;
        };
        if state.midi_input == input {
            return false;
        }
        state.midi_input = input;
        true
    }

    /// Write a note the engine captured into the pattern it was played over.
    ///
    /// The engine reports a note when its key comes up, already folded into
    /// `pattern` — so `start_tick` is a position in that pattern and needs no
    /// further arithmetic. `pattern` is the engine's, not `current_pattern`:
    /// the selection may have moved while the key was held, or a selection
    /// command may have been refused. What this adds is the pattern's own
    /// bounds: a note played over the loop point starts where it was played
    /// and is trimmed to the end rather than overhanging into nothing. A
    /// pattern the session does not have records nothing.
    pub fn record_note(
        &mut self,
        channel: usize,
        pattern: usize,
        note: u8,
        velocity: u8,
        start_tick: u32,
        length_ticks: u32,
    ) -> Option<NoteEdit> {
        let length = self.recorded_pattern_length(pattern);
        if self.channels.len() <= channel || length == 0 {
            return None;
        }
        let start_tick = start_tick.min(length.saturating_sub(1));
        let duration = length_ticks.max(1).min(length.saturating_sub(start_tick).max(1));
        let event = {
            let state = &mut self.channels[channel];
            let id = state.next_note_id;
            state.next_note_id = id.wrapping_add(1).max(1);
            let event = NoteEvent::new(id, start_tick, duration, note, velocity);
            state.notes[pattern].push(event);
            state.notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
            event
        };
        Some(NoteEdit {
            commands: vec![EngineCommand::UpsertNote {
                pattern: pattern as u8,
                channel: channel as u8,
                note: event,
            }],
            // The recorded channel's row, not the selected one: recording goes
            // where the input is routed, which need not be what is on screen.
            cells: None,
            notes: 1,
        })
    }

    /// One pattern's length in ticks, for recording, or 0 if there is no such
    /// pattern.
    fn recorded_pattern_length(&self, pattern: usize) -> u32 {
        self.pattern_lengths
            .get(pattern)
            .map_or(0, |steps| *steps as u32 * mooloop_core::TICKS_PER_STEP)
    }

    /// Act on one message the engine forwarded.
    ///
    /// Order matters and is deliberate: a learn gesture in progress consumes
    /// the message and binds it, because the whole point of learn is that the
    /// next control you touch is the one you meant — including a control that
    /// is already bound to something else, which is how a mapping is
    /// corrected. Only when nothing is learning does the map get the message.
    /// `playing` is the transport's present state, which only the caller
    /// draining the engine's position events knows. Passed in rather than
    /// mirrored here, because a second copy of "is it playing" is exactly the
    /// kind of duplicated fact that goes quietly wrong.
    pub fn apply_control_input(
        &mut self,
        message: &MidiMessage,
        ports: &[MidiPortInfo],
        playing: bool,
    ) -> ControlEffects {
        let mut effects = ControlEffects::default();
        if let Some(learn) = self.control_learn.clone() {
            if let Some(binding) = learn.resolve(message, ports) {
                self.control_learn = None;
                self.control_map.bind(binding.clone());
                self.control_state.resolve(&self.control_map, ports);
                effects.learned = Some(binding);
                effects.edits = true;
                return effects;
            }
            // A message that names no control -- a note-off, a transport
            // message -- leaves the gesture waiting rather than cancelling it.
            return effects;
        }

        // A transport message from outside is a transport gesture, and it
        // does not need a binding: a device that sends Start is asking for
        // exactly one thing and there is nothing to configure about it.
        //
        // Song Position is a transport message that is *not* one of the
        // gestures -- it is a seek -- so it is asked about separately. Folding
        // it into `external_transport` left it returning `None`, which took
        // the whole arm with it and made an external locate do nothing at all.
        if message.kind.is_transport() {
            if let Some(gesture) = external_transport(message) {
                effects
                    .commands
                    .extend(self.apply_transport_control(gesture, playing));
            }
            if let MidiKind::SongPosition { beats } = message.kind {
                // 96, the one resolution `Session::to_project` writes. Read
                // from the constant rather than spelled again here, so a
                // project that ever carries another one moves this with it.
                let ticks = MidiMessage::song_position_ticks(
                    beats,
                    mooloop_core::time::DEFAULT_PPQ,
                );
                effects.commands.push(EngineCommand::Seek {
                    tick: f64::from(ticks),
                });
            }
            return effects;
        }

        // Reading a target's present value is a walk, so it is done only for
        // the bindings that actually heard the message -- which `apply` sees
        // to by asking through this closure rather than up front.
        let map = std::mem::take(&mut self.control_map);
        let mut state = std::mem::take(&mut self.control_state);
        let outcomes = {
            let session = &*self;
            state.apply(&map, message, |target| match target {
                ControlTarget::Param(address) => session.param_normalized(*address).unwrap_or(0.0),
                ControlTarget::Transport(_) => 0.0,
            })
        };
        self.control_map = map;
        self.control_state = state;

        for (index, outcome) in outcomes {
            let Some(binding) = self.control_map.bindings.get(index) else {
                continue;
            };
            match (binding.target, outcome) {
                (ControlTarget::Transport(gesture), ControlOutcome::Fire) => {
                    effects
                        .commands
                        .extend(self.apply_transport_control(gesture, playing));
                }
                (ControlTarget::Param(address), ControlOutcome::Set(value)) => {
                    if let Some(command) = self.set_param_normalized(address, value) {
                        effects.commands.push(command);
                    }
                    // Where the parameter actually ended up, which is not
                    // always what was asked for: a stepped parameter
                    // quantizes. That read-back is what lets this binding
                    // notice later that something else has moved the
                    // parameter off it (`PickupState::wrote`), so a knob that
                    // has taken over stops dragging the value back from
                    // wherever the mouse just put it.
                    let observed = self.param_normalized(address).unwrap_or(value);
                    self.control_state.wrote(index, observed);
                    effects.moved.push(address);
                    effects.edits = true;
                }
                _ => {}
            }
        }
        effects
    }

    /// Begin a learn gesture: the next control touched binds to `target`.
    pub fn begin_control_learn(&mut self, target: ControlTarget, bind_port: bool) {
        self.control_learn = Some(ControlLearn { target, bind_port });
    }

    /// What a binding's target is called, for a mapping list.
    ///
    /// Resolved against the project every time it is asked rather than stored
    /// beside the binding: a channel gets renamed and a device gets moved, and
    /// a cached name would then be a second copy of a fact the project already
    /// holds. A target that names nothing here says so -- a binding onto a
    /// device that has been deleted is inert, not a mistake, and hiding it
    /// would leave a knob that does nothing with nothing on screen to explain
    /// it.
    pub fn control_target_label(&self, target: &ControlTarget) -> String {
        let address = match target {
            ControlTarget::Transport(gesture) => {
                return format!("Transport \u{b7} {}", gesture.label())
            }
            ControlTarget::Param(address) => *address,
        };
        let Some(descriptor) = self.param_descriptor(address) else {
            return "Unavailable parameter".to_owned();
        };
        let scope = match address.scope {
            EffectTarget::Channel(channel) => self
                .channels
                .get(usize::from(channel))
                .map(|state| state.name.clone()),
            EffectTarget::Bus(bus) => self
                .buses
                .get(usize::from(bus))
                .map(|setup| setup.bus.name.clone()),
        };
        // `param_descriptor` has already answered for this address, so a scope
        // that resolves to nothing here cannot happen -- but naming the owner
        // is worth more than a panic would be.
        let scope = scope.unwrap_or_else(|| "?".to_owned());
        let owner = match address.owner {
            ParamOwner::Source => match address.scope {
                EffectTarget::Channel(channel) => self
                    .channels
                    .get(usize::from(channel))
                    .map(|state| state.kind.label().to_owned())
                    .unwrap_or_else(|| "?".to_owned()),
                EffectTarget::Bus(_) => "?".to_owned(),
            },
            ParamOwner::Effect { device } => self
                .chain_for(address.scope)
                .and_then(|chain| {
                    let slot = mooloop_core::device_slot(chain, device)?;
                    Some(format!("{} {}", chain.get(slot)?.kind().label(), slot + 1))
                })
                .unwrap_or_else(|| "?".to_owned()),
            ParamOwner::Strip => "Strip".to_owned(),
            ParamOwner::Modulator { .. } | ParamOwner::SourceRoute { .. } => "?".to_owned(),
        };
        format!("{scope} \u{b7} {owner} \u{b7} {}", descriptor.name)
    }

    /// Every binding as a mapping list draws it, in map order.
    ///
    /// The index each row carries is its position in the map, which is the
    /// handle every mutation below takes. That makes the list and the
    /// mutations agree by construction, and it is why removal re-reads the
    /// list rather than shifting an index the caller is holding.
    pub fn control_binding_views(&self, ports: &[MidiPortInfo]) -> Vec<ControlBindingView> {
        let unresolved = self.control_map.unresolved(ports);
        self.control_map
            .bindings
            .iter()
            .enumerate()
            .map(|(index, binding)| ControlBindingView {
                index,
                source: binding.source.detail_label(),
                target: self.control_target_label(&binding.target),
                mode: binding.mode.label().to_owned(),
                takeover: binding.mode.takeover(),
                inverted: binding.inverted(),
                unresolved: unresolved.contains(&index),
                transport: matches!(binding.target, ControlTarget::Transport(_)),
            })
            .collect()
    }

    /// One binding's target, by its position in the map. What "relearn this
    /// row" needs.
    pub fn control_binding_target(&self, index: usize) -> Option<ControlTarget> {
        self.control_map.bindings.get(index).map(|b| b.target)
    }

    /// Drop one binding by its position in the map.
    pub fn remove_control_binding(&mut self, index: usize, ports: &[MidiPortInfo]) -> bool {
        if index >= self.control_map.bindings.len() {
            return false;
        }
        self.control_map.bindings.remove(index);
        self.resolve_control_map(ports);
        true
    }

    /// Switch one absolute binding between pickup and jump. Silently does
    /// nothing for a binding with no takeover to switch, which is what the
    /// editor draws: the switch is not offered on a row that has none.
    pub fn set_control_binding_takeover(&mut self, index: usize, takeover: Takeover) -> bool {
        let Some(binding) = self.control_map.bindings.get_mut(index) else {
            return false;
        };
        let ControlMode::Absolute { takeover: current } = &mut binding.mode else {
            return false;
        };
        if *current == takeover {
            return false;
        }
        *current = takeover;
        // The control has to catch the parameter again: the rule it is caught
        // under has just changed underneath it.
        self.release_control_pickup();
        true
    }

    /// Point one binding's range forwards or backwards.
    pub fn set_control_binding_inverted(&mut self, index: usize, inverted: bool) -> bool {
        let Some(binding) = self.control_map.bindings.get_mut(index) else {
            return false;
        };
        if binding.inverted() == inverted {
            return false;
        }
        binding.set_inverted(inverted);
        self.release_control_pickup();
        true
    }

    pub fn cancel_control_learn(&mut self) {
        self.control_learn = None;
    }

    pub fn control_learn_target(&self) -> Option<ControlTarget> {
        self.control_learn.as_ref().map(|learn| learn.target)
    }

    /// Re-resolve every binding's port. Call it when a project loads and when
    /// the port list changes.
    pub fn resolve_control_map(&mut self, ports: &[MidiPortInfo]) {
        let map = std::mem::take(&mut self.control_map);
        self.control_state.resolve(&map, ports);
        self.control_map = map;
    }

    /// Forget every binding's pickup, because the values it was tracking may
    /// all have moved. Undo, project load, preset recall.
    pub fn release_control_pickup(&mut self) {
        self.control_state.release_all();
    }

    /// Forget one target's pickup, because the on-screen control moved it.
    pub fn release_control_pickup_for(&mut self, target: ControlTarget) {
        let map = std::mem::take(&mut self.control_map);
        self.control_state.release(&map, &target);
        self.control_map = map;
    }

    /// Carry out one transport gesture, whatever asked for it.
    ///
    /// One place, so a mapped pad, an external Start message, a shortcut and a
    /// toolbar button cannot come to disagree about what Play means.
    pub fn apply_transport_control(
        &mut self,
        gesture: TransportControl,
        playing: bool,
    ) -> Vec<EngineCommand> {
        match gesture {
            TransportControl::Play => vec![EngineCommand::Play],
            TransportControl::Stop => vec![EngineCommand::Stop],
            TransportControl::Pause => vec![EngineCommand::Pause],
            TransportControl::PlayPause => vec![if playing {
                EngineCommand::Pause
            } else {
                EngineCommand::Play
            }],
            TransportControl::ReturnToStart => vec![EngineCommand::Seek { tick: 0.0 }],
            TransportControl::ToggleRecord => {
                self.record_armed = !self.record_armed;
                vec![EngineCommand::SetRecordArmed(self.record_armed)]
            }
            TransportControl::ToggleLoop => {
                let enabled = !self.loop_range.enabled;
                self.set_loop_enabled(enabled).into_iter().collect()
            }
        }
    }

    /// Whether recording is armed.
    pub fn record_armed(&self) -> bool {
        self.record_armed
    }

    /// Arm or disarm recording from the interface.
    pub fn set_record_armed(&mut self, armed: bool) -> Option<EngineCommand> {
        if self.record_armed == armed {
            return None;
        }
        self.record_armed = armed;
        Some(EngineCommand::SetRecordArmed(armed))
    }
}

/// The transport gesture a system message asks for, if it is one.
///
/// MIDI's Stop is a pause -- it holds position, and Continue resumes from
/// where it stopped -- so it maps to [`TransportControl::Pause`] and not to
/// [`TransportControl::Stop`], which returns to the start. Getting that
/// backwards would make an external sequencer's stop button silently rewind
/// the song.
fn external_transport(message: &MidiMessage) -> Option<TransportControl> {
    match message.kind {
        MidiKind::Start => Some(TransportControl::Play),
        MidiKind::Continue => Some(TransportControl::Play),
        MidiKind::Stop => Some(TransportControl::Pause),
        // A song position is a seek, not a gesture; the caller adds it.
        MidiKind::SongPosition { .. } => None,
        _ => None,
    }
}

/// A parameter's present value and how to write it, resolved from its address.
///
/// Both halves in one place because they have to agree: a control that reads
/// one parameter and writes another is worse than one that does nothing. They
/// share `descriptor`, `natural` and one `match` over [`ParamOwner`], so a
/// fourth owner kind cannot be given a reader and left without a writer.
///
/// Every write below goes through the *same* project mutation and the same
/// `EngineCommand` the interface's own control produces. That is not a
/// coincidence to be preserved by care -- it is why this is worth having at
/// all, and why a knob on a desk and a knob on the screen cannot come to
/// disagree.
impl Session {
    /// The descriptor a parameter is held to, or `None` for an address that
    /// names nothing here.
    ///
    /// Modulator slots and a generator's internal routes are deliberately
    /// outside this pass, as they are outside the modulation shelf's: a
    /// route's amount belongs to a patch rather than to a device's control
    /// surface, and binding a knob to one is a question nobody has asked yet.
    pub fn param_descriptor(&self, address: ParamAddr) -> Option<&'static ParamDescriptor> {
        match address.owner {
            ParamOwner::Source => {
                let EffectTarget::Channel(channel) = address.scope else {
                    return None;
                };
                self.channels
                    .get(usize::from(channel))?
                    .generator_params()
                    .kind()
                    .descriptor(address.param)
            }
            ParamOwner::Effect { device } => {
                let chain = self.chain_for(address.scope)?;
                let slot = mooloop_core::device_slot(chain, device)?;
                chain.get(slot)?.kind().descriptor(address.param)
            }
            ParamOwner::Strip => mooloop_core::modulation::strip_descriptor(address.param),
            ParamOwner::Modulator { .. } | ParamOwner::SourceRoute { .. } => None,
        }
    }

    /// A parameter's present value in its natural units.
    pub fn param_natural(&self, address: ParamAddr) -> Option<f32> {
        match address.owner {
            ParamOwner::Source => {
                let EffectTarget::Channel(channel) = address.scope else {
                    return None;
                };
                self.channels
                    .get(usize::from(channel))?
                    .generator_params()
                    .get(address.param)
            }
            ParamOwner::Effect { device } => {
                let chain = self.chain_for(address.scope)?;
                let slot = mooloop_core::device_slot(chain, device)?;
                chain.get(slot)?.params.get(address.param)
            }
            ParamOwner::Strip => match (address.scope, address.param) {
                (EffectTarget::Channel(channel), STRIP_PARAM_VOLUME) => {
                    Some(self.channels.get(usize::from(channel))?.volume)
                }
                (EffectTarget::Channel(channel), STRIP_PARAM_PAN) => {
                    Some(self.channels.get(usize::from(channel))?.pan)
                }
                (EffectTarget::Bus(bus), STRIP_PARAM_VOLUME) => {
                    Some(self.buses.get(usize::from(bus))?.bus.volume)
                }
                (EffectTarget::Bus(bus), STRIP_PARAM_PAN) => {
                    Some(self.buses.get(usize::from(bus))?.bus.pan)
                }
                _ => None,
            },
            ParamOwner::Modulator { .. } | ParamOwner::SourceRoute { .. } => None,
        }
    }

    /// A parameter's present value, normalized against its descriptor.
    pub fn param_normalized(&self, address: ParamAddr) -> Option<f32> {
        let descriptor = self.param_descriptor(address)?;
        let natural = self.param_natural(address)?;
        Some(descriptor.to_normalized(natural))
    }

    /// Write a parameter from normalized control travel, returning the command
    /// the engine needs.
    ///
    /// `None` means nothing was written -- an address that names no parameter,
    /// or one whose owner this pass does not reach. A caller must not treat
    /// `None` as "written but no command needed"; the two writes that produce
    /// no command are not here.
    pub fn set_param_normalized(
        &mut self,
        address: ParamAddr,
        normalized: f32,
    ) -> Option<EngineCommand> {
        let descriptor = self.param_descriptor(address)?;
        let value = descriptor.from_normalized(normalized.clamp(0.0, 1.0));
        match address.owner {
            ParamOwner::Source => {
                let EffectTarget::Channel(channel) = address.scope else {
                    return None;
                };
                let index = usize::from(channel);
                let written = self
                    .channels
                    .get_mut(index)?
                    .set_generator_param(address.param, value)?;
                Some(EngineCommand::SetChannelGeneratorParam {
                    channel,
                    id: address.param,
                    value: written,
                })
            }
            ParamOwner::Effect { device } => {
                let chain = self.chain_for(address.scope)?;
                let slot = mooloop_core::device_slot(chain, device)?;
                let written = self
                    .chain_for_mut(address.scope)?
                    .get_mut(slot)?
                    .params
                    .set(address.param, value)?;
                Some(EngineCommand::SetEffectParam {
                    target: address.scope,
                    slot: slot as u8,
                    id: address.param,
                    value: written,
                })
            }
            ParamOwner::Strip => match (address.scope, address.param) {
                (EffectTarget::Channel(channel), STRIP_PARAM_VOLUME) => {
                    self.set_channel_volume(i32::from(channel), value)
                }
                (EffectTarget::Channel(channel), STRIP_PARAM_PAN) => {
                    self.set_channel_pan(i32::from(channel), value)
                }
                (EffectTarget::Bus(bus), STRIP_PARAM_VOLUME) => {
                    self.set_bus_volume(i32::from(bus), value)
                }
                (EffectTarget::Bus(bus), STRIP_PARAM_PAN) => {
                    self.set_bus_pan(i32::from(bus), value)
                }
                _ => None,
            },
            ParamOwner::Modulator { .. } | ParamOwner::SourceRoute { .. } => None,
        }
    }

    /// The effect chain a scope names.
    fn chain_for(&self, scope: EffectTarget) -> Option<&Vec<EffectSlotState>> {
        match scope {
            EffectTarget::Channel(channel) => {
                Some(&self.channels.get(usize::from(channel))?.effects)
            }
            EffectTarget::Bus(bus) => Some(&self.buses.get(usize::from(bus))?.effects),
        }
    }

    fn chain_for_mut(&mut self, scope: EffectTarget) -> Option<&mut Vec<EffectSlotState>> {
        match scope {
            EffectTarget::Channel(channel) => {
                Some(&mut self.channels.get_mut(usize::from(channel))?.effects)
            }
            EffectTarget::Bus(bus) => Some(&mut self.buses.get_mut(usize::from(bus))?.effects),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mooloop_core::{
        ControlMode, ControlSource, DeviceId, EffectTarget, MidiChannelFilter, MidiInputSource,
        MidiKind, MidiPortFilter, MidiPortId, MidiRouteSource, ParamCurve, Takeover,
        SYSTEM_CHANNEL, TICKS_PER_STEP,
    };

    fn ports() -> Vec<MidiPortInfo> {
        vec![MidiPortInfo {
            id: MidiPortId(0),
            name: "Launchkey MK3".to_owned(),
        }]
    }

    fn cc(controller: u8, value: u8) -> MidiMessage {
        MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: 0,
            kind: MidiKind::ControlChange { controller, value },
        }
    }

    fn system(kind: MidiKind) -> MidiMessage {
        MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: SYSTEM_CHANNEL,
            kind,
        }
    }

    const VOLUME: ParamAddr = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);

    /// A recorded note lands in the pattern at the position it was played and
    /// for the length it was held, on the channel the engine says -- not on
    /// the selected one, because recording follows the routing rather than
    /// the screen.
    #[test]
    fn a_recorded_note_lands_in_the_pattern_it_was_played_over() {
        let mut session = Session::default();
        session.channels.push(crate::channel::ChannelState::new(1));
        session.selected = 0;

        let edit = session
            .record_note(1, 0, 64, 90, TICKS_PER_STEP * 3, TICKS_PER_STEP * 2)
            .expect("a recorded note is an edit");
        assert_eq!(edit.notes, 1);
        let notes = &session.channels[1].notes[0];
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].note, 64);
        assert_eq!(notes[0].velocity, 90);
        assert_eq!(notes[0].start_tick, TICKS_PER_STEP * 3);
        assert_eq!(notes[0].duration_ticks, TICKS_PER_STEP * 2);
        assert!(session.channels[0].notes[0].is_empty());
        assert!(matches!(
            edit.commands[0],
            EngineCommand::UpsertNote { channel: 1, pattern: 0, .. }
        ));
    }

    /// A note lands in the pattern the engine folded it into, whatever the
    /// selection is by the time it arrives, and is trimmed to that pattern's
    /// length rather than the selected one's.
    #[test]
    fn a_recorded_note_lands_in_the_pattern_the_engine_names() {
        let mut session = Session::default();
        session.add_pattern().expect("a second pattern");
        session.pattern_lengths[0] = 4;
        assert_eq!(session.current_pattern, 1);

        let edit = session
            .record_note(0, 0, 60, 100, TICKS_PER_STEP * 2, TICKS_PER_STEP * 8)
            .expect("an edit");
        assert!(session.channels[0].notes[1].is_empty(), "not the selected pattern");
        let note = session.channels[0].notes[0][0];
        assert_eq!(note.start_tick, TICKS_PER_STEP * 2);
        assert_eq!(note.duration_ticks, TICKS_PER_STEP * 2, "trimmed to pattern 0");
        assert!(matches!(
            edit.commands[0],
            EngineCommand::UpsertNote { pattern: 0, .. }
        ));

        // A pattern the session does not have is refused rather than guessed.
        assert!(session.record_note(0, 7, 60, 100, 0, 1).is_none());
    }

    /// A note held past the end of the pattern is trimmed to it rather than
    /// overhanging into nothing, and one played on the last tick still starts
    /// inside the pattern.
    #[test]
    fn a_recorded_note_is_trimmed_to_the_pattern() {
        let mut session = Session::default();
        let length = session.pattern_lengths[0] as u32 * TICKS_PER_STEP;

        session
            .record_note(0, 0, 60, 100, length - TICKS_PER_STEP, TICKS_PER_STEP * 8)
            .expect("an edit");
        let note = session.channels[0].notes[0][0];
        assert_eq!(note.start_tick, length - TICKS_PER_STEP);
        assert_eq!(note.duration_ticks, TICKS_PER_STEP);

        session.record_note(0, 0, 62, 100, length * 2, 4).expect("an edit");
        let note = session.channels[0].notes[0]
            .iter()
            .find(|note| note.note == 62)
            .expect("the second note");
        assert_eq!(note.start_tick, length - 1, "clamped inside the pattern");
        assert!(note.duration_ticks >= 1);
    }

    /// A channel's input resolves to what the engine runs on, in channel
    /// order, and a channel nobody has configured still follows the selection.
    #[test]
    fn routing_is_resolved_in_channel_order() {
        let mut session = Session::default();
        session.channels.push(crate::channel::ChannelState::new(1));
        assert!(session.set_channel_midi_input(
            1,
            ChannelMidiInput {
                source: MidiInputSource::Port("Launchkey MK3".to_owned()),
                channel: MidiChannelFilter::One(9),
            }
        ));
        // Setting the same thing twice is not an edit.
        assert!(!session.set_channel_midi_input(
            1,
            ChannelMidiInput {
                source: MidiInputSource::Port("Launchkey MK3".to_owned()),
                channel: MidiChannelFilter::One(9),
            }
        ));

        let routing = session.midi_routing(&ports());
        assert_eq!(routing.len(), 2);
        assert_eq!(routing[0].source, MidiRouteSource::FollowSelection);
        assert_eq!(routing[1].source, MidiRouteSource::Port(MidiPortId(0)));
        assert_eq!(routing[1].channel, MidiChannelFilter::One(9));

        // The document an install carries resolves to the same thing, which
        // is what lets the install hand the engine its routing.
        assert_eq!(
            Session::project_midi_routing(&session.project_snapshot(120, 50), &ports()),
            routing
        );
    }

    /// A bound CC moves the fader, through the same write the on-screen fader
    /// uses -- so the project holds the new value and the engine is told once.
    #[test]
    fn a_bound_cc_moves_the_parameter_and_the_project_holds_it() {
        let mut session = Session::default();
        let mut binding = ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(VOLUME),
        );
        binding.mode = ControlMode::Absolute {
            takeover: Takeover::Jump,
        };
        session.control_map.bind(binding);
        session.resolve_control_map(&ports());

        let effects = session.apply_control_input(&cc(7, 127), &ports(), false);
        assert_eq!(effects.moved, vec![VOLUME]);
        assert!(effects.edits);
        assert!(matches!(
            effects.commands[0],
            EngineCommand::SetChannelVolume { channel: 0, .. }
        ));
        // The descriptor's top, and the project is holding it.
        assert!(session.channels[0].volume > 1.0);
        assert_eq!(session.param_normalized(VOLUME), Some(1.0));

        // And back down.
        let effects = session.apply_control_input(&cc(7, 0), &ports(), false);
        assert_eq!(effects.moved, vec![VOLUME]);
        assert_eq!(session.channels[0].volume, 0.0);
    }

    /// Pickup, end to end: the knob has to reach the fader before the fader
    /// moves. Without it, touching a mapped knob after loading a song jumps
    /// every bound parameter to wherever the desk was left.
    #[test]
    fn a_pickup_binding_does_not_move_the_parameter_until_it_is_caught() {
        let mut session = Session::default();
        session.channels[0].volume = 1.0;
        session.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(VOLUME),
        ));
        session.resolve_control_map(&ports());

        // The fader is at 1.0 of a range topping out well above it, so a knob
        // at the bottom is below it and moves nothing.
        let effects = session.apply_control_input(&cc(7, 0), &ports(), false);
        assert!(effects.moved.is_empty());
        assert_eq!(session.channels[0].volume, 1.0);

        // All the way up crosses it, and from there it drives.
        let effects = session.apply_control_input(&cc(7, 127), &ports(), false);
        assert_eq!(effects.moved, vec![VOLUME]);
    }

    /// A transport message from outside starts and pauses the transport with
    /// no binding at all, and MIDI's Stop is a *pause* -- it holds position.
    /// Mapping it to Stop would make an external sequencer's stop button
    /// silently rewind the song.
    #[test]
    fn external_transport_messages_drive_the_transport() {
        let mut session = Session::default();
        let effects = session.apply_control_input(&system(MidiKind::Start), &ports(), false);
        assert_eq!(effects.commands, vec![EngineCommand::Play]);
        assert!(!effects.edits, "starting playback is not an edit");

        let effects = session.apply_control_input(&system(MidiKind::Stop), &ports(), true);
        assert_eq!(effects.commands, vec![EngineCommand::Pause]);

        let effects = session.apply_control_input(&system(MidiKind::Continue), &ports(), false);
        assert_eq!(effects.commands, vec![EngineCommand::Play]);

        // Song position seeks, in ticks: sixteen MIDI beats is four quarters,
        // which at 96 PPQ is tick 384.
        let effects = session.apply_control_input(
            &system(MidiKind::SongPosition { beats: 16 }),
            &ports(),
            false,
        );
        assert_eq!(effects.commands, vec![EngineCommand::Seek { tick: 384.0 }]);
    }

    /// One mapped pad drives play and pause, and which it does depends on the
    /// transport rather than on anything this side remembers.
    #[test]
    fn a_mapped_pad_toggles_the_transport() {
        let mut session = Session::default();
        session.control_map.bind(ControlBinding::new(
            ControlSource::Note {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                note: 36,
            },
            ControlTarget::Transport(TransportControl::PlayPause),
        ));
        session.resolve_control_map(&ports());
        let pad = MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: 0,
            kind: MidiKind::NoteOn {
                note: 36,
                velocity: 100,
            },
        };

        let effects = session.apply_control_input(&pad, &ports(), false);
        assert_eq!(effects.commands, vec![EngineCommand::Play]);
        let effects = session.apply_control_input(&pad, &ports(), true);
        assert_eq!(effects.commands, vec![EngineCommand::Pause]);
    }

    /// Record arm is a toggle wherever it is asked for, and the engine is told
    /// each time.
    #[test]
    fn record_arm_toggles_from_a_gesture_and_from_the_interface() {
        let mut session = Session::default();
        assert!(!session.record_armed());
        assert_eq!(
            session.apply_transport_control(TransportControl::ToggleRecord, false),
            vec![EngineCommand::SetRecordArmed(true)]
        );
        assert!(session.record_armed());
        assert_eq!(
            session.apply_transport_control(TransportControl::ToggleRecord, false),
            vec![EngineCommand::SetRecordArmed(false)]
        );
        // Setting it to what it already is says nothing to the engine.
        assert_eq!(session.set_record_armed(false), None);
        assert_eq!(
            session.set_record_armed(true),
            Some(EngineCommand::SetRecordArmed(true))
        );
    }

    /// Learn takes the next control touched, whatever it was already bound to
    /// -- that is how a mapping is corrected -- and consumes the message
    /// rather than also acting on it.
    #[test]
    fn learn_takes_the_next_control_and_does_not_also_act_on_it() {
        let mut session = Session::default();
        let mut binding = ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(VOLUME),
        );
        binding.mode = ControlMode::Absolute {
            takeover: Takeover::Jump,
        };
        session.control_map.bind(binding);
        session.resolve_control_map(&ports());

        let pan = ControlTarget::Param(ParamAddr::strip(
            EffectTarget::Channel(0),
            mooloop_core::STRIP_PARAM_PAN,
        ));
        session.begin_control_learn(pan, false);
        assert_eq!(session.control_learn_target(), Some(pan));

        // The same knob that drives volume. It binds, and the volume does not
        // move on the way past.
        let volume_before = session.channels[0].volume;
        let effects = session.apply_control_input(&cc(7, 127), &ports(), false);
        assert!(effects.learned.is_some());
        assert!(effects.commands.is_empty());
        assert_eq!(session.channels[0].volume, volume_before);
        assert_eq!(session.control_learn_target(), None);
        assert_eq!(session.control_map.bindings.len(), 1, "the knob was rebound");
        assert_eq!(session.control_map.binding_for(&pan).map(|b| b.source.label()), Some("CC 7".to_owned()));

        // And now it drives pan -- but it still has to catch it first. A
        // learn gesture binds a knob; it does not hand the knob's position to
        // the parameter, which would slam a centred pan hard left the instant
        // it was mapped to a knob somebody had left at zero.
        session.apply_control_input(&cc(7, 127), &ports(), false);
        assert_eq!(session.channels[0].pan, 0.0, "not caught yet");
        session.apply_control_input(&cc(7, 0), &ports(), false);
        assert!(
            session.channels[0].pan < -0.9,
            "crossing centre takes over"
        );
        session.apply_control_input(&cc(7, 127), &ports(), false);
        assert!(session.channels[0].pan > 0.9);
    }

    /// A learn waiting for a control is not cancelled by a message that names
    /// none.
    #[test]
    fn a_note_off_does_not_complete_a_learn() {
        let mut session = Session::default();
        session.begin_control_learn(ControlTarget::Param(VOLUME), false);
        let effects = session.apply_control_input(
            &MidiMessage {
                offset: 0,
                port: MidiPortId(0),
                channel: 0,
                kind: MidiKind::NoteOff { note: 36 },
            },
            &ports(),
            false,
        );
        assert!(effects.is_empty());
        assert_eq!(
            session.control_learn_target(),
            Some(ControlTarget::Param(VOLUME))
        );
        session.cancel_control_learn();
        assert_eq!(session.control_learn_target(), None);
    }

    /// A binding reaches a device parameter, not only the strip, and the
    /// descriptor's own clamp is what gets stored.
    #[test]
    fn a_binding_reaches_an_effect_parameter() {
        let mut session = Session::default();
        let mut slot = mooloop_core::EffectSlotState::of_kind(mooloop_core::EffectKind::Filter);
        slot.id = DeviceId(0);
        session.channels[0].effects.push(slot);

        let descriptor = mooloop_core::EffectKind::Filter.descriptors()[0];
        let address = ParamAddr::effect(EffectTarget::Channel(0), DeviceId(0), descriptor.id);
        assert_eq!(
            session.param_descriptor(address).map(|d| d.id),
            Some(descriptor.id)
        );
        let command = session
            .set_param_normalized(address, 1.0)
            .expect("an effect parameter is written");
        assert!(matches!(
            command,
            EngineCommand::SetEffectParam { slot: 0, .. }
        ));
        assert_eq!(session.param_normalized(address), Some(1.0));

        // And the generator, which is the third owner kind this pass reaches.
        let generator = session.channels[0].generator_params().kind().descriptors()[0];
        let address = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: ParamOwner::Source,
            param: generator.id,
        };
        let command = session
            .set_param_normalized(address, 0.0)
            .expect("a generator parameter is written");
        assert!(matches!(
            command,
            EngineCommand::SetChannelGeneratorParam { channel: 0, .. }
        ));
        assert_eq!(session.param_normalized(address), Some(0.0));
    }

    /// An address this pass does not reach writes nothing rather than writing
    /// somewhere else.
    #[test]
    fn an_unreachable_address_writes_nothing() {
        let mut session = Session::default();
        let modulator = ParamAddr {
            scope: EffectTarget::Channel(0),
            owner: mooloop_core::ParamOwner::Modulator { slot: 0 },
            param: 0,
        };
        assert_eq!(session.param_descriptor(modulator), None);
        assert_eq!(session.param_normalized(modulator), None);
        assert_eq!(session.set_param_normalized(modulator, 1.0), None);

        // And a channel that does not exist.
        let missing = ParamAddr::strip(EffectTarget::Channel(200), STRIP_PARAM_VOLUME);
        assert_eq!(session.param_normalized(missing), None);
        assert_eq!(session.set_param_normalized(missing, 1.0), None);
    }

    /// A knob that has taken over stops taking over the moment the on-screen
    /// control is moved -- which is the whole of what pickup is for, and the
    /// half of it that a caught binding used to skip. Without this, a fader
    /// that had caught the value pulled it straight back to the fader's own
    /// position on its next message, undoing the mouse.
    #[test]
    fn an_on_screen_move_takes_a_caught_control_off_the_parameter() {
        let mut session = Session::default();
        session.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(VOLUME),
        ));
        session.resolve_control_map(&ports());

        // Catch the value: the fader sweeps past wherever the parameter is.
        session.apply_control_input(&cc(7, 0), &ports(), false);
        session.apply_control_input(&cc(7, 127), &ports(), false);
        assert_eq!(session.param_normalized(VOLUME), Some(1.0));
        // Caught: a small move now follows the fader.
        session.apply_control_input(&cc(7, 100), &ports(), false);
        let followed = session.param_normalized(VOLUME).expect("volume exists");
        assert!(followed < 1.0 && followed > 0.5, "followed to {followed}");

        // The mouse moves the same parameter somewhere else.
        session.set_param_normalized(VOLUME, 0.1);
        assert_eq!(session.param_normalized(VOLUME), Some(0.1));

        // The fader's next message must not snatch it back.
        let effects = session.apply_control_input(&cc(7, 101), &ports(), false);
        assert!(
            effects.moved.is_empty(),
            "a released control has to catch the value again"
        );
        assert_eq!(session.param_normalized(VOLUME), Some(0.1));

        // And it catches again on the way past.
        session.apply_control_input(&cc(7, 0), &ports(), false);
        assert_eq!(session.param_normalized(VOLUME), Some(0.0));
    }

    /// Following a control does not release it. The read-back is compared
    /// with what the parameter took, not with what was asked for, so a
    /// stepped parameter's quantization does not read as somebody else's
    /// edit -- which would have made pickup re-arm on every message and
    /// nothing follow anything.
    #[test]
    fn a_quantized_parameter_does_not_release_its_own_control() {
        let mut session = Session::default();
        // A parameter with a handful of positions, so that every write
        // quantizes and the value read back is not the value asked for.
        let stepped = session.channels[0]
            .generator_params()
            .kind()
            .descriptors()
            .iter()
            .find(|descriptor| matches!(descriptor.curve, ParamCurve::Stepped(n) if n > 4))
            .map(|descriptor| ParamAddr {
                scope: EffectTarget::Channel(0),
                owner: ParamOwner::Source,
                param: descriptor.id,
            })
            .expect("the default generator has a stepped parameter");
        // The premise the rest of this test rests on: asking for a position
        // between two detents does not land on it.
        session.set_param_normalized(stepped, 0.787);
        assert_ne!(session.param_normalized(stepped), Some(0.787));
        session.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(stepped),
        ));
        session.resolve_control_map(&ports());

        session.apply_control_input(&cc(7, 0), &ports(), false);
        session.apply_control_input(&cc(7, 127), &ports(), false);
        // Caught. Now walk it down message by message; every one must land.
        for value in [110u8, 100, 90, 80] {
            let effects = session.apply_control_input(&cc(7, value), &ports(), false);
            assert!(
                !effects.moved.is_empty(),
                "a caught control stayed caught at {value}"
            );
        }
    }

    /// A mapping list names its targets from the project, so renaming a
    /// channel renames the row -- which is the whole reason the label is
    /// resolved rather than stored.
    #[test]
    fn a_binding_row_names_its_target_from_the_project() {
        let mut session = Session::default();
        session.channels[0].name = "Bass".to_owned();
        session.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Named("Launchkey MK3".to_owned()),
                channel: MidiChannelFilter::One(2),
                controller: 7,
            },
            ControlTarget::Param(VOLUME),
        ));
        let mut pad = ControlBinding::new(
            ControlSource::Note {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                note: 36,
            },
            ControlTarget::Transport(TransportControl::PlayPause),
        );
        // What a learn gesture makes of a pad, written out here because this
        // one was not learned.
        pad.mode = ControlMode::Toggle;
        session.control_map.bind(pad);
        session.resolve_control_map(&ports());

        let rows = session.control_binding_views(&ports());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, "CC 7 \u{b7} ch 3 \u{b7} Launchkey MK3");
        assert_eq!(rows[0].target, "Bass \u{b7} Strip \u{b7} Volume");
        assert_eq!(rows[0].mode, "Absolute, pickup");
        assert_eq!(rows[0].takeover, Some(Takeover::Pickup));
        assert!(!rows[0].transport);
        assert!(!rows[0].unresolved);
        assert_eq!(rows[1].source, "Note 36 \u{b7} omni \u{b7} any port");
        assert_eq!(rows[1].target, "Transport \u{b7} Play/Pause");
        assert_eq!(rows[1].mode, "Toggle");
        // A toggle has no takeover, so the editor offers no switch for one.
        assert_eq!(rows[1].takeover, None);
        assert!(rows[1].transport);

        session.channels[0].name = "Sub".to_owned();
        assert_eq!(
            session.control_binding_views(&ports())[0].target,
            "Sub \u{b7} Strip \u{b7} Volume"
        );
    }

    /// A binding onto a device that has left says so rather than vanishing,
    /// and a binding whose keyboard is unplugged is marked rather than
    /// dropped.
    #[test]
    fn a_binding_row_reports_what_it_cannot_reach() {
        let mut session = Session::default();
        session.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Named("Faderfox".to_owned()),
                channel: MidiChannelFilter::Omni,
                controller: 1,
            },
            ControlTarget::Param(ParamAddr::effect(
                EffectTarget::Channel(0),
                DeviceId(99),
                0,
            )),
        ));
        session.resolve_control_map(&ports());

        let rows = session.control_binding_views(&ports());
        assert_eq!(rows[0].target, "Unavailable parameter");
        assert!(rows[0].unresolved, "Faderfox is not plugged in");
    }

    /// The editor's three mutations are by map position, and each one leaves
    /// the map and the resolved state agreeing.
    #[test]
    fn the_editor_edits_a_binding_by_its_position() {
        let mut session = Session::default();
        session.control_map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Param(VOLUME),
        ));
        session.resolve_control_map(&ports());

        assert_eq!(
            session.control_binding_target(0),
            Some(ControlTarget::Param(VOLUME))
        );
        assert_eq!(session.control_binding_target(1), None);

        assert!(session.set_control_binding_takeover(0, Takeover::Jump));
        assert!(
            !session.set_control_binding_takeover(0, Takeover::Jump),
            "setting a takeover it already has changes nothing"
        );
        assert_eq!(
            session.control_binding_views(&ports())[0].mode,
            "Absolute, jump"
        );

        assert!(session.set_control_binding_inverted(0, true));
        assert!(session.control_binding_views(&ports())[0].inverted);
        // Inversion is the range's ends, and a knob at the top now asks for
        // the bottom of the parameter.
        let effects = session.apply_control_input(&cc(7, 127), &ports(), false);
        assert_eq!(session.param_normalized(VOLUME), Some(0.0));
        assert!(effects.edits);

        assert!(session.remove_control_binding(0, &ports()));
        assert!(session.control_map.bindings.is_empty());
        assert!(!session.remove_control_binding(0, &ports()));
        // The resolved state was rebuilt with the map, so a message that used
        // to move the fader now moves nothing.
        let effects = session.apply_control_input(&cc(7, 0), &ports(), false);
        assert!(effects.is_empty());
    }
}
