//! A mutex the audio callback can be caught taking (MOO-173).
//!
//! `docs/AUDIO_ARCHITECTURE.md` says the callback takes no lock. The
//! allocation half of that contract has had a check for a long time: the
//! engine's `CountingAllocator` counts, per thread, every call into the
//! allocator, and a test that runs blocks through the `Executor` fails if
//! the count moved. This module is the same instrument for locks: a
//! per-thread count of acquisitions, which the same tests read before and
//! after each block.
//!
//! **What it can see, honestly.** Only locks taken through [`Mutex`] here --
//! locks on types we own. A `std::sync::Mutex` on Linux is a compare-and-swap
//! in user space when it is uncontended, with no system call and no function
//! anything outside `std` could wrap, so no test can observe one being
//! taken; nor `stderr().lock()` inside an `eprintln!`, nor a lock inside a
//! hosted plugin's own code. A lock on the render path therefore has to be
//! this type for the check to see it, and a render-path crate's lock sites
//! use it for that reason.
//!
//! **Zero cost in release.** The counter exists only under
//! `debug_assertions`, which the test profile has and a release build does
//! not; without it [`Mutex::lock`] is `std`'s, inlined, and
//! [`locks_taken`] is a constant zero. A lock-counting test run under
//! `--release` therefore passes without checking anything, which
//! [`counting`] lets a test assert against.
//!
//! An attempt is counted whether or not it succeeds: a `try_lock` that finds
//! the mutex free takes it, and one that finds it held is the audio thread
//! contending with another thread for a cache line, which is the thing the
//! contract forbids either way.

use std::sync::{LockResult, MutexGuard, TryLockResult};

#[cfg(debug_assertions)]
thread_local! {
    /// Lock attempts made on this thread. `const`-initialised so touching it
    /// cannot allocate, and read through `try_with` so an attempt during TLS
    /// teardown is dropped rather than panicking.
    static LOCK_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline(always)]
fn note_attempt() {
    #[cfg(debug_assertions)]
    let _ = LOCK_CALLS.try_with(|calls| calls.set(calls.get() + 1));
}

/// How many times this thread has tried to take a [`Mutex`]. Never
/// decreases; compare two readings. Always zero when [`counting`] is false.
pub fn locks_taken() -> usize {
    #[cfg(debug_assertions)]
    {
        LOCK_CALLS.try_with(std::cell::Cell::get).unwrap_or(0)
    }
    #[cfg(not(debug_assertions))]
    {
        0
    }
}

/// Whether this build counts lock attempts at all.
pub const fn counting() -> bool {
    cfg!(debug_assertions)
}

/// `std::sync::Mutex`, counted. Same methods, same guard, same poisoning;
/// [`Mutex::lock`] and [`Mutex::try_lock`] add one to [`locks_taken`] on the
/// calling thread first.
#[derive(Default)]
pub struct Mutex<T: ?Sized>(std::sync::Mutex<T>);

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self(std::sync::Mutex::new(value))
    }

    pub fn into_inner(self) -> LockResult<T> {
        self.0.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    pub fn lock(&self) -> LockResult<MutexGuard<'_, T>> {
        note_attempt();
        self.0.lock()
    }

    #[inline]
    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, T>> {
        note_attempt();
        self.0.try_lock()
    }

    /// Takes no lock (`&mut` already proves exclusive access), so it is not
    /// counted.
    pub fn get_mut(&mut self) -> LockResult<&mut T> {
        self.0.get_mut()
    }
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_attempt_is_counted_on_its_own_thread_and_nowhere_else() {
        assert!(counting(), "the test profile builds with debug assertions");
        let mutex = Mutex::new(0u32);
        let before = locks_taken();
        *mutex.lock().expect("unpoisoned") += 1;
        assert_eq!(locks_taken() - before, 1);

        let held = mutex.lock().expect("unpoisoned");
        assert!(mutex.try_lock().is_err(), "held above");
        drop(held);
        assert_eq!(locks_taken() - before, 3, "a failed try_lock is an attempt");

        std::thread::scope(|scope| {
            scope.spawn(|| {
                drop(mutex.lock());
            });
        });
        assert_eq!(locks_taken() - before, 3, "another thread's lock is its own");

        let mut mutex = mutex;
        *mutex.get_mut().expect("unpoisoned") += 1;
        assert_eq!(locks_taken() - before, 3, "get_mut takes no lock");
        assert_eq!(mutex.into_inner().expect("unpoisoned"), 2);
    }
}
