//! Sample-accurate event lists — the note/automation pipe between the
//! sequencer and the nodes.
//!
//! Every node receives its block's events as a sorted [`EventList`]: each
//! event carries a sample offset into the block, exactly like VST3's
//! `IEventList` / LV2 atom sequences / CLAP's event list. Nodes split their
//! processing at event offsets, so note timing is sample-accurate regardless
//! of block size.
//!
//! [`EventList`] has fixed inline capacity and never allocates, so it is safe
//! to fill on the realtime thread. The sequencer emits events in time order;
//! nodes may rely on that (documented contract, not enforced).

use mooloop_core::BufferEvent;

/// Events a node may receive or emit. Extendable without changing the list
/// mechanics (automation params, MPE, etc. slot in here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    /// `id` pairs note-on/off across overlapping notes of the same pitch.
    NoteOn {
        id: u64,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        id: u64,
        note: u8,
    },
    /// Release every active voice quickly. Used by channel choke groups.
    Choke,
    /// Generic parameter automation point. `id` is node-defined.
    ParamValue {
        id: u32,
        value: f32,
    },
    /// A depth inside one of the generator's own modulation routes, by the
    /// route's durable id.
    ///
    /// Separate from [`Self::ParamValue`] because it is not a parameter: a
    /// device whose modulation is part of its patch has automatable values
    /// that belong to a route rather than to the device's table, and giving
    /// them a `u32` id would mean carving a permanent block out of that table
    /// for a route capacity that is deliberately provisional. Nodes with no
    /// internal routes ignore it.
    SourceRouteAmount {
        route: u16,
        amount: f32,
    },
    /// Atomic retained-audio edit for a buffer insert.
    Buffer(BufferEvent),
    /// End of a gated retained-audio edit — the note-off half of an event
    /// whose duration is `Gate`. Latching edits ignore it, so a release can
    /// be sent unconditionally without cancelling an unrelated event.
    BufferRelease,
    /// Move a retained-audio scrub platter by a signed distance in frames.
    /// The first one detaches the head; position is the input and rate is
    /// derived from it, so this is a platter rather than a rate control.
    BufferScrub {
        delta_frames: f32,
    },
}

/// One event and its position inside the current block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimedEvent {
    /// Frames into the block, `0..frames`.
    pub offset: u32,
    pub event: Event,
}

/// Generous headroom: a 128-frame block at 999 bpm with 16 steps/bar holds
/// ~2 notes. A full bar of 32nds across a giant block is still far below cap.
const MAX_EVENTS: usize = 256;

/// A fixed-capacity, allocation-free list of sample-timed events.
pub struct EventList {
    buf: [TimedEvent; MAX_EVENTS],
    len: usize,
}

impl EventList {
    pub const fn empty() -> Self {
        const DUMMY: TimedEvent = TimedEvent {
            offset: 0,
            event: Event::NoteOff { id: 0, note: 0 },
        };
        Self {
            buf: [DUMMY; MAX_EVENTS],
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Append an event. Callers must append in time order. Returns `false`
    /// (dropping the event) if the list is full.
    pub fn push(&mut self, event: TimedEvent) -> bool {
        if self.len == MAX_EVENTS {
            return false;
        }
        self.buf[self.len] = event;
        self.len += 1;
        true
    }

    /// Insert by sample offset with deterministic ordering at equal offsets:
    /// note-offs, parameter changes, then note-ons. This lets a retrigger end
    /// the old voice before starting the new one without allocating a sort
    /// buffer on the realtime thread.
    ///
    /// The parameter-before-note-on half of that order is a published
    /// contract, not an implementation detail. A generator that *latches*
    /// parameters at note-on — DS-01 does, because a drum hit's shape is
    /// decided when it begins — depends on it: a route aimed at a hit must
    /// reach it, and under the opposite order it would land on the next one
    /// instead. That failure is silent and no synth test would catch it,
    /// which is why `parameter_changes_precede_note_ons_at_one_offset`
    /// pins it here rather than leaving it to whichever device noticed.
    pub fn push_ordered(&mut self, event: TimedEvent) -> bool {
        if self.len == MAX_EVENTS {
            return false;
        }
        let key = event_sort_key(&event);
        let index =
            self.buf[..self.len].partition_point(|existing| event_sort_key(existing) <= key);
        self.buf.copy_within(index..self.len, index + 1);
        self.buf[index] = event;
        self.len += 1;
        true
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &TimedEvent> {
        self.buf[..self.len].iter()
    }
}

fn event_sort_key(event: &TimedEvent) -> (u32, u8) {
    let priority = match event.event {
        Event::NoteOff { .. } | Event::Choke => 0,
        Event::ParamValue { .. }
        | Event::SourceRouteAmount { .. }
        | Event::Buffer(_)
        | Event::BufferRelease
        | Event::BufferScrub { .. } => 1,
        Event::NoteOn { .. } => 2,
    };
    (event.offset, priority)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_iterate_in_order() {
        let mut list = EventList::empty();
        for i in 0..3u32 {
            assert!(list.push(TimedEvent {
                offset: i * 10,
                event: Event::NoteOn {
                    id: u64::from(i),
                    note: 60 + i as u8,
                    velocity: 100,
                },
            }));
        }
        let offsets: Vec<u32> = list.iter().map(|e| e.offset).collect();
        assert_eq!(offsets, [0, 10, 20]);
        assert_eq!(list.len(), 3);
        list.clear();
        assert!(list.is_empty());
    }

    #[test]
    fn overflow_drops_without_panicking() {
        let mut list = EventList::empty();
        for _ in 0..MAX_EVENTS {
            assert!(list.push(TimedEvent {
                offset: 0,
                event: Event::NoteOff { id: 0, note: 0 },
            }));
        }
        assert!(!list.push(TimedEvent {
            offset: 0,
            event: Event::NoteOff { id: 0, note: 0 },
        }));
        assert_eq!(list.len(), MAX_EVENTS);
    }

    #[test]
    fn ordered_insert_puts_note_off_before_retrigger() {
        let mut list = EventList::empty();
        assert!(list.push_ordered(TimedEvent {
            offset: 12,
            event: Event::NoteOn {
                id: 2,
                note: 60,
                velocity: 100
            },
        }));
        assert!(list.push_ordered(TimedEvent {
            offset: 12,
            event: Event::NoteOff { id: 1, note: 60 },
        }));
        assert!(matches!(
            list.iter().next().unwrap().event,
            Event::NoteOff { id: 1, .. }
        ));
    }

    /// The contract `push_ordered` documents, pinned: a parameter event at
    /// offset `n` is visible to a note-on at offset `n`, whichever order the
    /// two were pushed in.
    #[test]
    fn parameter_changes_precede_note_ons_at_one_offset() {
        for note_first in [true, false] {
            let mut list = EventList::empty();
            let note = TimedEvent {
                offset: 64,
                event: Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity: 100,
                },
            };
            let param = TimedEvent {
                offset: 64,
                event: Event::ParamValue { id: 7, value: 0.5 },
            };
            if note_first {
                assert!(list.push_ordered(note));
                assert!(list.push_ordered(param));
            } else {
                assert!(list.push_ordered(param));
                assert!(list.push_ordered(note));
            }
            let kinds: Vec<u8> = list.iter().map(event_sort_key).map(|key| key.1).collect();
            assert_eq!(kinds, vec![1, 2], "note-on preceded the parameter it needs");
        }
    }
}
