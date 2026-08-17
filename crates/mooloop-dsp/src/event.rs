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
    /// MIDI note number 0..=127, velocity 0..=127.
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    /// Generic parameter automation point. `id` is node-defined.
    ParamValue { id: u32, value: f32 },
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
            event: Event::NoteOff { note: 0 },
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
                event: Event::NoteOff { note: 0 },
            }));
        }
        assert!(!list.push(TimedEvent {
            offset: 0,
            event: Event::NoteOff { note: 0 },
        }));
        assert_eq!(list.len(), MAX_EVENTS);
    }
}
