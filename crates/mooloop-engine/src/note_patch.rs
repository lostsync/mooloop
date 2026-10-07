//! Notes through the song's patch (`docs/plans/song-patch/07-notes-through-the-patch.md`).
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
//! **Every note the pass plays has an id of its own**, from a range the
//! sequencer, the keyboard and auditions do not reach in practice, and each
//! link keeps a table from the id it heard to the id it played. A NoteOff
//! releases exactly that; a Choke on the source releases everything the link
//! played from it, at the same offset. The tables are fixed-size, built off
//! the audio thread with the set: a NoteOn that finds its link's table full,
//! or its channel's list full, is refused and counted on the notes-out tag,
//! never half-played -- its NoteOff then finds nothing to release.
//!
//! **A link that goes while notes sound releases them** at the start of the
//! next block ([`NotePass::carry_from`]), on whichever seat its channel has
//! moved to. More releases than the pass has room for become a Choke on the
//! channel, so nothing is left sounding that nothing can name.

use mooloop_core::{CompiledModulation, CompiledNoteLink, ModSourceId, MAX_CHANNELS};
use mooloop_dsp::{Event, EventList, TimedEvent};

/// How many notes one link can hold sounding at once, as many as a channel's
/// sequenced voices ([`crate::voices::MAX_SEQUENCED_VOICES`]): every source's
/// voice pool is smaller, so a link that fills it is already stealing.
pub(crate) const MAX_LINK_NOTES: usize = 64;

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
    /// Whether link `i` is the first from its notes-in tag, so the tag
    /// counts each note it passes once however many wires leave it.
    counts_input: Vec<bool>,
    taken: [bool; MAX_CHANNELS],
    any_taken: bool,
    /// The distinct seats the links read, and where each one's notes start
    /// in `heard`.
    sources: Vec<u8>,
    heard: Vec<TimedEvent>,
    heard_at: Vec<(u32, u32)>,
    owed: Vec<Owed>,
    choke_owed: [bool; MAX_CHANNELS],
    next_id: u64,
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
        let mut sources: Vec<u8> = links.iter().map(|link| link.from_seat).collect();
        sources.sort_unstable();
        sources.dedup();
        let counts_input = (0..links.len())
            .map(|at| !links[..at].iter().any(|earlier| earlier.from == links[at].from))
            .collect();
        let mut taken = [false; MAX_CHANNELS];
        for &seat in &plan.taken {
            taken[usize::from(seat)] = true;
        }
        Self {
            tables: vec![LinkTable::default(); links.len()],
            counts_input,
            taken,
            any_taken: !plan.taken.is_empty(),
            heard: Vec::with_capacity(sources.len() * EventList::CAPACITY),
            heard_at: vec![(0, 0); sources.len()],
            sources,
            owed: Vec::with_capacity(MAX_OWED),
            choke_owed: [false; MAX_CHANNELS],
            next_id: 0,
            tag_ids: plan.tags.iter().map(|tag| tag.id).collect(),
            tag_notes: vec![0; plan.tags.len()],
            tag_refused: vec![0; plan.tags.len()],
            links,
        }
    }

    /// Take every link's held notes from `previous` that names the same
    /// link, and owe a release for every note a link that is gone still
    /// holds. Audio thread: compares and copies, allocating nothing.
    pub(crate) fn carry_from(&mut self, previous: &NotePass) {
        self.next_id = previous.next_id;
        for (at, id) in self.tag_ids.iter().enumerate() {
            if let Some(from) = previous.tag_ids.iter().position(|old| old == id) {
                self.tag_notes[at] = previous.tag_notes[from];
                self.tag_refused[at] = previous.tag_refused[from];
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
            && self.owed.is_empty()
            && !self.any_taken
            && !self.choke_owed.iter().any(|&choke| choke)
    }

    /// Run the pass over the first `live` channels' complete event lists.
    /// With `panicked`, every table is emptied after the Chokes the panic
    /// put in every list have released what they held. Allocates nothing.
    pub(crate) fn process(&mut self, events: &mut [Box<EventList>], live: usize, panicked: bool) {
        if self.idle() {
            return;
        }
        let live = live.min(events.len());
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
        for seat in 0..live {
            if std::mem::take(&mut self.choke_owed[seat]) {
                let _ = events[seat].push_ordered(TimedEvent {
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
        // A taken channel's own notes reach only the patch. Its chokes,
        // parameters and bends stay.
        for (seat, list) in events.iter_mut().enumerate().take(live) {
            if self.any_taken && self.taken[seat] {
                list.retain(|event| !matches!(event.event, Event::NoteOn { .. } | Event::NoteOff { .. }));
            }
        }
        for at in 0..self.links.len() {
            let link = self.links[at];
            let Some(source) = self.sources.iter().position(|&seat| seat == link.from_seat) else {
                continue;
            };
            let (start, end) = self.heard_at[source];
            let to = usize::from(link.to_seat);
            for index in start as usize..end as usize {
                let heard = self.heard[index];
                match heard.event {
                    Event::NoteOn { id, note, velocity } => {
                        if self.counts_input[at] {
                            bump(&mut self.tag_notes, link.from_tag);
                        }
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

    /// How many notes link `at` holds sounding.
    #[cfg(test)]
    pub(crate) fn held(&self, at: usize) -> usize {
        self.tables.get(at).map_or(0, |table| table.len)
    }
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
        pass.process(&mut events, 2, false);
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
        pass.process(&mut events, 2, false);
        assert_eq!(count(&events[1], false), 1);
        assert_eq!(pass.held(0), MAX_LINK_NOTES - 1);

        // Room for one again.
        for list in &mut events {
            list.clear();
        }
        events[0].push(on(500));
        pass.process(&mut events, 2, false);
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
        pass.process(&mut events, 2, false);
        for list in &mut events {
            list.clear();
        }
        events[0].push(TimedEvent {
            offset: 7,
            event: Event::Choke,
        });
        pass.process(&mut events, 2, false);
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
        before.process(&mut events, 2, false);
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
        gone.process(&mut events, 2, false);
        let released = events[1].iter().find_map(|event| match event.event {
            Event::NoteOff { id, .. } => Some((id, event.offset)),
            _ => None,
        });
        assert_eq!(released, played.map(|id| (id, 0)));
    }
}
