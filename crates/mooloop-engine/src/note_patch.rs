//! Notes through the song's patch (`docs/plans/song-patch/07-notes-through-the-patch.md`,
//! `08-note-boxes.md`).
//!
//! A note wire from a channel's **Notes in** tag to a channel's **Notes out**
//! tag plays every note the first channel plays on the second as well, in
//! the same block at the same offset. The pass runs once per block, after
//! every channel's event list is complete (sequencer notes, owed releases,
//! chokes, the keyboard and auditions) and before anything renders or the
//! gate table is read, so whichever channel renders first, the copy is
//! already in the list it reads.
//!
//! **What a notes-in tag reads is the channel's own part**: the list as it
//! stands before the pass, not what the patch sends it. That is what keeps a
//! channel that takes its own notes and plays them back to itself (a chord
//! on a channel's own part) from hearing its output again.
//!
//! **Note boxes** (step 08) sit between the tags. Each reads one stream of
//! notes -- its tag's, or the box before it -- and writes its own, in the
//! order the set compiled them, so every box has read its input before
//! anything reads it. A box keeps a table from each note it heard to the
//! pitches it played for it, so a NoteOff releases exactly those, whatever
//! its settings have done since; two notes that land on one pitch play it
//! once and release it with the last. A Choke into a box releases everything
//! it holds. A transpose's and a chance's control inlet are read once a
//! block, as the patch ticked them last. A gate box writes no notes: it
//! files what reached it under each control tick, for the set to read.
//!
//! **Every note the pass plays has an id of its own**, from a range the
//! sequencer, the keyboard and auditions do not reach in practice, and each
//! link keeps a table from the id it heard to the id it played. A NoteOff
//! releases exactly that; a Choke on the source releases everything the link
//! played from it, at the same offset. The tables are fixed-size, built off
//! the audio thread with the set: a NoteOn that finds its link's table full,
//! or its channel's list full, is refused and counted on the notes-out tag,
//! never half-played -- its NoteOff then finds nothing to release. A box
//! refuses the same way, with nothing to count it on.
//!
//! **A link that goes while notes sound releases them** at the start of the
//! next block ([`NotePass::carry_from`]), on whichever seat its channel has
//! moved to. More releases than the pass has room for become a Choke on the
//! channel, so nothing is left sounding that nothing can name.

use mooloop_core::harmony::{self, Pitches, MAX_CHORD_NOTES};
use mooloop_core::{
    CompiledModulation, CompiledModule, CompiledNoteBox, CompiledNoteLink, ModSourceId, ModulatorParams,
    MAX_CHANNELS,
};
use mooloop_dsp::{
    Event, EventList, ModulatorSet, NoteGateTick, TimedEvent, CONTROL_RATE_FRAMES, MAX_CONTROL_TICKS_PER_BLOCK,
};

/// How many notes one link can hold sounding at once, as many as a channel's
/// sequenced voices ([`crate::voices::MAX_SEQUENCED_VOICES`]): every source's
/// voice pool is smaller, so a link that fills it is already stealing. A
/// box holds as many notes in, and as many pitches out.
pub(crate) const MAX_LINK_NOTES: usize = 64;

/// The most events one note box writes in a block: twice a channel's list,
/// room for a chord on every note of a busy one.
const BOX_EVENTS: usize = 2 * EventList::CAPACITY;

/// Releases a change of set can owe before the rest become Chokes.
const MAX_OWED: usize = 256;

/// The first id the pass plays a note under. The sequencer's ids are
/// `(instance << 32) | note`, whose instance stays far below 2^30 in a song
/// (256 patterns times the playlist's ticks); the keyboard's and auditions'
/// are the last 384 ids below `u64::MAX`.
const FIRST_PATCH_ID: u64 = 0xC000_0000_0000_0000;
/// How many ids the pass counts through before it wraps.
const PATCH_IDS: u64 = 1 << 61;

/// One note a link is holding: the id it heard and the id it played.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Held {
    input: u64,
    output: u64,
    note: u8,
}

/// What one link is holding.
#[derive(Debug, Clone, Copy)]
struct LinkTable {
    held: [Held; MAX_LINK_NOTES],
    len: usize,
}

impl Default for LinkTable {
    fn default() -> Self {
        Self {
            held: [Held::default(); MAX_LINK_NOTES],
            len: 0,
        }
    }
}

impl LinkTable {
    fn notes(&self) -> &[Held] {
        &self.held[..self.len]
    }

    fn take(&mut self, input: u64) -> Option<Held> {
        let at = self.notes().iter().position(|held| held.input == input)?;
        let held = self.held[at];
        self.held[at] = self.held[self.len - 1];
        self.len -= 1;
        Some(held)
    }
}

/// One note a box heard and the pitches it played for it.
#[derive(Debug, Clone, Copy, Default)]
struct BoxHeld {
    input: u64,
    pitches: [u8; MAX_CHORD_NOTES],
    len: u8,
}

/// One pitch a box has sounding, the id it plays it under, and how many of
/// the notes it heard share it.
#[derive(Debug, Clone, Copy, Default)]
struct Sounding {
    note: u8,
    id: u64,
    refs: u8,
}

/// What one note box is holding, and a chance box's generator.
#[derive(Debug, Clone, Copy)]
struct BoxTable {
    held: [BoxHeld; MAX_LINK_NOTES],
    held_len: usize,
    sounding: [Sounding; MAX_LINK_NOTES],
    sounding_len: usize,
    rng: u32,
}

impl BoxTable {
    fn new(seed: u32) -> Self {
        Self {
            held: [BoxHeld::default(); MAX_LINK_NOTES],
            held_len: 0,
            sounding: [Sounding::default(); MAX_LINK_NOTES],
            sounding_len: 0,
            rng: seeded(seed),
        }
    }

    /// Uniform `0..1`: xorshift32, as a random box draws.
    fn next_unit(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// A chance box's generator as the transport starts, from the box's seed.
/// Odd, so xorshift can never stick at zero.
fn seeded(seed: u32) -> u32 {
    0x2545_F491 ^ (seed.wrapping_add(1).wrapping_mul(0x85EB_CA6B) | 1)
}

/// What the note boxes read besides their notes, each block.
#[derive(Clone, Copy)]
pub(crate) struct BoxInputs<'a> {
    /// The set's modules, as their params stand now (a retune changes them
    /// in place).
    pub(crate) modules: &'a [CompiledModule],
    /// The patch as it ticked last: a transpose's and a chance's control
    /// inlet.
    pub(crate) set: &'a ModulatorSet,
    /// Whether the transport is playing: a chance box's generator restarts
    /// as it starts.
    pub(crate) playing: bool,
    /// How long the block is, for the gate boxes' control ticks.
    pub(crate) frames: usize,
}

/// A release owed to a seat at the start of the next block.
#[derive(Debug, Clone, Copy, Default)]
struct Owed {
    seat: u8,
    id: u64,
    note: u8,
}

/// The note pass of one compiled set. See the module.
pub(crate) struct NotePass {
    links: Vec<CompiledNoteLink>,
    tables: Vec<LinkTable>,
    boxes: Vec<CompiledNoteBox>,
    box_tables: Vec<BoxTable>,
    /// Each box's seed, its generator's start.
    box_seeds: Vec<u32>,
    /// Each box's slot among the gate boxes, for a gate box.
    gate_slots: Vec<Option<u16>>,
    /// `gate_ticks[tick * gates + slot]`: what reached each gate box in
    /// each control tick of this block.
    gate_ticks: Vec<NoteGateTick>,
    gates: usize,
    /// The notes-in tags anything reads, and the seat each hears: each
    /// counts the NoteOns it passes once, however many wires leave it.
    in_tags: Vec<(u16, u8)>,
    /// By seat. On the heap with the rest, so a song with no note wires
    /// pays the render graph a few pointers rather than two banks.
    taken: Box<[bool]>,
    any_taken: bool,
    /// The distinct seats the links and boxes read, and where each one's
    /// notes start in `heard`.
    sources: Vec<u8>,
    heard: Vec<TimedEvent>,
    heard_at: Vec<(u32, u32)>,
    /// What each box wrote this block, and where in `flow`.
    flow: Vec<TimedEvent>,
    flow_at: Vec<(u32, u32)>,
    owed: Vec<Owed>,
    choke_owed: Box<[bool]>,
    next_id: u64,
    /// The ids a box plays its pitches under, inside the pass.
    next_box_id: u64,
    was_playing: bool,
    tag_ids: Vec<ModSourceId>,
    /// Each tag's NoteOn count, wrapping: a notes-in tag's the notes it
    /// passed, a notes-out tag's the notes it played.
    tag_notes: Vec<u32>,
    /// Each notes-out tag's refused NoteOns, wrapping.
    tag_refused: Vec<u32>,
}

impl Default for NotePass {
    fn default() -> Self {
        Self::new(&CompiledModulation::default())
    }
}

impl NotePass {
    /// Allocates: built with the set, off the audio thread.
    pub(crate) fn new(plan: &CompiledModulation) -> Self {
        let links = plan.notes.clone();
        let boxes = plan.note_boxes.clone();
        let mut sources: Vec<u8> = links
            .iter()
            .map(|link| link.from_seat)
            .chain(boxes.iter().map(|note_box| note_box.root_seat))
            .collect();
        sources.sort_unstable();
        sources.dedup();
        let mut in_tags: Vec<(u16, u8)> = links
            .iter()
            .map(|link| (link.from_tag, link.from_seat))
            .chain(boxes.iter().map(|note_box| (note_box.root_tag, note_box.root_seat)))
            .collect();
        in_tags.sort_unstable();
        in_tags.dedup();
        let mut taken = vec![false; MAX_CHANNELS].into_boxed_slice();
        for &seat in &plan.taken {
            taken[usize::from(seat)] = true;
        }
        let box_seeds: Vec<u32> = boxes
            .iter()
            .map(|note_box| plan.modules.get(usize::from(note_box.module)).map_or(0, |module| module.seed))
            .collect();
        let gate_slots = boxes
            .iter()
            .map(|note_box| {
                let slot = plan.gates.iter().position(|&at| at == note_box.module)?;
                u16::try_from(slot).ok()
            })
            .collect();
        Self {
            tables: vec![LinkTable::default(); links.len()],
            box_tables: box_seeds.iter().map(|&seed| BoxTable::new(seed)).collect(),
            box_seeds,
            gate_slots,
            gate_ticks: vec![NoteGateTick::default(); plan.gates.len() * MAX_CONTROL_TICKS_PER_BLOCK],
            gates: plan.gates.len(),
            in_tags,
            taken,
            any_taken: !plan.taken.is_empty(),
            heard: Vec::with_capacity(sources.len() * EventList::CAPACITY),
            heard_at: vec![(0, 0); sources.len()],
            sources,
            flow: Vec::with_capacity(boxes.len() * BOX_EVENTS),
            flow_at: vec![(0, 0); boxes.len()],
            boxes,
            owed: Vec::with_capacity(MAX_OWED),
            choke_owed: vec![false; MAX_CHANNELS].into_boxed_slice(),
            next_id: 0,
            next_box_id: 0,
            was_playing: false,
            tag_ids: plan.tags.iter().map(|tag| tag.id).collect(),
            tag_notes: vec![0; plan.tags.len()],
            tag_refused: vec![0; plan.tags.len()],
            links,
        }
    }

    /// Take every link's and box's held notes from `previous` that names
    /// the same link or box, and owe a release for every note a link that
    /// is gone still holds. Audio thread: compares and copies, allocating
    /// nothing.
    pub(crate) fn carry_from(&mut self, previous: &NotePass) {
        self.next_id = previous.next_id;
        self.next_box_id = previous.next_box_id;
        self.was_playing = previous.was_playing;
        for (at, id) in self.tag_ids.iter().enumerate() {
            if let Some(from) = previous.tag_ids.iter().position(|old| old == id) {
                self.tag_notes[at] = previous.tag_notes[from];
                self.tag_refused[at] = previous.tag_refused[from];
            }
        }
        // A box on the same path holds what it held; a box that is gone
        // takes nothing with it, because every note it played that still
        // sounds is held by a link, and released with the link.
        for (at, note_box) in self.boxes.iter().enumerate() {
            if let Some(from) = previous.boxes.iter().position(|old| old.path == note_box.path) {
                self.box_tables[at] = previous.box_tables[from];
            }
        }
        for owed in &previous.owed {
            self.owe(*owed);
        }
        for (seat, &choke) in previous.choke_owed.iter().enumerate() {
            self.choke_owed[seat] |= choke;
        }
        for (old, table) in previous.links.iter().zip(&previous.tables) {
            match self.links.iter().position(|link| link.key() == old.key()) {
                Some(at) => self.tables[at] = *table,
                None => {
                    // Released on whichever seat the channel sits at now; a
                    // channel the new set does not name keeps its seat, and
                    // a release of an id no voice there has is ignored.
                    let seat = self
                        .links
                        .iter()
                        .find_map(|link| {
                            (link.to_channel == old.to_channel)
                                .then_some(link.to_seat)
                                .or((link.from_channel == old.to_channel).then_some(link.from_seat))
                        })
                        .unwrap_or(old.to_seat);
                    for held in table.notes() {
                        self.owe(Owed {
                            seat,
                            id: held.output,
                            note: held.note,
                        });
                    }
                }
            }
        }
    }

    fn owe(&mut self, owed: Owed) {
        if self.owed.len() < self.owed.capacity() {
            self.owed.push(owed);
        } else {
            self.choke_owed[usize::from(owed.seat)] = true;
        }
    }

    /// Whether the pass has anything to do.
    fn idle(&self) -> bool {
        self.links.is_empty()
            && self.boxes.is_empty()
            && self.owed.is_empty()
            && !self.any_taken
            && !self.choke_owed.iter().any(|&choke| choke)
    }

    /// Run the pass over the first `live` channels' complete event lists.
    /// With `panicked`, every table is emptied after the Chokes the panic
    /// put in every list have released what they held. Allocates nothing.
    pub(crate) fn process(
        &mut self,
        events: &mut [Box<EventList>],
        live: usize,
        panicked: bool,
        inputs: BoxInputs,
    ) {
        if self.idle() {
            return;
        }
        let live = live.min(events.len());
        // A chance box draws the same notes from each start of the
        // transport, so a bounce plays what the playback before it did.
        if inputs.playing && !self.was_playing {
            for (table, &seed) in self.box_tables.iter_mut().zip(&self.box_seeds) {
                table.rng = seeded(seed);
            }
        }
        self.was_playing = inputs.playing;
        // What a change of set owed, first, so a note the block starts again
        // at offset 0 starts after the old one is let go. One that does not
        // fit becomes a Choke on the next block.
        for at in 0..self.owed.len() {
            let owed = self.owed[at];
            let seat = usize::from(owed.seat);
            if seat < live
                && !events[seat].push_ordered(TimedEvent {
                    offset: 0,
                    event: Event::NoteOff {
                        id: owed.id,
                        note: owed.note,
                    },
                })
            {
                self.choke_owed[seat] = true;
            }
        }
        self.owed.clear();
        for (owed, list) in self.choke_owed.iter_mut().zip(events.iter_mut()).take(live) {
            if std::mem::take(owed) {
                let _ = list.push_ordered(TimedEvent {
                    offset: 0,
                    event: Event::Choke,
                });
            }
        }
        // What each source channel plays, before anything is taken from it
        // or sent to it.
        self.heard.clear();
        for (at, &seat) in self.sources.iter().enumerate() {
            let start = self.heard.len() as u32;
            if let Some(list) = events.get(usize::from(seat)).filter(|_| usize::from(seat) < live) {
                for event in list.iter() {
                    if matches!(event.event, Event::NoteOn { .. } | Event::NoteOff { .. } | Event::Choke)
                        && self.heard.len() < self.heard.capacity()
                    {
                        self.heard.push(*event);
                    }
                }
            }
            self.heard_at[at] = (start, self.heard.len() as u32);
        }
        for &(tag, seat) in &self.in_tags {
            let Some(source) = self.sources.iter().position(|&held| held == seat) else {
                continue;
            };
            let (start, end) = self.heard_at[source];
            for heard in &self.heard[start as usize..end as usize] {
                if matches!(heard.event, Event::NoteOn { .. }) {
                    bump(&mut self.tag_notes, tag);
                }
            }
        }
        // A taken channel's own notes reach only the patch. Its chokes,
        // parameters and bends stay.
        for (seat, list) in events.iter_mut().enumerate().take(live) {
            if self.any_taken && self.taken[seat] {
                list.retain(|event| !matches!(event.event, Event::NoteOn { .. } | Event::NoteOff { .. }));
            }
        }
        self.run_boxes(inputs);
        for at in 0..self.links.len() {
            let link = self.links[at];
            let Some((start, end)) = self.stream(link.from_seat, link.via) else {
                continue;
            };
            let to = usize::from(link.to_seat);
            for index in start as usize..end as usize {
                let heard = match link.via {
                    None => self.heard[index],
                    Some(_) => self.flow[index],
                };
                match heard.event {
                    Event::NoteOn { id, note, velocity } => {
                        if to >= live || self.tables[at].len == MAX_LINK_NOTES {
                            bump(&mut self.tag_refused, link.to_tag);
                            continue;
                        }
                        let output = FIRST_PATCH_ID + self.next_id;
                        let played = events[to].push_ordered(TimedEvent {
                            offset: heard.offset,
                            event: Event::NoteOn {
                                id: output,
                                note,
                                velocity,
                            },
                        });
                        if !played {
                            bump(&mut self.tag_refused, link.to_tag);
                            continue;
                        }
                        self.next_id = (self.next_id + 1) % PATCH_IDS;
                        let table = &mut self.tables[at];
                        table.held[table.len] = Held { input: id, output, note };
                        table.len += 1;
                        bump(&mut self.tag_notes, link.to_tag);
                    }
                    Event::NoteOff { id, .. } => {
                        if let Some(held) = self.tables[at].take(id) {
                            self.release(events, live, link.to_seat, heard.offset, held);
                        }
                    }
                    Event::Choke => {
                        while let Some(held) = self.tables[at].notes().last().copied() {
                            self.tables[at].len -= 1;
                            self.release(events, live, link.to_seat, heard.offset, held);
                        }
                    }
                    _ => {}
                }
            }
        }
        if panicked {
            for table in &mut self.tables {
                table.len = 0;
            }
            for table in &mut self.box_tables {
                table.held_len = 0;
                table.sounding_len = 0;
            }
        }
    }

    /// Where the notes a link or box reads are this block: in `heard` for
    /// its tag's seat, or in `flow` for the box `via`.
    fn stream(&self, seat: u8, via: Option<u16>) -> Option<(u32, u32)> {
        match via {
            None => {
                let source = self.sources.iter().position(|&held| held == seat)?;
                Some(self.heard_at[source])
            }
            Some(at) => self.flow_at.get(usize::from(at)).copied(),
        }
    }

    /// Run every note box over what reached it, in the compiled order.
    fn run_boxes(&mut self, inputs: BoxInputs) {
        self.flow.clear();
        let ticks = inputs.frames.div_ceil(CONTROL_RATE_FRAMES).clamp(1, MAX_CONTROL_TICKS_PER_BLOCK);
        for row in self.gate_ticks.chunks_mut(self.gates.max(1)).take(ticks) {
            row.fill(NoteGateTick::default());
        }
        for at in 0..self.boxes.len() {
            let note_box = self.boxes[at];
            let start = self.flow.len() as u32;
            let Some((from, to)) = self.stream(note_box.root_seat, note_box.input) else {
                self.flow_at[at] = (start, start);
                continue;
            };
            let params = inputs
                .modules
                .get(usize::from(note_box.module))
                .map_or(ModulatorParams::Unknown, |module| module.params);
            let control = inputs.set.note_control(usize::from(note_box.module));
            for index in from as usize..to as usize {
                let heard = match note_box.input {
                    None => self.heard[index],
                    Some(_) => self.flow[index],
                };
                if let Some(slot) = self.gate_slots[at] {
                    let tick = (heard.offset as usize / CONTROL_RATE_FRAMES).min(ticks - 1);
                    let gate = &mut self.gate_ticks[tick * self.gates + usize::from(slot)];
                    match heard.event {
                        Event::NoteOn { note, velocity, .. } => {
                            gate.events.note_ons = gate.events.note_ons.saturating_add(1);
                            gate.struck = Some((f32::from(note) / 127.0, f32::from(velocity) / 127.0));
                        }
                        Event::NoteOff { .. } => gate.events.note_offs = gate.events.note_offs.saturating_add(1),
                        Event::Choke => gate.events.choke = true,
                        _ => {}
                    }
                    continue;
                }
                match heard.event {
                    Event::NoteOn { id, note, velocity } => {
                        let table = &mut self.box_tables[at];
                        let Some(pitches) = box_pitches(params, control, note, table) else {
                            continue;
                        };
                        self.play(at, start, heard.offset, id, pitches, velocity);
                    }
                    Event::NoteOff { id, .. } => self.end(at, heard.offset, id),
                    Event::Choke => {
                        while let Some(held) = self.box_tables[at].held[..self.box_tables[at].held_len].last() {
                            let input = held.input;
                            self.end(at, heard.offset, input);
                        }
                    }
                    _ => {}
                }
            }
            self.flow_at[at] = (start, self.flow.len() as u32);
        }
    }

    /// Box `at` plays `pitches` for the note `input` it heard, at `offset`.
    /// A pitch it has sounding already is shared, not played again. Refused
    /// whole when the box holds all it can, or has no room left this block
    /// for these and a release of everything it would then hold.
    fn play(&mut self, at: usize, start: u32, offset: u32, input: u64, pitches: Pitches, velocity: u8) {
        let table = &mut self.box_tables[at];
        // In range and each pitch once: a note pushed past either end is
        // dropped, and its NoteOff with it.
        let mut notes = [0u8; MAX_CHORD_NOTES];
        let mut len = 0;
        for &pitch in pitches.as_slice() {
            if let Some(pitch) = u8::try_from(pitch).ok().filter(|&pitch| pitch <= 127) {
                if !notes[..len].contains(&pitch) {
                    notes[len] = pitch;
                    len += 1;
                }
            }
        }
        if len == 0 || table.held_len == MAX_LINK_NOTES {
            return;
        }
        let new = notes[..len]
            .iter()
            .filter(|&&note| !table.sounding[..table.sounding_len].iter().any(|held| held.note == note))
            .count();
        let written = self.flow.len() - start as usize;
        if table.sounding_len + new > MAX_LINK_NOTES || written + new + table.sounding_len + new > BOX_EVENTS {
            return;
        }
        let mut held = BoxHeld {
            input,
            pitches: [0; MAX_CHORD_NOTES],
            len: len as u8,
        };
        held.pitches[..len].copy_from_slice(&notes[..len]);
        table.held[table.held_len] = held;
        table.held_len += 1;
        for &note in &notes[..len] {
            if let Some(sounding) = table.sounding[..table.sounding_len].iter_mut().find(|held| held.note == note) {
                sounding.refs = sounding.refs.saturating_add(1);
                continue;
            }
            let id = self.next_box_id;
            self.next_box_id = self.next_box_id.wrapping_add(1);
            table.sounding[table.sounding_len] = Sounding { note, id, refs: 1 };
            table.sounding_len += 1;
            self.flow.push(TimedEvent {
                offset,
                event: Event::NoteOn { id, note, velocity },
            });
        }
    }

    /// Box `at` lets go of the note `input` it heard, at `offset`: each of
    /// its pitches no other note still shares is released.
    fn end(&mut self, at: usize, offset: u32, input: u64) {
        let table = &mut self.box_tables[at];
        let Some(index) = table.held[..table.held_len].iter().position(|held| held.input == input) else {
            return;
        };
        let held = table.held[index];
        table.held[index] = table.held[table.held_len - 1];
        table.held_len -= 1;
        for &note in &held.pitches[..usize::from(held.len)] {
            let Some(index) = table.sounding[..table.sounding_len].iter().position(|held| held.note == note) else {
                continue;
            };
            let sounding = &mut table.sounding[index];
            sounding.refs = sounding.refs.saturating_sub(1);
            if sounding.refs > 0 {
                continue;
            }
            let id = sounding.id;
            table.sounding[index] = table.sounding[table.sounding_len - 1];
            table.sounding_len -= 1;
            // `play` kept room for this.
            if self.flow.len() < self.flow.capacity() {
                self.flow.push(TimedEvent {
                    offset,
                    event: Event::NoteOff { id, note },
                });
            }
        }
    }

    /// Release `held` on `seat` at `offset`, or owe it to the next block
    /// when the list is full.
    fn release(&mut self, events: &mut [Box<EventList>], live: usize, seat: u8, offset: u32, held: Held) {
        if usize::from(seat) >= live {
            return;
        }
        let released = events[usize::from(seat)].push_ordered(TimedEvent {
            offset,
            event: Event::NoteOff {
                id: held.output,
                note: held.note,
            },
        });
        if !released {
            self.owe(Owed {
                seat,
                id: held.output,
                note: held.note,
            });
        }
    }

    /// Each tag's NoteOn count and refused count, in tag order.
    pub(crate) fn tag_counts(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.tag_notes.iter().copied().zip(self.tag_refused.iter().copied())
    }

    /// What reached each gate box in control tick `tick` of this block, by
    /// slot.
    pub(crate) fn gate_row(&self, tick: usize) -> &[NoteGateTick] {
        self.gate_ticks
            .get(tick * self.gates..(tick + 1) * self.gates)
            .unwrap_or(&[])
    }

    /// How many notes link `at` holds sounding.
    #[cfg(test)]
    pub(crate) fn held(&self, at: usize) -> usize {
        self.tables.get(at).map_or(0, |table| table.len)
    }
}

/// The pitches a note box plays for `note`, or `None` for a NoteOn a
/// chance box lets fall. `control` is what its second inlet read.
fn box_pitches(params: ModulatorParams, control: f32, note: u8, table: &mut BoxTable) -> Option<Pitches> {
    Some(match params {
        ModulatorParams::Chord(chord) => harmony::chord(note, chord.quality, chord.inversion),
        ModulatorParams::Modal(modal) => harmony::modal_chord(note, modal.root, modal.mode, modal.seventh),
        ModulatorParams::Scale(scale) => Pitches::one(harmony::snap(note, scale.root, scale.mode)),
        ModulatorParams::Transpose(transpose) => {
            // A wire adds whole semitones, ±1 an octave.
            let more = (control.clamp(-4.0, 4.0) * 12.0).round() as i16;
            Pitches::one(i16::from(note) + i16::from(transpose.semitones) + more)
        }
        ModulatorParams::Chance(chance) => {
            let probability = (chance.probability + control).clamp(0.0, 1.0);
            if table.next_unit() >= probability {
                return None;
            }
            Pitches::one(i16::from(note))
        }
        _ => return None,
    })
}

fn bump(counts: &mut [u32], at: u16) {
    if let Some(count) = counts.get_mut(usize::from(at)) {
        *count = count.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{CanvasPoint, ChannelId, Jack, SongModulation, SongTag, TagKind, Wire};

    /// Seat 0's notes wired to seat 1.
    fn linked() -> NotePass {
        let song = SongModulation {
            tags: vec![
                SongTag {
                    id: ModSourceId(1),
                    at: CanvasPoint::default(),
                    kind: TagKind::NotesIn {
                        channel: Some(ChannelId(10)),
                        take: false,
                    },
                },
                SongTag {
                    id: ModSourceId(2),
                    at: CanvasPoint::default(),
                    kind: TagKind::NotesOut {
                        channel: Some(ChannelId(11)),
                    },
                },
            ],
            wires: vec![Wire {
                from: Jack::new(ModSourceId(1), 0),
                to: Jack::new(ModSourceId(2), 0),
                bend: None,
                late: false,
            }],
            next_source_id: 3,
            ..SongModulation::default()
        };
        let plan = CompiledModulation::compile(&song, |id| u8::try_from(id.0 - 10).ok());
        assert_eq!(plan.notes.len(), 1);
        NotePass::new(&plan)
    }

    /// One block through `pass`, no box in it reading a control.
    fn run(pass: &mut NotePass, events: &mut [Box<EventList>]) {
        let set = ModulatorSet::default();
        let inputs = BoxInputs {
            modules: &[],
            set: &set,
            playing: true,
            frames: 256,
        };
        pass.process(events, 2, false, inputs);
    }

    fn lists() -> Vec<Box<EventList>> {
        (0..2).map(|_| Box::new(EventList::empty())).collect()
    }

    fn on(id: u64) -> TimedEvent {
        TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id,
                note: 60,
                velocity: 100,
            },
        }
    }

    fn off(id: u64) -> TimedEvent {
        TimedEvent {
            offset: 1,
            event: Event::NoteOff { id, note: 60 },
        }
    }

    fn count(list: &EventList, note_on: bool) -> usize {
        list.iter()
            .filter(|event| match event.event {
                Event::NoteOn { .. } => note_on,
                Event::NoteOff { .. } => !note_on,
                _ => false,
            })
            .count()
    }

    /// **The capacity boundary** (`CAPACITY_POLICY.md`): a link holds
    /// [`MAX_LINK_NOTES`] notes. One more is refused whole, counted on the
    /// notes-out tag, and its NoteOff then releases nothing; a note ending
    /// makes room again.
    #[test]
    fn a_full_link_refuses_and_counts() {
        let mut pass = linked();
        let mut events = lists();
        for id in 0..MAX_LINK_NOTES as u64 + 2 {
            events[0].push(on(id));
        }
        run(&mut pass, &mut events);
        assert_eq!(count(&events[1], true), MAX_LINK_NOTES);
        assert_eq!(pass.held(0), MAX_LINK_NOTES);
        let counts: Vec<_> = pass.tag_counts().collect();
        assert_eq!(counts, [(MAX_LINK_NOTES as u32 + 2, 0), (MAX_LINK_NOTES as u32, 2)]);

        // The refused notes end: nothing to release. One played note ends.
        for list in &mut events {
            list.clear();
        }
        events[0].push(off(MAX_LINK_NOTES as u64));
        events[0].push(off(MAX_LINK_NOTES as u64 + 1));
        events[0].push(off(0));
        run(&mut pass, &mut events);
        assert_eq!(count(&events[1], false), 1);
        assert_eq!(pass.held(0), MAX_LINK_NOTES - 1);

        // Room for one again.
        for list in &mut events {
            list.clear();
        }
        events[0].push(on(500));
        run(&mut pass, &mut events);
        assert_eq!(count(&events[1], true), 1);
        assert_eq!(pass.held(0), MAX_LINK_NOTES);
    }

    /// A Choke on the source releases everything the link played, at its
    /// offset; a panic empties the tables too.
    #[test]
    fn a_choke_on_the_source_releases_the_link() {
        let mut pass = linked();
        let mut events = lists();
        events[0].push(on(1));
        events[0].push(on(2));
        run(&mut pass, &mut events);
        for list in &mut events {
            list.clear();
        }
        events[0].push(TimedEvent {
            offset: 7,
            event: Event::Choke,
        });
        run(&mut pass, &mut events);
        assert_eq!(count(&events[1], false), 2);
        assert!(events[1].iter().all(|event| event.offset == 7));
        assert_eq!(pass.held(0), 0);
    }

    /// A set without the link owes a release for each note it held, played
    /// at the start of the next block; one with it keeps them.
    #[test]
    fn a_link_that_goes_releases_its_notes_next_block() {
        let mut before = linked();
        let mut events = lists();
        events[0].push(on(1));
        run(&mut before, &mut events);
        let played = events[1].iter().find_map(|event| match event.event {
            Event::NoteOn { id, .. } => Some(id),
            _ => None,
        });

        let mut kept = linked();
        kept.carry_from(&before);
        assert_eq!(kept.held(0), 1);

        let mut gone = NotePass::new(&CompiledModulation::default());
        gone.carry_from(&before);
        for list in &mut events {
            list.clear();
        }
        run(&mut gone, &mut events);
        let released = events[1].iter().find_map(|event| match event.event {
            Event::NoteOff { id, .. } => Some((id, event.offset)),
            _ => None,
        });
        assert_eq!(released, played.map(|id| (id, 0)));
    }
}
