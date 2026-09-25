//! The driver with no audio device: the engine runs on a thread of its own,
//! at the pace a device would set, and its output goes nowhere.
//!
//! It exists so that "no JACK" is a state the application can be in rather
//! than a reason to exit before any window opens (P2 in
//! `reports/teams-2026-09-22.md`). Edits, the transport, recording from the
//! master bus and every meter work on it; nothing is heard. The interface says
//! why ([`crate::AudioState::NoDevice`]) and offers to reconnect. It is also
//! the one way to run the real engine, with its real rings and executor, in a
//! test with no audio server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mooloop_core::MidiPortId;

use crate::executor::Executor;

/// The rate the engine is built for when there is no device to ask. The
/// commonest rate a device reports, so a project that reconnects to one is
/// most likely to keep it.
pub(crate) const NULL_SAMPLE_RATE: u32 = 48_000;

/// Frames per block: 10.7 ms at [`NULL_SAMPLE_RATE`], the size of a
/// comfortable device buffer.
pub(crate) const NULL_BLOCK: u32 = 512;

/// The running null driver. Dropping it stops its thread.
pub(crate) struct NullDriver {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Why there is no device, for [`crate::AudioState::NoDevice`].
    reason: String,
}

impl NullDriver {
    /// Run `executor` on a thread of its own, a block every block's length.
    pub(crate) fn start(executor: Executor, reason: String) -> Self {
        let running = Arc::new(AtomicBool::new(true));
        let thread = {
            let running = running.clone();
            std::thread::Builder::new()
                .name("mooloop-no-audio".to_owned())
                .spawn(move || run(executor, &running))
        };
        let thread = match thread {
            Ok(thread) => Some(thread),
            Err(error) => {
                mooloop_core::log_error!(
                    "audio",
                    "could not start the engine without a device ({error}); nothing will render"
                );
                None
            }
        };
        Self {
            running,
            thread,
            reason,
        }
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }

    /// A handle on the thread's life, for a test: the thread's closure owns
    /// a clone of `running` until the thread returns, so once this driver is
    /// dropped the flag is freed exactly when the thread has exited. No new
    /// state, and no hook in the loop.
    #[cfg(test)]
    pub(crate) fn thread_liveness(&self) -> std::sync::Weak<AtomicBool> {
        Arc::downgrade(&self.running)
    }
}

impl Drop for NullDriver {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Render a block, then sleep until the next one is due. A block that comes
/// late -- the machine was asleep, the thread was not scheduled -- is not
/// made up for with a burst: the clock restarts from now.
fn run(mut executor: Executor, running: &AtomicBool) {
    // Before the first block, so the executor's first-block warm-up finds
    // this thread already warm (MOO-177).
    crate::executor::prepare_audio_thread();
    let frames = NULL_BLOCK as usize;
    let (mut out_l, mut out_r) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let period =
        Duration::from_nanos(u64::from(NULL_BLOCK) * 1_000_000_000 / u64::from(NULL_SAMPLE_RATE));
    let mut due = Instant::now();
    while running.load(Ordering::Relaxed) {
        executor.process_contained(
            std::iter::empty::<(MidiPortId, u32, &[u8])>(),
            &[],
            &[],
            &mut out_l,
            &mut out_r,
        );
        due += period;
        match due.checked_duration_since(Instant::now()) {
            Some(wait) => std::thread::sleep(wait),
            None => due = Instant::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{AudioState, CommandSink, EngineHandle};
    use mooloop_core::{EngineCommand, EngineEvent};
    use std::time::{Duration, Instant};

    /// The whole engine on no device: commands go in through the real ring,
    /// the executor renders them on the null driver's thread, and positions
    /// come back through the real event ring. This is what the application
    /// runs on when there is no JACK.
    #[test]
    fn the_engine_renders_blocks_with_no_audio_device() {
        let mut handle = EngineHandle::without_device("no device, for a test");
        assert_eq!(
            handle.audio_state(),
            AudioState::NoDevice("no device, for a test".to_owned())
        );
        assert!(handle.send(EngineCommand::Play));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut furthest = 0;
        while Instant::now() < deadline && furthest == 0 {
            while let Some(event) = handle.poll() {
                if let EngineEvent::Position { tick, playing: true, .. } = event {
                    furthest = furthest.max(tick);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(furthest > 0, "the transport never moved on the null driver");
        assert!(handle.take_load().blocks > 0);
        assert_eq!(handle.sample_rate(), super::NULL_SAMPLE_RATE);
    }

    /// Reconnect replaces the whole engine and the new one runs: with no
    /// JACK where the tests run it lands on the null driver again, with the
    /// reason the driver gave, and the transport still moves on the fresh
    /// rings -- which is what the interface installs the song into (MOO-118).
    /// Where a JACK server is running it lands there instead, and the check
    /// is the same.
    #[test]
    fn a_reconnected_engine_renders() {
        let mut handle = EngineHandle::without_device("before");
        let state = handle.reconnect(crate::AudioConfig::default());
        assert_ne!(state, AudioState::NoDevice("before".to_owned()));
        assert!(handle.send(EngineCommand::Play));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut moved = false;
        while Instant::now() < deadline && !moved {
            while let Some(event) = handle.poll() {
                moved |= matches!(event, EngineEvent::Position { playing: true, .. });
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(moved, "the reconnected engine never reported the transport moving");
    }

    /// The effect host's fault count (MOO-176) works the way the output
    /// guard's does: on an engine with no device, and on the engine a
    /// reconnect builds, with the count carried across rather than started
    /// again.
    #[test]
    fn the_effect_host_counts_faults_before_and_after_a_reconnect() {
        let mut handle = EngineHandle::without_device("effect faults");
        install_nan_through_an_effect(&mut handle);
        let before = effect_faults_past(&mut handle, 0);

        handle.reconnect(crate::AudioConfig::default());
        let carried = handle.effect_faults();
        assert!(carried >= before, "the reconnect reset the effect fault count to {carried}");

        install_nan_through_an_effect(&mut handle);
        effect_faults_past(&mut handle, carried);
    }

    fn effect_faults_past(handle: &mut EngineHandle, floor: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            while handle.poll().is_some() {}
            let faults = handle.effect_faults();
            if faults > floor {
                return faults;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the effect host counted nothing past {floor}");
    }

    /// The NaN sampler below, through a filter: the first slot's input is
    /// where the effect host finds it.
    fn install_nan_through_an_effect(handle: &mut EngineHandle) {
        use mooloop_core::{EffectKind, EffectSlotState, NoteEvent, Project, ProjectChannel};
        use mooloop_dsp::{ChannelAudioSnapshot, SampleData};
        use std::sync::Arc;
        let mut channel = ProjectChannel::sampler(0, 1);
        channel.notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
        channel
            .setup
            .push_effect(EffectSlotState::of_kind(EffectKind::Filter))
            .expect("room");
        let project = Project {
            channels: vec![channel],
            ..Project::default()
        };
        let nan = Arc::new(SampleData {
            frames: vec![[f32::NAN, f32::NAN]; 24_000],
            sample_rate: super::NULL_SAMPLE_RATE,
            root_note: 60,
        });
        assert!(handle.install_project(
            Arc::new(project),
            vec![ChannelAudioSnapshot::sample(nan)],
            crate::InputState::default(),
            false,
        ));
        assert!(handle.send(EngineCommand::Play));
    }

    /// A sampler at unity playing a file of NaN from the downbeat: the
    /// device the output guard (MOO-93) exists for.
    fn install_a_device_that_emits_nan(handle: &mut EngineHandle) {
        use mooloop_core::{NoteEvent, Project, ProjectChannel};
        use mooloop_dsp::{ChannelAudioSnapshot, SampleData};
        use std::sync::Arc;
        let mut channel = ProjectChannel::sampler(0, 1);
        channel
            .setup
            .sampler_state_mut()
            .expect("a sampler channel")
            .params
            .output_gain = 1.0;
        channel.notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
        let project = Project {
            channels: vec![channel],
            ..Project::default()
        };
        let nan = Arc::new(SampleData {
            frames: vec![[f32::NAN, f32::NAN]; 24_000],
            sample_rate: super::NULL_SAMPLE_RATE,
            root_note: 60,
        });
        assert!(handle.install_project(
            Arc::new(project),
            vec![ChannelAudioSnapshot::sample(nan)],
            crate::InputState::default(),
            false,
        ));
        assert!(handle.send(EngineCommand::Play));
    }

    /// Wait for the guard's fault count to pass `floor`, and return it.
    fn faults_past(handle: &mut EngineHandle, floor: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            while handle.poll().is_some() {}
            let faults = handle.output_faults();
            if faults > floor {
                return faults;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the output guard counted nothing past {floor}");
    }

    /// The master's output guard runs on an engine with no device, and on
    /// the engine a reconnect builds: the rebuilt master tail still scrubs
    /// and counts, and the count the interface reads carries across the
    /// reconnect rather than starting again (MOO-118 over MOO-93).
    #[test]
    fn the_output_guard_runs_before_and_after_a_reconnect() {
        let mut handle = EngineHandle::without_device("guard");
        install_a_device_that_emits_nan(&mut handle);
        let before = faults_past(&mut handle, 0);

        handle.reconnect(crate::AudioConfig::default());
        let carried = handle.output_faults();
        assert!(carried >= before, "the reconnect reset the fault count to {carried}");

        install_a_device_that_emits_nan(&mut handle);
        faults_past(&mut handle, carried);
    }

    /// Dropping the handle stops the null driver's thread rather than leaving
    /// it rendering for nobody.
    ///
    /// It asserts that the thread *exits*, not how fast the machine is: a
    /// 1 s bound on the drop failed on a starved macOS runner with the thread
    /// stopping correctly (MOO-231). The thread owns a clone of the driver's
    /// `running` flag until it returns, so the flag outliving the driver
    /// means the thread is still alive. The deadline is one only a hung
    /// thread misses.
    ///
    /// The drop runs on a thread of its own, so a drop that never returns (a
    /// thread never told to stop, joined anyway) fails at the deadline rather
    /// than hanging the suite. What drops there is the driver the handle
    /// owns, which is the whole of what dropping the handle does to the
    /// thread: `EngineHandle` has no `Drop` of its own, and the driver is
    /// what can be sent to another thread on every platform.
    #[test]
    fn dropping_the_handle_stops_the_thread() {
        let mut handle = EngineHandle::without_device("test");
        let crate::Driver::Null(driver) = std::mem::replace(&mut handle.driver, crate::Driver::Closed)
        else {
            panic!("an engine with no device runs on the null driver");
        };
        let thread = driver.thread_liveness();
        assert!(thread.upgrade().is_some(), "the thread is running");
        drop(handle);
        let dropper = std::thread::spawn(move || drop(driver));
        let deadline = Instant::now() + Duration::from_secs(30);
        while thread.upgrade().is_some() {
            assert!(
                Instant::now() < deadline,
                "the null driver's thread was still running 30 s after its driver was dropped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        dropper.join().expect("dropping the driver did not panic");
    }
}
