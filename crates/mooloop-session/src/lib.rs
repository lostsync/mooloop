//! The live application model: session state, edit logic, undo, and the
//! commands that reach the engine.
//!
//! Nothing here knows what a window is. `mooloop-ui` owns the window, the
//! models, the callbacks, and the projection into them; this crate owns
//! everything the application would still need if that view were replaced.
//! The boundary is enforced by the absence of `slint` from `Cargo.toml`
//! rather than by convention: the session speaks `String` and `PathBuf`, and
//! the view converts.

pub mod audio_file;
pub mod automation;
pub mod browser;
pub mod channel;
pub mod command;
pub mod dialogs;
pub mod document;
#[cfg(test)]
mod edit_cost;

/// A counting allocator, installed only for this crate's own tests.
///
/// `edit_cost::undo_entry_memory` asks how much memory an undo entry holds,
/// and resident set size cannot answer it: pages are the wrong granularity
/// and freed memory is not returned to the OS, so a warmed-up allocator
/// reported several project sizes as costing nothing at all.
#[cfg(test)]
pub(crate) struct CountingAllocator {
    live: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl CountingAllocator {
    pub(crate) fn live(&self) -> usize {
        self.live.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        self.live
            .fetch_add(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        self.live
            .fetch_sub(layout.size(), std::sync::atomic::Ordering::Relaxed);
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

#[cfg(test)]
#[global_allocator]
pub(crate) static COUNTING: CountingAllocator = CountingAllocator {
    live: std::sync::atomic::AtomicUsize::new(0),
};
pub mod effects;
pub mod engine;
pub mod history;
pub mod mixer;
pub mod modulation;
pub mod notes;
pub mod project;
pub mod rack;
pub mod roll;
pub mod sample;
pub mod sampler;
pub mod session;
pub mod steps;
pub mod transport;
pub mod values;
