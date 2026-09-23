//! The voices the sequencer started and has not yet ended, per channel
//! (MOO-99).
//!
//! Before this, nothing recorded which voices the sequencer had started, so
//! the only release the engine could send was [`Event::Choke`] to a whole
//! channel -- which also cut every key the player was holding -- and an edit
//! that moved a note's off out of reach sent nothing at all: shorten, delete
//! or step off a sounding pad note and it droned until Stop.
//!
//! The table is filled by *reading* what the sequencer scheduled, after it
//! has scheduled a block and before the keyboard and auditions are added to
//! the same lists, so it holds sequencer voices and nothing else. A release
//! is a `NoteOff` for exactly those, which is what lets a loop fold end the
//! pattern's notes and leave a held chord ringing.
//!
//! Fixed-size and `Copy`, so it lives on the channel strip and travels with
//! it through an install, the way the voices it describes do. It is sized for
//! what a channel can realistically have sounding at once, not for the
//! pattern's note store; a channel that overflows it is remembered, and its
//! next release-everything falls back to a `Choke` rather than leave a voice
//! the table never saw.

use mooloop_dsp::{Event, EventList, TimedEvent};

/// How many sequenced voices one channel's table can name at once.
///
/// Every source's own voice pool is smaller than this, so a channel that
/// fills it is already stealing voices.
pub(crate) const MAX_SEQUENCED_VOICES: usize = 64;

/// Where a sequenced voice came from: the pattern that scheduled it and, in
/// Song mode, the start tick of the placement it was scheduled through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct VoiceOrigin {
    pub pattern: u8,
    /// `None` in Pattern mode, which has no placements.
    pub placement: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SequencedVoice {
    /// The id the sequencer put on the `NoteOn`, and will put on its
    /// `NoteOff`: `(instance << 32) | note id`.
    pub id: u64,
    pub note: u8,
    pub origin: VoiceOrigin,
    /// Owed a `NoteOff` at the start of the next block.
    release: bool,
}

impl SequencedVoice {
    /// The id of the stored note this voice is playing, without the instance.
    pub fn note_id(&self) -> u32 {
        self.id as u32
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SequencedVoices {
    voices: [SequencedVoice; MAX_SEQUENCED_VOICES],
    len: usize,
    /// A `NoteOn` arrived with the table full, so a voice is sounding that
    /// no release can name.
    overflowed: bool,
    /// The next delivery owes the channel a `Choke`: a release-everything
    /// was asked for while [`Self::overflowed`].
    owe_choke: bool,
}

impl Default for SequencedVoices {
    fn default() -> Self {
        Self::new()
    }
}

impl SequencedVoices {
    pub const fn new() -> Self {
        Self {
            voices: [SequencedVoice {
                id: 0,
                note: 0,
                origin: VoiceOrigin {
                    pattern: 0,
                    placement: None,
                },
                release: false,
            }; MAX_SEQUENCED_VOICES],
            len: 0,
            overflowed: false,
            owe_choke: false,
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = &SequencedVoice> {
        self.voices[..self.len].iter()
    }

    /// The sequencer started `id`. A second `NoteOn` with the same id is one
    /// voice to release, not two.
    pub fn started(&mut self, id: u64, note: u8, origin: VoiceOrigin) {
        if let Some(voice) = self.voices[..self.len].iter_mut().find(|voice| voice.id == id) {
            voice.note = note;
            voice.origin = origin;
            voice.release = false;
            return;
        }
        if self.len == MAX_SEQUENCED_VOICES {
            self.overflowed = true;
            return;
        }
        self.voices[self.len] = SequencedVoice {
            id,
            note,
            origin,
            release: false,
        };
        self.len += 1;
    }

    /// The sequencer ended `id` itself.
    pub fn ended(&mut self, id: u64) {
        if let Some(index) = self.voices[..self.len].iter().position(|voice| voice.id == id) {
            self.remove(index);
        }
    }

    fn remove(&mut self, index: usize) {
        self.len -= 1;
        self.voices[index] = self.voices[self.len];
    }

    /// Mark every voice `owed` answers true for, to be released at the start
    /// of the next block.
    pub fn release_where(&mut self, owed: impl Fn(&SequencedVoice) -> bool) {
        for voice in self.voices[..self.len].iter_mut() {
            if owed(voice) {
                voice.release = true;
            }
        }
    }

    /// Mark every voice, and owe a `Choke` too if one was never recorded.
    pub fn release_all(&mut self) {
        self.release_where(|_| true);
        if self.overflowed {
            self.owe_choke = true;
            self.overflowed = false;
        }
    }

    /// Forget everything, for a caller that is choking the channel anyway.
    pub fn forget_all(&mut self) {
        self.len = 0;
        self.overflowed = false;
        self.owe_choke = false;
    }

    /// Push the owed releases into `events` at `offset` and drop them from
    /// the table.
    pub fn deliver(&mut self, offset: u32, events: &mut EventList) {
        if self.owe_choke {
            self.owe_choke = false;
            events.push_ordered(TimedEvent {
                offset,
                event: Event::Choke,
            });
        }
        let mut index = 0;
        while index < self.len {
            let voice = self.voices[index];
            if voice.release {
                events.push_ordered(TimedEvent {
                    offset,
                    event: Event::NoteOff {
                        id: voice.id,
                        note: voice.note,
                    },
                });
                self.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Read a block the sequencer has just scheduled into `events`, and end
    /// every voice still sounding at each of `folds` -- the frames at which
    /// the transport turned back at a loop end, in order.
    ///
    /// A voice sounding at a fold has its `NoteOff` somewhere past the loop
    /// end, which the transport is not travelling towards; without this a
    /// pad held across the loop point is joined by another one every pass.
    /// Only the sequencer's voices: a key the player is holding is theirs,
    /// and survives the fold (MOO-99).
    ///
    /// `origin` says where a voice id came from; it is asked once per
    /// `NoteOn`.
    pub fn observe(
        &mut self,
        events: &mut EventList,
        folds: &[u32],
        origin: impl Fn(u64) -> VoiceOrigin,
    ) {
        const ROOM: usize = 2 * MAX_SEQUENCED_VOICES;
        let mut releases = [(0u32, 0u64, 0u8); ROOM];
        let mut released = 0;
        let mut choke_at: Option<u32> = None;
        let mut next_fold = 0;
        let mut fold = |table: &mut Self, at: u32| {
            for voice in table.voices[..table.len].iter() {
                if released < ROOM {
                    releases[released] = (at, voice.id, voice.note);
                    released += 1;
                } else {
                    choke_at = choke_at.or(Some(at));
                }
            }
            if table.overflowed {
                choke_at = choke_at.or(Some(at));
                table.overflowed = false;
            }
            table.len = 0;
        };
        for event in events.iter() {
            while next_fold < folds.len() && event.offset >= folds[next_fold] {
                fold(self, folds[next_fold]);
                next_fold += 1;
            }
            match event.event {
                Event::NoteOn { id, note, .. } => self.started(id, note, origin(id)),
                Event::NoteOff { id, .. } => self.ended(id),
                _ => {}
            }
        }
        while next_fold < folds.len() {
            fold(self, folds[next_fold]);
            next_fold += 1;
        }
        for &(offset, id, note) in &releases[..released] {
            events.push_ordered(TimedEvent {
                offset,
                event: Event::NoteOff { id, note },
            });
        }
        if let Some(offset) = choke_at {
            events.push_ordered(TimedEvent {
                offset,
                event: Event::Choke,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: VoiceOrigin = VoiceOrigin {
        pattern: 0,
        placement: None,
    };

    fn on(offset: u32, id: u64, note: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOn {
                id,
                note,
                velocity: 100,
            },
        }
    }

    fn off(offset: u32, id: u64, note: u8) -> TimedEvent {
        TimedEvent {
            offset,
            event: Event::NoteOff { id, note },
        }
    }

    #[test]
    fn a_voice_the_sequencer_ended_is_forgotten_and_one_it_did_not_is_kept() {
        let mut table = SequencedVoices::new();
        let mut events = EventList::empty();
        events.push_ordered(on(0, 1, 60));
        events.push_ordered(on(10, 2, 62));
        events.push_ordered(off(20, 1, 60));
        table.observe(&mut events, &[], |_| ORIGIN);
        assert_eq!(table.iter().map(|voice| voice.id).collect::<Vec<_>>(), [2]);
    }

    /// A fold ends what was sounding at it, at its frame, and nothing
    /// scheduled after it: the voice the next lap starts is the new one.
    #[test]
    fn a_fold_releases_what_is_sounding_at_it_and_keeps_what_starts_after() {
        let mut table = SequencedVoices::new();
        table.started(7, 48, ORIGIN);
        let mut events = EventList::empty();
        events.push_ordered(on(100, 8, 50));
        events.push_ordered(on(300, 9, 52));
        table.observe(&mut events, &[200], |_| ORIGIN);

        let releases: Vec<(u32, u64)> = events
            .iter()
            .filter_map(|event| match event.event {
                Event::NoteOff { id, .. } => Some((event.offset, id)),
                _ => None,
            })
            .collect();
        assert_eq!(releases, [(200, 7), (200, 8)]);
        assert_eq!(table.iter().map(|voice| voice.id).collect::<Vec<_>>(), [9]);
    }

    #[test]
    fn a_marked_voice_is_released_once_and_an_unmarked_one_is_not() {
        let mut table = SequencedVoices::new();
        table.started(1, 60, ORIGIN);
        table.started(2, 62, ORIGIN);
        table.release_where(|voice| voice.note_id() == 2);
        let mut events = EventList::empty();
        table.deliver(0, &mut events);
        assert_eq!(events.iter().copied().collect::<Vec<_>>(), [off(0, 2, 62)]);
        let mut again = EventList::empty();
        table.deliver(0, &mut again);
        assert!(again.is_empty());
        assert_eq!(table.len(), 1);
    }

    /// A voice the table never recorded can only be ended by a choke, so a
    /// release-everything after an overflow sends one.
    #[test]
    fn an_overflowed_table_owes_a_choke_on_release_all() {
        let mut table = SequencedVoices::new();
        for id in 0..=MAX_SEQUENCED_VOICES as u64 {
            table.started(id, 60, ORIGIN);
        }
        table.release_all();
        let mut events = EventList::empty();
        table.deliver(0, &mut events);
        assert!(events.iter().any(|event| event.event == Event::Choke));
        assert!(table.is_empty());
    }
}
