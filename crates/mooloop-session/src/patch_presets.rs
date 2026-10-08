//! Patches you can keep (`docs/plans/song-patch/09-patches-you-can-keep.md`):
//! the session's half of a patch preset. Lifting a canvas selection writes
//! what each tag and assignment was bound to as text; landing one puts the
//! boxes where the canvas was clicked; an unbound assignment is bound by the
//! assignment gesture, armed from its tag.

use crate::session::Session;
use mooloop_core::{CanvasPoint, ModSourceId, ModSourceRef, ParamAddr, SongModulation, TagKind};

impl Session {
    /// The patch preset `nodes` make ([`SongModulation::fragment`]), with
    /// every tag's hint the name of what it reads or plays and every
    /// assignment's its destination's name. `None` when `nodes` holds no box
    /// or tag of the song.
    pub fn patch_fragment(&self, nodes: &[ModSourceId]) -> Option<SongModulation> {
        let fragment = self.modulation.fragment(
            nodes,
            |kind| self.tag_hint(kind),
            |destination| self.destination_hint(destination),
        );
        (!fragment.modules.is_empty() || !fragment.tags.is_empty()).then_some(fragment)
    }

    /// Land patch preset `fragment` with its top-left at `at`, and select
    /// what landed. Returns the new ids, boxes first.
    pub fn land_patch(&mut self, fragment: &SongModulation, at: CanvasPoint) -> Vec<ModSourceId> {
        let landed = self.modulation.land(fragment, at);
        if let Some(first) = fragment.modules.first().and(landed.first()) {
            self.modulation_selected.set(Some(ModSourceRef::Id(*first)));
        }
        landed
    }

    /// Arm the assignment gesture from unbound assignment `id`: its box is
    /// armed, and the next knob it is dragged onto binds it, at the depth it
    /// was saved with as the drag's start. Arming it again disarms. Returns
    /// the armed box's name, `None` when the gesture is now off.
    pub fn arm_loose_route(&mut self, id: ModSourceId) -> Option<String> {
        let source = ModSourceRef::Id(self.modulation.loose_route(id)?.source);
        if self.modulation_loose.get() == Some(id) && self.modulation_armed.get() == Some(source) {
            self.modulation_loose.set(None);
            self.modulation_armed.set(None);
            return None;
        }
        self.modulation_selected.set(Some(source));
        self.modulation_armed.set(Some(source));
        self.modulation_loose.set(Some(id));
        self.modulation_source_name(source)
    }

    /// The unbound assignment the armed gesture binds, while it is armed
    /// from it.
    pub fn armed_loose_route(&self) -> Option<&mooloop_core::LooseRoute> {
        let route = self.modulation.loose_route(self.modulation_loose.get()?)?;
        (self.modulation_armed.get() == Some(ModSourceRef::Id(route.source))).then_some(route)
    }

    /// What a tag's hint says it was bound to: the source an inlet tag read,
    /// as its picker lists it, or the channel a notes tag read or played.
    fn tag_hint(&self, kind: TagKind) -> Option<String> {
        match kind {
            TagKind::Inlet { bind: Some(source) } => self
                .inlet_source_name(source)
                .map(|name| name.trim_end_matches(crate::modulation::LATE_NOTE).to_string()),
            TagKind::Inlet { bind: None } => None,
            TagKind::NotesIn { channel, .. } | TagKind::NotesOut { channel } => {
                let index = self.channel_index(channel?)?;
                Some(self.channels.get(index)?.name.clone())
            }
        }
    }

    /// What an unbound assignment's hint says it was aimed at: `Cutoff on
    /// Pad`, or empty for a destination the song cannot name.
    fn destination_hint(&self, destination: ParamAddr) -> String {
        if let Some(row) = self
            .plugin_destinations()
            .into_iter()
            .find(|row| row.address == destination)
        {
            return format!("{} on {}", row.name, row.device);
        }
        match self.modulation_destination(destination) {
            Some((device, descriptor)) => format!("{} on {device}", descriptor.name),
            None => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ArmedRoute;
    use mooloop_core::{EffectTarget, ModPolarity, ModRoute, ModulatorKind, STRIP_PARAM_VOLUME};

    /// The kick pump saved off the canvas and landed again: its tag and
    /// assignment come back unbound with what they were bound to as hints,
    /// and the assignment gesture armed from the assignment's tag binds it
    /// at its saved depth and polarity.
    #[test]
    fn a_patch_lands_hinted_and_its_assignment_binds_from_its_tag() {
        let mut session = Session::default();
        assert!(session.add_modulation_source(ModulatorKind::Envelope));
        let envelope = session.modulation.modules[0].id;
        let gate = session.modulation.tags[0].id;
        let volume = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        session.modulation.routes.push(ModRoute::from_module(
            envelope,
            volume,
            -0.6,
            ModPolarity::Unipolar,
        ));
        let channel = session.channels[0].name.clone();

        let fragment = session.patch_fragment(&[gate, envelope]).unwrap();
        assert_eq!(fragment.hint(gate), Some(format!("{channel} · gate").as_str()));
        assert_eq!(fragment.loose.len(), 1);
        assert!(fragment.loose[0].hint.starts_with("Volume on "), "{}", fragment.loose[0].hint);
        assert!(session.patch_fragment(&[]).is_none());

        let landed = session.land_patch(&fragment, CanvasPoint::new(600, 400));
        let (new_envelope, loose) = (landed[0], landed[2]);
        assert_eq!(session.modulation_selected.get(), Some(ModSourceRef::Id(new_envelope)));
        assert!(session.modulation.loose_route(loose).is_some());

        assert!(session.arm_loose_route(loose).is_some());
        let source = ModSourceRef::Id(new_envelope);
        assert_eq!(session.modulation_armed.get(), Some(source));
        assert_eq!(session.modulation_depth_for(source, volume), -0.6, "the drag starts at its depth");
        let ArmedRoute::Added(route) = session.arm_modulation_route(volume, -0.5) else {
            panic!("the knob binds it");
        };
        assert_eq!((route.source, route.depth, route.polarity), (source, -0.5, ModPolarity::Unipolar));
        assert!(session.modulation.loose.is_empty());
        assert_eq!(session.modulation_loose.get(), None);

        // Arming one again disarms; removing one is a canvas delete.
        let landed = session.land_patch(&fragment, CanvasPoint::new(0, 900));
        assert!(session.arm_loose_route(landed[2]).is_some());
        assert!(session.arm_loose_route(landed[2]).is_none());
        assert_eq!(session.modulation_armed.get(), None);
        assert!(session.remove_patch_node(landed[2]));
        assert!(session.modulation.loose.is_empty());
    }
}
