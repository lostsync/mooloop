//! Modulation edits: sources, routes, and the assignment gesture.
//!
//! The song owns the modulation set (`docs/plans/archive/song-modulation/`), and
//! these verbs edit it. Each returns whether it changed anything; none talks
//! to the engine. The pump's reconciler ([`Session::sync_modulation`])
//! resolves the song's set and sends what differs from what it last sent: a
//! retune as one small command, any change of shape as a whole new set.
//!
//! The modulation pane lists every source in the song in one order
//! ([`Session::modulation_sources`]): the song's modules, then each
//! channel's published control outlets and its keyboard. A source's place in
//! that list is the handle the pane's rows carry, and the only thing a
//! handle means; selection and arming name the source itself -- a module by
//! its durable id, an outlet or the keyboard by its channel -- so a reorder,
//! a channel change or a new module cannot retarget them.
//!
//! An outlet is not a module: nothing mints it, nothing reorders it, and it
//! cannot be removed. It is selectable and armable on the same terms, which
//! is what lets one selection and one assignment gesture serve both.

use crate::session::Session;
use mooloop_core::modulation::{
    MAX_GENERATOR_OUTLETS, PERFORMANCE_DESCRIPTORS, PERFORMANCE_SOURCES,
};
use mooloop_core::{
    Bend, CanvasPoint, CompiledSource, InletSource, InputSource, Jack, ModPolarity, ModRoute, ModSourceId,
    ModSourceRef, ModulatorKind, ModulatorParams, OutletDescriptor, PublishesOutlets, TagKind,
    WireRefusal,
};

/// What an inlet tag's list adds to a source read a block late.
pub(crate) const LATE_NOTE: &str = " (a block late)";

/// What can feed an inlet, as the canvas's inlet picker offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchFeed {
    /// Nothing: the inlet's wire is removed.
    None,
    /// A channel's notes as a gate, through the song's gate tag for it.
    Gate(mooloop_core::ChannelId),
    /// An outlet already on the canvas.
    Outlet(Jack),
}

/// Why the session refused a wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchRefusal {
    /// The wire itself cannot exist ([`WireRefusal`]).
    Wire(WireRefusal),
    /// The inlet already has a wire.
    InletTaken,
}

/// What a list of every module in the song calls `module`: its name, or its
/// kind for one that has none.
fn module_name(module: &mooloop_core::SongModule) -> String {
    if module.name.is_empty() {
        module.params.kind().label().to_string()
    } else {
        module.name.clone()
    }
}

/// Every source's latest output, as the knobs read it: each module by its
/// position in the set the engine runs ([`Session::sync_modulation`]), each
/// channel's outlets and keyboard by its seat, and each tag.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModulationLevels {
    pub modules: Vec<f32>,
    pub channels: Vec<([f32; MAX_GENERATOR_OUTLETS], [f32; PERFORMANCE_SOURCES])>,
    /// Each tag's count of NoteOns and value, in the set last sent's tag
    /// order.
    pub tags: Vec<(u32, f32)>,
}

impl ModulationLevels {
    /// The output of one resolved source; zero for one not read.
    pub fn level(&self, source: CompiledSource) -> f32 {
        match source {
            CompiledSource::Module(at) => self.modules.get(usize::from(at)).copied(),
            CompiledSource::Outlet { seat, outlet } => self
                .channels
                .get(usize::from(seat))
                .and_then(|(outlets, _)| outlets.get(usize::from(outlet)).copied()),
            CompiledSource::Performance { seat, source } => self
                .channels
                .get(usize::from(seat))
                .and_then(|(_, performance)| performance.get(usize::from(source)).copied()),
            CompiledSource::Tag(at) => self.tags.get(usize::from(at)).map(|&(_, value)| value),
        }
        .unwrap_or(0.0)
    }
}

impl Session {
    /// The address of the selected channel's generator parameter `param`, as
    /// the device the channel runs now -- which is what a press on the
    /// source face means. `None` when nothing is selected.
    ///
    /// The one place the window builds a generator address from a face
    /// callback, so the kind cannot be left out of one (MOO-135).
    pub fn selected_source_address(&self, param: u32) -> Option<mooloop_core::ParamAddr> {
        let state = self.channels.get(self.selected)?;
        Some(mooloop_core::ParamAddr::source(
            mooloop_core::EffectTarget::Channel(self.selected as u8),
            state.kind(),
            param,
        ))
    }

    /// Every source the modulation pane lists, in its order: the song's
    /// boxes in list order, then its bound inlet tags in theirs. A channel's
    /// generator outlets and keyboard are read through tags (song patch step
    /// 06), so they are sources once a tag reads them.
    pub fn modulation_sources(&self) -> Vec<ModSourceRef> {
        let modules = self.modulation.modules.iter().map(|module| ModSourceRef::Id(module.id));
        let tags = self.modulation.tags.iter().filter_map(|tag| match tag.kind {
            TagKind::Inlet { bind: Some(_) } => Some(ModSourceRef::Id(tag.id)),
            _ => None,
        });
        modules.chain(tags).collect()
    }

    /// The source at `index` in [`Self::modulation_sources`].
    pub fn modulation_source_at(&self, index: usize) -> Option<ModSourceRef> {
        self.modulation_sources().get(index).copied()
    }

    /// Where `source` sits in [`Self::modulation_sources`], if the song still
    /// has it.
    pub fn modulation_source_index(&self, source: ModSourceRef) -> Option<usize> {
        self.modulation_sources().iter().position(|listed| *listed == source)
    }

    /// The selected source's place in the pane's list.
    pub fn modulation_selected_index(&self) -> Option<usize> {
        self.modulation_selected
            .get()
            .and_then(|source| self.modulation_source_index(source))
    }

    /// The armed source's place in the pane's list.
    pub fn modulation_armed_index(&self) -> Option<usize> {
        self.modulation_armed
            .get()
            .and_then(|source| self.modulation_source_index(source))
    }

    /// Drop a selection or an arming whose source the song no longer has: a
    /// removed module, a channel gone, or an outlet the channel's new
    /// generator does not publish.
    pub fn forget_gone_modulation_sources(&self) {
        for cell in [&self.modulation_selected, &self.modulation_armed] {
            if cell
                .get()
                .is_some_and(|source| self.modulation_source_index(source).is_none())
            {
                cell.set(None);
            }
        }
    }

    /// The declaration of an outlet or keyboard source, or of the tag that
    /// reads one; `None` for anything else and for an outlet its channel's
    /// generator does not publish.
    pub fn modulation_outlet(&self, source: ModSourceRef) -> Option<&'static OutletDescriptor> {
        match self.modulation.channel_source(source) {
            ModSourceRef::GeneratorOutlet { channel, outlet } => {
                let state = self.channels.get(self.channel_index(channel)?)?;
                state.kind().control_outlet(outlet)
            }
            ModSourceRef::Performance { channel, source } => {
                self.channel_index(channel)?;
                PERFORMANCE_DESCRIPTORS.get(usize::from(source))
            }
            ModSourceRef::Id(_) | ModSourceRef::LocalSlot(_) => None,
        }
    }

    /// The channel an outlet or keyboard source, or the tag that reads one,
    /// belongs to, by name.
    pub fn modulation_source_channel(&self, source: ModSourceRef) -> Option<&str> {
        let channel = match self.modulation.channel_source(source) {
            ModSourceRef::GeneratorOutlet { channel, .. } | ModSourceRef::Performance { channel, .. } => {
                channel
            }
            ModSourceRef::Id(_) | ModSourceRef::LocalSlot(_) => return None,
        };
        Some(self.channels.get(self.channel_index(channel)?)?.name.as_str())
    }

    /// The source's live output, from the last read off the engine
    /// ([`Self::read_modulation_levels`]): what a tile's meter draws.
    pub fn modulation_source_level(&self, source: ModSourceRef) -> f32 {
        let resolved = match source {
            ModSourceRef::Id(id) => match self.modulation_sent.position_of(id) {
                Some(at) => u16::try_from(at).ok().map(CompiledSource::Module),
                None => self
                    .modulation_sent
                    .tag_position_of(id)
                    .and_then(|at| u16::try_from(at).ok())
                    .map(CompiledSource::Tag),
            },
            ModSourceRef::GeneratorOutlet { channel, outlet } => self
                .channel_index(channel)
                .and_then(|seat| u8::try_from(seat).ok())
                .zip(u8::try_from(outlet).ok())
                .map(|(seat, outlet)| CompiledSource::Outlet { seat, outlet }),
            ModSourceRef::Performance { channel, source } => self
                .channel_index(channel)
                .and_then(|seat| u8::try_from(seat).ok())
                .zip(u8::try_from(source).ok())
                .map(|(seat, source)| CompiledSource::Performance { seat, source }),
            ModSourceRef::LocalSlot(_) => None,
        };
        resolved.map_or(0.0, |source| self.modulation_levels.borrow().level(source))
    }

    /// Read every source's output off the engine: `module` for a list
    /// position in the set last sent, `channel` for a seat, `tag` for a
    /// tag's count of NoteOns and value by its place among the tags. Returns whether
    /// anything moved, which is when the knobs and wires need redrawing.
    pub fn read_modulation_levels(
        &self,
        module: impl Fn(usize) -> f32,
        channel: impl Fn(usize) -> ([f32; MAX_GENERATOR_OUTLETS], [f32; PERFORMANCE_SOURCES]),
        tag: impl Fn(usize) -> (u32, f32),
    ) -> bool {
        let mut levels = self.modulation_levels.borrow_mut();
        let modules = self.modulation_sent.modules.len();
        let tags = self.modulation_sent.tags.len();
        let seats = self.channels.len();
        let mut moved = levels.modules.len() != modules
            || levels.channels.len() != seats
            || levels.tags.len() != tags;
        levels.tags.resize(tags, (0, 0.0));
        for (at, value) in levels.tags.iter_mut().enumerate() {
            let next = tag(at);
            moved |= *value != next;
            *value = next;
        }
        levels.modules.resize(modules, 0.0);
        levels.channels.resize(seats, ([0.0; MAX_GENERATOR_OUTLETS], [0.0; PERFORMANCE_SOURCES]));
        for (at, value) in levels.modules.iter_mut().enumerate() {
            let next = module(at);
            moved |= *value != next;
            *value = next;
        }
        for (seat, value) in levels.channels.iter_mut().enumerate() {
            let next = channel(seat);
            moved |= *value != next;
            *value = next;
        }
        moved
    }

    /// What patch node `node` is putting on its outlet as of the last read:
    /// a box's value, or a tag's value (a gate tag's is 1 while a note is
    /// held) and how many NoteOns it has passed. Zero for a node the engine
    /// does not run.
    pub fn patch_node_activity(&self, node: ModSourceId) -> (f32, u32) {
        if self.modulation.module(node).is_some() {
            return (self.modulation_source_level(ModSourceRef::Id(node)), 0);
        }
        let (notes, value) = self
            .modulation_sent
            .tags
            .iter()
            .position(|tag| tag.id == node)
            .and_then(|at| self.modulation_levels.borrow().tags.get(at).copied())
            .unwrap_or_default();
        (value, notes)
    }

    /// The live offset the routes onto `destination` add right now, as a
    /// fraction of its range, under `policy`.
    pub fn live_offset(
        &self,
        destination: mooloop_core::ParamAddr,
        policy: &mooloop_core::ModDestinationDescriptor,
    ) -> f32 {
        let levels = self.modulation_levels.borrow();
        self.modulation_sent
            .offset_for(destination, policy, |source| levels.level(source))
    }

    /// How many of the song's routes land on `destination`, whatever drives
    /// them: the dots under a knob.
    pub fn route_count(&self, destination: mooloop_core::ParamAddr) -> usize {
        self.modulation
            .routes
            .iter()
            .filter(|route| route.destination == destination)
            .count()
    }

    /// Select the source at `index` in the pane's list, or clear the
    /// selection.
    pub fn set_modulation_selected_index(&self, index: Option<usize>) {
        self.modulation_selected
            .set(index.and_then(|index| self.modulation_source_at(index)));
    }

    /// Arm the source at `index` in the pane's list, or disarm.
    pub fn set_modulation_armed_index(&self, index: Option<usize>) {
        self.modulation_armed
            .set(index.and_then(|index| self.modulation_source_at(index)));
    }

    /// Whether a direct modulation-knob gesture is currently open.
    pub fn gesture_open(&self) -> bool {
        self.gesture_before.is_some()
    }

    /// Opens a source's editor in the pane.
    ///
    /// Selection is separate from assignment, so looking at an LFO does not
    /// hijack knob gestures anywhere. If assignment is already active it
    /// follows the newly selected source; otherwise this has no effect on
    /// ordinary parameter edits.
    ///
    /// A published outlet is selectable on the same terms as a module. It has
    /// no editor -- the device that publishes it owns its behaviour, and the
    /// pane shows its declaration instead -- but it is armable.
    pub fn select_modulation_source(&mut self, index: i32) -> bool {
        let Some(source) = usize::try_from(index)
            .ok()
            .and_then(|index| self.modulation_source_at(index))
        else {
            return false;
        };
        self.modulation_selected.set(Some(source));
        if self.modulation_armed.get().is_some() {
            self.modulation_armed.set(Some(source));
        }
        true
    }

    /// Moves the module at `index` to `target` in the song's list: the order
    /// the engine ticks them in, so which modules a Math module reads this
    /// tick. Only modules move; an outlet's place is its channel's.
    ///
    /// Selection and arming name modules by durable id, so they follow the
    /// module. The engine carries every module's running state across the
    /// new order by identity.
    pub fn move_modulation_source(&mut self, index: i32, target: i32) -> bool {
        let (Ok(index), Ok(target)) = (usize::try_from(index), usize::try_from(target)) else {
            return false;
        };
        self.modulation.move_module(index, target)
    }

    /// Arms or disarms the assignment gesture.
    ///
    /// Returns the armed source's name, or `None` when assignment is now off
    /// -- which is also the answer when nothing was selected to arm.
    pub fn toggle_modulation_assignment(&mut self) -> Option<String> {
        let next = if self.modulation_armed.get().is_some() {
            None
        } else {
            self.modulation_selected.get()
        };
        self.modulation_armed.set(next);
        self.modulation_source_name(next?)
    }

    /// What the pane and the assignment badge call `source`: a module by its
    /// song-wide name, an outlet or the keyboard by its channel and its own
    /// name. `None` for a source the song no longer has.
    pub fn modulation_source_name(&self, source: ModSourceRef) -> Option<String> {
        match source {
            ModSourceRef::Id(id) => match self.modulation.module(id) {
                Some(module) => Some(module_name(module)),
                // What it reads, without the picker's note that it reads it
                // late: the tag on the canvas says that.
                None => match self.modulation.tag(id)?.kind {
                    TagKind::Inlet { bind: Some(source) } => self
                        .inlet_source_name(source)
                        .map(|name| name.trim_end_matches(LATE_NOTE).to_string()),
                    _ => None,
                },
            },
            ModSourceRef::GeneratorOutlet { .. } | ModSourceRef::Performance { .. } => {
                let outlet = self.modulation_outlet(source)?;
                let channel = self.modulation_source_channel(source)?;
                Some(format!("{channel} {}", outlet.name))
            }
            ModSourceRef::LocalSlot(_) => None,
        }
    }

    /// Adds a new module to the song, made on the selected channel, selects
    /// it and disarms.
    ///
    /// Its input defaults to the selected channel's notes, so an Envelope
    /// added with a channel selected gates from that channel and an LFO
    /// retriggers from it. A Math module reads nothing until one is picked.
    /// There is no limit on how many a song has.
    pub fn add_modulation_source(&mut self, kind: ModulatorKind) -> bool {
        let Some(channel) = self.channels.get(self.selected) else {
            return false;
        };
        let (home, name) = (channel.id, channel.name.clone());
        let input = match kind {
            ModulatorKind::Math => InputSource::None,
            _ => InputSource::ChannelNotes(home),
        };
        let id = self
            .modulation
            .add_module(kind.default_params(), input, home, &name);
        self.modulation_selected.set(Some(ModSourceRef::Id(id)));
        self.modulation_armed.set(None);
        true
    }

    /// Renames the module at `index`. Empty, or what it is already called,
    /// changes nothing.
    pub fn rename_modulation_source(&mut self, index: i32, name: &str) -> bool {
        let name = name.trim();
        let Some(id) = self.module_at(index) else {
            return false;
        };
        let Some(module) = self.modulation.module_mut(id) else {
            return false;
        };
        if name.is_empty() || module.name == name {
            return false;
        }
        module.name = name.to_string();
        true
    }

    /// Sets one parameter of the module at `index`. `false` when nothing
    /// moved.
    ///
    /// A change inside an open knob gesture is marked so the gesture's own
    /// undo entry is recorded on release rather than one per frame. The
    /// reconciler sends it as one retune, because a knob drag makes one of
    /// these a frame.
    pub fn set_modulator_param(&mut self, index: i32, id: i32, value: f32) -> bool {
        let Ok(id) = u32::try_from(id) else {
            return false;
        };
        let Some(source) = self.module_at(index) else {
            return false;
        };
        let Some(module) = self.modulation.module_mut(source) else {
            return false;
        };
        let previous = module.params.get(id);
        module.params.set(id, value);
        if module.params.get(id) == previous {
            return false;
        }
        // A select set to fewer inputs, or an arithmetic box made a clip,
        // loses the jacks it no longer has and the wires in them.
        self.modulation.drop_dead_wires();
        if self.gesture_open() {
            self.gesture_changed = true;
        }
        true
    }

    /// Removes the module at `index` from the song and everything routed
    /// from it.
    pub fn remove_modulation_source(&mut self, index: i32) -> bool {
        let Some(source) = self.module_at(index) else {
            return false;
        };
        if !self.modulation.remove_module(source) {
            return false;
        }
        let gone = Some(ModSourceRef::Id(source));
        if self.modulation_selected.get() == gone {
            self.modulation_selected.set(None);
        }
        if self.modulation_armed.get() == gone {
            self.modulation_armed.set(None);
        }
        true
    }

    /// Points a module's input at an outlet: the Envelope's gate, the LFO's
    /// retrigger, the Step's advance and the Random's trigger take a
    /// channel's notes; a Math module takes another module's output.
    ///
    /// Refuses an input of the wrong sort for the module, a channel or a
    /// module the song does not have, and a Math module reading itself.
    pub fn set_module_input(&mut self, id: ModSourceId, input: InputSource) -> bool {
        let Some(module) = self.modulation.module(id) else {
            return false;
        };
        let math = matches!(module.params, ModulatorParams::Math(_));
        let fits = match input {
            InputSource::None => true,
            InputSource::ChannelNotes(channel) => !math && self.channel_index(channel).is_some(),
            InputSource::Module(read) => {
                math && read != id && self.modulation.module(read).is_some()
            }
        };
        if !fits || self.modulation.input_of(id) == input {
            return false;
        }
        self.modulation.set_input(id, input)
    }

    /// Adds a new box of `kind` at `at` on the patch canvas: what
    /// [`Self::add_modulation_source`] does, placed where it was asked for.
    pub fn add_patch_box(&mut self, kind: ModulatorKind, at: CanvasPoint) -> Option<ModSourceId> {
        if !self.add_modulation_source(kind) {
            return None;
        }
        let id = self.modulation.modules.last()?.id;
        self.modulation.move_node(id, at);
        Some(id)
    }

    /// Makes the box `text` names at `at`, unwired (song patch step 04): its
    /// first word is the box and what follows its settings
    /// ([`mooloop_core::box_text::parse`]). A name the vocabulary does not
    /// have makes an unknown box that keeps `text`. `None` for empty text.
    pub fn type_patch_box(&mut self, text: &str, at: CanvasPoint) -> Option<ModSourceId> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let params = mooloop_core::box_text::parse(text).unwrap_or(ModulatorParams::Unknown);
        let (home, name) = self
            .channels
            .get(self.selected)
            .map(|channel| (channel.id, channel.name.clone()))
            .unwrap_or_default();
        let id = self
            .modulation
            .add_module(params, InputSource::None, home, &name);
        let module = self.modulation.module_mut(id)?;
        module.at = at;
        if params == ModulatorParams::Unknown {
            module.text = text.to_string();
        }
        self.modulation_selected.set(Some(ModSourceRef::Id(id)));
        Some(id)
    }

    /// Retypes box `id` to what `text` names. A box that stays its kind
    /// keeps its identity and takes the new settings, losing only the wires
    /// into jacks it no longer has. A box that becomes another kind is a new
    /// box in its place, with a new id: each wire follows it onto the jack
    /// of the same name, if it has one, and its routes follow it if it
    /// still has the outlet they came from. Returns the box's id after, or
    /// `None` when nothing changed (or `text` is empty).
    pub fn retype_patch_box(&mut self, id: ModSourceId, text: &str) -> Option<ModSourceId> {
        let text = text.trim();
        let old = self.modulation.module(id)?.clone();
        if text.is_empty() {
            return None;
        }
        let params = mooloop_core::box_text::parse(text).unwrap_or(ModulatorParams::Unknown);
        let unknown_text = if params == ModulatorParams::Unknown {
            text.to_string()
        } else {
            String::new()
        };
        if params.kind() == old.params.kind() {
            if params == old.params && unknown_text == old.text {
                return None;
            }
            let module = self.modulation.module_mut(id)?;
            module.params = params;
            module.text = unknown_text;
            self.modulation.drop_dead_wires();
            return Some(id);
        }
        let (home, name) = self
            .channels
            .get(self.selected)
            .map(|channel| (channel.id, channel.name.clone()))
            .unwrap_or_default();
        let new = self
            .modulation
            .add_module(params, InputSource::None, home, &name);
        // In the old box's place: on the canvas, and in the list, which is
        // the order the boxes are listed and tie-broken in.
        let index = self.modulation.modules.iter().position(|module| module.id == id)?;
        let mut module = self.modulation.modules.pop()?;
        module.at = old.at;
        module.text = unknown_text;
        self.modulation.modules.insert(index, module);
        // Wires, by jack name.
        let (before, after) = (old.params.ports(), params.ports());
        let same = |from: Option<mooloop_core::Port>, ports: &[mooloop_core::Port]| {
            from.and_then(|port| ports.iter().position(|other| other.name == port.name))
                .and_then(|port| u8::try_from(port).ok())
        };
        let mut wires = Vec::new();
        for wire in &self.modulation.wires {
            let mut wire = *wire;
            if wire.to.node == id {
                let Some(port) = same(before.inlet(wire.to.port), after.inlets) else {
                    continue;
                };
                wire.to = Jack::new(new, port);
            }
            if wire.from.node == id {
                let Some(port) = same(before.outlet(wire.from.port), after.outlets) else {
                    continue;
                };
                wire.from = Jack::new(new, port);
            }
            wires.push(wire);
        }
        self.modulation.wires = wires;
        // Routes, if the outlet they read is still there.
        let follows = same(before.outlet(0), after.outlets).is_some();
        let (from, to) = (ModSourceRef::Id(id), ModSourceRef::Id(new));
        if follows {
            for route in &mut self.modulation.routes {
                if route.source == from {
                    route.source = to;
                }
            }
            for place in &mut self.modulation.route_places {
                if place.source == from {
                    place.source = to;
                }
            }
        }
        self.modulation.route_places.retain(|place| place.source != from);
        self.modulation.remove_module(id);
        if self.modulation_selected.get() == Some(from) {
            self.modulation_selected.set(Some(to));
        }
        if self.modulation_armed.get() == Some(from) {
            self.modulation_armed.set(follows.then_some(to));
        }
        Some(new)
    }

    /// Moves boxes and tags on the canvas, all of them as one edit: a drag
    /// of a selection is one undo step. `false` when none of them moved.
    pub fn move_patch_nodes(&mut self, moves: &[(ModSourceId, CanvasPoint)]) -> bool {
        let mut moved = false;
        for &(node, at) in moves {
            let was = self
                .modulation
                .module(node)
                .map(|module| module.at)
                .or_else(|| self.modulation.tag(node).map(|tag| tag.at));
            // An unbound assignment that was never put anywhere moves too.
            let loose = self.modulation.loose_route(node).map(|route| route.at);
            if was.is_some_and(|was| was != at) || loose.is_some_and(|was| was != Some(at)) {
                moved |= self.modulation.move_node(node, at);
            }
        }
        moved
    }

    /// Opens or folds box `id`'s face on the canvas (song patch step 05).
    /// `false` when it was already so, or there is no such box.
    pub fn set_patch_box_open(&mut self, id: ModSourceId, open: bool) -> bool {
        match self.modulation.module_mut(id) {
            Some(module) if module.open != open => {
                module.open = open;
                true
            }
            _ => false,
        }
    }

    /// Sets one of box `id`'s settings from its face: what
    /// [`Self::set_modulator_param`] does, by the box rather than its place
    /// in the list.
    pub fn set_patch_box_param(&mut self, id: ModSourceId, param: u32, value: f32) -> bool {
        let Some(index) = self.modulation_source_index(ModSourceRef::Id(id)) else {
            return false;
        };
        let Ok(param) = i32::try_from(param) else {
            return false;
        };
        self.set_modulator_param(index as i32, param, value)
    }

    /// Removes a box or a tag from the song, with its wires; a box takes its
    /// routes too, and the selection and arming that named it.
    pub fn remove_patch_node(&mut self, node: ModSourceId) -> bool {
        if self.modulation.remove_loose(node) {
            return true;
        }
        if self.modulation.remove_tag(node) {
            return true;
        }
        match self.modulation_source_index(ModSourceRef::Id(node)) {
            Some(index) => self.remove_modulation_source(index as i32),
            None => false,
        }
    }

    /// Wires outlet `from` into inlet `to`. Refuses a jack the song does not
    /// have, a note jack into a control jack or the reverse, a box into
    /// itself, and an inlet that already has a wire: the canvas replaces one
    /// by disconnecting it first, in the same edit. A wire that closes a
    /// loop is allowed.
    pub fn connect_patch(&mut self, from: Jack, to: Jack) -> Result<(), PatchRefusal> {
        self.modulation.check_wire(from, to).map_err(PatchRefusal::Wire)?;
        if self.modulation.wire_into(to).is_some() {
            return Err(PatchRefusal::InletTaken);
        }
        self.modulation.connect(from, to).map_err(PatchRefusal::Wire)
    }

    /// Sets or clears the hand-placed bend of the wire into inlet `to`: the
    /// canvas's drag of a cable's middle. `false` when nothing changed.
    pub fn set_wire_bend(&mut self, to: Jack, bend: Option<Bend>) -> bool {
        self.modulation.bend_wire(to, bend)
    }

    /// Removes the wire into inlet `to`. `false` when there was none.
    pub fn disconnect_patch(&mut self, to: Jack) -> bool {
        self.modulation.disconnect(to)
    }

    /// Adds a tag of `kind` at `at`. Refuses one naming a channel the song
    /// does not have; an unbound tag is the empty `[ ]` slot and always fits.
    pub fn add_patch_tag(&mut self, kind: TagKind, at: CanvasPoint) -> Option<ModSourceId> {
        if kind.channel().is_some_and(|channel| self.channel_index(channel).is_none()) {
            return None;
        }
        Some(self.modulation.add_tag(kind, at))
    }

    /// Wires outlet `from` into inlet `to`, replacing whatever fed it: the
    /// canvas's drop. `Ok(false)` when that wire is already there.
    pub fn rewire_patch(&mut self, from: Jack, to: Jack) -> Result<bool, PatchRefusal> {
        self.modulation
            .check_wire(from, to)
            .map_err(PatchRefusal::Wire)?;
        if self
            .modulation
            .wire_into(to)
            .is_some_and(|wire| wire.from == from)
        {
            return Ok(false);
        }
        self.modulation.disconnect(to);
        self.connect_patch(from, to).map(|()| true)
    }

    /// Puts the assignment tags of the routes at these indices in the song's
    /// route list where they were dropped. `false` when none moved.
    pub fn place_patch_routes(&mut self, places: &[(usize, CanvasPoint)]) -> bool {
        let mut moved = false;
        for &(index, at) in places {
            let Some(route) = self.modulation.routes.get(index).copied() else {
                continue;
            };
            if self.modulation.route_at(route.source, route.destination) != Some(at) {
                moved |= self
                    .modulation
                    .place_route(route.source, route.destination, at);
            }
        }
        moved
    }

    /// Removes a canvas selection in one edit: the wires into `inlets`, the
    /// routes at `routes` (indices into the song's route list), and the boxes
    /// and tags `nodes`, with their wires and routes. `false` when none of it
    /// was there.
    pub fn remove_patch_selection(
        &mut self,
        nodes: &[ModSourceId],
        inlets: &[Jack],
        routes: &[usize],
    ) -> bool {
        let mut changed = false;
        for &inlet in inlets {
            changed |= self.modulation.disconnect(inlet);
        }
        let mut routes = routes.to_vec();
        routes.sort_unstable();
        routes.dedup();
        for index in routes.into_iter().rev() {
            changed |= self.remove_route(index as i32);
        }
        for &node in nodes {
            changed |= self.remove_patch_node(node);
        }
        changed
    }

    /// What the inlet picker offers for inlet `to`, in order, with the name
    /// each is listed under: **None**, every channel's notes as a gate and
    /// every other bound inlet tag when the inlet takes control, then every
    /// other box's outlet of the inlet's sort.
    pub fn patch_feed_options(&self, to: Jack) -> Vec<(PatchFeed, String)> {
        let Some(inlet) = self
            .modulation
            .ports_of(to.node)
            .and_then(|ports| ports.inlet(to.port))
        else {
            return Vec::new();
        };
        let mut options = vec![(PatchFeed::None, "None".to_string())];
        if inlet.sort == mooloop_core::JackSort::Control {
            options.extend(self.gate_outlets().filter_map(|(input, name)| match input {
                InputSource::ChannelNotes(channel) => Some((PatchFeed::Gate(channel), name)),
                _ => None,
            }));
        }
        if inlet.sort == mooloop_core::JackSort::Control {
            for tag in &self.modulation.tags {
                let TagKind::Inlet { bind: Some(source) } = tag.kind else {
                    continue;
                };
                if matches!(source, InletSource::Gate(_)) {
                    continue;
                }
                if let Some(name) = self.inlet_source_name(source) {
                    options.push((PatchFeed::Outlet(Jack::new(tag.id, 0)), name));
                }
            }
        }
        for module in &self.modulation.modules {
            if module.id == to.node {
                continue;
            }
            let ports = module.params.kind().ports();
            for (port, outlet) in ports.outlets.iter().enumerate() {
                if outlet.sort != inlet.sort {
                    continue;
                }
                let name = if ports.outlets.len() > 1 {
                    format!("{} · {}", module_name(module), outlet.name)
                } else {
                    module_name(module)
                };
                options.push((PatchFeed::Outlet(Jack::new(module.id, port as u8)), name));
            }
        }
        options
    }

    /// Where inlet `to`'s feed sits in [`Self::patch_feed_options`].
    pub fn patch_feed_choice(&self, to: Jack) -> usize {
        let current = match self.modulation.wire_into(to) {
            None => PatchFeed::None,
            Some(wire) => match self.modulation.tag(wire.from.node).map(|tag| tag.kind) {
                Some(TagKind::Inlet {
                    bind: Some(mooloop_core::InletSource::Gate(channel)),
                }) => PatchFeed::Gate(channel),
                _ => PatchFeed::Outlet(wire.from),
            },
        };
        self.patch_feed_options(to)
            .iter()
            .position(|(feed, _)| *feed == current)
            .unwrap_or(0)
    }

    /// Feeds inlet `to` from `feed`, replacing its wire: a channel's gate
    /// comes through the song's gate tag for it, made beside the box if the
    /// song has none. Returns whether anything changed.
    pub fn feed_patch_inlet(&mut self, to: Jack, feed: PatchFeed) -> bool {
        let from = match feed {
            PatchFeed::None => return self.disconnect_patch(to),
            PatchFeed::Outlet(from) => from,
            PatchFeed::Gate(channel) => {
                if self.channel_index(channel).is_none() {
                    return false;
                }
                let y = self
                    .modulation
                    .module(to.node)
                    .map_or(0, |module| module.at.y);
                Jack::new(self.modulation.gate_tag(channel, y), 0)
            }
        };
        self.rewire_patch(from, to).unwrap_or(false)
    }

    /// What an inlet tag's picker offers, in order, with the name each is
    /// listed under (song patch step 06): the transport first, then for
    /// each channel by seat its notes as a gate, its generator's control
    /// outlets and its keyboard.
    pub fn inlet_sources(&self) -> Vec<(InletSource, String)> {
        let mut sources: Vec<(InletSource, String)> = [
            InletSource::Beat,
            InletSource::Bar,
            InletSource::PatternPosition,
            InletSource::Pattern,
        ]
        .into_iter()
        .filter_map(|source| Some((source, self.inlet_source_name(source)?)))
        .collect();
        for channel in &self.channels {
            let id = channel.id;
            sources.push((InletSource::Gate(id), format!("{} · gate", channel.name)));
            for outlet in channel.kind().control_outlets() {
                let source = InletSource::Outlet {
                    channel: id,
                    outlet: outlet.id,
                };
                if let Some(name) = self.inlet_source_name(source) {
                    sources.push((source, name));
                }
            }
            for performance in &PERFORMANCE_DESCRIPTORS {
                let source = InletSource::Performance {
                    channel: id,
                    source: performance.id,
                };
                if let Some(name) = self.inlet_source_name(source) {
                    sources.push((source, name));
                }
            }
        }
        sources
    }

    /// What an inlet tag bound to `source` reads, as its tag says it: a
    /// short lowercase word, `None` for an outlet the channel's generator
    /// does not publish.
    pub fn inlet_source_kind(&self, source: InletSource) -> Option<String> {
        Some(match source {
            InletSource::Gate(_) => "gate".to_string(),
            InletSource::Outlet { channel, outlet } => self
                .channels
                .get(self.channel_index(channel)?)?
                .kind()
                .control_outlet(outlet)?
                .name
                .to_lowercase(),
            InletSource::Performance { source, .. } => {
                PERFORMANCE_DESCRIPTORS.get(usize::from(source))?.name.to_lowercase()
            }
            InletSource::Beat => "beat".to_string(),
            InletSource::Bar => "bar".to_string(),
            InletSource::PatternPosition => "pattern pos".to_string(),
            InletSource::Pattern => "pattern".to_string(),
        })
    }

    /// `source` as the picker lists it: its channel and what it reads, a
    /// transport source by what it is. `None` for a channel the song does
    /// not have or an outlet its generator does not publish.
    pub fn inlet_source_name(&self, source: InletSource) -> Option<String> {
        Some(match source {
            InletSource::Beat => "Beat".to_string(),
            InletSource::Bar => "Bar".to_string(),
            InletSource::PatternPosition => "Pattern position".to_string(),
            InletSource::Pattern => "Pattern (topmost row)".to_string(),
            InletSource::Gate(channel) | InletSource::Outlet { channel, .. } | InletSource::Performance { channel, .. } => {
                let name = &self.channels.get(self.channel_index(channel)?)?.name;
                let kind = self.inlet_source_kind(source)?;
                if source.late() {
                    format!("{name} · {kind}{LATE_NOTE}")
                } else {
                    format!("{name} · {kind}")
                }
            }
        })
    }

    /// Binds inlet tag `id` to `bind`, or empties it. Refuses a source
    /// [`Self::inlet_source_name`] cannot name. Returns whether anything
    /// changed.
    pub fn bind_patch_tag(&mut self, id: ModSourceId, bind: Option<InletSource>) -> bool {
        if bind.is_some_and(|source| self.inlet_source_name(source).is_none()) {
            return false;
        }
        self.modulation.bind_tag(id, bind)
    }

    /// Binds notes tag `id` to `channel`, or empties it: which channel's
    /// notes a notes-in tag reads, or which channel a notes-out tag plays
    /// (song patch step 07). `false` when nothing changed or the song has
    /// no such channel.
    pub fn bind_notes_tag(&mut self, id: ModSourceId, channel: Option<mooloop_core::ChannelId>) -> bool {
        if channel.is_some_and(|channel| self.channel_index(channel).is_none()) {
            return false;
        }
        self.modulation.bind_notes(id, channel)
    }

    /// Sets whether notes-in tag `id` takes its channel's notes, so only the
    /// patch's output plays them, rather than copying them.
    pub fn set_notes_take(&mut self, id: ModSourceId, take: bool) -> bool {
        self.modulation.set_take(id, take)
    }

    /// Arms `source` for the assignment gesture, or disarms it when it is
    /// the one armed: the canvas's click on an outlet or an assignment tag.
    /// Selects it too, so the surface below shows what is being assigned.
    /// Returns the armed source's name, `None` when nothing is armed now.
    pub fn toggle_patch_arm(&mut self, source: ModSourceRef) -> Option<String> {
        self.modulation_source_index(source)?;
        self.modulation_selected.set(Some(source));
        let next = (self.modulation_armed.get() != Some(source)).then_some(source);
        self.modulation_armed.set(next);
        self.modulation_loose.set(None);
        self.modulation_source_name(next?)
    }

    /// What module `id`'s input picker offers, in order, with the name each
    /// is listed under: **None**, then every outlet that sends what the
    /// module takes. A Math module takes a control value, so it lists every
    /// other module in the song; the other four take gates, so they list
    /// every outlet that sends notes (song modulation step 03).
    pub fn module_input_options(&self, id: ModSourceId) -> Vec<(InputSource, String)> {
        let Some(module) = self.modulation.module(id) else {
            return Vec::new();
        };
        let mut options = vec![(InputSource::None, "None".to_string())];
        if matches!(module.params, ModulatorParams::Math(_)) {
            options.extend(
                self.modulation
                    .modules
                    .iter()
                    .filter(|other| other.id != id)
                    .map(|other| (InputSource::Module(other.id), module_name(other))),
            );
        } else {
            options.extend(self.gate_outlets());
        }
        options
    }

    /// Every outlet in the song that sends gates, as an input names it, with
    /// its name: today each channel's notes. The next kind of gate source
    /// joins the picker here.
    fn gate_outlets(&self) -> impl Iterator<Item = (InputSource, String)> + '_ {
        self.channels.iter().enumerate().map(|(seat, channel)| {
            (
                InputSource::ChannelNotes(channel.id),
                format!("{} · {}", seat + 1, channel.name),
            )
        })
    }

    /// Where module `id`'s input sits in [`Self::module_input_options`], and
    /// what is worth saying about it: a gate is a channel's notes, and a
    /// Math module reads a module picked while listed before it this tick
    /// and one picked while listed after it a tick late ([`mooloop_core::Wire::late`]).
    pub fn module_input_choice(&self, id: ModSourceId) -> (usize, &'static str) {
        let Some(module) = self.modulation.module(id) else {
            return (0, "");
        };
        let index = self
            .module_input_options(id)
            .iter()
            .position(|(input, _)| *input == self.modulation.input_of(id))
            .unwrap_or(0);
        let note = match (module.params, self.modulation.input_of(id)) {
            (ModulatorParams::Envelope(_), _) => "CHANNEL NOTE GATE",
            (ModulatorParams::Math(_), InputSource::Module(_)) => {
                let late = self
                    .modulation
                    .wire_into(mooloop_core::Jack::new(id, module.params.kind().input_port()))
                    .is_some_and(|wire| wire.late);
                if late {
                    "READS THE PREVIOUS TICK"
                } else {
                    "READS THIS TICK"
                }
            }
            _ => "",
        };
        (index, note)
    }

    /// The module at `index` in the pane's list, by identity; `None` for a
    /// tag.
    pub fn module_at(&self, index: i32) -> Option<ModSourceId> {
        match self.modulation_source_at(usize::try_from(index).ok()?)? {
            ModSourceRef::Id(id) if self.modulation.module(id).is_some() => Some(id),
            _ => None,
        }
    }

    /// The song's routes from `source`, with each one's place in the song's
    /// route list: what the pane lists under a selected source.
    pub fn routes_from(&self, source: ModSourceRef) -> Vec<(usize, ModRoute)> {
        self.modulation
            .routes
            .iter()
            .enumerate()
            .filter(|(_, route)| route.source == source)
            .map(|(index, route)| (index, *route))
            .collect()
    }

    /// Sets the polarity of the song's route at `index`. Empty when it is
    /// already that.
    pub fn set_route_polarity(&mut self, index: i32, polarity: i32) -> bool {
        let next = if polarity == 1 {
            ModPolarity::Unipolar
        } else {
            ModPolarity::Bipolar
        };
        let Some(route) = usize::try_from(index)
            .ok()
            .and_then(|index| self.modulation.routes.get_mut(index))
        else {
            return false;
        };
        if route.polarity == next {
            return false;
        }
        route.polarity = next;
        true
    }

    /// Removes the song's route at `index`.
    pub fn remove_route(&mut self, index: i32) -> bool {
        let Some(index) = usize::try_from(index)
            .ok()
            .filter(|index| *index < self.modulation.routes.len())
        else {
            return false;
        };
        self.modulation.routes.remove(index);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{EffectTarget, ParamAddr, STRIP_PARAM_VOLUME};

    /// The inlet tag reading channel 0's outlet `outlet`, made as the
    /// canvas makes one: `None` when the generator does not publish it.
    fn outlet_tag(session: &mut Session, outlet: u16) -> Option<ModSourceId> {
        let source = mooloop_core::InletSource::Outlet {
            channel: session.channels[0].id,
            outlet,
        };
        session.inlet_source_name(source)?;
        Some(session.modulation.inlet_tag(source, None))
    }

    /// Where the tag reading channel 0's outlet `outlet` sits in the pane's
    /// list, made if need be; -1 when the generator does not publish it.
    fn outlet_index(session: &mut Session, outlet: u16) -> i32 {
        outlet_tag(session, outlet)
            .and_then(|tag| session.modulation_source_index(ModSourceRef::Id(tag)))
            .map_or(-1, |index| index as i32)
    }

    /// What the reconciler sent, by kind.
    #[derive(Default)]
    struct Sink {
        retuned: usize,
        routes: usize,
        sets: usize,
    }

    impl mooloop_engine::CommandSink for Sink {
        fn send(&mut self, cmd: mooloop_core::EngineCommand) -> bool {
            match cmd {
                mooloop_core::EngineCommand::SetModulator { .. } => self.retuned += 1,
                mooloop_core::EngineCommand::SetModRoute { .. } => self.routes += 1,
                other => panic!("the reconciler sent {other:?}"),
            }
            true
        }
        fn send_structural(&mut self, cmd: mooloop_engine::StructuralCommand) -> bool {
            assert!(matches!(cmd, mooloop_engine::StructuralCommand::SetModulation { .. }));
            self.sets += 1;
            true
        }
        fn send_deferred(
            &mut self,
            _: mooloop_core::EngineCommand,
            _: mooloop_core::MusicalEdge,
        ) -> bool {
            false
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
    }

    impl Sink {
        fn take(&mut self) -> (usize, usize, usize) {
            let sent = (self.retuned, self.routes, self.sets);
            *self = Self::default();
            sent
        }
    }

    /// **The pump's reconciler sends the engine what changed, and only
    /// that** (song modulation step 02): a change of shape as a whole set, a
    /// module's params and a route's depth as narrow retunes, and nothing at
    /// all when nothing changed.
    #[test]
    fn the_reconciler_sends_a_set_for_shape_and_a_retune_for_values() {
        let mut sink = Sink::default();
        let mut session = armed_lfo();
        session.sync_modulation(&mut sink);
        assert_eq!(sink.take(), (0, 0, 1), "a module added is a new set");
        session.sync_modulation(&mut sink);
        assert_eq!(sink.take(), (0, 0, 0), "nothing changed, nothing sent");

        assert!(session.set_modulator_param(0, mooloop_core::modulation::LFO_PARAM_RATE_HZ as i32, 0.7));
        session.sync_modulation(&mut sink);
        assert_eq!(sink.take(), (1, 0, 0), "a param is a retune");

        session.toggle_modulation_assignment();
        let fader = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        assert!(matches!(
            session.arm_modulation_route(fader, 0.3),
            crate::session::ArmedRoute::Added(_)
        ));
        session.sync_modulation(&mut sink);
        assert_eq!(sink.take(), (0, 0, 1), "a route added is a new set");
        assert!(matches!(
            session.arm_modulation_route(fader, 0.6),
            crate::session::ArmedRoute::Added(_)
        ));
        session.sync_modulation(&mut sink);
        assert_eq!(sink.take(), (0, 1, 0), "a depth is a retune");

        assert!(session.remove_modulation_source(0));
        session.sync_modulation(&mut sink);
        assert_eq!(sink.take(), (0, 0, 1), "a module removed is a new set");
    }

    fn armed_lfo() -> Session {
        let mut session = Session::default();
        assert!(
            session.add_modulation_source(ModulatorKind::Lfo),
            "an empty rack has a free slot"
        );
        session
    }

    /// The reason selection and arming are re-derived by id: a reorder that
    /// left them pointing at slot numbers would silently retarget the
    /// assignment gesture at whatever moved into that slot.
    #[test]
    fn reordering_sources_carries_selection_and_arming_with_the_module() {
        let mut session = armed_lfo();
        assert!(
            session.add_modulation_source(ModulatorKind::Envelope),
            "rack has room"
        );
        // The envelope is second in the list, and both selected and armed.
        assert_eq!(session.modulation_selected_index(), Some(1));
        session.toggle_modulation_assignment();
        assert_eq!(session.modulation_armed_index(), Some(1));

        assert!(
            session.move_modulation_source(1, 0),
            "both slots are occupied"
        );

        assert_eq!(
            session.modulation_selected_index(),
            Some(0),
            "selection stayed on the place instead of following the module"
        );
        assert_eq!(session.modulation_armed_index(), Some(0));
        assert!(matches!(
            session.modulation.modules[0].params,
            ModulatorParams::Envelope(_)
        ));
    }

    /// A new envelope's gate defaults to the channel it was added on, because
    /// the gate is a jack and has no descriptor default to fall back on.
    #[test]
    fn a_new_envelope_gates_from_its_own_channel() {
        let mut session = Session::default();
        session.add_channel(mooloop_core::DeviceKind::Sampler);
        assert_eq!(session.selected, 1);

        assert!(
            session.add_modulation_source(ModulatorKind::Envelope),
            "rack has room"
        );

        let id = session.module_at(0).expect("the envelope was added");
        assert_eq!(
            Some(session.modulation.input_of(id)),
            session.channel_id(1).map(InputSource::ChannelNotes)
        );

        // A gate pointed at a channel the song does not have is refused.
        let stranger = mooloop_core::ChannelId(999);
        assert!(!session
            .set_module_input(id, InputSource::ChannelNotes(stranger)));
        let first = session.channel_id(0).expect("the song has a first channel");
        assert!(session
            .set_module_input(id, InputSource::ChannelNotes(first)));
        assert_eq!(
            Some(session.modulation.input_of(id)),
            Some(InputSource::ChannelNotes(first))
        );
    }

    /// The patch verbs (song patch step 01): a box lands where it was put,
    /// a drag of two is one edit, a wire refuses a taken inlet, the wrong
    /// sort and a box into itself, and removing a box or a tag takes its
    /// wires.
    #[test]
    fn the_patch_verbs_place_wire_and_remove() {
        let mut session = Session::default();
        let lead = session.channel_id(0).expect("a first channel");
        let lfo = session
            .add_patch_box(ModulatorKind::Lfo, CanvasPoint::new(400, 80))
            .expect("an LFO");
        let math = session
            .add_patch_box(ModulatorKind::Math, CanvasPoint::new(560, 80))
            .expect("a Math box");
        assert_eq!(session.modulation.module(lfo).map(|module| module.at), Some(CanvasPoint::new(400, 80)));
        assert_eq!(session.modulation.input_of(lfo), InputSource::ChannelNotes(lead), "made on the selected channel");
        let gate = session.modulation.tags[0].id;

        assert!(session.move_patch_nodes(&[
            (lfo, CanvasPoint::new(400, 200)),
            (gate, CanvasPoint::new(16, 200)),
        ]));
        assert!(!session.move_patch_nodes(&[(lfo, CanvasPoint::new(400, 200))]), "already there");
        assert_eq!(session.modulation.tag(gate).map(|tag| tag.at), Some(CanvasPoint::new(16, 200)));

        let math_in = Jack::new(math, 0);
        assert_eq!(session.connect_patch(Jack::new(lfo, 0), math_in), Ok(()));
        assert_eq!(session.modulation.input_of(math), InputSource::Module(lfo));
        assert_eq!(
            session.connect_patch(Jack::new(gate, 0), math_in),
            Err(PatchRefusal::InletTaken),
            "the canvas disconnects first"
        );
        assert_eq!(
            session.connect_patch(Jack::new(math, 0), math_in),
            Err(PatchRefusal::Wire(WireRefusal::IntoItself))
        );
        let notes = session
            .add_patch_tag(TagKind::NotesIn { channel: Some(lead), take: false }, CanvasPoint::new(16, 300))
            .expect("a notes tag on a channel the song has");
        assert_eq!(
            session.connect_patch(Jack::new(notes, 0), Jack::new(lfo, 0)),
            Err(PatchRefusal::Wire(WireRefusal::WrongSort))
        );
        assert!(session
            .add_patch_tag(TagKind::NotesOut { channel: Some(mooloop_core::ChannelId(999)) }, CanvasPoint::default())
            .is_none());
        // A loop is allowed.
        assert_eq!(session.connect_patch(Jack::new(math, 0), Jack::new(lfo, 0)), Ok(()));

        assert!(session.disconnect_patch(math_in));
        assert!(!session.disconnect_patch(math_in));
        assert!(session.remove_patch_node(gate));
        assert_eq!(session.modulation.input_of(lfo), InputSource::None);
        assert!(session.remove_patch_node(math));
        assert!(session.modulation.wires.is_empty());
        assert!(session.remove_patch_node(notes));
        assert!(!session.remove_patch_node(notes));
    }

    /// A notes tag binds to a channel the song has, a notes-in tag takes or
    /// copies, and a wire between two bound ones is a note link the engine
    /// is sent (song patch step 07).
    #[test]
    fn notes_tags_bind_take_and_link() {
        let mut session = Session::default();
        let lead = session.channel_id(0).expect("a first channel");
        let notes_in = session
            .add_patch_tag(TagKind::NotesIn { channel: None, take: false }, CanvasPoint::new(16, 300))
            .expect("a notes-in tag");
        let notes_out = session
            .add_patch_tag(TagKind::NotesOut { channel: None }, CanvasPoint::new(400, 300))
            .expect("a notes-out tag");
        assert_eq!(session.connect_patch(Jack::new(notes_in, 0), Jack::new(notes_out, 0)), Ok(()));
        assert!(session.modulation_plan().notes.is_empty(), "unbound tags link nothing");

        assert!(!session.bind_notes_tag(notes_in, Some(mooloop_core::ChannelId(999))));
        assert!(session.bind_notes_tag(notes_in, Some(lead)));
        assert!(!session.bind_notes_tag(notes_in, Some(lead)), "already bound");
        assert!(session.bind_notes_tag(notes_out, Some(lead)));
        let plan = session.modulation_plan();
        assert_eq!(plan.notes.len(), 1);
        assert!(plan.taken.is_empty());

        assert!(session.set_notes_take(notes_in, true));
        assert!(!session.set_notes_take(notes_in, true));
        assert!(!session.set_notes_take(notes_out, true), "only a notes-in tag takes");
        let taken = session.modulation_plan();
        assert_eq!(taken.taken, [0]);
        assert!(!taken.same_shape(&plan), "a take arrives as a new set");
    }

    /// Typing makes the box its first word names, with what follows as its
    /// settings and no wires; a name the vocabulary lacks makes an unknown
    /// box that keeps what was typed.
    #[test]
    fn typing_makes_the_box_it_names() {
        use mooloop_core::box_text::spell;
        let mut session = Session::default();
        assert_eq!(session.type_patch_box("   ", CanvasPoint::new(0, 0)), None);
        let times = session
            .type_patch_box("*   -.5", CanvasPoint::new(40, 60))
            .unwrap();
        let module = session.modulation.module(times).unwrap();
        assert_eq!(spell(&module.params, &module.text), "* -0.5");
        assert_eq!(module.at, CanvasPoint::new(40, 60));
        assert!(session.modulation.wires.is_empty(), "a typed box starts unwired");
        assert_eq!(
            session.modulation_selected.get(),
            Some(ModSourceRef::Id(times))
        );

        let chord = session
            .type_patch_box("arp up", CanvasPoint::new(40, 120))
            .unwrap();
        let module = session.modulation.module(chord).unwrap();
        assert_eq!(module.params, ModulatorParams::Unknown);
        assert_eq!(spell(&module.params, &module.text), "arp up");
    }

    /// Retyping within a kind keeps the box and drops only the wires into
    /// jacks it lost; retyping to another kind is a new box that takes the
    /// wires on jacks of the same name, and the routes when it keeps the
    /// outlet they read.
    #[test]
    fn retyping_keeps_what_still_fits() {
        use mooloop_core::{EffectTarget, ParamAddr, STRIP_PARAM_VOLUME};
        let mut session = Session::default();
        let at = |x| CanvasPoint::new(x, 40);
        let lfo = session.type_patch_box("lfo", at(0)).unwrap();
        let other = session.type_patch_box("lfo", at(100)).unwrap();
        let select = session.type_patch_box("select 4", at(200)).unwrap();
        let out = Jack::new(lfo, 0);
        for port in [0, 1, 4] {
            session.rewire_patch(out, Jack::new(select, port)).unwrap();
        }
        assert_eq!(session.retype_patch_box(select, "select 4"), None, "nothing changed");
        assert_eq!(session.retype_patch_box(select, "select 2"), Some(select));
        let into: Vec<_> = session.modulation.wires.iter().map(|wire| wire.to.port).collect();
        assert_eq!(into, [0, 1], "e went with the inputs past b");

        // An LFO with a route, a wire in and a wire out, retyped to a slew:
        // `rate` is gone, `out` is not.
        let volume = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        session.modulation.routes.push(ModRoute::from_module(
            other,
            volume,
            0.5,
            ModPolarity::Bipolar,
        ));
        session.rewire_patch(out, Jack::new(other, 0)).unwrap();
        session.rewire_patch(Jack::new(other, 0), Jack::new(select, 2)).unwrap();
        session.modulation_armed.set(Some(ModSourceRef::Id(other)));
        let slew = session.retype_patch_box(other, "slew 0.5").unwrap();
        assert_ne!(slew, other, "a new kind is a new box");
        assert!(session.modulation.module(other).is_none());
        assert_eq!(session.modulation.module(slew).unwrap().at, at(100));
        assert_eq!(
            session.modulation.wire_into(Jack::new(select, 2)).map(|wire| wire.from),
            Some(Jack::new(slew, 0)),
            "its outlet's wire followed"
        );
        assert_eq!(
            session.modulation.wire_into(Jack::new(slew, 0)),
            None,
            "rate is not slew's in"
        );
        assert_eq!(session.modulation.routes[0].source, ModSourceRef::Id(slew));
        assert_eq!(session.modulation_armed.get(), Some(ModSourceRef::Id(slew)));

        // A counter's outlet is `index`, not `out`: the routes stay behind.
        let counter = session.retype_patch_box(slew, "counter 4").unwrap();
        assert!(session.modulation.routes.is_empty());
        assert_eq!(session.modulation_armed.get(), None);
        assert_eq!(session.modulation.wire_into(Jack::new(select, 2)), None);
        assert_eq!(session.modulation.module(counter).unwrap().at, at(100));
    }

    /// An outlet assigned to a box's knob (song patch step 05) makes a
    /// route onto the box, named as the box reads; a box never takes its own
    /// outlet, a stepped knob takes nothing, and removing the box takes the
    /// routes onto it.
    #[test]
    fn an_outlet_assigns_to_another_box_knob() {
        use crate::session::ArmedRoute;
        use mooloop_core::{ParamAddr, COUNTER_PARAM_STEPS, LFO_PARAM_RATE_HZ};
        let mut session = Session::default();
        let at = |x| CanvasPoint::new(x, 40);
        let lfo = session.type_patch_box("lfo", at(0)).unwrap();
        let other = session.type_patch_box("lfo", at(100)).unwrap();
        let counter = session.type_patch_box("counter 4", at(200)).unwrap();
        let rate = ParamAddr::modulator(other, LFO_PARAM_RATE_HZ);
        session.modulation_armed.set(Some(ModSourceRef::Id(lfo)));
        assert!(matches!(session.arm_modulation_route(rate, 0.3), ArmedRoute::Added(_)));
        assert_eq!(session.route_count(rate), 1);
        assert_eq!(
            session.modulation_destination(rate).map(|(name, descriptor)| (name, descriptor.name)),
            Some(("lfo".to_string(), "Rate"))
        );
        let own = ParamAddr::modulator(lfo, LFO_PARAM_RATE_HZ);
        assert!(matches!(session.arm_modulation_route(own, 0.3), ArmedRoute::Unchanged));
        let steps = ParamAddr::modulator(counter, COUNTER_PARAM_STEPS);
        assert!(matches!(session.arm_modulation_route(steps, 0.3), ArmedRoute::Unchanged));

        assert!(session.remove_patch_node(other));
        assert!(session.modulation.routes.is_empty(), "the route onto the box went with it");
    }

    /// The canvas's own verbs: a drop replaces an inlet's wire, the inlet
    /// picker feeds from a channel's gate tag or any outlet of the inlet's
    /// sort, assignment tags keep their places, and a selection goes in one
    /// edit.
    #[test]
    fn the_canvas_verbs_rewire_feed_place_and_remove() {
        use mooloop_core::{EffectTarget, ParamAddr, STRIP_PARAM_PAN, STRIP_PARAM_VOLUME};
        let mut session = Session::default();
        let lead = session.channel_id(0).expect("a first channel");
        let lfo = session
            .add_patch_box(ModulatorKind::Lfo, CanvasPoint::new(200, 80))
            .unwrap();
        let other = session
            .add_patch_box(ModulatorKind::Lfo, CanvasPoint::new(400, 80))
            .unwrap();
        let math = session
            .add_patch_box(ModulatorKind::Math, CanvasPoint::new(400, 200))
            .unwrap();
        let math_in = Jack::new(math, 0);

        assert_eq!(session.rewire_patch(Jack::new(lfo, 0), math_in), Ok(true));
        assert_eq!(
            session.rewire_patch(Jack::new(lfo, 0), math_in),
            Ok(false),
            "already so"
        );
        assert_eq!(
            session.rewire_patch(Jack::new(other, 0), math_in),
            Ok(true),
            "replaced"
        );
        assert_eq!(
            session.modulation.input_of(math),
            InputSource::Module(other)
        );
        assert_eq!(
            session
                .modulation
                .wires
                .iter()
                .filter(|wire| wire.to == math_in)
                .count(),
            1
        );

        let rate = Jack::new(lfo, 0);
        let options = session.patch_feed_options(rate);
        assert_eq!(options[0], (PatchFeed::None, "None".to_string()));
        let gate_name = format!("1 · {}", session.channels[0].name);
        assert!(
            options.contains(&(PatchFeed::Gate(lead), gate_name)),
            "{options:?}"
        );
        assert!(options
            .iter()
            .any(|(feed, _)| *feed == PatchFeed::Outlet(Jack::new(math, 0))));
        assert!(
            !options
                .iter()
                .any(|(feed, _)| *feed == PatchFeed::Outlet(Jack::new(lfo, 0))),
            "not itself"
        );
        assert!(session.feed_patch_inlet(rate, PatchFeed::Gate(lead)));
        assert_eq!(
            options[session.patch_feed_choice(rate)].0,
            PatchFeed::Gate(lead)
        );
        assert!(session.feed_patch_inlet(rate, PatchFeed::Outlet(Jack::new(math, 0))));
        assert_eq!(
            options[session.patch_feed_choice(rate)].0,
            PatchFeed::Outlet(Jack::new(math, 0))
        );
        assert!(session.feed_patch_inlet(rate, PatchFeed::None));
        assert!(
            !session.feed_patch_inlet(rate, PatchFeed::None),
            "nothing to remove"
        );

        let source = ModSourceRef::Id(lfo);
        for destination in [STRIP_PARAM_VOLUME, STRIP_PARAM_PAN] {
            session.modulation.routes.push(ModRoute::from_module(
                lfo,
                ParamAddr::strip(EffectTarget::Channel(0), destination),
                0.5,
                ModPolarity::Bipolar,
            ));
        }
        let volume = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        assert!(session.place_patch_routes(&[(0, CanvasPoint::new(180, 260))]));
        assert!(!session
            .place_patch_routes(&[(0, CanvasPoint::new(180, 260)), (9, CanvasPoint::default())]));
        assert_eq!(
            session.modulation.route_at(source, volume),
            Some(CanvasPoint::new(180, 260))
        );

        assert_eq!(
            session.toggle_patch_arm(source),
            session.modulation_source_name(source)
        );
        assert!(session.modulation_armed.get().is_some());
        assert_eq!(session.modulation_selected.get(), Some(source));
        assert_eq!(
            session.toggle_patch_arm(source),
            None,
            "the second click disarms"
        );

        assert!(session.remove_patch_selection(&[other], &[], &[1]));
        assert_eq!(session.modulation.routes.len(), 1, "the pan route went");
        assert_eq!(
            session.modulation.input_of(math),
            InputSource::None,
            "with the box went its wire"
        );
        assert!(!session.remove_patch_selection(&[other], &[math_in], &[5]));
    }

    /// Arming toggles, and reports the badge the status bar names.
    #[test]
    fn assignment_arms_the_selected_source_and_disarms_on_the_second_press() {
        let mut session = armed_lfo();

        let armed = session
            .toggle_modulation_assignment()
            .expect("a source is selected");
        assert!(armed.ends_with("LFO 1"), "{armed}");
        assert_eq!(session.modulation_armed_index(), Some(0));

        assert_eq!(session.toggle_modulation_assignment(), None);
        assert_eq!(session.modulation_armed_index(), None);
    }

    /// A parameter set to the value it already holds is not an edit, so it
    /// must not reach the engine or the undo history.
    #[test]
    fn setting_a_parameter_to_what_it_already_is_reports_nothing() {
        let mut session = armed_lfo();
        let id = 0;

        let first = session.set_modulator_param(0, id, 0.25);
        assert!(first);
        assert!(!session.set_modulator_param(0, id, 0.25));

        assert!(!session.set_modulator_param(9, id, 0.5));
        assert!(!session.set_modulator_param(-1, id, 0.5));
    }

    /// Removing a source clears the selection and arming that pointed at it,
    /// and takes its routes with it.
    #[test]
    fn removing_a_source_disarms_it_and_drops_its_routes() {
        let mut session = armed_lfo();
        session.toggle_modulation_assignment();
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        assert!(matches!(
            session.arm_modulation_route(destination, 0.5),
            crate::session::ArmedRoute::Added(_)
        ));

        assert!(session.remove_modulation_source(0), "the LFO is first");

        assert_eq!(session.modulation_selected_index(), None);
        assert_eq!(session.modulation_armed_index(), None);
        assert!(session.modulation.routes.is_empty());
        assert!(!session.remove_modulation_source(0));
    }

    /// The keyboard's mod wheel can be read on every channel, whatever its
    /// generator (MOO-128), through an inlet tag (song patch step 06): the
    /// tag is selectable, armable, named for what it reads, and its route
    /// reads the keyboard as a route from the wheel did.
    #[test]
    fn the_mod_wheel_arms_and_authors_a_performance_route() {
        let mut session = Session::default();
        assert_eq!(session.channels[0].kind(), mooloop_core::DeviceKind::Sampler);
        let source = mooloop_core::InletSource::Performance {
            channel: session.channels[0].id,
            source: mooloop_core::modulation::PERFORMANCE_MOD_WHEEL,
        };
        assert!(
            session.inlet_sources().iter().any(|(listed, _)| *listed == source),
            "every channel offers its keyboard"
        );
        let tag = session.modulation.inlet_tag(source, None);
        let wheel = session.modulation_source_index(ModSourceRef::Id(tag)).expect("a bound tag is a source");
        assert!(session.select_modulation_source(wheel as i32));
        let name = session.toggle_modulation_assignment().expect("armed");
        assert!(name.ends_with("mod wheel"), "{name}");
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        let crate::session::ArmedRoute::Added(route) =
            session.arm_modulation_route(destination, 0.5)
        else {
            panic!("the armed wheel did not author a route");
        };
        assert_eq!(route.source, ModSourceRef::Id(tag));
        assert_eq!(
            session.modulation_plan().chain_routes(EffectTarget::Channel(0))[0].resolved,
            CompiledSource::Performance {
                seat: 0,
                source: mooloop_core::modulation::PERFORMANCE_MOD_WHEEL as u8,
            }
        );
    }

    /// An ML-P8 channel, selected, with its outlet band available.
    fn mlp8_channel() -> Session {
        let mut session = Session::default();
        session.reset_channel_source(0, mooloop_core::DeviceKind::MlP8);
        session
    }

    /// The whole point of the picker: an outlet is selectable and armable on
    /// the same terms as a module, and it is named by the outlet rather than
    /// by the slot it occupies in the flat address space.
    #[test]
    fn a_published_outlet_selects_and_arms_like_a_module() {
        let mut session = mlp8_channel();
        let gate = outlet_index(&mut session, mooloop_core::mlp8::OUTLET_GATE);

        assert!(session.select_modulation_source(gate));
        assert_eq!(session.modulation_selected_index(), Some(gate as usize));
        let name = session.toggle_modulation_assignment().expect("armed");
        assert!(name.ends_with(" gate"), "the badge did not name the outlet: {name}");
        assert_eq!(session.modulation_armed_index(), Some(gate as usize));
    }

    /// An outlet belongs to whichever generator the channel holds. A sampler
    /// publishes nothing, so the band is not there to select from -- the same
    /// answer an empty rack slot gives.
    #[test]
    fn a_generator_that_publishes_nothing_offers_no_outlets() {
        let mut session = Session::default();
        assert_eq!(session.channels[0].kind(), mooloop_core::DeviceKind::Sampler);
        for outlet in 0..mooloop_core::modulation::MAX_GENERATOR_OUTLETS as u16 {
            assert_eq!(outlet_index(&mut session, outlet), -1);
        }
        // Its notes and its keyboard are all a tag can read from it.
        let channel = session.channels[0].id;
        assert!(session
            .inlet_sources()
            .iter()
            .all(|(source, _)| !matches!(source, mooloop_core::InletSource::Outlet { channel: of, .. } if *of == channel)));
        assert!(session.modulation_sources().is_empty(), "no tag, no source");

        // Nor does an audio outlet become selectable by living in the same
        // table as the control ones: ML-P8 publishes fourteen and offers
        // seven, and `Osc 1` is past the run a route can name.
        let mut mlp8 = mlp8_channel();
        assert_eq!(outlet_index(&mut mlp8, mooloop_core::mlp8::OUTLET_OSC1), -1);
    }

    /// The gesture authors an outlet route by its durable outlet id rather
    /// than by the slot it was armed from, and leaves the polarity at the
    /// destination's default.
    ///
    /// The polarity is worth asserting because the obvious guess is wrong.
    /// `ModPolarity::Unipolar` lifts a rack module's `-1..1` into `0..1`; an
    /// outlet already publishes in its declared range, so a unipolar outlet
    /// under a unipolar route would rest half a depth *above* the base and
    /// reach only half the swing.
    #[test]
    fn assigning_from_an_outlet_authors_a_durable_outlet_route() {
        let mut session = mlp8_channel();
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);

        let index = outlet_index(&mut session, mooloop_core::mlp8::OUTLET_GATE);
        session.select_modulation_source(index);
        session.toggle_modulation_assignment();
        let crate::session::ArmedRoute::Added(gate) =
            session.arm_modulation_route(destination, 0.4)
        else {
            panic!("the armed outlet did not author a route");
        };
        let gate_tag = outlet_tag(&mut session, mooloop_core::mlp8::OUTLET_GATE).unwrap();
        assert_eq!(gate.source, ModSourceRef::Id(gate_tag));
        assert_eq!(
            session.modulation_plan().chain_routes(EffectTarget::Channel(0))[0].resolved,
            CompiledSource::Outlet {
                seat: 0,
                outlet: mooloop_core::mlp8::OUTLET_GATE as u8,
            },
            "the route reads the outlet as a route from it did"
        );
        assert_eq!(gate.polarity, ModPolarity::Bipolar);

        let index = outlet_index(&mut session, mooloop_core::mlp8::OUTLET_LFO);
        session.select_modulation_source(index);
        let crate::session::ArmedRoute::Added(lfo) =
            session.arm_modulation_route(destination, 0.4)
        else {
            panic!("selecting a second outlet did not follow the arming");
        };
        assert_eq!(lfo.polarity, ModPolarity::Bipolar);
        let lfo_tag = outlet_tag(&mut session, mooloop_core::mlp8::OUTLET_LFO).unwrap();
        assert_eq!(lfo.source, ModSourceRef::Id(lfo_tag));

        // Two rows, one per outlet: an outlet route dedupes on its source the
        // way a module's does, rather than stacking.
        session.arm_modulation_route(destination, 0.6);
        let rows = session.modulation.routes.len();
        assert_eq!(rows, 2);
    }

    /// An outlet belongs to the generator, not to the channel. Swapping the
    /// device out from under an armed gesture leaves a slot naming nothing,
    /// and the gesture has to say so -- reporting a full matrix instead would
    /// send the user looking for routes to delete.
    #[test]
    fn swapping_the_generator_disarms_rather_than_misreporting() {
        let mut session = mlp8_channel();
        let destination = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        let index = outlet_index(&mut session, mooloop_core::mlp8::OUTLET_GATE);
        session.select_modulation_source(index);
        session.toggle_modulation_assignment();

        session.reset_channel_source(0, mooloop_core::DeviceKind::Sampler);

        assert!(matches!(
            session.arm_modulation_route(destination, 0.4),
            crate::session::ArmedRoute::Unchanged
        ));
        assert!(session.modulation.routes.is_empty());
    }

    /// A reorder moves modules. An outlet is not in the rack and does not
    /// move, so a gesture armed on one has to survive a drag it had nothing
    /// to do with.
    #[test]
    fn reordering_modules_leaves_an_armed_outlet_alone() {
        let mut session = mlp8_channel();
        assert!(
            session.add_modulation_source(ModulatorKind::Lfo),
            "an empty rack has a free slot"
        );
        assert!(
            session.add_modulation_source(ModulatorKind::Envelope),
            "rack has room"
        );
        let trigger = outlet_index(&mut session, mooloop_core::mlp8::OUTLET_TRIGGER);
        session.select_modulation_source(trigger);
        session.toggle_modulation_assignment();

        assert!(session.move_modulation_source(1, 0), "two modules to reorder");

        let source = ModSourceRef::Id(outlet_tag(&mut session, mooloop_core::mlp8::OUTLET_TRIGGER).unwrap());
        assert_eq!(session.modulation_selected.get(), Some(source));
        assert_eq!(session.modulation_armed.get(), Some(source));
    }
}
