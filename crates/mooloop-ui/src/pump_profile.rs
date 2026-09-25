//! What the UI thread spends its time on: the pump, section by section, and
//! Slint's own frames. Off unless `MOOLOOP_PROFILE_UI` is set, and then a
//! measurement tool, not a feature.
//!
//! ```sh
//! MOOLOOP_PROFILE_UI=1 mooloop song.mooloop            # a report every 5 s
//! MOOLOOP_PROFILE_UI=scenario mooloop song.mooloop     # a fixed run, then quit
//! ```
//!
//! Each report says, for its window: how many pump ticks ran and what they
//! cost (mean, p99 and worst, per section), how many frames Slint rendered
//! and what each cost on this thread (`BeforeRendering` to `AfterRendering`,
//! which is where bindings, layout and drawing run; the buffer swap is not
//! in it), and the CPU every thread of the process used, from
//! `/proc/self/task/*/schedstat` -- so the UI thread's share can be read
//! beside the audio thread's in one line.
//!
//! The `scenario` run is how a song is measured with nobody at the window:
//! stopped on the saved layout, playing on it, playing on each of the mixer,
//! the device rack and the playlist, stopped again, then quit. The song is
//! only read. Written for the 2026-09-25 UI-thread performance survey.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The pump's sections, in the order a tick runs them.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Section {
    /// Signals, reconnect, browser picks and inspections, export progress,
    /// document results, autosave, sample resets and loads, takes.
    Inbox,
    /// The held-message and command-queue drain into the engine ring.
    Forward,
    /// `service_plugins`, `record_finished_plugin_edits`,
    /// `refresh_plugin_faces`.
    Plugins,
    /// `sync_compensation`.
    SyncCompensation,
    /// `sync_sampler_stretch`.
    SyncStretch,
    /// `sync_audio_graph`.
    SyncAudioGraph,
    /// `sync_console_sums`, `sync_solo`, `sync_channel_solo`.
    SyncConsoleSolo,
    /// `sync_track_graph`.
    SyncTrackGraph,
    /// `claimed_notes` and the title refresh.
    Claimed,
    /// `handle.drain()`: positions, xruns, control input, recorded notes.
    Events,
    /// Replace-take positions, control-surface drain, `settle_edit_streams`.
    Control,
    /// The once-a-second port scan and load report.
    Seconds,
    /// Strip reduction, spectrum subscriptions, master needle, input meter,
    /// every bus meter and the mixer strip rows.
    Meters,
    /// The rack's device meters, dynamics, spectra and buffer rows.
    DeviceRows,
    /// The sampler playhead and the live modulation offsets.
    Playhead,
}

const SECTIONS: [(&str, Section); 15] = [
    ("inbox", Section::Inbox),
    ("forward", Section::Forward),
    ("plugins", Section::Plugins),
    ("sync_compensation", Section::SyncCompensation),
    ("sync_stretch", Section::SyncStretch),
    ("sync_audio_graph", Section::SyncAudioGraph),
    ("sync_console+solo", Section::SyncConsoleSolo),
    ("sync_track_graph", Section::SyncTrackGraph),
    ("claimed_notes", Section::Claimed),
    ("events", Section::Events),
    ("control", Section::Control),
    ("seconds", Section::Seconds),
    ("meters", Section::Meters),
    ("device_rows", Section::DeviceRows),
    ("playhead+mod", Section::Playhead),
];

/// Nanoseconds, one entry per tick (or frame) in the window.
#[derive(Default)]
struct Samples(Vec<u32>);

impl Samples {
    fn push(&mut self, value: Duration) {
        self.0.push(value.as_nanos().min(u128::from(u32::MAX)) as u32);
    }

    /// (mean, p99, max) in microseconds.
    fn summary(&mut self) -> (f64, f64, f64) {
        if self.0.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        self.0.sort_unstable();
        let sum: u64 = self.0.iter().map(|&n| u64::from(n)).sum();
        let mean = sum as f64 / self.0.len() as f64 / 1_000.0;
        let p99 = self.0[(self.0.len() * 99 / 100).min(self.0.len() - 1)] as f64 / 1_000.0;
        let max = *self.0.last().unwrap() as f64 / 1_000.0;
        (mean, p99, max)
    }

    fn total_ms(&self) -> f64 {
        self.0.iter().map(|&n| u64::from(n)).sum::<u64>() as f64 / 1e6
    }
}

/// Every thread of this process: (tid, name, nanoseconds on a CPU).
fn thread_times() -> Vec<(u32, String, u64)> {
    let mut threads = Vec::new();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return threads;
    };
    for task in tasks.flatten() {
        let Some(tid) = task.file_name().to_str().and_then(|s| s.parse().ok()) else {
            continue;
        };
        let path = task.path();
        let name = std::fs::read_to_string(path.join("comm"))
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        let on_cpu = std::fs::read_to_string(path.join("schedstat"))
            .ok()
            .and_then(|s| s.split_whitespace().next()?.parse().ok())
            .unwrap_or(0);
        threads.push((tid, name, on_cpu));
    }
    threads
}

pub(crate) struct PumpProfile {
    enabled: bool,
    phase: String,
    window_start: Instant,
    tick_start: Instant,
    lap: Instant,
    /// This tick's time per section, folded into `sections` at `end`.
    current: [Duration; SECTIONS.len()],
    sections: Vec<Samples>,
    ticks: Samples,
    frames: Samples,
    frame_started: Option<Instant>,
    /// Gaps between one tick's start and the next, to see the timer's real
    /// rate under load.
    periods: Samples,
    last_tick_start: Option<Instant>,
    threads_at_start: Vec<(u32, String, u64)>,
    ui_tid: u32,
    /// Flush on a clock rather than on a scenario phase change.
    periodic: Option<Duration>,
}

pub(crate) type Shared = Rc<RefCell<PumpProfile>>;

/// `MOOLOOP_PROFILE_UI`'s value, if it is set to anything but empty.
pub(crate) fn requested() -> Option<String> {
    std::env::var("MOOLOOP_PROFILE_UI").ok().filter(|v| !v.is_empty())
}

impl PumpProfile {
    pub(crate) fn new(mode: Option<&str>) -> Shared {
        let now = Instant::now();
        let enabled = mode.is_some();
        let periodic = match mode {
            Some("scenario") | None => None,
            Some(_) => Some(Duration::from_secs(5)),
        };
        let ui_tid = std::fs::read_link("/proc/thread-self")
            .ok()
            .and_then(|p| p.file_name()?.to_str()?.parse().ok())
            .unwrap_or(0);
        Rc::new(RefCell::new(PumpProfile {
            enabled,
            phase: "start".into(),
            window_start: now,
            tick_start: now,
            lap: now,
            current: [Duration::ZERO; SECTIONS.len()],
            sections: (0..SECTIONS.len()).map(|_| Samples::default()).collect(),
            ticks: Samples::default(),
            frames: Samples::default(),
            frame_started: None,
            periods: Samples::default(),
            last_tick_start: None,
            threads_at_start: if enabled { thread_times() } else { Vec::new() },
            ui_tid,
            periodic,
        }))
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn begin(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_tick_start {
            self.periods.push(now - last);
        }
        self.last_tick_start = Some(now);
        self.tick_start = now;
        self.lap = now;
        self.current = [Duration::ZERO; SECTIONS.len()];
    }

    /// Charge the time since the last lap to `section`.
    pub(crate) fn lap(&mut self, section: Section) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        self.current[section as usize] += now - self.lap;
        self.lap = now;
    }

    pub(crate) fn end(&mut self) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        self.ticks.push(now - self.tick_start);
        for (index, spent) in self.current.iter().enumerate() {
            self.sections[index].push(*spent);
        }
        if let Some(every) = self.periodic {
            if now - self.window_start >= every {
                self.flush();
            }
        }
    }

    pub(crate) fn frame_begin(&mut self) {
        if self.enabled {
            self.frame_started = Some(Instant::now());
        }
    }

    pub(crate) fn frame_end(&mut self) {
        if let Some(started) = self.frame_started.take() {
            self.frames.push(started.elapsed());
        }
    }

    /// Report the window so far under the current phase, then start a new
    /// window under `phase`.
    pub(crate) fn enter(&mut self, phase: &str) {
        if !self.enabled {
            return;
        }
        self.flush();
        self.phase = phase.to_owned();
    }

    pub(crate) fn flush(&mut self) {
        let now = Instant::now();
        let seconds = (now - self.window_start).as_secs_f64().max(1e-9);
        let phase = self.phase.clone();
        let ticks = self.ticks.0.len();
        let pump_ms = self.ticks.total_ms();
        let (tick_mean, tick_p99, tick_max) = self.ticks.summary();
        let (period_mean, period_p99, period_max) = self.periods.summary();
        let frames = self.frames.0.len();
        let render_ms = self.frames.total_ms();
        let (frame_mean, frame_p99, frame_max) = self.frames.summary();
        eprintln!(
            "ui-profile [{phase}] {seconds:.1}s | pump {ticks} ticks ({:.1}/s, period mean {:.2} ms p99 {:.2} max {:.2}) \
             mean {tick_mean:.1} us p99 {tick_p99:.1} max {tick_max:.1}, {:.2}% of the thread's wall time \
             | frames {frames} ({:.1}/s) render mean {frame_mean:.0} us p99 {frame_p99:.0} max {frame_max:.0}, {:.2}% of wall",
            ticks as f64 / seconds,
            period_mean / 1000.0,
            period_p99 / 1000.0,
            period_max / 1000.0,
            pump_ms / 10.0 / seconds,
            frames as f64 / seconds,
            render_ms / 10.0 / seconds,
        );
        for (index, (name, _)) in SECTIONS.iter().enumerate() {
            let samples = &mut self.sections[index];
            let total = samples.total_ms();
            let (mean, p99, max) = samples.summary();
            eprintln!(
                "ui-profile [{phase}]   {name:<18} mean {mean:>8.2} us  p99 {p99:>8.2}  max {max:>9.2}  ({:>4.1}% of pump)",
                if pump_ms > 0.0 { total * 100.0 / pump_ms } else { 0.0 },
            );
            samples.0.clear();
        }
        // CPU per thread over the window, heaviest first.
        let threads = thread_times();
        let mut used: Vec<(f64, String, bool)> = threads
            .iter()
            .filter_map(|(tid, name, on_cpu)| {
                let before = self
                    .threads_at_start
                    .iter()
                    .find(|(t, _, _)| t == tid)
                    .map_or(0, |(_, _, ns)| *ns);
                let percent = on_cpu.saturating_sub(before) as f64 / 1e7 / seconds;
                (percent >= 0.05).then(|| (percent, name.clone(), *tid == self.ui_tid))
            })
            .collect();
        used.sort_by(|a, b| b.0.total_cmp(&a.0));
        let total: f64 = used.iter().map(|(p, _, _)| p).sum();
        let listed: Vec<String> = used
            .iter()
            .map(|(p, name, ui)| format!("{name}{} {p:.1}%", if *ui { " (ui)" } else { "" }))
            .collect();
        eprintln!(
            "ui-profile [{phase}]   cpu {total:.1}% of one core: {}",
            listed.join(", ")
        );
        self.threads_at_start = threads;
        self.ticks.0.clear();
        self.frames.0.clear();
        self.periods.0.clear();
        self.window_start = now;
    }
}
