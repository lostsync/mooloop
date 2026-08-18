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
    /// Generic parameter automation point. `id` is node-defined.
    ParamValue {
        id: u32,
        value: f32,
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
        Event::NoteOff { .. } => 0,
        Event::ParamValue { .. } => 1,
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
}
