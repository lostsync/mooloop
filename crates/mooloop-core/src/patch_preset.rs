//! Patches you can keep (`docs/plans/song-patch/09-patches-you-can-keep.md`):
//! a selection of the canvas lifted out as a fragment, and a fragment landed
//! back in a song.
//!
//! A fragment is a [`SongModulation`] of its own, so a patch preset is saved
//! and read the way a song's `modulation` table is. What it lifts is the
//! selected boxes and tags, the wires among them, and every assignment their
//! boxes drive. A tag is lifted **unbound**, with a hint of what it was bound
//! to ([`TagHint`]); an assignment onto anything outside the fragment is
//! lifted as a [`LooseRoute`], its destination written as text. So a fragment
//! never names a channel, a device or a box of the song it came from, and
//! landing one never reaches into the song it lands in.

use crate::mod_metadata::ModSourceId;
use crate::modulation::{ModRoute, SongModulation, UNRESOLVED_SLOT};
use crate::patch::{CanvasPoint, LooseRoute, RoutePlace, TagHint, TagKind};
use crate::{ModSourceRef, ParamAddr};

impl SongModulation {
    /// The hint shown in tag `tag`'s empty slot, if a preset gave it one.
    pub fn hint(&self, tag: ModSourceId) -> Option<&str> {
        self.hints
            .iter()
            .find(|hint| hint.tag == tag)
            .map(|hint| hint.text.as_str())
    }

    /// The unbound assignment `id`, if the song holds one.
    pub fn loose_route(&self, id: ModSourceId) -> Option<&LooseRoute> {
        self.loose.iter().find(|route| route.id == id)
    }

    /// The fragment `nodes` make: the boxes and tags among them, the wires
    /// with both ends among them, and every assignment their boxes and tags
    /// drive.
    ///
    /// Every tag comes unbound, with `tag_hint` of what it was bound to (or
    /// the hint it already had); an assignment onto a knob of a box in the
    /// fragment stays a route, and any other becomes a [`LooseRoute`] hinted
    /// with `route_hint` of its destination. The fragment keeps the song's
    /// ids, and its top-left is moved to the canvas origin, so where it lands
    /// is wherever [`Self::land`] puts it.
    pub fn fragment(
        &self,
        nodes: &[ModSourceId],
        tag_hint: impl Fn(TagKind) -> Option<String>,
        route_hint: impl Fn(ParamAddr) -> String,
    ) -> SongModulation {
        let held = |id: ModSourceId| nodes.contains(&id);
        let mut fragment = SongModulation {
            next_source_id: self.next_source_id,
            ..SongModulation::default()
        };
        fragment.modules = self
            .modules
            .iter()
            .filter(|module| held(module.id))
            .map(|module| crate::SongModule {
                rack: None,
                ..module.clone()
            })
            .collect();
        for tag in self.tags.iter().filter(|tag| held(tag.id)) {
            let hint = tag_hint(tag.kind)
                .or_else(|| self.hint(tag.id).map(str::to_string))
                .filter(|text| !text.is_empty());
            fragment.tags.push(crate::SongTag {
                kind: tag.kind.unbound(),
                ..*tag
            });
            if let Some(text) = hint {
                fragment.hints.push(TagHint { tag: tag.id, text });
            }
        }
        fragment.wires = self
            .wires
            .iter()
            .filter(|wire| held(wire.from.node) && held(wire.to.node))
            .copied()
            .collect();
        for route in &self.routes {
            let ModSourceRef::Id(source) = route.source else {
                continue;
            };
            if !held(source) {
                continue;
            }
            let at = self.route_at(route.source, route.destination);
            if route.destination.module().is_some_and(held) {
                fragment.routes.push(ModRoute {
                    source_slot: UNRESOLVED_SLOT,
                    ..*route
                });
                if let Some(at) = at {
                    fragment.route_places.push(RoutePlace {
                        source: route.source,
                        destination: route.destination,
                        at,
                    });
                }
                continue;
            }
            let id = fragment.mint();
            fragment.loose.push(LooseRoute {
                id,
                source,
                hint: route_hint(route.destination),
                depth: route.depth,
                polarity: route.polarity,
                at,
            });
        }
        for route in self.loose.iter().filter(|route| held(route.source)) {
            let id = fragment.mint();
            fragment.loose.push(LooseRoute {
                id,
                ..route.clone()
            });
        }
        let origin = fragment.origin();
        fragment.shift(CanvasPoint::new(-origin.x, -origin.y));
        fragment
    }

    /// Land `fragment` in the song with its top-left at `at`: every box, tag
    /// and unbound assignment under a fresh id, its wires and routes
    /// following. Returns the new ids of what landed, boxes first, then tags,
    /// then unbound assignments.
    pub fn land(&mut self, fragment: &SongModulation, at: CanvasPoint) -> Vec<ModSourceId> {
        let origin = fragment.origin();
        let shift = |point: CanvasPoint| CanvasPoint::new(point.x - origin.x + at.x, point.y - origin.y + at.y);
        let mut ids: Vec<(ModSourceId, ModSourceId)> = Vec::new();
        for old in fragment
            .modules
            .iter()
            .map(|module| module.id)
            .chain(fragment.tags.iter().map(|tag| tag.id))
        {
            let new = self.mint();
            ids.push((old, new));
        }
        let new_id = |old: ModSourceId| ids.iter().find(|(was, _)| *was == old).map(|(_, id)| *id);
        let mut landed: Vec<ModSourceId> = ids.iter().map(|(_, id)| *id).collect();
        for module in &fragment.modules {
            let Some(id) = new_id(module.id) else { continue };
            self.modules.push(crate::SongModule {
                id,
                at: shift(module.at),
                rack: None,
                ..module.clone()
            });
        }
        for tag in &fragment.tags {
            let Some(id) = new_id(tag.id) else { continue };
            self.tags.push(crate::SongTag {
                id,
                at: shift(tag.at),
                kind: tag.kind,
            });
        }
        for hint in &fragment.hints {
            if let Some(tag) = new_id(hint.tag) {
                self.hints.push(TagHint {
                    tag,
                    text: hint.text.clone(),
                });
            }
        }
        for wire in &fragment.wires {
            let (Some(from), Some(to)) = (new_id(wire.from.node), new_id(wire.to.node)) else {
                continue;
            };
            let mut wire = *wire;
            wire.from.node = from;
            wire.to.node = to;
            if let Some(bend) = &mut wire.bend {
                bend.at += match bend.axis {
                    crate::BendAxis::Horizontal => at.y - origin.y,
                    crate::BendAxis::Vertical => at.x - origin.x,
                };
            }
            self.wires.push(wire);
        }
        let new_source = |source: ModSourceRef| match source {
            ModSourceRef::Id(id) => new_id(id).map(ModSourceRef::Id),
            other => Some(other),
        };
        let new_destination = |mut destination: ParamAddr| {
            if let crate::ParamOwner::Modulator { module } = &mut destination.owner {
                *module = new_id(*module)?;
            }
            Some(destination)
        };
        for route in &fragment.routes {
            let (Some(source), Some(destination)) = (new_source(route.source), new_destination(route.destination)) else {
                continue;
            };
            if self
                .routes
                .iter()
                .any(|held| held.source == source && held.destination == destination)
            {
                continue;
            }
            self.routes.push(ModRoute {
                source,
                source_slot: UNRESOLVED_SLOT,
                destination,
                ..*route
            });
            if let Some(place) = fragment.route_at(route.source, route.destination) {
                self.route_places.push(RoutePlace {
                    source,
                    destination,
                    at: shift(place),
                });
            }
        }
        for route in &fragment.loose {
            let Some(source) = new_id(route.source) else { continue };
            let id = self.mint();
            self.loose.push(LooseRoute {
                id,
                source,
                at: route.at.map(shift),
                ..route.clone()
            });
            landed.push(id);
        }
        landed
    }

    /// Bind unbound assignment `id` to `destination` at `depth`: it becomes
    /// a route, with its polarity and its tag's place. One already running
    /// from its box to `destination` takes the depth. Returns whether the
    /// song held it.
    pub fn bind_loose(&mut self, id: ModSourceId, destination: ParamAddr, depth: f32) -> bool {
        let Some(index) = self.loose.iter().position(|route| route.id == id) else {
            return false;
        };
        let loose = self.loose.remove(index);
        let source = ModSourceRef::Id(loose.source);
        match self
            .routes
            .iter_mut()
            .find(|route| route.source == source && route.destination == destination)
        {
            Some(route) => route.depth = depth,
            None => self.routes.push(ModRoute {
                source,
                source_slot: UNRESOLVED_SLOT,
                destination,
                depth,
                polarity: loose.polarity,
            }),
        }
        if let Some(at) = loose.at {
            self.place_route(source, destination, at);
        }
        true
    }

    /// Remove unbound assignment `id`. Returns whether the song held it.
    pub fn remove_loose(&mut self, id: ModSourceId) -> bool {
        let before = self.loose.len();
        self.loose.retain(|route| route.id != id);
        self.loose.len() != before
    }

    /// The top-left of everything placed: boxes, tags, and assignment tags
    /// put somewhere.
    fn origin(&self) -> CanvasPoint {
        let points = self
            .modules
            .iter()
            .map(|module| module.at)
            .chain(self.tags.iter().map(|tag| tag.at))
            .chain(self.loose.iter().filter_map(|route| route.at))
            .chain(self.route_places.iter().map(|place| place.at));
        let mut origin: Option<CanvasPoint> = None;
        for point in points {
            origin = Some(match origin {
                Some(held) => CanvasPoint::new(held.x.min(point.x), held.y.min(point.y)),
                None => point,
            });
        }
        origin.unwrap_or_default()
    }

    /// Move everything placed by `by`, bends included.
    fn shift(&mut self, by: CanvasPoint) {
        let moved = |point: &mut CanvasPoint| {
            point.x += by.x;
            point.y += by.y;
        };
        self.modules.iter_mut().for_each(|module| moved(&mut module.at));
        self.tags.iter_mut().for_each(|tag| moved(&mut tag.at));
        self.loose.iter_mut().filter_map(|route| route.at.as_mut()).for_each(moved);
        self.route_places.iter_mut().for_each(|place| moved(&mut place.at));
        for bend in self.wires.iter_mut().filter_map(|wire| wire.bend.as_mut()) {
            bend.at += match bend.axis {
                crate::BendAxis::Horizontal => by.y,
                crate::BendAxis::Vertical => by.x,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulation::{InputSource, ModPolarity, ModulatorKind};
    use crate::patch::InletSource;
    use crate::{ChannelId, EffectTarget};

    fn cutoff() -> ParamAddr {
        ParamAddr::strip(EffectTarget::Channel(1), 3)
    }

    /// The kick pump: the kick's gate into an envelope, the envelope pulling
    /// the pad's level down.
    fn kick_pump() -> (SongModulation, Vec<ModSourceId>) {
        let kick = ChannelId(7);
        let mut song = SongModulation::default();
        let envelope = song.add_module(
            ModulatorKind::Envelope.default_params(),
            InputSource::ChannelNotes(kick),
            kick,
            "Kick",
        );
        let gate = song.input_of_tag(envelope);
        song.routes.push(ModRoute {
            source: ModSourceRef::Id(envelope),
            source_slot: UNRESOLVED_SLOT,
            destination: cutoff(),
            depth: -0.6,
            polarity: ModPolarity::Unipolar,
        });
        song.move_node(gate, CanvasPoint::new(100, 300));
        song.move_node(envelope, CanvasPoint::new(220, 300));
        (song, vec![gate, envelope])
    }

    impl SongModulation {
        fn input_of_tag(&self, module: ModSourceId) -> ModSourceId {
            self.wires
                .iter()
                .find(|wire| wire.to.node == module)
                .map(|wire| wire.from.node)
                .expect("the module is wired")
        }
    }

    fn hinted(song: &SongModulation, nodes: &[ModSourceId]) -> SongModulation {
        song.fragment(
            nodes,
            |kind| match kind {
                TagKind::Inlet {
                    bind: Some(InletSource::Gate(_)),
                } => Some("Kick".to_string()),
                _ => None,
            },
            |_| "Level on Pad".to_string(),
        )
    }

    #[test]
    fn the_kick_pump_round_trips_unbound_and_hinted() {
        let (song, nodes) = kick_pump();
        let fragment = hinted(&song, &nodes);
        assert_eq!(fragment.modules.len(), 1);
        assert_eq!(fragment.tags.len(), 1);
        assert_eq!(fragment.tags[0].kind, TagKind::Inlet { bind: None }, "saved unbound");
        assert_eq!(fragment.hint(fragment.tags[0].id), Some("Kick"));
        assert!(fragment.routes.is_empty(), "no address leaves the song");
        assert_eq!(fragment.loose.len(), 1);
        assert_eq!(fragment.loose[0].hint, "Level on Pad");
        assert_eq!(fragment.loose[0].depth, -0.6);
        assert!(fragment.modules[0].rack.is_none());
        // Its top-left is the origin.
        assert_eq!(fragment.tags[0].at, CanvasPoint::new(0, 0));
        assert_eq!(fragment.modules[0].at, CanvasPoint::new(120, 0));

        // Through the file and back.
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Doc {
            modulation: SongModulation,
        }
        let text = toml::to_string(&Doc {
            modulation: fragment.clone(),
        })
        .unwrap();
        let read: Doc = toml::from_str(&text).unwrap();
        assert_eq!(read.modulation.hints, fragment.hints);
        assert_eq!(read.modulation.loose, fragment.loose);
        assert_eq!(read.modulation.wires, fragment.wires);

        // Into a song that already has boxes: fresh ids, where it was put.
        let (mut other, _) = kick_pump();
        let before = other.clone();
        let landed = other.land(&read.modulation, CanvasPoint::new(400, 40));
        assert_eq!(landed.len(), 3, "a box, a tag and an unbound assignment");
        for id in &landed {
            assert!(before.module(*id).is_none() && before.tag(*id).is_none());
        }
        let (envelope, gate, loose) = (landed[0], landed[1], landed[2]);
        assert_eq!(other.module(envelope).unwrap().at, CanvasPoint::new(520, 40));
        assert_eq!(other.tag(gate).unwrap().at, CanvasPoint::new(400, 40));
        assert_eq!(other.tag(gate).unwrap().kind, TagKind::Inlet { bind: None });
        assert_eq!(other.hint(gate), Some("Kick"));
        assert_eq!(other.input_of_tag(envelope), gate);
        assert_eq!(other.routes, before.routes, "nothing bound in the song it landed in");
        assert_eq!(other.loose_route(loose).unwrap().source, envelope);

        // Binding the tag drops its hint; binding the assignment makes it a
        // route with its polarity.
        assert!(other.bind_tag(gate, Some(InletSource::Gate(ChannelId(2)))));
        assert_eq!(other.hint(gate), None);
        let destination = ParamAddr::strip(EffectTarget::Channel(0), 3);
        assert!(other.bind_loose(loose, destination, -0.5));
        assert!(other.loose.is_empty());
        let route = other
            .routes
            .iter()
            .find(|route| route.source == ModSourceRef::Id(envelope))
            .unwrap();
        assert_eq!((route.destination, route.depth, route.polarity), (destination, -0.5, ModPolarity::Unipolar));
    }

    #[test]
    fn a_wire_or_route_leaving_the_selection_is_not_saved_but_one_inside_is() {
        let (mut song, nodes) = kick_pump();
        let envelope = nodes[1];
        let lfo = song.add_module(
            ModulatorKind::Lfo.default_params(),
            InputSource::None,
            ChannelId::UNASSIGNED,
            "",
        );
        // The LFO moves the envelope's first knob.
        let knob = ParamAddr::modulator(envelope, ModulatorKind::Envelope.descriptors()[0].id);
        song.routes.push(ModRoute {
            source: ModSourceRef::Id(lfo),
            source_slot: UNRESOLVED_SLOT,
            destination: knob,
            depth: 0.25,
            polarity: ModPolarity::Bipolar,
        });
        // Only the envelope and the LFO: the gate's wire stays behind.
        let fragment = hinted(&song, &[envelope, lfo]);
        assert!(fragment.wires.is_empty());
        assert!(fragment.tags.is_empty());
        assert_eq!(fragment.routes.len(), 1, "a knob inside the fragment stays bound");
        assert_eq!(fragment.loose.len(), 1, "the level route is loose");

        let mut other = SongModulation::default();
        let landed = other.land(&fragment, CanvasPoint::new(0, 0));
        let (new_envelope, new_lfo) = (landed[0], landed[1]);
        assert_eq!(other.routes.len(), 1);
        assert_eq!(other.routes[0].source, ModSourceRef::Id(new_lfo));
        assert_eq!(other.routes[0].destination.module(), Some(new_envelope));
    }

    #[test]
    fn removing_a_box_takes_its_unbound_assignments_and_a_tag_its_hint() {
        let (song, nodes) = kick_pump();
        let fragment = hinted(&song, &nodes);
        let mut other = SongModulation::default();
        let landed = other.land(&fragment, CanvasPoint::new(0, 0));
        assert!(other.move_node(landed[2], CanvasPoint::new(5, 6)));
        assert_eq!(other.loose[0].at, Some(CanvasPoint::new(5, 6)));
        other.remove_tag(landed[1]);
        assert!(other.hints.is_empty());
        other.remove_module(landed[0]);
        assert!(other.loose.is_empty());
        assert!(other.is_empty());
        // Ids are not handed out twice.
        let tag = other.add_tag(TagKind::Inlet { bind: None }, CanvasPoint::default());
        assert!(!landed.contains(&tag));
    }
}
