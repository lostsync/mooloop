//! What the audio callback costs, and whether it is being given the thread it
//! needs, published from the realtime thread to the GUI.
//!
//! An xrun is the only dropout the host tells us about, and it is the last
//! symptom rather than the first: by the time JACK counts one, the callback
//! has already missed a deadline. Two numbers arrive before that and say
//! which of the two possible faults is happening.
//!
//! *Work time* is how long `process` spends. Against the block's own budget
//! -- 128 frames at 48 kHz is 2.67 ms -- it says whether the engine is simply
//! doing too much for the buffer size it was given.
//!
//! *Wake-up period* is the gap between one callback and the next. It should
//! be the budget, near enough; when it is a multiple of it, the engine did
//! nothing wrong and the operating system did not run the thread. That is a
//! different fault with a different fix, and without this number the two are
//! indistinguishable from inside the program.
//!
//! Same transport as [`crate::meters`] and for the same reason: the GUI wants
//! the recent worst case, never the backlog, and the audio thread must not
//! queue, allocate, or format anything to publish it.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Whether the callback thread is scheduled as a realtime thread.
///
/// Not a `bool`, because "we have not looked yet" is a real state: the check
/// runs on the callback's own thread, which does not exist until the first
/// block, so anything reading before then must be able to say nothing rather
/// than say no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeStatus {
    /// No block has been rendered yet, so the thread has not been asked.
    Unknown,
    /// `SCHED_FIFO` or `SCHED_RR`: the scheduler will run this thread ahead
    /// of ordinary work.
    Realtime,
    /// An ordinary time-shared thread. Audio will glitch whenever the machine
    /// is busy, no matter how cheap the block is.
    TimeShared,
    /// The platform has no way to answer.
    Unsupported,
}

impl RealtimeStatus {
    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Realtime,
            2 => Self::TimeShared,
            3 => Self::Unsupported,
            _ => Self::Unknown,
        }
    }

    fn code(self) -> u32 {
        match self {
            Self::Unknown => 0,
            Self::Realtime => 1,
            Self::TimeShared => 2,
            Self::Unsupported => 3,
        }
    }
}

/// One window of callback timing, as read and cleared by the GUI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadSnapshot {
    /// Blocks rendered in the window. Zero means the engine is not running,
    /// and every other field should be ignored.
    pub blocks: u32,
    /// Mean share of the block budget spent inside `process`, 1.0 being the
    /// whole of it.
    pub mean_load: f32,
    /// The worst single block in the window, on the same scale.
    pub peak_load: f32,
    /// Blocks that took longer than their own budget. One is audible.
    pub over_budget: u32,
    /// Callbacks that arrived more than half a budget late. The engine was
    /// ready; the thread was not running.
    pub late_wakeups: u32,
    /// The worst wake-up gap in the window, as a multiple of the budget.
    pub peak_period: f32,
    /// How the callback thread is scheduled.
    pub realtime: RealtimeStatus,
}

impl LoadSnapshot {
    /// Whether this window contains anything a musician would have heard.
    pub fn had_trouble(&self) -> bool {
        self.over_budget > 0 || self.late_wakeups > 0
    }
}

/// Realtime-side counters for one window. Written by the audio thread with
/// relaxed stores, drained by the GUI.
pub struct LoadMeters {
    blocks: AtomicU32,
    over_budget: AtomicU32,
    late_wakeups: AtomicU32,
    /// Work time summed over the window, in nanoseconds. `u64` because a
    /// second of 64-frame blocks is 750 additions and the sum is real time.
    work_nanos: AtomicU64,
    /// Budget summed over the same blocks, so the mean is a ratio of two
    /// sums rather than a mean of ratios -- an expensive block matters in
    /// proportion to its length.
    budget_nanos: AtomicU64,
    peak_work_permille: AtomicU32,
    peak_period_permille: AtomicU32,
    realtime: AtomicU32,
}

impl LoadMeters {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            blocks: AtomicU32::new(0),
            over_budget: AtomicU32::new(0),
            late_wakeups: AtomicU32::new(0),
            work_nanos: AtomicU64::new(0),
            budget_nanos: AtomicU64::new(0),
            peak_work_permille: AtomicU32::new(0),
            peak_period_permille: AtomicU32::new(0),
            realtime: AtomicU32::new(RealtimeStatus::Unknown.code()),
        })
    }

    /// Record one block. `work` and `budget` are nanoseconds; `period` is the
    /// gap since the previous callback entered, or `None` for the first block
    /// of a run, which has no previous callback to be late relative to.
    ///
    /// Called from the audio thread. Eight relaxed atomic operations, no
    /// division by a runtime zero, no allocation, no branch on a lock.
    pub fn record(&self, work: u64, budget: u64, period: Option<u64>) {
        if budget == 0 {
            return;
        }
        self.blocks.fetch_add(1, Ordering::Relaxed);
        self.work_nanos.fetch_add(work, Ordering::Relaxed);
        self.budget_nanos.fetch_add(budget, Ordering::Relaxed);
        if work > budget {
            self.over_budget.fetch_add(1, Ordering::Relaxed);
        }
        raise(&self.peak_work_permille, permille(work, budget));
        if let Some(period) = period {
            // Half a budget of slack before a late wake-up is called one: a
            // host is entitled to jitter, and counting that as a fault would
            // make the number say "trouble" on a machine doing nothing wrong.
            if period * 2 > budget * 3 {
                self.late_wakeups.fetch_add(1, Ordering::Relaxed);
            }
            raise(&self.peak_period_permille, permille(period, budget));
        }
    }

    /// Publish how the callback thread is scheduled. Cheap enough to call
    /// every block, but the caller asks the kernel only once.
    pub fn set_realtime(&self, status: RealtimeStatus) {
        self.realtime.store(status.code(), Ordering::Relaxed);
    }

    /// The status without draining the window, for a caller that only wants
    /// to know whether the thread is realtime.
    pub fn realtime(&self) -> RealtimeStatus {
        RealtimeStatus::from_code(self.realtime.load(Ordering::Relaxed))
    }

    /// Read the window and start a new one.
    ///
    /// The fields are cleared one at a time rather than together, so a block
    /// landing mid-drain can be counted in one window and timed in the next.
    /// That is a rounding error in a number whose whole purpose is to be read
    /// by a human once a second, and the alternative is a lock on the audio
    /// thread.
    pub fn take(&self) -> LoadSnapshot {
        let blocks = self.blocks.swap(0, Ordering::Relaxed);
        let work = self.work_nanos.swap(0, Ordering::Relaxed);
        let budget = self.budget_nanos.swap(0, Ordering::Relaxed);
        let over_budget = self.over_budget.swap(0, Ordering::Relaxed);
        let late_wakeups = self.late_wakeups.swap(0, Ordering::Relaxed);
        let peak_work = self.peak_work_permille.swap(0, Ordering::Relaxed);
        let peak_period = self.peak_period_permille.swap(0, Ordering::Relaxed);
        LoadSnapshot {
            blocks,
            mean_load: if budget == 0 {
                0.0
            } else {
                work as f32 / budget as f32
            },
            peak_load: peak_work as f32 / 1000.0,
            over_budget,
            late_wakeups,
            peak_period: peak_period as f32 / 1000.0,
            realtime: self.realtime(),
        }
    }
}

/// A ratio in thousandths, saturating rather than wrapping: a block that
/// somehow took four seconds should read as "enormous", not as "small".
fn permille(value: u64, budget: u64) -> u32 {
    u32::try_from(value.saturating_mul(1000) / budget).unwrap_or(u32::MAX)
}

/// Raise a held peak. One writer, so a compare-exchange loop cannot spin
/// against another audio thread; it only retries against the GUI's clearing
/// swap, which happens once a second.
fn raise(cell: &AtomicU32, value: u32) {
    let mut current = cell.load(Ordering::Relaxed);
    while value > current {
        match cell.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(seen) => current = seen,
        }
    }
}

/// Ask the kernel how *this* thread is scheduled.
///
/// Called from the callback thread itself, because that is the only thread
/// whose answer matters: JACK -- and pipewire-jack behind it -- creates the
/// thread and requests realtime scheduling for it, and whether that request
/// was granted is invisible from anywhere else in the program.
#[cfg(unix)]
pub fn thread_realtime_status() -> RealtimeStatus {
    // SAFETY: `sched_getscheduler(0)` reads the calling thread's policy and
    // takes no pointer. It cannot fail for pid 0.
    match unsafe { libc::sched_getscheduler(0) } {
        libc::SCHED_FIFO | libc::SCHED_RR => RealtimeStatus::Realtime,
        code if code < 0 => RealtimeStatus::Unsupported,
        _ => RealtimeStatus::TimeShared,
    }
}

#[cfg(not(unix))]
pub fn thread_realtime_status() -> RealtimeStatus {
    RealtimeStatus::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: u64 = 2_666_666;

    #[test]
    fn a_quiet_window_reports_no_trouble() {
        let meters = LoadMeters::new();
        for _ in 0..100 {
            meters.record(BUDGET / 4, BUDGET, Some(BUDGET));
        }
        let snapshot = meters.take();
        assert_eq!(snapshot.blocks, 100);
        assert!(!snapshot.had_trouble());
        assert!((snapshot.mean_load - 0.25).abs() < 0.01, "{snapshot:?}");
        assert!((snapshot.peak_load - 0.25).abs() < 0.01, "{snapshot:?}");
    }

    #[test]
    fn an_overrunning_block_is_counted_and_held_as_the_peak() {
        let meters = LoadMeters::new();
        for _ in 0..99 {
            meters.record(BUDGET / 4, BUDGET, Some(BUDGET));
        }
        meters.record(BUDGET * 2, BUDGET, Some(BUDGET));
        let snapshot = meters.take();
        assert_eq!(snapshot.over_budget, 1);
        assert_eq!(snapshot.late_wakeups, 0);
        assert!(snapshot.had_trouble());
        assert!((snapshot.peak_load - 2.0).abs() < 0.01, "{snapshot:?}");
    }

    #[test]
    fn a_late_wakeup_is_a_separate_fault_from_a_slow_block() {
        let meters = LoadMeters::new();
        // Cheap blocks throughout: nothing the engine did could explain this.
        for _ in 0..99 {
            meters.record(BUDGET / 10, BUDGET, Some(BUDGET));
        }
        meters.record(BUDGET / 10, BUDGET, Some(BUDGET * 4));
        let snapshot = meters.take();
        assert_eq!(snapshot.over_budget, 0);
        assert_eq!(snapshot.late_wakeups, 1);
        assert!((snapshot.peak_period - 4.0).abs() < 0.01, "{snapshot:?}");
    }

    #[test]
    fn ordinary_host_jitter_is_not_a_late_wakeup() {
        let meters = LoadMeters::new();
        // Forty per cent late, which is inside the half-budget allowance.
        meters.record(BUDGET / 10, BUDGET, Some(BUDGET * 14 / 10));
        assert_eq!(meters.take().late_wakeups, 0);
    }

    #[test]
    fn taking_a_window_starts_the_next_one_empty() {
        let meters = LoadMeters::new();
        meters.record(BUDGET * 2, BUDGET, Some(BUDGET * 4));
        assert!(meters.take().had_trouble());
        let snapshot = meters.take();
        assert_eq!(snapshot.blocks, 0);
        assert!(!snapshot.had_trouble());
        assert_eq!(snapshot.peak_load, 0.0);
    }

    #[test]
    fn the_first_block_of_a_run_has_no_period_to_be_late_against() {
        let meters = LoadMeters::new();
        meters.record(BUDGET / 4, BUDGET, None);
        let snapshot = meters.take();
        assert_eq!(snapshot.blocks, 1);
        assert_eq!(snapshot.late_wakeups, 0);
        assert_eq!(snapshot.peak_period, 0.0);
    }
}
