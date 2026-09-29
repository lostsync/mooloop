//! The in-repo CLAP test double (`docs/plans/plugin-hosting/01-spike.md`).
//!
//! CI cannot install third-party plugins, so every plugin-hosting step's
//! automated tests run against this one library. It exports one CLAP entry
//! whose factory holds nine plugins, [`PLUGIN_IDS`]:
//!
//! | id | what |
//! | --- | --- |
//! | [`GAIN_ID`] | stereo gain effect; `gain` (dB, modulatable), `latency` (stepped), `fail`, `nudge` (moves its own gain) |
//! | [`GAIN_GUI_ID`] | the same, declaring the `gui` extension, with a timer and an fd behind it and probes a test reads ([`PROBE_IDS`]) |
//! | [`GAIN_MONO_ID`] | the gain with one mono input and one mono output (MOO-266) |
//! | [`GAIN_MONO_IN_ID`] | the gain with a mono input and a stereo output: both outputs are the input |
//! | [`GAIN_MONO_OUT_ID`] | the gain with a stereo input and a mono output: the output is the left input |
//! | [`SINE_ID`] | one sine voice per note id, with a release tail; `level` (dB, modulatable, MOO-314) |
//! | [`SINE_GUI_ID`] | the same, declaring the `gui` extension, with a timer and an fd behind it |
//! | [`SIDECHAIN_ID`] | a unity effect with a sidechain input and two extra outputs, its main ports at 1 and 2, and probes on the sidechain (MOO-308) |
//! | [`GAIN_CHATTY_ID`] | the stereo gain, logging one `Debug` line from every `process` call, as a plugin with debug logging left on does (MOO-324) |
//!
//! The plugins without a GUI are the ones that matter most: a plugin that has
//! no GUI at all (Airwindows is the named case) is a path the host has to
//! handle from the start, not an afterthought.
//!
//! Parameter ids are deliberately sparse (10, 20, 30, 50, and 4 000 000 000,
//! which no `i32` holds) so that nothing written
//! against this plugin can confuse a parameter's id with its position in the
//! plugin's list (`AGENTS.md`, "Parameter identity across the session
//! boundary").
//!
//! **Strict mode** (MOO-311). Every plugin here holds the host to CLAP's
//! threading rules the way a plugin built on `clap-helpers` with
//! `MisbehaviourHandler::Terminate` does (Odin2 is the named case): a host
//! that breaks one has the process aborted, with the reason on stderr. What
//! it checks is `HostServices`'s: `start_processing` and `stop_processing`
//! on an audio thread and never on the main one, `stop_processing` on the
//! thread that last called `process` (a host that stops a processor from a
//! thread it never ran on has lost track of its audio thread), and
//! `note_ports.get` inside `note_ports.count`. It is on unless
//! [`LENIENT_ENV`] is set, and then a violation is only logged as
//! `HostMisbehaving`, as the other thread checks here are.
//!
//! The crate is built as a `cdylib` for the host to load by path, and as an
//! `rlib` only so cargo builds it for the host crate's tests. Nothing here is
//! `unsafe`: `clack-plugin` is a safe API, and the only unsafe code is inside
//! `clack_export_entry!`.

#![deny(unsafe_code)]

use clack_extensions::log::{HostLog, LogSeverity};
use clack_extensions::thread_check::HostThreadCheck;
use clack_plugin::entry::prelude::*;
use clack_plugin::prelude::*;
use std::ffi::{CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};

mod gain;
mod gui;
mod sidechain;
mod sine;

pub use gain::{
    GAIN_DB_DEFAULT, GAIN_DB_MAX, GAIN_DB_MIN, LATENCY_STEPS, NUDGE_DB, PARAM_FAIL, PARAM_GAIN,
    PARAM_LATENCY, PARAM_NUDGE, STATE_MAGIC,
};
pub use gui::{
    GUI_DEFAULT_SIZE, GUI_MIN_SIZE, GUI_TIMER_MS, PROBE_FDS_LIVE, PROBE_FD_READS, PROBE_GUIS_LEAKED,
    PROBE_GUIS_LIVE, PROBE_IDS, PROBE_TIMERS_LIVE, PROBE_TIMER_TICKS,
};
pub use sidechain::{AUX_LEVEL, PROBE_SIDECHAIN_BLOCKS, PROBE_SIDECHAIN_LOUD};
pub use sine::{
    LEVEL_DB_DEFAULT, LEVEL_DB_MAX, LEVEL_DB_MIN, PARAM_LEVEL, RELEASE_SECONDS, SINE_AMPLITUDE,
};

/// The stereo gain effect, without a GUI.
pub const GAIN_ID: &str = "mooloop.test.gain";
/// The stereo gain effect, declaring the `gui` extension.
pub const GAIN_GUI_ID: &str = "mooloop.test.gain.gui";
/// The sine instrument, without a GUI.
pub const SINE_ID: &str = "mooloop.test.sine";
/// The sine instrument, declaring the `gui` extension.
pub const SINE_GUI_ID: &str = "mooloop.test.sine.gui";
/// The gain, mono in and mono out (MOO-266).
pub const GAIN_MONO_ID: &str = "mooloop.test.gain.mono";
/// The gain, mono in and stereo out (MOO-266).
pub const GAIN_MONO_IN_ID: &str = "mooloop.test.gain.mono-in";
/// The gain, stereo in and mono out (MOO-266).
pub const GAIN_MONO_OUT_ID: &str = "mooloop.test.gain.mono-out";
/// A unity effect with a sidechain and extra outputs (MOO-308).
pub const SIDECHAIN_ID: &str = "mooloop.test.sidechain";
/// The stereo gain, logging from its audio thread (MOO-324).
pub const GAIN_CHATTY_ID: &str = "mooloop.test.gain.chatty";

/// Every plugin the factory lists, in its order: a test that checks what a
/// scan found reads this rather than its own copy.
pub const PLUGIN_IDS: [&str; 9] = [
    GAIN_ID,
    GAIN_GUI_ID,
    SINE_ID,
    SINE_GUI_ID,
    GAIN_MONO_ID,
    GAIN_MONO_IN_ID,
    GAIN_MONO_OUT_ID,
    SIDECHAIN_ID,
    GAIN_CHATTY_ID,
];

/// A copy of this library whose file name ends in this aborts the process
/// as its entry initialises.
pub const CRASHES_ON_SCAN: &str = "crashes-on-scan.clap";
/// A copy of this library whose file name ends in this never returns from
/// its entry's initialisation.
pub const HANGS_ON_SCAN: &str = "hangs-on-scan.clap";

/// Set (to anything) in the environment of the process that loads this
/// library, strict mode only logs what it would otherwise abort on.
pub const LENIENT_ENV: &str = "MOOLOOP_TEST_PLUGIN_LENIENT";

/// The vendor every descriptor reports.
pub const VENDOR: &str = "mooloop";

/// The entry `clap_entry` points at.
pub struct TestEntry {
    factory: PluginFactoryWrapper<TestFactory>,
}

impl Entry for TestEntry {
    fn new(bundle_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        // The scanner's misbehaving plugins (MOO-80): the same library,
        // copied under a name that makes it crash or hang while it loads, so
        // a test can prove that doing either in the scan child costs mooloop
        // nothing. No other name changes anything.
        let path = bundle_path.map(CStr::to_bytes).unwrap_or_default();
        if path.ends_with(CRASHES_ON_SCAN.as_bytes()) {
            std::process::abort();
        }
        if path.ends_with(HANGS_ON_SCAN.as_bytes()) {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        }
        Ok(Self {
            factory: PluginFactoryWrapper::new(TestFactory::new()),
        })
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.factory);
    }
}

struct TestFactory {
    descriptors: [PluginDescriptor; PLUGIN_IDS.len()],
}

impl TestFactory {
    fn new() -> Self {
        use clack_plugin::plugin::features::{
            AUDIO_EFFECT, INSTRUMENT, MONO, STEREO, SYNTHESIZER, UTILITY,
        };

        let version = env!("CARGO_PKG_VERSION");
        let describe = |id: &str, name: &str| {
            PluginDescriptor::new(id, name)
                .with_vendor(VENDOR)
                .with_version(version)
        };
        Self {
            descriptors: [
                describe(GAIN_ID, "Test Gain").with_features([AUDIO_EFFECT, UTILITY, STEREO]),
                describe(GAIN_GUI_ID, "Test Gain (GUI)")
                    .with_features([AUDIO_EFFECT, UTILITY, STEREO]),
                describe(SINE_ID, "Test Sine").with_features([INSTRUMENT, SYNTHESIZER, STEREO]),
                describe(SINE_GUI_ID, "Test Sine (GUI)")
                    .with_features([INSTRUMENT, SYNTHESIZER, STEREO]),
                describe(GAIN_MONO_ID, "Test Gain (mono)").with_features([AUDIO_EFFECT, UTILITY, MONO]),
                describe(GAIN_MONO_IN_ID, "Test Gain (mono in)").with_features([AUDIO_EFFECT, UTILITY]),
                describe(GAIN_MONO_OUT_ID, "Test Gain (mono out)").with_features([AUDIO_EFFECT, UTILITY]),
                describe(SIDECHAIN_ID, "Test Sidechain").with_features([AUDIO_EFFECT, UTILITY, STEREO]),
                describe(GAIN_CHATTY_ID, "Test Gain (chatty)")
                    .with_features([AUDIO_EFFECT, UTILITY, STEREO]),
            ],
        }
    }
}

impl PluginFactoryImpl for TestFactory {
    fn plugin_count(&self) -> u32 {
        self.descriptors.len() as u32
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        self.descriptors.get(index as usize)
    }

    fn create_plugin<'a>(
        &'a self,
        host_info: HostInfo<'a>,
        plugin_id: &CStr,
    ) -> Option<PluginInstance<'a>> {
        let index = self
            .descriptors
            .iter()
            .position(|d| d.id() == Some(plugin_id))?;
        let descriptor = &self.descriptors[index];
        Some(match index {
            0 => PluginInstance::new::<gain::GainPlugin<false>>(
                host_info,
                descriptor,
                gain::GainShared::new,
                gain::GainMain::new,
            ),
            1 => PluginInstance::new::<gain::GainPlugin<true>>(
                host_info,
                descriptor,
                gain::GainShared::new,
                gain::GainMain::new,
            ),
            2 => PluginInstance::new::<sine::SinePlugin<false>>(
                host_info,
                descriptor,
                sine::SineShared::new,
                sine::SineMain::new,
            ),
            3 => PluginInstance::new::<sine::SinePlugin<true>>(
                host_info,
                descriptor,
                sine::SineShared::new,
                sine::SineMain::new,
            ),
            7 => PluginInstance::new::<sidechain::SidechainPlugin>(
                host_info,
                descriptor,
                sidechain::SidechainShared::new,
                sidechain::SidechainMain::new,
            ),
            8 => PluginInstance::new::<gain::GainPlugin<false>>(
                host_info,
                descriptor,
                gain::GainShared::chatty,
                gain::GainMain::new,
            ),
            _ => {
                let (inputs, outputs) = match index {
                    4 => (1, 1),
                    5 => (1, 2),
                    _ => (2, 1),
                };
                PluginInstance::new::<gain::GainPlugin<false>>(
                    host_info,
                    descriptor,
                    move |host| gain::GainShared::with_ports(host, inputs, outputs),
                    gain::GainMain::new,
                )
            }
        })
    }
}

clack_export_entry!(TestEntry);

/// The host services every test plugin uses: logging, checking that it is
/// called on the thread CLAP says it will be, and strict mode.
///
/// A process or activate call on the wrong thread is logged as
/// `HostMisbehaving` rather than panicking, so a host test can collect the
/// log and fail with the message. What strict mode covers aborts instead
/// (the crate's documentation).
struct HostServices<'a> {
    host: HostSharedHandle<'a>,
    log: Option<HostLog>,
    thread_check: Option<HostThreadCheck>,
    /// The thread that created the plugin: the host's main thread.
    main_thread: u64,
    /// The thread that last started or ran the processor, 0 for none since
    /// the last stop.
    audio_thread: AtomicU64,
}

/// This thread's number, unique for the life of the process and never 0:
/// a `ThreadId` cannot live in an atomic.
fn thread_token() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    thread_local! {
        static TOKEN: u64 = NEXT.fetch_add(1, Ordering::Relaxed);
    }
    TOKEN.with(|token| *token)
}

impl<'a> HostServices<'a> {
    fn new(host: HostSharedHandle<'a>) -> Self {
        Self {
            log: host.get_extension(),
            thread_check: host.get_extension(),
            host,
            main_thread: thread_token(),
            audio_thread: AtomicU64::new(0),
        }
    }

    /// Whether this is an audio thread: never the thread that created the
    /// plugin, and never one the host itself says is not.
    fn on_an_audio_thread(&self) -> bool {
        let host_says_no = self
            .thread_check
            .is_some_and(|check| check.is_audio_thread(&self.host) == Some(false));
        thread_token() != self.main_thread && !host_says_no
    }

    /// Strict mode's check on `start_processing`.
    fn strict_start(&self, plugin: &str) {
        if !self.on_an_audio_thread() {
            self.strict_violation(&format!("{plugin}: start_processing called off the audio thread"));
        }
        self.audio_thread.store(thread_token(), Ordering::Relaxed);
    }

    /// Record the thread a `process` call came on, for [`Self::strict_stop`].
    fn strict_process(&self) {
        self.audio_thread.store(thread_token(), Ordering::Relaxed);
    }

    /// Strict mode's check on `stop_processing`: an audio thread, and the one
    /// the processor last ran on.
    fn strict_stop(&self, plugin: &str) {
        let last = self.audio_thread.swap(0, Ordering::Relaxed);
        if !self.on_an_audio_thread() {
            self.strict_violation(&format!("{plugin}: stop_processing called off the audio thread"));
        } else if last != 0 && last != thread_token() {
            self.strict_violation(&format!(
                "{plugin}: stop_processing called on a thread the processor never ran on"
            ));
        }
    }

    /// Strict mode's check on `note_ports.get`.
    fn strict_note_port(&self, plugin: &str, index: u32, count: u32) {
        if index >= count {
            self.strict_violation(&format!(
                "{plugin}: note_ports.get called with an index out of bounds: {index} >= {count}"
            ));
        }
    }

    /// Log `message` as host misbehaviour, print it, and abort the process
    /// unless [`LENIENT_ENV`] is set.
    fn strict_violation(&self, message: &str) {
        if let Ok(text) = CString::new(message) {
            self.log(LogSeverity::HostMisbehaving, &text);
        }
        if std::env::var_os(LENIENT_ENV).is_some() {
            return;
        }
        eprintln!("mooloop-test-plugin strict mode: {message}; aborting, as a strict plugin would");
        std::process::abort();
    }

    fn log(&self, severity: LogSeverity, message: &CStr) {
        if let Some(log) = self.log {
            log.log(&self.host, severity, message);
        }
    }

    /// Log `what` as host misbehaviour unless the host says this is its main
    /// thread. Says nothing when the host has no thread-check extension.
    fn expect_main_thread(&self, what: &CStr) {
        if let Some(check) = self.thread_check {
            if check.is_main_thread(&self.host) == Some(false) {
                self.log(LogSeverity::HostMisbehaving, what);
            }
        }
    }

    /// Log `what` as host misbehaviour unless the host says this is an audio
    /// thread.
    fn expect_audio_thread(&self, what: &CStr) {
        if let Some(check) = self.thread_check {
            if check.is_audio_thread(&self.host) == Some(false) {
                self.log(LogSeverity::HostMisbehaving, what);
            }
        }
    }
}

/// An `f64` shared between the main thread and the audio thread.
struct AtomicF64(AtomicU64);

impl AtomicF64 {
    fn new(value: f64) -> Self {
        Self(AtomicU64::new(value.to_bits()))
    }

    fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn store(&self, value: f64) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}
