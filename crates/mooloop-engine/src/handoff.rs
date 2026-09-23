//! Moving the state an audio callback owns from one callback to the next
//! without a lock on the realtime side.
//!
//! A driver whose device can be reopened -- Core Audio, where another device
//! or buffer size is a new stream on a new thread -- has to carry what the
//! callback owns (the executor, its rings) from the old stream's callback to
//! the new one's. It used to live behind a mutex both sides took, and the
//! callback `try_lock`ed it every block (MOO-22): against the letter of "no
//! locks in the callback", and, once the control thread took the same lock
//! once a second to retry an input device, a block of silence whenever the
//! two met.
//!
//! Here the callback owns the state outright, in a [`Held`]. When its stream
//! is dropped, the `Held` parks the state in a [`Parked`] slot; the next
//! stream's callback takes it from there on its first block. The only thing
//! either callback does to the slot is one atomic swap, and only while it has
//! no state of its own -- which is the gap between two streams, when there is
//! nothing to render anyway. The control thread never touches the state
//! again: what it used to change under the lock, it sends through a ring.

use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Arc;

/// A slot holding at most one boxed `T`, handed between threads with atomic
/// swaps: no lock, no wait, no allocation.
pub(crate) struct Parked<T> {
    slot: AtomicPtr<T>,
}

// SAFETY: the slot owns the `T` behind its pointer exactly as a
// `Mutex<Option<Box<T>>>` would, and moves it between threads only whole, by
// swapping the pointer out; nothing ever reads through a pointer the slot
// still holds.
unsafe impl<T: Send> Send for Parked<T> {}
unsafe impl<T: Send> Sync for Parked<T> {}

impl<T> Parked<T> {
    pub(crate) fn new(value: Box<T>) -> Self {
        Self {
            slot: AtomicPtr::new(Box::into_raw(value)),
        }
    }

    /// Take what is parked, if anything. One atomic swap: safe to call from
    /// the audio thread.
    pub(crate) fn take(&self) -> Option<Box<T>> {
        let pointer = self.slot.swap(std::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: a non-null pointer in the slot came from `Box::into_raw` in
        // `new` or `park`, and the swap above made this call its only owner.
        (!pointer.is_null()).then(|| unsafe { Box::from_raw(pointer) })
    }

    /// Park `value`. Anything already parked is dropped, which cannot happen
    /// while one stream runs at a time: only a callback's [`Held`] parks, and
    /// only the state it took from here.
    pub(crate) fn park(&self, value: Box<T>) {
        let previous = self.slot.swap(Box::into_raw(value), Ordering::AcqRel);
        if !previous.is_null() {
            // SAFETY: as in `take`.
            drop(unsafe { Box::from_raw(previous) });
        }
    }
}

impl<T> Drop for Parked<T> {
    fn drop(&mut self) {
        drop(self.take());
    }
}

/// What one callback owns: the state, once it has taken it from `parked`.
/// Dropped with its stream, which parks the state for the next one.
pub(crate) struct Held<T> {
    parked: Arc<Parked<T>>,
    value: Option<Box<T>>,
}

impl<T> Held<T> {
    /// Nothing held yet: the first [`Self::get`] takes the parked state.
    pub(crate) fn new(parked: Arc<Parked<T>>) -> Self {
        Self {
            parked,
            value: None,
        }
    }

    /// The state, taking it from the slot the first time. `None` only while
    /// the previous stream's callback still holds it, which is the one block
    /// a reopen can cost.
    pub(crate) fn get(&mut self) -> Option<&mut T> {
        if self.value.is_none() {
            self.value = self.parked.take();
        }
        self.value.as_deref_mut()
    }
}

impl<T> Drop for Held<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            self.parked.park(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Held, Parked};
    use std::sync::Arc;

    /// The state follows the streams: the first callback takes it, dropping
    /// that callback parks it, and the next one takes it with everything the
    /// first did to it.
    #[test]
    fn the_state_moves_from_one_callback_to_the_next() {
        let parked = Arc::new(Parked::new(Box::new(0u32)));
        let mut first = Held::new(parked.clone());
        *first.get().expect("the first callback takes the parked state") += 1;

        // A second stream is built while the first still plays: it gets
        // nothing, and renders silence rather than waiting.
        let mut second = Held::new(parked.clone());
        assert!(second.get().is_none());

        drop(first);
        let value = second.get().expect("the state was parked when the first stream went");
        assert_eq!(*value, 1);
    }

    /// Nothing is lost or leaked when the driver goes with the state parked
    /// or held.
    #[test]
    fn dropping_everything_drops_the_state_once() {
        let witness = Arc::new(());
        let parked = Arc::new(Parked::new(Box::new(witness.clone())));
        let mut held = Held::new(parked.clone());
        assert!(held.get().is_some());
        assert_eq!(Arc::strong_count(&witness), 2);
        drop(held);
        drop(parked);
        assert_eq!(Arc::strong_count(&witness), 1, "the state was leaked");
    }

    /// Two threads passing the state back and forth never both hold it.
    #[test]
    fn the_state_is_never_held_twice() {
        let parked = Arc::new(Parked::new(Box::new(0u64)));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let parked = parked.clone();
                std::thread::spawn(move || {
                    let mut done = 0;
                    while done < 10_000 {
                        let mut held = Held::new(parked.clone());
                        if let Some(value) = held.get() {
                            *value += 1;
                            done += 1;
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(*parked.take().unwrap(), 40_000);
    }
}
