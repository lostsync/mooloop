//! The host's side of a plugin's event loop: the timers and file
//! descriptors it registers, and the one pass the pump makes over them
//! (`docs/plans/plugin-hosting/11-plugin-gui-windows.md`, policy 1, MOO-300).
//!
//! Most Linux plugin GUIs (JUCE, DPF) do not run an event loop of their
//! own. They register a timer and their X11 connection's fd with the host
//! and expect to be called back from the host's main thread. Without that a
//! GUI can open and then never repaint or take input.
//!
//! **Nothing here waits.** The pump is wait-free (`00-status.md`, "Three
//! things the tree already gives"), so a timer fires when its period has
//! elapsed at the moment the pump looks, at most once a look, and the fds
//! are `poll`ed with a zero timeout. A timer shorter than the pump's 8 ms
//! fires every tick, which CLAP allows ("the host may adjust the period").
//!
//! This is format-neutral bookkeeping; the CLAP adapter owns one per
//! instance and makes the calls into the plugin.

use std::time::{Duration, Instant};

/// A file descriptor, as the OS numbers it.
pub type Fd = i32;

/// Readiness bits, in the order CLAP numbers them (`CLAP_POSIX_FD_READ` and
/// so on), so the adapter converts nothing.
pub const FD_READ: u32 = 1 << 0;
pub const FD_WRITE: u32 = 1 << 1;
pub const FD_ERROR: u32 = 1 << 2;

#[derive(Debug, Clone, Copy)]
struct Timer {
    id: u32,
    period: Duration,
    due: Instant,
}

/// One instance's registered timers and fds.
#[derive(Debug, Default)]
pub struct HostIo {
    timers: Vec<Timer>,
    next_timer: u32,
    fds: Vec<(Fd, u32)>,
}

impl HostIo {
    /// Register a timer with a period of `period_ms`, first due one period
    /// from `now`. Returns its id, unique for this instance.
    pub fn register_timer(&mut self, period_ms: u32, now: Instant) -> u32 {
        let id = self.next_timer;
        self.next_timer = self.next_timer.wrapping_add(1);
        let period = Duration::from_millis(u64::from(period_ms.max(1)));
        self.timers.push(Timer {
            id,
            period,
            due: now + period,
        });
        id
    }

    /// Returns whether `id` was registered.
    pub fn unregister_timer(&mut self, id: u32) -> bool {
        let before = self.timers.len();
        self.timers.retain(|timer| timer.id != id);
        self.timers.len() != before
    }

    pub fn has_timer(&self, id: u32) -> bool {
        self.timers.iter().any(|timer| timer.id == id)
    }

    /// Returns whether `fd` was new. A negative fd is refused.
    pub fn register_fd(&mut self, fd: Fd, flags: u32) -> bool {
        if fd < 0 || self.fds.iter().any(|&(known, _)| known == fd) {
            return false;
        }
        self.fds.push((fd, flags));
        true
    }

    /// Returns whether `fd` was registered.
    pub fn modify_fd(&mut self, fd: Fd, flags: u32) -> bool {
        match self.fds.iter_mut().find(|(known, _)| *known == fd) {
            Some(entry) => {
                entry.1 = flags;
                true
            }
            None => false,
        }
    }

    /// Returns whether `fd` was registered.
    pub fn unregister_fd(&mut self, fd: Fd) -> bool {
        let before = self.fds.len();
        self.fds.retain(|&(known, _)| known != fd);
        self.fds.len() != before
    }

    pub fn has_fd(&self, fd: Fd) -> bool {
        self.fds.iter().any(|&(known, _)| known == fd)
    }

    pub fn timer_count(&self) -> usize {
        self.timers.len()
    }

    pub fn fd_count(&self) -> usize {
        self.fds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.timers.is_empty() && self.fds.is_empty()
    }

    /// Push the id of every timer due at `now` into `due`, and move each one
    /// a period on from `now`. A timer that fell several periods behind
    /// (the pump was held up) fires once, not once per period missed.
    pub fn take_due_timers(&mut self, now: Instant, due: &mut Vec<u32>) {
        for timer in &mut self.timers {
            if now >= timer.due {
                due.push(timer.id);
                timer.due = now + timer.period;
            }
        }
    }

    /// Copy the registered fds and what each waits for into `into`.
    pub fn fds(&self, into: &mut Vec<(Fd, u32)>) {
        into.extend_from_slice(&self.fds);
    }
}

/// `poll` every fd in `fds` for what it asked for, with a zero timeout, and
/// push the ready ones into `ready` with what they are ready for. Never
/// waits. `scratch` is reused between calls so a tick allocates nothing once
/// it has grown.
#[cfg(unix)]
pub fn poll_ready(fds: &[(Fd, u32)], scratch: &mut Vec<sys::PollFd>, ready: &mut Vec<(Fd, u32)>) {
    scratch.clear();
    scratch.extend(fds.iter().map(|&(fd, flags)| sys::PollFd {
        fd,
        events: sys::events(flags),
        revents: 0,
    }));
    if scratch.is_empty() || !sys::poll_now(scratch) {
        return;
    }
    ready.extend(
        scratch
            .iter()
            .filter(|entry| entry.revents != 0)
            .map(|entry| (entry.fd, sys::readiness(entry.revents))),
    );
}

/// Nothing is ever ready where there is no `poll`.
#[cfg(not(unix))]
pub fn poll_ready(_fds: &[(Fd, u32)], _scratch: &mut Vec<sys::PollFd>, _ready: &mut Vec<(Fd, u32)>) {}

#[cfg(unix)]
pub mod sys {
    //! `poll(2)`, declared here because this crate has no `libc`: the C
    //! library is linked into every Unix program anyway, and this is the one
    //! call the host needs from it. The struct and constants are POSIX and
    //! the same on Linux and macOS; `nfds_t` is the one type that differs.

    use std::ffi::{c_int, c_short};

    use super::{FD_ERROR, FD_READ, FD_WRITE};

    const POLLIN: c_short = 0x001;
    const POLLOUT: c_short = 0x004;
    const POLLERR: c_short = 0x008;
    const POLLHUP: c_short = 0x010;
    const POLLNVAL: c_short = 0x020;

    #[cfg(target_os = "linux")]
    type NfdsT = std::ffi::c_ulong;
    #[cfg(not(target_os = "linux"))]
    type NfdsT = std::ffi::c_uint;

    /// `struct pollfd`.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct PollFd {
        pub(super) fd: c_int,
        pub(super) events: c_short,
        pub(super) revents: c_short,
    }

    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: NfdsT, timeout: c_int) -> c_int;
    }

    /// `poll` with a zero timeout. False on an error (`EINTR` included):
    /// nothing is ready this tick, and the next one looks again.
    pub(super) fn poll_now(fds: &mut [PollFd]) -> bool {
        let Ok(count) = NfdsT::try_from(fds.len()) else {
            return false;
        };
        // SAFETY: `fds` is a live, exclusively borrowed slice of `count`
        // `struct pollfd`s, which is all `poll` reads and writes, and a zero
        // timeout returns at once.
        unsafe { poll(fds.as_mut_ptr(), count, 0) > 0 }
    }

    pub(super) fn events(flags: u32) -> c_short {
        let mut events = 0;
        if flags & FD_READ != 0 {
            events |= POLLIN;
        }
        if flags & FD_WRITE != 0 {
            events |= POLLOUT;
        }
        // Errors and hang-ups are always reported, asked for or not.
        events
    }

    pub(super) fn readiness(revents: c_short) -> u32 {
        let mut flags = 0;
        if revents & POLLIN != 0 {
            flags |= FD_READ;
        }
        if revents & POLLOUT != 0 {
            flags |= FD_WRITE;
        }
        if revents & (POLLERR | POLLHUP | POLLNVAL) != 0 {
            flags |= FD_ERROR;
        }
        flags
    }
}

#[cfg(not(unix))]
pub mod sys {
    /// Never built: nothing is polled where there is no `poll`.
    #[derive(Debug, Clone, Copy)]
    pub struct PollFd;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timer_fires_once_its_period_has_passed_and_once_however_late() {
        let start = Instant::now();
        let mut io = HostIo::default();
        let id = io.register_timer(10, start);
        let mut due = Vec::new();
        io.take_due_timers(start + Duration::from_millis(5), &mut due);
        assert!(due.is_empty(), "not yet");
        io.take_due_timers(start + Duration::from_millis(100), &mut due);
        assert_eq!(due, [id], "ten periods late is still one call");
        due.clear();
        io.take_due_timers(start + Duration::from_millis(105), &mut due);
        assert!(due.is_empty(), "the next is a period after the late one");
        assert!(io.unregister_timer(id));
        assert!(!io.unregister_timer(id));
        io.take_due_timers(start + Duration::from_secs(1), &mut due);
        assert!(due.is_empty() && io.is_empty());
    }

    #[test]
    fn timer_ids_are_unique_and_fds_are_registered_once() {
        let now = Instant::now();
        let mut io = HostIo::default();
        let (a, b) = (io.register_timer(16, now), io.register_timer(16, now));
        assert_ne!(a, b);
        assert!(io.register_fd(7, FD_READ));
        assert!(!io.register_fd(7, FD_READ), "twice is refused");
        assert!(!io.register_fd(-1, FD_READ));
        assert!(io.modify_fd(7, FD_READ | FD_WRITE));
        assert!(!io.modify_fd(8, FD_READ));
        assert_eq!((io.timer_count(), io.fd_count()), (2, 1));
        assert!(io.unregister_fd(7) && !io.has_fd(7));
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_is_ready_to_read_only_once_something_is_written() {
        use std::io::{Read, Write};
        use std::os::fd::AsRawFd;

        let (mut reader, mut writer) = std::io::pipe().expect("a pipe");
        let fds = [(reader.as_raw_fd(), FD_READ)];
        let (mut scratch, mut ready) = (Vec::new(), Vec::new());
        let started = Instant::now();
        poll_ready(&fds, &mut scratch, &mut ready);
        assert!(ready.is_empty(), "an empty pipe is not ready");
        assert!(started.elapsed() < Duration::from_millis(100), "and the poll did not wait");
        writer.write_all(b"x").expect("a write");
        poll_ready(&fds, &mut scratch, &mut ready);
        assert_eq!(ready, [(reader.as_raw_fd(), FD_READ)]);
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).expect("the byte");
        ready.clear();
        drop(writer);
        poll_ready(&fds, &mut scratch, &mut ready);
        assert!(
            ready.first().is_some_and(|&(_, flags)| flags & FD_ERROR != 0),
            "a hung-up pipe reports it: {ready:?}"
        );
    }
}
