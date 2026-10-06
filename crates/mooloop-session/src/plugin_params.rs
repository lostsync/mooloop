//! A hosted plugin's parameters, as the session addresses and edits them
//! (`docs/plans/plugin-hosting/07-parameters-and-state.md`, MOO-82).
//!
//! **Two integers, never interchanged** (`AGENTS.md`, "Parameter identity
//! across the session boundary"). A plugin parameter's **id** is the
//! plugin's own `u32` -- any value, `4_000_000_000` included -- and it is what
//! a route, a lane and an engine command carry, in `ParamAddr.param` under
//! `ParamOwner::PluginParam { device }` (MOO-74). Its **index** is its
//! position in the slot's `PluginSlotState::params`, dense from zero, and it
//! is the only one of the two that may cross into Slint, whose `int` is an
//! `i32`: an id above `i32::MAX` would wrap negative on the way to the face
//! and come back naming another parameter, or none. So a face hands back an
//! index, [`Session::plugin_param_id`] turns it into the id here, on the
//! Rust side, and nothing casts one into the other.
//!
//! A plugin parameter has no `&'static` descriptor. Its range, its steps and
//! whether it takes a lane or a route come from the list the plugin reported
//! (`PluginParamInfo`), which the song keeps, so a missing plugin's lanes and
//! routes still read.

use mooloop_core::{
    device_slot, DeviceId, EffectParams, EffectTarget, EngineCommand, GeneratorParams,
    ModDestinationDescriptor, ParamAddr, ParamOwner, PluginParamInfo, PluginSlotId,
};

use crate::session::Session;

/// A normalized value in a plugin parameter's own plain units: linear over
/// its range, and on a whole position for a stepped one. The same law the
/// engine's control pass applies to a lane (`mooloop_dsp::HostedParam::plain`),
/// so a knob and a lane at one position send the plugin one value.
pub fn plugin_plain(info: &PluginParamInfo, normalized: f32) -> f64 {
    let value = info.min + f64::from(normalized.clamp(0.0, 1.0)) * (info.max - info.min);
    if info.stepped.is_some() {
        value.round()
    } else {
        value
    }
}

/// The inverse of [`plugin_plain`], for drawing a value on a knob.
pub fn plugin_normalized(info: &PluginParamInfo, plain: f64) -> f32 {
    let span = info.max - info.min;
    if span <= 0.0 {
        return 0.0;
    }
    ((plain - info.min) / span).clamp(0.0, 1.0) as f32
}

impl Session {
    /// The plugin slot of the device `device` on `scope`, if that device is
    /// a hosted plugin: a device on the chain, or the plugin instrument that
    /// is a channel's source, named by `source_device` (MOO-312). The two
    /// never share an id: a channel mints both from one counter.
    pub fn plugin_slot_of(&self, scope: EffectTarget, device: DeviceId) -> Option<PluginSlotId> {
        if let Some(slot) = self.plugin_source_slot_of(scope, device) {
            return Some(slot);
        }
        let effects = self.effect_chain_of(scope)?;
        match effects.get(device_slot(effects, device)?)?.params {
            EffectParams::Plugin(slot) => Some(slot),
            _ => None,
        }
    }

    /// The plugin slot of channel `scope`'s source, when `device` is the id
    /// its source slot holds and that source is a plugin. An id a replaced
    /// instrument held names nothing: its lanes and routes went with it
    /// (`Session::forget_replaced_source_device`).
    pub(crate) fn plugin_source_slot_of(&self, scope: EffectTarget, device: DeviceId) -> Option<PluginSlotId> {
        let EffectTarget::Channel(channel) = scope else {
            return None;
        };
        let channel = self.channels.get(usize::from(channel))?;
        if !device.is_assigned() || channel.source_device != device {
            return None;
        }
        match channel.generator {
            GeneratorParams::Plugin(slot) => Some(slot),
            _ => None,
        }
    }

    /// The selected channel's plugin instrument: its source slot's id and
    /// the plugin's slot. `None` when its source is not a plugin.
    pub fn plugin_source(&self) -> Option<(DeviceId, PluginSlotId)> {
        let channel = self.channels.get(self.selected)?;
        let scope = EffectTarget::Channel(u8::try_from(self.selected).ok()?);
        let slot = self.plugin_source_slot_of(scope, channel.source_device)?;
        Some((channel.source_device, slot))
    }

    /// What the song knows about the plugin parameter `address` names: the
    /// entry in its slot's list with that id. `None` for an address that is
    /// not a plugin parameter, and for one whose id the plugin's list no
    /// longer has -- which is kept, and reads as missing (MOO-74).
    pub fn plugin_param_info(&self, address: ParamAddr) -> Option<&PluginParamInfo> {
        let ParamOwner::PluginParam { device } = address.owner else {
            return None;
        };
        let slot = self.plugin_slot_of(address.scope, device)?;
        self.plugins
            .get(&slot)?
            .params
            .iter()
            .find(|info| info.id == address.param)
    }

    /// The modulation policy for `address`, native or plugin: the one
    /// question route arming asks before it authors a route. A plugin
    /// parameter's comes from its own flags
    /// ([`ModDestinationDescriptor::for_plugin_param`]), the same function
    /// the engine's control pass asks, so the two cannot disagree about
    /// whether a route is live.
    pub fn modulation_policy(&self, address: ParamAddr) -> Option<ModDestinationDescriptor> {
        if let ParamOwner::PluginParam { .. } = address.owner {
            let info = self.plugin_param_info(address)?;
            return Some(ModDestinationDescriptor::for_plugin_param(
                info.id,
                info.stepped.is_some(),
                info.modulatable,
            ));
        }
        self.modulation_destination(address)
            .map(|(_, descriptor)| ModDestinationDescriptor::for_param(descriptor))
    }

    /// Whether a new lane may be opened on `address`: for a generator
    /// parameter only when it names the kind its channel runs now, for a
    /// plugin parameter only when the song knows it and the plugin marks it
    /// automatable, and always for any other native destination. An existing
    /// lane is always reopened, missing parameter or not.
    ///
    /// A generator address naming another kind is inert (MOO-135). An old
    /// lane on one is kept and listed as missing so it can be removed
    /// (MOO-270), and picking that row must reopen it, never make a second
    /// inert lane in a pattern that has none.
    pub fn lane_allowed(&self, address: ParamAddr) -> bool {
        match address.owner {
            ParamOwner::PluginParam { .. } => self
                .plugin_param_info(address)
                .is_some_and(|info| info.automatable),
            ParamOwner::Source { kind } => {
                let EffectTarget::Channel(channel) = address.scope else {
                    return false;
                };
                self.channels
                    .get(channel as usize)
                    .is_some_and(|state| kind == Some(state.kind()))
            }
            _ => true,
        }
    }

    /// The id of parameter `index` in `slot`'s list: the one way a face's
    /// index becomes the id a command and an address carry.
    pub fn plugin_param_id(&self, slot: PluginSlotId, index: usize) -> Option<u32> {
        Some(self.plugins.get(&slot)?.params.get(index)?.id)
    }

    /// The index of parameter `id` in `slot`'s list: what a face is handed
    /// for it.
    pub fn plugin_param_index(&self, slot: PluginSlotId, id: u32) -> Option<usize> {
        self.plugins
            .get(&slot)?
            .params
            .iter()
            .position(|info| info.id == id)
    }

    /// Parameter `index`'s value now, in the plugin's plain units: what the
    /// plugin last reported or was sent, read from the live instance. `None`
    /// when the plugin is not hosted.
    pub fn plugin_param_value(&self, slot: PluginSlotId, index: usize) -> Option<f64> {
        self.plugin_rack.param_value(slot, index)
    }

    /// Set parameter `index` of the plugin on row `row` of the chain the rack
    /// is pointed at to `normalized` of its range: the face's edit, by index
    /// (step 08 draws the face). Returns the command to send, which is the
    /// ordinary `SetEffectParam` a native knob sends, carrying the plugin's
    /// id and its plain value -- there is no command of its own for a plugin.
    ///
    /// The plugin holds the value, so the song has it once the plugin's
    /// state is captured after it arrives. The rack counts the send as an
    /// edit of the plugin, closed when edits go quiet, and the pump records
    /// it as one undo step (`capture_plugin_edits`). Refused for a hidden
    /// parameter, which has no face.
    pub fn set_plugin_param(&mut self, row: usize, index: usize, normalized: f32) -> Option<EngineCommand> {
        let target = self.effect_target;
        let EffectParams::Plugin(slot) = self.effect_chain_of(target)?.get(row)?.params else {
            return None;
        };
        let info = self.plugins.get(&slot)?.params.get(index)?;
        if info.hidden {
            return None;
        }
        let (id, value) = (info.id, plugin_plain(info, normalized));
        self.plugin_rack.note_param_sent(slot, index, value);
        Some(EngineCommand::SetEffectParam {
            target,
            slot: u8::try_from(row).ok()?,
            id,
            value: value as f32,
        })
    }

    /// Set parameter `index` of the selected channel's plugin instrument to
    /// `normalized` of its range: [`Self::set_plugin_param`] for the source,
    /// by index, with the same law and the same edit counting. Returns
    /// `SetChannelGeneratorParam` carrying the plugin's own id and its plain
    /// value, which is what the engine takes on a plugin source (MOO-314).
    /// Refused for a hidden parameter, and when the source is not a plugin.
    pub fn set_plugin_source_param(&mut self, index: usize, normalized: f32) -> Option<EngineCommand> {
        let scope = EffectTarget::Channel(u8::try_from(self.selected).ok()?);
        let (device, slot) = self.plugin_source()?;
        let id = self.plugin_param_id(slot, index)?;
        self.set_plugin_param_at(ParamAddr::plugin_param(scope, device, id), normalized)
    }

    /// Set the plugin parameter `address` names to `normalized` of its
    /// range, wherever the plugin is: the path a MIDI binding takes, which
    /// holds an address, not a face's index. The command is the one the
    /// device's own knob sends -- `SetEffectParam` for a device on a chain,
    /// `SetChannelGeneratorParam` for a channel's instrument -- with the
    /// plugin's id and plain value, and the send counts as an edit of the
    /// plugin as a knob's does. `None` for a parameter the plugin does not
    /// list now (missing), a hidden one, or an address naming no plugin.
    pub fn set_plugin_param_at(&mut self, address: ParamAddr, normalized: f32) -> Option<EngineCommand> {
        let ParamOwner::PluginParam { device } = address.owner else {
            return None;
        };
        let slot = self.plugin_slot_of(address.scope, device)?;
        let index = self.plugin_param_index(slot, address.param)?;
        let info = self.plugins.get(&slot)?.params.get(index)?;
        if info.hidden {
            return None;
        }
        let (id, value) = (info.id, plugin_plain(info, normalized));
        let command = if self.plugin_source_slot_of(address.scope, device).is_some() {
            let EffectTarget::Channel(channel) = address.scope else {
                return None;
            };
            EngineCommand::SetChannelGeneratorParam {
                channel,
                id,
                value: value as f32,
            }
        } else {
            let row = device_slot(self.effect_chain_of(address.scope)?, device)?;
            EngineCommand::SetEffectParam {
                target: address.scope,
                slot: u8::try_from(row).ok()?,
                id,
                value: value as f32,
            }
        };
        self.plugin_rack.note_param_sent(slot, index, value);
        Some(command)
    }

    /// The plugin parameter `address` names, as a knob's travel from 0 to 1:
    /// its value now, read from the live instance, against the range the
    /// plugin reported. `None` when the plugin is not hosted or does not
    /// list the id.
    pub fn plugin_param_normalized(&self, address: ParamAddr) -> Option<f32> {
        let ParamOwner::PluginParam { device } = address.owner else {
            return None;
        };
        let slot = self.plugin_slot_of(address.scope, device)?;
        let info = self.plugin_param_info(address)?;
        let index = self.plugin_param_index(slot, address.param)?;
        Some(plugin_normalized(info, self.plugin_param_value(slot, index)?))
    }

    /// What the plugin device `device` on `scope` is called in a list: the
    /// plugin's name and its place on the chain, "Test Gain 2", or the
    /// plugin's name alone for a channel's instrument.
    pub fn plugin_device_label(&self, scope: EffectTarget, device: DeviceId) -> Option<String> {
        let slot = self.plugin_slot_of(scope, device)?;
        let name = &self.plugins.get(&slot)?.plugin.name;
        if self.plugin_source_slot_of(scope, device).is_some() {
            return Some(name.clone());
        }
        let position = device_slot(self.effect_chain_of(scope)?, device)?;
        Some(format!("{name} {}", position + 1))
    }
}

/// One plugin parameter a lane or a route on the selected channel can name,
/// as the lane menu and the modulation shelf show it (MOO-228).
///
/// `missing` is Adam's MOO-74 case: a lane or route names an id the plugin's
/// list no longer has. It is kept and saved, plays nothing, and is named by
/// its id, the only name left for it; it reads normally again the moment the
/// list has the id back.
#[derive(Clone, Debug, PartialEq)]
pub struct PluginDestination {
    pub address: ParamAddr,
    /// "Test Gain 2": the plugin's name and its place in the chain, the way
    /// a native insert is "Filter 2".
    pub device: String,
    pub name: String,
    pub missing: bool,
    /// Whether a new lane may be opened on it ([`Session::lane_allowed`]).
    pub lane_allowed: bool,
}

impl Session {
    /// Every plugin parameter on the selected channel: its plugin
    /// instrument's first, when its source is one, then its own chain's,
    /// device by device in chain order. Each device's listed parameters come
    /// first in the plugin's order (hidden ones left out) and then, marked
    /// missing, every id a lane or route on it names that the list no longer
    /// has.
    pub fn plugin_destinations(&self) -> Vec<PluginDestination> {
        let mut rows = Vec::new();
        let Some(channel) = self.channels.get(self.selected) else {
            return rows;
        };
        if let Some((device, slot)) = self.plugin_source() {
            self.push_plugin_destinations(&mut rows, device, slot, None);
        }
        for (position, effect) in channel.effects.iter().enumerate() {
            if let EffectParams::Plugin(slot) = effect.params {
                self.push_plugin_destinations(&mut rows, effect.id, slot, Some(position));
            }
        }
        rows
    }

    /// One plugin device's rows for [`Self::plugin_destinations`]. A device
    /// on the chain is named with its place, "Test Gain 2", as a native
    /// insert is "Filter 2"; the instrument, of which there is one, by its
    /// plugin's name alone.
    fn push_plugin_destinations(
        &self,
        rows: &mut Vec<PluginDestination>,
        device: DeviceId,
        slot: PluginSlotId,
        position: Option<usize>,
    ) {
        let Some(saved) = self.plugins.get(&slot) else {
            return;
        };
        let Some(channel) = self.channels.get(self.selected) else {
            return;
        };
        let scope = EffectTarget::Channel(self.selected as u8);
        let label = match position {
            Some(position) => format!("{} {}", saved.plugin.name, position + 1),
            None => saved.plugin.name.clone(),
        };
        for info in saved.params.iter().filter(|info| !info.hidden) {
            rows.push(PluginDestination {
                address: ParamAddr::plugin_param(scope, device, info.id),
                device: label.clone(),
                name: info.name.clone(),
                missing: false,
                lane_allowed: info.automatable,
            });
        }
        // The ids something still names and the list does not: every
        // pattern's lanes and every route, in the order they are found.
        let named = channel
            .automation
            .iter()
            .flatten()
            .map(|lane| lane.target)
            .chain(self.modulation.routes.iter().map(|route| route.destination));
        for address in named {
            let ParamOwner::PluginParam { device: owner } = address.owner else {
                continue;
            };
            if owner != device
                || address.scope != scope
                || saved.param(address.param).is_some_and(|info| !info.hidden)
                || rows.iter().any(|row| row.address == address)
            {
                continue;
            }
            // A hidden parameter is left out of the list above but a lane or
            // route can still name it: it reads as missing under its own
            // name, so it can be opened and removed (MOO-381).
            let name = saved
                .param(address.param)
                .map_or_else(|| format!("Parameter {}", address.param), |info| info.name.clone());
            rows.push(PluginDestination {
                address,
                device: label.clone(),
                name,
                missing: true,
                lane_allowed: false,
            });
        }
    }

    /// The live modulation offset on each of `slot`'s parameters, by dense
    /// index, for the device `device` on the chain `scope`: what a plugin
    /// face's rings draw, on the same terms as
    /// [`Session::destination_offsets`] for a native face.
    pub fn plugin_destination_offsets(
        &self,
        scope: EffectTarget,
        device: DeviceId,
        slot: PluginSlotId,
    ) -> Vec<f32> {
        let Some(saved) = self.plugins.get(&slot) else {
            return Vec::new();
        };
        saved
            .params
            .iter()
            .map(|info| {
                let policy = ModDestinationDescriptor::for_plugin_param(
                    info.id,
                    info.stepped.is_some(),
                    info.modulatable,
                );
                self.live_offset(ParamAddr::plugin_param(scope, device, info.id), &policy)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{EffectKind, EffectSlotState, PluginFormat, PluginRef, PluginSlotState};

    fn nudge() -> PluginParamInfo {
        PluginParamInfo {
            id: 4_000_000_000,
            name: "Nudge".into(),
            module: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            stepped: None,
            automatable: true,
            modulatable: false,
            hidden: false,
        }
    }

    fn gain() -> PluginParamInfo {
        PluginParamInfo {
            id: 10,
            name: "Gain".into(),
            module: String::new(),
            min: -60.0,
            max: 12.0,
            default: 0.0,
            stepped: None,
            automatable: true,
            modulatable: true,
            hidden: false,
        }
    }

    /// A session whose selected channel holds one plugin device listing
    /// `params`, and the device's address parts.
    fn session_with(params: Vec<PluginParamInfo>) -> (Session, PluginSlotId, mooloop_core::DeviceId) {
        let mut project = mooloop_core::Project::default();
        let slot = project.add_plugin_slot(PluginSlotState {
            params,
            ..PluginSlotState::new(PluginRef {
                format: PluginFormat::Clap,
                id: "test.plugin".into(),
                name: "Test".into(),
                vendor: String::new(),
                version: String::new(),
            })
        });
        let mut device = EffectSlotState::of_kind(EffectKind::Plugin);
        device.params = EffectParams::Plugin(slot);
        project.channels[0].setup.push_effect(device).expect("room");
        project.assign_device_ids();
        let mut session = Session::default();
        session.replace_project(&project, &[]);
        let device = session.channels[0].effects[0].id;
        (session, slot, device)
    }

    /// A session whose selected channel's source is a plugin listing
    /// `params`, and the source slot's id.
    fn instrument_with(params: Vec<PluginParamInfo>) -> (Session, PluginSlotId, DeviceId) {
        let mut project = mooloop_core::Project::default();
        let slot = project.add_plugin_slot(PluginSlotState {
            params,
            ..PluginSlotState::new(PluginRef {
                format: PluginFormat::Clap,
                id: "test.instrument".into(),
                name: "Synth".into(),
                vendor: String::new(),
                version: String::new(),
            })
        });
        project.channels[0].setup.source = mooloop_core::ChannelSource::Plugin(slot);
        project.assign_device_ids();
        let mut session = Session::default();
        session.replace_project(&project, &[]);
        let device = session.channels[0].source_device;
        assert!(device.is_assigned(), "a plugin source is given an id");
        (session, slot, device)
    }

    /// The source arm: the instrument's id resolves to its slot, a knob on
    /// it sends the plugin's id and plain value in the command the engine
    /// takes for a plugin source, and once the source is replaced the old id
    /// names nothing.
    #[test]
    fn an_instruments_id_resolves_to_its_slot_until_it_is_replaced() {
        let hidden = PluginParamInfo {
            id: 7,
            hidden: true,
            ..nudge()
        };
        let (mut session, slot, device) = instrument_with(vec![gain(), hidden]);
        let scope = EffectTarget::Channel(0);
        let address = ParamAddr::plugin_param(scope, device, 10);
        assert_eq!(session.plugin_slot_of(scope, device), Some(slot));
        assert_eq!(session.plugin_source(), Some((device, slot)));
        assert_eq!(session.plugin_param_info(address).map(|info| info.id), Some(10));
        assert!(session.lane_allowed(address));
        assert!(session.modulation_policy(address).expect("a policy").allowed);
        assert_eq!(session.plugin_device_label(scope, device).as_deref(), Some("Synth"));
        assert_eq!(session.plugin_destinations().len(), 1, "the hidden one is left out");

        assert_eq!(
            session.set_plugin_source_param(0, 0.5),
            Some(EngineCommand::SetChannelGeneratorParam {
                channel: 0,
                id: 10,
                value: -24.0,
            })
        );
        assert_eq!(session.set_plugin_source_param(1, 0.5), None, "a hidden parameter has no knob");
        assert_eq!(session.set_plugin_param_at(address, 1.0).map(|_| ()), Some(()));

        session.reset_channel_source(0, mooloop_core::DeviceKind::DrumSynth);
        assert_eq!(session.plugin_slot_of(scope, device), None);
        assert_eq!(session.plugin_source(), None);
        assert_eq!(session.set_plugin_source_param(0, 0.5), None);
        assert!(session.plugin_destinations().is_empty());
    }

    /// The four-billion id goes out as a dense index and comes back as the
    /// same id, through the knob's path and the route's, and nothing on the
    /// way holds it in an `i32` (the orchestrator's condition on MOO-78).
    #[test]
    fn an_id_above_i32_max_crosses_as_an_index_and_comes_back_whole() {
        let (mut session, slot, device) = session_with(vec![gain(), nudge()]);
        let index = session.plugin_param_index(slot, 4_000_000_000).expect("listed");
        assert_eq!(index, 1);
        assert!(i32::try_from(index).is_ok(), "an index is what crosses into Slint");
        assert_eq!(session.plugin_param_id(slot, index), Some(4_000_000_000));

        // The knob: an index in, the id out in the command.
        let command = session.set_plugin_param(0, index, 1.0).expect("a command");
        assert_eq!(
            command,
            EngineCommand::SetEffectParam {
                target: EffectTarget::Channel(0),
                slot: 0,
                id: 4_000_000_000,
                value: 1.0,
            }
        );

        // The address a route or lane carries, and what it resolves to.
        let address = ParamAddr::plugin_param(EffectTarget::Channel(0), device, 4_000_000_000);
        assert_eq!(session.plugin_param_info(address).map(|info| info.id), Some(4_000_000_000));
        assert!(session.lane_allowed(address));
        let policy = session.modulation_policy(address).expect("a policy");
        assert!(!policy.allowed, "Nudge is not modulatable");
        let gain_address = ParamAddr::plugin_param(EffectTarget::Channel(0), device, 10);
        assert!(session.modulation_policy(gain_address).expect("a policy").allowed);
    }

    /// A lane may not be opened on a parameter the plugin will not let be
    /// automated, or on one the song does not know; a native one is never
    /// asked.
    #[test]
    fn a_lane_opens_only_on_an_automatable_parameter_the_song_knows() {
        let (session, _, device) = session_with(vec![PluginParamInfo {
            automatable: false,
            ..gain()
        }]);
        let scope = EffectTarget::Channel(0);
        assert!(!session.lane_allowed(ParamAddr::plugin_param(scope, device, 10)));
        assert!(!session.lane_allowed(ParamAddr::plugin_param(scope, device, 99)));
        assert!(session.lane_allowed(ParamAddr::effect(scope, device, 0)));
    }

    #[test]
    fn a_knob_position_and_a_plain_value_are_one_law_both_ways() {
        let gain = gain();
        assert_eq!(plugin_plain(&gain, 0.0), -60.0);
        assert_eq!(plugin_plain(&gain, 1.0), 12.0);
        assert_eq!(plugin_normalized(&gain, plugin_plain(&gain, 0.25)), 0.25);
        let stepped = PluginParamInfo {
            min: 0.0,
            max: 2.0,
            stepped: Some(3),
            ..gain
        };
        assert_eq!(plugin_plain(&stepped, 0.3), 1.0);
    }

    /// One modulator at 0.4 through a half-depth bipolar route reads 0.2 on
    /// a native destination and on a plugin parameter alike: the plugin's
    /// offsets are the native sum, by dense index. It also pins
    /// `destination_offsets` across MOO-228's extraction of the source split
    /// it shares with the plugin path.
    #[test]
    fn a_plugin_parameter_reads_the_same_offset_a_native_one_does() {
        let (mut session, slot, device) = session_with(vec![nudge(), gain()]);
        assert!(
            session.add_modulation_source(mooloop_core::ModulatorKind::Lfo),
            "room for a modulator"
        );
        let scope = EffectTarget::Channel(0);
        let volume = ParamAddr::strip(scope, mooloop_core::STRIP_PARAM_VOLUME);
        let gain = ParamAddr::plugin_param(scope, device, 10);
        for destination in [volume, gain] {
            let lfo = session.module_at(0).expect("the LFO");
            session.modulation.routes.push(mooloop_core::ModRoute::from_module(
                lfo,
                destination,
                0.5,
                mooloop_core::ModPolarity::Bipolar,
            ));
        }
        session.modulation_sent = session.modulation_plan();
        session.modulation_levels.borrow_mut().modules = vec![0.4];

        let native = session.destination_offsets(&mooloop_core::STRIP_DESCRIPTORS, |param| {
            ParamAddr::strip(scope, param)
        });
        assert!((native[mooloop_core::STRIP_PARAM_VOLUME as usize] - 0.2).abs() < 1e-6, "{native:?}");
        // Nudge takes no modulation and reads zero; Gain, index 1, reads the sum.
        let plugin = session.plugin_destination_offsets(scope, device, slot);
        assert_eq!(plugin.len(), 2);
        assert_eq!(plugin[0], 0.0);
        assert!((plugin[1] - 0.2).abs() < 1e-6, "{plugin:?}");
    }

    /// The lane menu's plugin rows: every listed, unhidden parameter under
    /// the device's name and chain place, and -- once the list loses one a
    /// lane names -- a missing row named by its id, which reads normally
    /// again when the list has it back (MOO-74).
    #[test]
    fn a_lane_on_a_parameter_the_plugin_stopped_listing_is_listed_as_missing() {
        let (mut session, slot, device) = session_with(vec![gain(), nudge()]);
        let scope = EffectTarget::Channel(0);
        let nudge_address = ParamAddr::plugin_param(scope, device, 4_000_000_000);
        let names = |session: &Session| {
            session
                .plugin_destinations()
                .into_iter()
                .map(|row| (row.device, row.name, row.missing))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&session),
            [
                ("Test 1".to_string(), "Gain".to_string(), false),
                ("Test 1".to_string(), "Nudge".to_string(), false),
            ]
        );
        session.open_automation_lane_at(nudge_address).expect("a lane");
        let listed = session.plugins[&slot].params.clone();
        session.plugins.get_mut(&slot).unwrap().params.retain(|info| info.id == 10);
        assert_eq!(
            names(&session),
            [
                ("Test 1".to_string(), "Gain".to_string(), false),
                ("Test 1".to_string(), "Parameter 4000000000".to_string(), true),
            ]
        );
        session.plugins.get_mut(&slot).unwrap().params = listed;
        assert!(names(&session).iter().all(|(_, _, missing)| !missing));
    }

    /// A parameter the plugin later marks hidden is left out of the menu, so
    /// a lane that still names it must not vanish with it: it reads as
    /// missing, under its own name, and can be opened and removed (MOO-381).
    #[test]
    fn a_lane_on_a_parameter_the_plugin_later_hid_is_still_listed() {
        let (mut session, slot, device) = session_with(vec![gain(), nudge()]);
        let nudge_address = ParamAddr::plugin_param(EffectTarget::Channel(0), device, 4_000_000_000);
        session.open_automation_lane_at(nudge_address).expect("a lane");
        session
            .plugins
            .get_mut(&slot)
            .unwrap()
            .params
            .iter_mut()
            .find(|info| info.id == 4_000_000_000)
            .unwrap()
            .hidden = true;
        let rows = session.plugin_destinations();
        let row = rows
            .iter()
            .find(|row| row.address == nudge_address)
            .expect("the lane's parameter still has a row");
        assert!(row.missing, "a hidden parameter is unavailable, like a missing one");
        assert_eq!(row.name, "Nudge");
        assert!(!row.lane_allowed);
        assert_eq!(rows.len(), 2, "one row each, none doubled");
    }
}
