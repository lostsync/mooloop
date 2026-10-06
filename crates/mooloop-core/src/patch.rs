//! The song's modulation as a patch: boxes with a place on a canvas, the
//! jacks each kind of box has, the tags that bring the song's sources in, and
//! the wires between them (`docs/plans/song-patch/`, step 01).
//!
//! A box is a [`SongModule`]; a tag is a [`SongTag`]; both are named by a
//! [`ModSourceId`] from one counter, so a wire names either the same way. A
//! module used to have one `input`; wires replace it, and
//! [`SongModulation::input_of`] reads the wire back as that single input for
//! everything that has not learned the graph yet (the engine's compile, the
//! shelf's input picker). Step 02 of the plan teaches the engine the graph.

use crate::mod_metadata::ModSourceId;
use crate::modulation::{InputSource, ModulatorKind, SongModulation};

/// A place on the patch canvas, in whole canvas units. The canvas is not the
/// screen: the pane scrolls over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct CanvasPoint {
    pub x: i32,
    pub y: i32,
}

impl CanvasPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// What a jack carries. A wire joins two jacks of one sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JackSort {
    /// A value every control tick.
    Control,
    /// Notes, each NoteOff following its NoteOn.
    Note,
}

/// One inlet or outlet in a kind's jack table: its name, which the canvas
/// shows, and its sort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Port {
    pub name: &'static str,
    pub sort: JackSort,
}

const fn control(name: &'static str) -> Port {
    Port {
        name,
        sort: JackSort::Control,
    }
}

const fn note(name: &'static str) -> Port {
    Port {
        name,
        sort: JackSort::Note,
    }
}

/// A kind's inlets and outlets, in port order. The one place a port's name
/// and sort live: the canvas's labels, the session's refusals and the
/// engine's inlet buffers all read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub inlets: &'static [Port],
    pub outlets: &'static [Port],
}

impl Ports {
    pub fn inlet(&self, port: u8) -> Option<Port> {
        self.inlets.get(usize::from(port)).copied()
    }

    pub fn outlet(&self, port: u8) -> Option<Port> {
        self.outlets.get(usize::from(port)).copied()
    }
}

const OUT: &[Port] = &[control("out")];
const LFO_IN: &[Port] = &[control("rate"), control("retrigger")];
const ENVELOPE_IN: &[Port] = &[control("gate")];
const STEP_IN: &[Port] = &[control("advance"), control("reset")];
const RANDOM_IN: &[Port] = &[control("trigger")];
const MATH_IN: &[Port] = &[control("in")];
const NOTES: &[Port] = &[note("notes")];

impl ModulatorKind {
    /// This kind's jacks.
    pub const fn ports(self) -> Ports {
        match self {
            Self::Lfo => Ports {
                inlets: LFO_IN,
                outlets: OUT,
            },
            Self::Envelope => Ports {
                inlets: ENVELOPE_IN,
                outlets: OUT,
            },
            Self::Step => Ports {
                inlets: STEP_IN,
                outlets: OUT,
            },
            Self::Random => Ports {
                inlets: RANDOM_IN,
                outlets: OUT,
            },
            Self::Math => Ports {
                inlets: MATH_IN,
                outlets: OUT,
            },
        }
    }

    /// The inlet a module's single input arrives on, which is what
    /// [`InputSource`] stood for: the LFO's `retrigger`, the Envelope's
    /// `gate`, the Step's `advance`, the Random's `trigger` and Math's `in`.
    pub const fn input_port(self) -> u8 {
        match self {
            Self::Lfo => 1,
            Self::Envelope | Self::Step | Self::Random | Self::Math => 0,
        }
    }
}

/// What a song inlet tag is bound to. Step 06 of the plan adds the
/// transport and the generator outlets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InletSource {
    /// A channel's notes as a gate, by durable identity.
    Gate(crate::ChannelId),
}

/// The kinds of tag. `None` in a binding is the empty `[ ]` slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "tag")]
pub enum TagKind {
    /// A control source from the song.
    Inlet {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bind: Option<InletSource>,
    },
    /// A channel's notes into the patch: copied, or taken so only the patch's
    /// output plays (step 07).
    NotesIn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<crate::ChannelId>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        take: bool,
    },
    /// Notes out of the patch, played on a channel (step 07).
    NotesOut {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<crate::ChannelId>,
    },
}

impl TagKind {
    pub const fn ports(self) -> Ports {
        match self {
            Self::Inlet { .. } => Ports {
                inlets: &[],
                outlets: OUT,
            },
            Self::NotesIn { .. } => Ports {
                inlets: &[],
                outlets: NOTES,
            },
            Self::NotesOut { .. } => Ports {
                inlets: NOTES,
                outlets: &[],
            },
        }
    }

    /// The channel this tag names, bound or not.
    pub const fn channel(self) -> Option<crate::ChannelId> {
        match self {
            Self::Inlet {
                bind: Some(InletSource::Gate(channel)),
            } => Some(channel),
            Self::Inlet { bind: None } => None,
            Self::NotesIn { channel, .. } | Self::NotesOut { channel } => channel,
        }
    }

    /// The same tag, unbound: what a tag naming a channel that is gone
    /// becomes.
    pub const fn unbound(self) -> Self {
        match self {
            Self::Inlet { .. } => Self::Inlet { bind: None },
            Self::NotesIn { take, .. } => Self::NotesIn {
                channel: None,
                take,
            },
            Self::NotesOut { .. } => Self::NotesOut { channel: None },
        }
    }
}

/// A song inlet or outlet on the canvas: a box with no face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SongTag {
    pub id: ModSourceId,
    #[serde(default)]
    pub at: CanvasPoint,
    #[serde(flatten)]
    pub kind: TagKind,
}

/// One jack of one box or tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Jack {
    pub node: ModSourceId,
    #[serde(default)]
    pub port: u8,
}

impl Jack {
    pub const fn new(node: ModSourceId, port: u8) -> Self {
        Self { node, port }
    }
}

/// Which way a dragged bend runs (step 05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BendAxis {
    /// The bend is a horizontal segment at `at` on the y axis.
    Horizontal,
    /// The bend is a vertical segment at `at` on the x axis.
    Vertical,
}

/// A cable bend dragged by hand. Unset means automatic routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bend {
    pub axis: BendAxis,
    pub at: i32,
}

/// A wire from one outlet to one inlet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Wire {
    pub from: Jack,
    pub to: Jack,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bend: Option<Bend>,
}

/// Why a wire was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireRefusal {
    /// One end names no box or tag, or a port the kind does not have.
    NoSuchJack,
    /// A control outlet into a note inlet, or the reverse.
    WrongSort,
    /// A box wired into itself.
    IntoItself,
}

/// Where the next box goes when nobody placed it: a grid in list order, the
/// way the module grid laid them out, right of the tag column.
pub const GRID_COLUMNS: i32 = 4;
pub const GRID_ORIGIN: CanvasPoint = CanvasPoint::new(160, 32);
pub const GRID_STEP: CanvasPoint = CanvasPoint::new(160, 112);
/// Where the tags a conversion makes go: a column at the canvas's left edge.
pub const TAG_COLUMN_X: i32 = 16;

/// The canvas place of the box at list position `index` in the default grid.
pub fn grid_place(index: usize) -> CanvasPoint {
    let index = i32::try_from(index).unwrap_or(i32::MAX / GRID_STEP.y);
    CanvasPoint::new(
        GRID_ORIGIN.x + (index % GRID_COLUMNS) * GRID_STEP.x,
        GRID_ORIGIN.y + (index / GRID_COLUMNS) * GRID_STEP.y,
    )
}

impl SongModulation {
    pub fn tag(&self, id: ModSourceId) -> Option<&SongTag> {
        self.tags.iter().find(|tag| tag.id == id)
    }

    /// The jacks of the box or tag `node`, if it is in the song.
    pub fn ports_of(&self, node: ModSourceId) -> Option<Ports> {
        self.module(node)
            .map(|module| module.params.kind().ports())
            .or_else(|| self.tag(node).map(|tag| tag.kind.ports()))
    }

    /// The wire into `inlet`, if there is one.
    pub fn wire_into(&self, inlet: Jack) -> Option<&Wire> {
        self.wires.iter().find(|wire| wire.to == inlet)
    }

    /// Whether a wire from `from` to `to` could exist, ignoring what is
    /// wired already.
    pub fn check_wire(&self, from: Jack, to: Jack) -> Result<(), WireRefusal> {
        if from.node == to.node {
            return Err(WireRefusal::IntoItself);
        }
        let outlet = self
            .ports_of(from.node)
            .and_then(|ports| ports.outlet(from.port))
            .ok_or(WireRefusal::NoSuchJack)?;
        let inlet = self
            .ports_of(to.node)
            .and_then(|ports| ports.inlet(to.port))
            .ok_or(WireRefusal::NoSuchJack)?;
        if outlet.sort != inlet.sort {
            return Err(WireRefusal::WrongSort);
        }
        Ok(())
    }

    /// Wire `from` into `to`. An inlet takes one wire, so one already there
    /// is replaced (its bend goes with it). A wire that closes a loop is
    /// allowed; how it runs is the engine's to say.
    pub fn connect(&mut self, from: Jack, to: Jack) -> Result<(), WireRefusal> {
        self.check_wire(from, to)?;
        self.wires.retain(|wire| wire.to != to);
        self.wires.push(Wire {
            from,
            to,
            bend: None,
        });
        Ok(())
    }

    /// Remove the wire into `inlet`. Returns whether there was one.
    pub fn disconnect(&mut self, inlet: Jack) -> bool {
        let before = self.wires.len();
        self.wires.retain(|wire| wire.to != inlet);
        self.wires.len() != before
    }

    /// Move a box or tag. Returns whether it is in the song.
    pub fn move_node(&mut self, node: ModSourceId, at: CanvasPoint) -> bool {
        if let Some(module) = self.module_mut(node) {
            module.at = at;
            return true;
        }
        match self.tags.iter_mut().find(|tag| tag.id == node) {
            Some(tag) => {
                tag.at = at;
                true
            }
            None => false,
        }
    }

    /// Add a tag of `kind` at `at`, and return its id.
    pub fn add_tag(&mut self, kind: TagKind, at: CanvasPoint) -> ModSourceId {
        let id = self.mint();
        self.tags.push(SongTag { id, at, kind });
        id
    }

    /// Remove a tag and its wires. Returns whether it was there.
    pub fn remove_tag(&mut self, id: ModSourceId) -> bool {
        let before = self.tags.len();
        self.tags.retain(|tag| tag.id != id);
        if self.tags.len() == before {
            return false;
        }
        self.drop_wires_of(id);
        true
    }

    pub(crate) fn drop_wires_of(&mut self, node: ModSourceId) {
        self.wires
            .retain(|wire| wire.from.node != node && wire.to.node != node);
    }

    /// The module's single input, read from the wire into its input inlet
    /// ([`ModulatorKind::input_port`]): a gate tag is a channel's notes, a
    /// module is that module, anything else (no wire, an unbound tag) is
    /// nothing.
    pub fn input_of(&self, id: ModSourceId) -> InputSource {
        let Some(module) = self.module(id) else {
            return InputSource::None;
        };
        let inlet = Jack::new(id, module.params.kind().input_port());
        let Some(wire) = self.wire_into(inlet) else {
            return InputSource::None;
        };
        if self.module(wire.from.node).is_some() {
            return InputSource::Module(wire.from.node);
        }
        match self.tag(wire.from.node).map(|tag| tag.kind) {
            Some(TagKind::Inlet {
                bind: Some(InletSource::Gate(channel)),
            }) => InputSource::ChannelNotes(channel),
            _ => InputSource::None,
        }
    }

    /// Wire module `id`'s input inlet to `input`: a channel's notes come from
    /// that channel's gate tag, made at the tag column if the song has none;
    /// a module from its outlet. `None` removes the wire. Returns whether the
    /// module is in the song.
    pub fn set_input(&mut self, id: ModSourceId, input: InputSource) -> bool {
        let Some(module) = self.module(id) else {
            return false;
        };
        let inlet = Jack::new(id, module.params.kind().input_port());
        let at_y = module.at.y;
        let from = match input {
            InputSource::None => {
                self.disconnect(inlet);
                return true;
            }
            InputSource::Module(read) => Jack::new(read, 0),
            InputSource::ChannelNotes(channel) => Jack::new(self.gate_tag(channel, at_y), 0),
        };
        if self.connect(from, inlet).is_err() {
            self.disconnect(inlet);
        }
        true
    }

    /// The song's gate tag for `channel`, made at the tag column level with
    /// `y` if there is none. One tag per channel, shared by every box that
    /// reads it (Adam drew it that way).
    pub fn gate_tag(&mut self, channel: crate::ChannelId, y: i32) -> ModSourceId {
        let kind = TagKind::Inlet {
            bind: Some(InletSource::Gate(channel)),
        };
        match self.tags.iter().find(|tag| tag.kind == kind) {
            Some(tag) => tag.id,
            None => self.add_tag(kind, CanvasPoint::new(TAG_COLUMN_X, y)),
        }
    }

    /// The default place for a new box: the first grid cell nothing sits in.
    pub fn free_place(&self) -> CanvasPoint {
        (0..)
            .map(grid_place)
            .find(|place| {
                !self.modules.iter().any(|module| module.at == *place)
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulation::ModulatorParams;
    use crate::ChannelId;

    fn song_with(kinds: &[ModulatorKind]) -> (SongModulation, Vec<ModSourceId>) {
        let mut song = SongModulation::default();
        let ids = kinds
            .iter()
            .map(|kind| {
                song.add_module(kind.default_params(), InputSource::None, ChannelId(1), "Kick 1")
            })
            .collect();
        (song, ids)
    }

    #[test]
    fn every_kind_has_its_input_inlet() {
        for kind in ModulatorKind::ALL {
            let port = kind.ports().inlet(kind.input_port());
            assert!(port.is_some(), "{kind:?}'s input port is not one of its inlets");
            assert_eq!(port.unwrap().sort, JackSort::Control);
        }
    }

    #[test]
    fn an_input_is_a_wire_and_reads_back() {
        let (mut song, ids) = song_with(&[ModulatorKind::Lfo, ModulatorKind::Envelope, ModulatorKind::Math]);
        let (lfo, envelope, math) = (ids[0], ids[1], ids[2]);
        let kick = ChannelId(1);
        assert!(song.set_input(lfo, InputSource::ChannelNotes(kick)));
        assert!(song.set_input(envelope, InputSource::ChannelNotes(kick)));
        assert!(song.set_input(math, InputSource::Module(lfo)));

        assert_eq!(song.tags.len(), 1, "two boxes reading the kick share one gate tag");
        assert_eq!(song.input_of(lfo), InputSource::ChannelNotes(kick));
        assert_eq!(song.input_of(envelope), InputSource::ChannelNotes(kick));
        assert_eq!(song.input_of(math), InputSource::Module(lfo));
        assert_eq!(
            song.wire_into(Jack::new(lfo, 1)).map(|wire| wire.from.node),
            Some(song.tags[0].id),
            "an LFO's notes arrive on its retrigger inlet",
        );

        assert!(song.set_input(math, InputSource::None));
        assert_eq!(song.input_of(math), InputSource::None);
        assert_eq!(song.wires.len(), 2);
    }

    #[test]
    fn an_inlet_takes_one_wire() {
        let (mut song, ids) = song_with(&[ModulatorKind::Lfo, ModulatorKind::Lfo, ModulatorKind::Math]);
        let math_in = Jack::new(ids[2], 0);
        song.connect(Jack::new(ids[0], 0), math_in).unwrap();
        song.connect(Jack::new(ids[1], 0), math_in).unwrap();
        assert_eq!(song.wires.len(), 1);
        assert_eq!(song.input_of(ids[2]), InputSource::Module(ids[1]));
    }

    #[test]
    fn a_wire_must_join_two_real_jacks_of_one_sort() {
        let (mut song, ids) = song_with(&[ModulatorKind::Lfo, ModulatorKind::Math]);
        let notes = song.add_tag(
            TagKind::NotesIn {
                channel: Some(ChannelId(1)),
                take: false,
            },
            CanvasPoint::default(),
        );
        assert_eq!(
            song.connect(Jack::new(notes, 0), Jack::new(ids[1], 0)),
            Err(WireRefusal::WrongSort),
        );
        assert_eq!(
            song.connect(Jack::new(ids[0], 0), Jack::new(ids[0], 1)),
            Err(WireRefusal::IntoItself),
        );
        assert_eq!(
            song.connect(Jack::new(ids[0], 3), Jack::new(ids[1], 0)),
            Err(WireRefusal::NoSuchJack),
        );
        assert_eq!(
            song.connect(Jack::new(ids[0], 0), Jack::new(ModSourceId(999), 0)),
            Err(WireRefusal::NoSuchJack),
        );
        // A loop is allowed.
        song.connect(Jack::new(ids[0], 0), Jack::new(ids[1], 0)).unwrap();
        song.connect(Jack::new(ids[1], 0), Jack::new(ids[0], 0)).unwrap();
        assert_eq!(song.wires.len(), 2);
    }

    #[test]
    fn removing_a_box_or_a_tag_takes_its_wires() {
        let (mut song, ids) = song_with(&[ModulatorKind::Lfo, ModulatorKind::Math]);
        song.set_input(ids[0], InputSource::ChannelNotes(ChannelId(1)));
        song.set_input(ids[1], InputSource::Module(ids[0]));
        let tag = song.tags[0].id;
        assert!(song.remove_tag(tag));
        assert_eq!(song.input_of(ids[0]), InputSource::None);
        assert!(song.remove_module(ids[0]));
        assert!(song.wires.is_empty());
    }

    /// A song saved by song modulation, before the patch: an Envelope gated
    /// by the kick, an LFO retriggered by the kick and a Math reading the
    /// LFO load as one shared gate tag, three wires and the same routes, laid
    /// out in the grid, and save and load again unchanged.
    #[test]
    fn a_song_saved_with_inputs_loads_as_wires() {
        use crate::modulation::{ModPolarity, ModRoute, UNRESOLVED_SLOT};
        use crate::{EffectTarget, ModSourceRef, ParamAddr, STRIP_PARAM_VOLUME};
        let kick = ChannelId(5);
        let (mut song, ids) =
            song_with(&[ModulatorKind::Envelope, ModulatorKind::Lfo, ModulatorKind::Math]);
        let inputs = [
            InputSource::ChannelNotes(kick),
            InputSource::ChannelNotes(kick),
            InputSource::Module(ids[1]),
        ];
        for (id, input) in ids.iter().zip(inputs) {
            song.set_input(*id, input);
        }
        for (id, depth) in [(ids[0], 0.25), (ids[2], 0.5)] {
            song.routes.push(ModRoute {
                source: ModSourceRef::Id(id),
                source_slot: UNRESOLVED_SLOT,
                destination: ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME),
                depth,
                polarity: ModPolarity::Bipolar,
            });
        }

        // The same song in the shape `main` wrote before this step: an
        // `input` on each module, no place, no tags and no wires.
        let mut old = toml::Value::try_from(&song).unwrap();
        let table = old.as_table_mut().unwrap();
        table.remove("tags");
        table.remove("wires");
        let modules = table.get_mut("modules").unwrap().as_array_mut().unwrap();
        for (module, input) in modules.iter_mut().zip(inputs) {
            let module = module.as_table_mut().unwrap();
            module.remove("at");
            module.insert("input".into(), toml::Value::try_from(input).unwrap());
        }
        let loaded: SongModulation = old.try_into().unwrap();

        assert_eq!(loaded.tags.len(), 1, "one gate tag for the kick, shared");
        assert_eq!(
            loaded.tags[0].kind,
            TagKind::Inlet {
                bind: Some(InletSource::Gate(kick))
            }
        );
        assert_eq!(loaded.wires.len(), 3);
        for (id, input) in ids.iter().zip(inputs) {
            assert_eq!(loaded.input_of(*id), input);
        }
        assert_eq!(loaded.routes, song.routes);
        for (index, module) in loaded.modules.iter().enumerate() {
            assert_eq!(module.at, grid_place(index), "laid out as the grid it showed");
        }
        assert!(
            loaded.tags[0].id.0 >= 3 && loaded.next_source_id > loaded.tags[0].id.0,
            "the tag's id is minted past the modules'"
        );

        let text = toml::to_string(&loaded).unwrap();
        assert!(!text.contains("\ninput ="), "nothing writes an input now:\n{text}");
        assert_eq!(toml::from_str::<SongModulation>(&text).unwrap(), loaded);
    }

    #[test]
    fn new_boxes_fill_the_grid() {
        let (song, _) = song_with(&[ModulatorKind::Lfo; 5]);
        let places: Vec<_> = song.modules.iter().map(|module| module.at).collect();
        assert_eq!(places[0], grid_place(0));
        assert_eq!(places[4], grid_place(4));
        assert_eq!(places[4].y, GRID_ORIGIN.y + GRID_STEP.y, "the fifth starts a row");
        let _ = ModulatorParams::Lfo(Default::default());
    }
}
