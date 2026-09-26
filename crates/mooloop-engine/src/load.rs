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
    /// Blocks played as silence because something in them panicked. The
    /// callback survives a panic (`Executor::process_contained`); this is how
    /// the rest of the program hears that one happened.
    pub faults: u32,
    /// How the callback thread is scheduled.
    pub realtime: RealtimeStatus,
    /// The latest callback in the window that passed
    /// [`HOT_SPOT_SHARE_PERCENT`] of its budget, and where its time went
    /// (MOO-236); `None` when none did.
    pub hot_spot: Option<HotSpot>,
    /// How many callbacks in the window passed it.
    pub hot_spots: u32,
}

impl LoadSnapshot {
    /// Whether this window contains anything a musician would have heard.
    pub fn had_trouble(&self) -> bool {
        self.over_budget > 0 || self.late_wakeups > 0
    }
}

/// Where a callback's time went (MOO-236): a channel's strip (its source and
/// chain, fader and sends) or a bus's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    Channel(u8),
    Bus(u8),
}

impl Site {
    /// Packed for an atomic: the kind in bit 8, the index below, and zero
    /// for "no site", which is why the kind bits start at one.
    fn code(self) -> u32 {
        match self {
            Self::Channel(index) => 0x100 | u32::from(index),
            Self::Bus(index) => 0x200 | u32::from(index),
        }
    }

    fn from_code(code: u32) -> Option<Self> {
        let index = (code & 0xFF) as u8;
        match code >> 8 {
            1 => Some(Self::Channel(index)),
            2 => Some(Self::Bus(index)),
            _ => None,
        }
    }
}

/// How many of a slow callback's costliest sites a [`HotSpot`] names.
pub const HOT_SPOT_SITES: usize = 3;

/// The share of its budget past which a callback publishes a [`HotSpot`]:
/// well before it is heard, so the record is there on the callbacks that
/// lead up to a dropout as well as on the dropout itself.
pub const HOT_SPOT_SHARE_PERCENT: u64 = 60;

/// One callback that ran long, and where (MOO-236).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotSpot {
    /// The song position at the block's start, in ticks.
    pub tick: u64,
    /// The block's length.
    pub frames: u32,
    /// The whole callback's work, and its budget, in nanoseconds.
    pub work_nanos: u64,
    pub budget_nanos: u64,
    /// The costliest sites, dearest first, with what each took in
    /// nanoseconds; `None` past the last one timed.
    pub sites: [Option<(Site, u32)>; HOT_SPOT_SITES],
}

/// A single-slot seqlock: the audio thread overwrites, the GUI reads the
/// latest. Every field is an atomic, so a torn read is a retry rather than
/// undefined behaviour, and the writer never waits on anything.
struct HotSpotSlot {
    /// Even when stable, odd while the audio thread is writing. Zero means
    /// nothing has been written since the last take.
    sequence: AtomicU64,
    tick: AtomicU64,
    frames: AtomicU32,
    work_nanos: AtomicU64,
    budget_nanos: AtomicU64,
    sites: [AtomicU32; HOT_SPOT_SITES],
    site_nanos: [AtomicU32; HOT_SPOT_SITES],
    /// Records written since the last take: the one read is the latest.
    written: AtomicU32,
}

impl HotSpotSlot {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            tick: AtomicU64::new(0),
            frames: AtomicU32::new(0),
            work_nanos: AtomicU64::new(0),
            budget_nanos: AtomicU64::new(0),
            sites: std::array::from_fn(|_| AtomicU32::new(0)),
            site_nanos: std::array::from_fn(|_| AtomicU32::new(0)),
            written: AtomicU32::new(0),
        }
    }

    fn write(&self, spot: &HotSpot) {
        // Odd: a reader that sees this, or sees it change, reads again.
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence.store(sequence.wrapping_add(1) | 1, Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        self.tick.store(spot.tick, Ordering::Relaxed);
        self.frames.store(spot.frames, Ordering::Relaxed);
        self.work_nanos.store(spot.work_nanos, Ordering::Relaxed);
        self.budget_nanos.store(spot.budget_nanos, Ordering::Relaxed);
        for (index, site) in spot.sites.iter().enumerate() {
            let (code, nanos) = site.map_or((0, 0), |(site, nanos)| (site.code(), nanos));
            self.sites[index].store(code, Ordering::Relaxed);
            self.site_nanos[index].store(nanos, Ordering::Relaxed);
        }
        // Even again, and never zero, which means "empty".
        let done = (sequence | 1).wrapping_add(1).max(2);
        self.sequence.store(done, Ordering::Release);
        self.written.fetch_add(1, Ordering::Relaxed);
    }

    /// The latest record and how many were written since the last take,
    /// then empty. A few retries at most: the writer holds the slot for a
    /// dozen stores, once a callback at most.
    fn take(&self) -> (Option<HotSpot>, u32) {
        let written = self.written.swap(0, Ordering::Relaxed);
        for _ in 0..64 {
            let before = self.sequence.load(Ordering::Acquire);
            if before == 0 {
                return (None, written);
            }
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let mut sites = [None; HOT_SPOT_SITES];
            for (index, site) in sites.iter_mut().enumerate() {
                *site = Site::from_code(self.sites[index].load(Ordering::Relaxed))
                    .map(|site| (site, self.site_nanos[index].load(Ordering::Relaxed)));
            }
            let spot = HotSpot {
                tick: self.tick.load(Ordering::Relaxed),
                frames: self.frames.load(Ordering::Relaxed),
                work_nanos: self.work_nanos.load(Ordering::Relaxed),
                budget_nanos: self.budget_nanos.load(Ordering::Relaxed),
                sites,
            };
            std::sync::atomic::fence(Ordering::Acquire);
            if self.sequence.load(Ordering::Relaxed) == before {
                // Empty it for the next window, unless a new record landed
                // in between, which the next take reads instead.
                let _ = self.sequence.compare_exchange(before, 0, Ordering::Relaxed, Ordering::Relaxed);
                return (Some(spot), written);
            }
        }
        (None, written)
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
    faults: AtomicU32,
    realtime: AtomicU32,
    /// The callback thread, as the kernel names it (a Linux thread id, a
    /// macOS Mach port), recorded on its first block; zero until then. The
    /// GUI asks the kernel about *this* thread each time it reads the status
    /// (MOO-215), because a driver may promote it to realtime after its first
    /// block, and rtkit may demote it at any time.
    thread: AtomicU64,
    /// Every callback since the engine started, never cleared: whether this
    /// has moved is whether the engine is running at all, which is a
    /// different question from the window's and must not be answered by
    /// whoever last drained it.
    callbacks: AtomicU64,
    /// The latest callback that ran long, and where (MOO-236).
    hot_spot: HotSpotSlot,
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
            faults: AtomicU32::new(0),
            realtime: AtomicU32::new(RealtimeStatus::Unknown.code()),
            thread: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            hot_spot: HotSpotSlot::new(),
        })
    }

    /// Publish a callback that ran long, over the last one if the GUI has
    /// not read it. Called from the audio thread: relaxed stores into one
    /// slot, no allocation, no lock, never a wait.
    pub fn publish_hot_spot(&self, spot: &HotSpot) {
        self.hot_spot.write(spot);
    }

    /// Callbacks since the engine started, including those that panicked.
    /// Never cleared, unlike the window [`Self::take`] drains.
    pub fn callbacks(&self) -> u64 {
        self.callbacks.load(Ordering::Relaxed)
    }

    /// Record a block that panicked and was played as silence. Called from
    /// the audio thread; two relaxed increments.
    pub fn record_fault(&self) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.faults.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one block. `work` and `budget` are nanoseconds; `period` is the
    /// gap since the previous callback entered, or `None` for the first block
    /// of a run, which has no previous callback to be late relative to.
    ///
    /// Called from the audio thread. Nine relaxed atomic operations, no
    /// division by a runtime zero, no allocation, no branch on a lock.
    pub fn record(&self, work: u64, budget: u64, period: Option<u64>) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
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

    /// Publish how the callback thread is scheduled.
    pub fn set_realtime(&self, status: RealtimeStatus) {
        self.realtime.store(status.code(), Ordering::Relaxed);
    }

    /// Record the calling thread as the callback's, and publish how it is
    /// scheduled now. Called once, from the callback's first block: two
    /// system calls then, and none on any block after it -- every later
    /// reading is made by whoever asks, from their own thread
    /// ([`Self::realtime`]).
    pub fn set_callback_thread(&self) {
        self.thread.store(current_thread_id(), Ordering::Relaxed);
        self.set_realtime(thread_realtime_status());
    }

    /// How the callback thread is scheduled **now**, without draining the
    /// window. Never call it from the callback.
    ///
    /// Asked of the kernel each time, about the thread the first block
    /// recorded (MOO-215). The first block's own answer used to stand for
    /// the whole session, and PipeWire's JACK layer can promote the thread
    /// after that block, so the readout said "not realtime" for a thread
    /// `chrt -p` reported as `SCHED_FIFO`. It is also how a demotion
    /// mid-session shows, which is the case the readout exists for. Before
    /// the first block, and on a platform with no way to ask about another
    /// thread, it is the last published answer.
    pub fn realtime(&self) -> RealtimeStatus {
        let thread = self.thread.load(Ordering::Relaxed);
        if thread != 0 {
            if let Some(status) = realtime_status_of(thread) {
                self.set_realtime(status);
                return status;
            }
        }
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
        let faults = self.faults.swap(0, Ordering::Relaxed);
        let (hot_spot, hot_spots) = self.hot_spot.take();
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
            faults,
            realtime: self.realtime(),
            hot_spot,
            hot_spots,
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
/// whose answer matters: the driver -- JACK and pipewire-jack behind it, or
/// Core Audio -- creates the thread and requests realtime scheduling for it,
/// and whether that request was granted is invisible from anywhere else in
/// the program.
#[cfg(target_os = "linux")]
pub fn thread_realtime_status() -> RealtimeStatus {
    // SAFETY: `sched_getscheduler(0)` reads the calling thread's policy and
    // takes no pointer. It cannot fail for pid 0.
    linux_policy_status(unsafe { libc::sched_getscheduler(0) })
}

#[cfg(target_os = "linux")]
fn linux_policy_status(policy: libc::c_int) -> RealtimeStatus {
    match policy {
        libc::SCHED_FIFO | libc::SCHED_RR => RealtimeStatus::Realtime,
        code if code < 0 => RealtimeStatus::Unsupported,
        _ => RealtimeStatus::TimeShared,
    }
}

/// The calling thread's kernel id, for [`LoadMeters::set_callback_thread`].
#[cfg(target_os = "linux")]
fn current_thread_id() -> u64 {
    // SAFETY: `gettid` takes no arguments and cannot fail.
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    u64::try_from(tid).unwrap_or(0)
}

/// How the thread `thread` names is scheduled, asked from any thread of
/// this process. A thread that has gone is `Unsupported`: the kernel no
/// longer has an answer, and repeating the last one would be a guess. Until
/// the kernel reuses its id, that is; after the callback thread ends, a
/// later thread may take its id and be the one described, which only
/// matters while no engine is running and nothing reads the badge.
#[cfg(target_os = "linux")]
fn realtime_status_of(thread: u64) -> Option<RealtimeStatus> {
    let tid = libc::pid_t::try_from(thread).ok()?;
    // SAFETY: reads another thread's policy by id; takes no pointer. An id
    // that is no longer a thread fails with ESRCH, read as `Unsupported`.
    Some(linux_policy_status(unsafe { libc::sched_getscheduler(tid) }))
}

/// macOS has no `SCHED_FIFO` for an audio thread to hold. Core Audio's I/O
/// thread runs under the Mach time-constraint policy instead, and asking for
/// that policy's current parameters is how a thread learns whether it has it:
/// when it does not, the kernel sets `get_default` and hands back the
/// defaults.
#[cfg(target_os = "macos")]
pub fn thread_realtime_status() -> RealtimeStatus {
    // SAFETY: `pthread_mach_thread_np` borrows the calling thread's port
    // without adding a right, so nothing needs deallocating.
    mach_policy_status(unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) })
}

/// The calling thread's Mach port, for [`LoadMeters::set_callback_thread`].
/// A port name is valid from any thread of the task that holds it.
#[cfg(target_os = "macos")]
fn current_thread_id() -> u64 {
    // SAFETY: as in `thread_realtime_status`.
    u64::from(unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) })
}

#[cfg(target_os = "macos")]
fn realtime_status_of(thread: u64) -> Option<RealtimeStatus> {
    Some(mach_policy_status(libc::mach_port_t::try_from(thread).ok()?))
}

/// The time-constraint policy question, asked of the thread `port` names.
#[cfg(target_os = "macos")]
fn mach_policy_status(port: libc::mach_port_t) -> RealtimeStatus {
    let mut policy = libc::thread_time_constraint_policy {
        period: 0,
        computation: 0,
        constraint: 0,
        preemptible: 0,
    };
    let mut count = libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT;
    let mut get_default: libc::boolean_t = 0;
    // SAFETY: `policy` is the struct the flavor names and `count` says how
    // many integers it holds; the kernel writes no more than that. A port
    // that no longer names a thread fails, read as `Unsupported`.
    let status = unsafe {
        libc::thread_policy_get(
            port,
            libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
            (&mut policy as *mut libc::thread_time_constraint_policy).cast(),
            &mut count,
            &mut get_default,
        )
    };
    match (status, get_default) {
        (libc::KERN_SUCCESS, 0) => RealtimeStatus::Realtime,
        (libc::KERN_SUCCESS, _) => RealtimeStatus::TimeShared,
        _ => RealtimeStatus::Unsupported,
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn thread_realtime_status() -> RealtimeStatus {
    RealtimeStatus::Unsupported
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn current_thread_id() -> u64 {
    0
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn realtime_status_of(_thread: u64) -> Option<RealtimeStatus> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: u64 = 2_666_666;

    /// Not a claim about the driver's thread, which only a running stream
    /// has, but a check that the question is asked correctly: a test thread
    /// is ordinary and time-shared, where a malformed call would come back
    /// `Unsupported`.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn an_ordinary_thread_is_reported_time_shared() {
        assert_eq!(thread_realtime_status(), RealtimeStatus::TimeShared);
    }

    /// **The status is the callback thread's as it is now, asked from the
    /// thread reading it** (MOO-215). A thread standing in for the callback
    /// records itself on its first block and waits. The reading, made from
    /// this thread, is that thread's. On Linux, where the kernel allows it,
    /// the stand-in is then promoted to `SCHED_FIFO` *after* its first block,
    /// which is what PipeWire's JACK layer does, and the next reading says
    /// realtime. Pointed at an id that is no thread, the next reading says
    /// the kernel has no answer instead of repeating the stored one.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_status_is_the_callback_threads_now_not_at_its_first_block() {
        use std::sync::mpsc;
        let meters = LoadMeters::new();
        assert_eq!(meters.realtime(), RealtimeStatus::Unknown, "no block yet");
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let callback = std::thread::spawn({
            let meters = Arc::clone(&meters);
            move || {
                meters.set_callback_thread();
                ready_tx.send(()).expect("the test is waiting");
                done_rx.recv().expect("the test says when");
            }
        });
        ready_rx.recv().expect("the stand-in recorded itself");
        assert_eq!(meters.take().realtime, RealtimeStatus::TimeShared);

        #[cfg(target_os = "linux")]
        {
            let tid = libc::pid_t::try_from(meters.thread.load(Ordering::Relaxed))
                .expect("a thread id fits a pid_t");
            let param = libc::sched_param { sched_priority: 1 };
            // SAFETY: sets another thread's policy by id, reading `param`.
            let promoted = unsafe { libc::sched_setscheduler(tid, libc::SCHED_FIFO, &param) } == 0;
            if promoted {
                assert_eq!(
                    meters.take().realtime,
                    RealtimeStatus::Realtime,
                    "promoted after its first block, and the reading followed"
                );
            } else {
                eprintln!(
                    "sched_setscheduler(SCHED_FIFO) was refused here (RLIMIT_RTPRIO), so the \
                     promotion half of this test did not run; the stand-in and the gone \
                     thread still did"
                );
            }
        }

        done_tx.send(()).expect("the stand-in is waiting");
        callback.join().expect("the stand-in did not panic");
        // Asked each time, not remembered: point the record at an id no
        // thread can have (above Linux's `pid_max` ceiling of 2^22), and the
        // reading is the kernel's "no such thread" rather than the
        // `TimeShared` stored a moment ago. Not the joined stand-in's own
        // id: a parallel test run reuses thread ids at once, and the first
        // draft of this read another test's thread.
        #[cfg(target_os = "linux")]
        {
            meters.thread.store(u64::from(i32::MAX.unsigned_abs()), Ordering::Relaxed);
            assert_eq!(
                meters.take().realtime,
                RealtimeStatus::Unsupported,
                "an id that is no thread has no policy to read"
            );
        }
    }

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

    fn spot(tick: u64) -> HotSpot {
        HotSpot {
            tick,
            frames: 128,
            work_nanos: BUDGET,
            budget_nanos: BUDGET,
            sites: [Some((Site::Channel(3), 900)), Some((Site::Bus(255), 400)), None],
        }
    }

    /// **The GUI reads the latest record once, with how many there were**
    /// (MOO-236), and a window with none reads none.
    #[test]
    fn a_hot_spot_is_the_latest_record_and_is_read_once() {
        let meters = LoadMeters::new();
        assert_eq!(meters.take().hot_spot, None);
        meters.publish_hot_spot(&spot(10));
        meters.publish_hot_spot(&spot(20));
        let snapshot = meters.take();
        assert_eq!(snapshot.hot_spot, Some(spot(20)));
        assert_eq!(snapshot.hot_spots, 2);
        let next = meters.take();
        assert_eq!((next.hot_spot, next.hot_spots), (None, 0));
    }

    /// A reader racing the writer never sees half of one record and half of
    /// another: every record read is one that was written whole.
    #[test]
    fn a_hot_spot_is_never_torn() {
        let meters = LoadMeters::new();
        let writer = {
            let meters = meters.clone();
            std::thread::spawn(move || {
                for tick in 0..200_000u64 {
                    let mut record = spot(tick);
                    record.frames = tick as u32;
                    record.work_nanos = tick * 3;
                    meters.publish_hot_spot(&record);
                }
            })
        };
        let mut read = 0;
        while !writer.is_finished() {
            if let Some(record) = meters.take().hot_spot {
                assert_eq!(u64::from(record.frames), record.tick, "{record:?}");
                assert_eq!(record.work_nanos, record.tick * 3, "{record:?}");
                read += 1;
            }
        }
        writer.join().unwrap();
        assert!(read > 0, "the reader never caught a record");
    }
}
