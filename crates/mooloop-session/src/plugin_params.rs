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
    device_slot, EffectParams, EffectTarget, EngineCommand, ModDestinationDescriptor, ParamAddr,
    ParamOwner, PluginParamInfo, PluginSlotId,
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
    /// The plugin slot of the device `device` on `scope`'s chain, if that
    /// device is a hosted plugin.
    pub fn plugin_slot_of(&self, scope: EffectTarget, device: mooloop_core::DeviceId) -> Option<PluginSlotId> {
        let effects = self.effect_chain_of(scope)?;
        match effects.get(device_slot(effects, device)?)?.params {
            EffectParams::Plugin(slot) => Some(slot),
            _ => None,
        }
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
            let EffectTarget::Channel(channel) = address.scope else {
                return None;
            };
            if channel as usize != self.selected {
                return None;
            }
            let info = self.plugin_param_info(address)?;
            return Some(ModDestinationDescriptor::for_plugin_param(
                info.id,
                info.stepped.is_some(),
                info.modulatable,
            ));
        }
        self.channel_modulation_destination(address)
            .map(|(_, descriptor)| ModDestinationDescriptor::for_param(descriptor))
    }

    /// Whether a new lane may be opened on `address`: always for a native
    /// destination, and for a plugin parameter only when the song knows it
    /// and the plugin marks it automatable. An existing lane is always
    /// reopened, missing parameter or not.
    pub fn lane_allowed(&self, address: ParamAddr) -> bool {
        match address.owner {
            ParamOwner::PluginParam { .. } => self
                .plugin_param_info(address)
                .is_some_and(|info| info.automatable),
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
}
