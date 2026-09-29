//! Where a callback's time went, channel by channel and bus by bus
//! (MOO-236).
//!
//! **Lap timing.** The block loop reads the clock once as each site's turn
//! begins, so a site's time runs until the next one starts. A strip that
//! sleeps or is skipped costs one clock read; there is no second read at the
//! end of each strip for the loop's several `continue`s to miss. The gap
//! before the bus walk and after it is closed with a lap of its own, so it
//! is charged to nobody rather than to the last channel.
//!
//! `Instant::now` is a vDSO read of the monotonic clock on Linux and
//! `mach_absolute_time` on macOS: no system call, no lock, no allocation.
//! Off unless the executor turns it on: an export has no budget to be over.

use std::time::Instant;

use mooloop_core::{MAX_BUSES, MAX_CHANNELS};

use crate::load::{Site, HOT_SPOT_SITES};

const SITES: usize = MAX_CHANNELS + MAX_BUSES;

/// Each site's time in the current block, in nanoseconds: the channels,
/// then the buses.
pub(crate) struct SiteTimes {
    enabled: bool,
    nanos: [u32; SITES],
    /// The site whose lap is running, and when it began.
    running: Option<(usize, Instant)>,
    /// How many channel entries the last block may have written, so the
    /// next clears only those rather than all 256.
    channels_used: usize,
}

impl SiteTimes {
    pub(crate) fn new() -> Self {
        Self {
            enabled: false,
            nanos: [0; SITES],
            running: None,
            channels_used: 0,
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.running = None;
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Start a block with every site at zero. `channels` bounds the
    /// channel indices this block can visit.
    #[inline]
    pub(crate) fn begin(&mut self, channels: usize) {
        if !self.enabled {
            return;
        }
        let used = self.channels_used.max(channels).min(MAX_CHANNELS);
        self.nanos[..used].fill(0);
        self.nanos[MAX_CHANNELS..].fill(0);
        self.channels_used = channels.min(MAX_CHANNELS);
        self.running = None;
    }

    /// Close the running lap, if any, and start one for `site`; `None`
    /// charges what follows to nobody.
    #[inline]
    pub(crate) fn lap(&mut self, site: Option<Site>) {
        if !self.enabled {
            return;
        }
        self.lap_at(site, Instant::now());
    }

    /// [`Self::lap`] at a given instant, so a test can hand it a clock no
    /// scheduler can stretch (MOO-293).
    #[inline]
    fn lap_at(&mut self, site: Option<Site>, now: Instant) {
        if let Some((slot, since)) = self.running.take() {
            let spent = now.saturating_duration_since(since).as_nanos();
            let spent = u32::try_from(spent).unwrap_or(u32::MAX);
            self.nanos[slot] = self.nanos[slot].saturating_add(spent);
        }
        self.running = site.map(|site| (slot_of(site), now));
    }

    /// The dearest sites of the last block, dearest first. One pass over
    /// the sites, keeping three: called only on a block that ran long.
    pub(crate) fn costliest(&self) -> [Option<(Site, u32)>; HOT_SPOT_SITES] {
        let mut top: [Option<(Site, u32)>; HOT_SPOT_SITES] = [None; HOT_SPOT_SITES];
        if !self.enabled {
            return top;
        }
        let channels = (0..self.channels_used).map(|index| (index, Site::Channel(index as u8)));
        let buses = (0..MAX_BUSES).map(|index| (MAX_CHANNELS + index, Site::Bus(index as u8)));
        for (slot, site) in channels.chain(buses) {
            let nanos = self.nanos[slot];
            if nanos == 0 {
                continue;
            }
            // Insert in order; the last entry falls off.
            let mut candidate = Some((site, nanos));
            for entry in &mut top {
                match (candidate, *entry) {
                    (Some((_, new)), Some((_, held))) if new <= held => {}
                    (Some(_), _) => std::mem::swap(entry, &mut candidate),
                    (None, _) => break,
                }
            }
        }
        top
    }
}

fn slot_of(site: Site) -> usize {
    match site {
        Site::Channel(index) => usize::from(index),
        Site::Bus(index) => MAX_CHANNELS + usize::from(index).min(MAX_BUSES - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spin(nanos: u64) {
        let start = Instant::now();
        while (start.elapsed().as_nanos() as u64) < nanos {
            std::hint::spin_loop();
        }
    }

    /// The costliest three, dearest first, a site that was visited twice
    /// charged for both laps, and the gap between walks charged to nobody.
    ///
    /// On instants the test writes down rather than the wall clock
    /// (MOO-293): a spin on a busy box can be preempted for longer than the
    /// margins between the sites, which reorders them, and once pushed a
    /// 2 ms spin past a 5 ms bound. Here every lap is exactly as long as
    /// written, so the order and every sum are asserted exactly.
    #[test]
    fn the_dearest_sites_come_first_and_a_gap_is_nobodys() {
        let start = Instant::now();
        let at = |micros: u64| start + std::time::Duration::from_micros(micros);
        let mut times = SiteTimes::new();
        times.set_enabled(true);
        times.begin(4);
        times.lap_at(Some(Site::Channel(0)), at(0));
        times.lap_at(Some(Site::Channel(1)), at(200));
        times.lap_at(Some(Site::Channel(2)), at(2_200));
        times.lap_at(None, at(2_200));
        times.lap_at(Some(Site::Bus(3)), at(7_200));
        times.lap_at(Some(Site::Channel(0)), at(8_200));
        times.lap_at(None, at(8_500));
        assert_eq!(
            times.costliest(),
            [
                Some((Site::Channel(1), 2_000_000)),
                Some((Site::Bus(3), 1_000_000)),
                // 200 us and 300 us, both laps. The 5 ms gap before the bus
                // is nobody's; charged to channel 2, it would come first.
                Some((Site::Channel(0), 500_000)),
            ]
        );
    }

    /// On the real clock a lap is charged at least the time spent in it.
    /// Only a lower bound: preemption can only lengthen a lap, so this is
    /// the one claim about wall time a busy box cannot break.
    #[test]
    fn a_lap_on_the_real_clock_is_charged_at_least_its_time() {
        let mut times = SiteTimes::new();
        times.set_enabled(true);
        times.begin(1);
        times.lap(Some(Site::Channel(0)));
        spin(2_000_000);
        times.lap(None);
        let (site, nanos) = times.costliest()[0].expect("one site ran");
        assert_eq!(site, Site::Channel(0));
        assert!(nanos >= 2_000_000, "{nanos}");
    }

    /// A new block starts from zero, and timing that is off records nothing.
    #[test]
    fn a_block_starts_empty_and_off_is_off() {
        let mut times = SiteTimes::new();
        times.set_enabled(true);
        times.begin(2);
        times.lap(Some(Site::Channel(1)));
        spin(100_000);
        times.lap(None);
        assert!(times.costliest()[0].is_some());
        times.begin(2);
        assert_eq!(times.costliest(), [None; HOT_SPOT_SITES]);

        let mut off = SiteTimes::new();
        off.begin(2);
        off.lap(Some(Site::Channel(1)));
        spin(100_000);
        off.lap(None);
        assert_eq!(off.costliest(), [None; HOT_SPOT_SITES]);
    }

    /// **A block names the channel that carried its work** (MOO-236): four
    /// drum channels, one of them through six reverbs, rendered with timing
    /// on. The reverb channel is the dearest site in nearly every block.
    #[test]
    fn the_channel_carrying_the_work_is_named_first() {
        use crate::render::RenderState;
        use mooloop_core::{EffectKind, EffectSlotState, NoteEvent, Project, ProjectChannel};

        let mut project = Project::default();
        project.channels.clear();
        for index in 0..4 {
            let mut channel = ProjectChannel::drum_synth(index, 1);
            channel.notes[0].push(NoteEvent::new(1, 0, 96, 36, 110));
            project.channels.push(channel);
        }
        for _ in 0..6 {
            project.channels[2]
                .setup
                .push_effect(EffectSlotState::of_kind(EffectKind::Reverb))
                .expect("room in the chain");
        }
        project.assign_channel_ids();
        let mut render = Box::new(RenderState::from_project(48_000, &project, &[]));
        render.set_site_timing(true);
        render.play();
        let blocks = 60;
        let mut named = 0;
        for _ in 0..blocks {
            render.process_block(128);
            if render.costliest_sites()[0].map(|(site, _)| site) == Some(Site::Channel(2)) {
                named += 1;
            }
        }
        assert!(named * 10 >= blocks * 9, "the reverb channel was dearest in {named} of {blocks} blocks");

        render.set_site_timing(false);
        render.process_block(128);
        assert_eq!(render.costliest_sites(), [None; HOT_SPOT_SITES]);
    }
}
