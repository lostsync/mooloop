//! The in-repo CLAP test double (`docs/plans/plugin-hosting/01-spike.md`).
//!
//! CI cannot install third-party plugins, so every plugin-hosting step's
//! automated tests run against this one library. It exports one CLAP entry
//! whose factory holds seven plugins, [`PLUGIN_IDS`]:
//!
//! | id | what |
//! | --- | --- |
//! | [`GAIN_ID`] | stereo gain effect; `gain` (dB, modulatable), `latency` (stepped), `fail`, `nudge` (moves its own gain) |
//! | [`GAIN_GUI_ID`] | the same, declaring the `gui` extension |
//! | [`GAIN_MONO_ID`] | the gain with one mono input and one mono output (MOO-266) |
//! | [`GAIN_MONO_IN_ID`] | the gain with a mono input and a stereo output: both outputs are the input |
//! | [`GAIN_MONO_OUT_ID`] | the gain with a stereo input and a mono output: the output is the left input |
//! | [`SINE_ID`] | one sine voice per note id, with a release tail |
//! | [`SINE_GUI_ID`] | the same, declaring the `gui` extension |
//!
//! The plugins without a GUI are the ones that matter most: a plugin that has
//! no GUI at all (Airwindows is the named case) is a path the host has to
//! handle from the start, not an afterthought.
//!
//! Parameter ids are deliberately sparse (10, 20, 30, and 4 000 000 000,
//! which no `i32` holds) so that nothing written
//! against this plugin can confuse a parameter's id with its position in the
//! plugin's list (`AGENTS.md`, "Parameter identity across the session
//! boundary").
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
use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};

mod gain;
mod gui;
mod sine;

pub use gain::{
    GAIN_DB_DEFAULT, GAIN_DB_MAX, GAIN_DB_MIN, LATENCY_STEPS, NUDGE_DB, PARAM_FAIL, PARAM_GAIN,
    PARAM_LATENCY, PARAM_NUDGE, STATE_MAGIC,
};
pub use gui::{GUI_DEFAULT_SIZE, GUI_MIN_SIZE};
pub use sine::{RELEASE_SECONDS, SINE_AMPLITUDE};

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

/// Every plugin the factory lists, in its order: a test that checks what a
/// scan found reads this rather than its own copy.
pub const PLUGIN_IDS: [&str; 7] = [
    GAIN_ID,
    GAIN_GUI_ID,
    SINE_ID,
    SINE_GUI_ID,
    GAIN_MONO_ID,
    GAIN_MONO_IN_ID,
    GAIN_MONO_OUT_ID,
];

/// A copy of this library whose file name ends in this aborts the process
/// as its entry initialises.
pub const CRASHES_ON_SCAN: &str = "crashes-on-scan.clap";
/// A copy of this library whose file name ends in this never returns from
/// its entry's initialisation.
pub const HANGS_ON_SCAN: &str = "hangs-on-scan.clap";

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

/// The host services every test plugin uses: logging, and checking that it
/// is called on the thread CLAP says it will be.
///
/// A call on the wrong thread is logged as `HostMisbehaving` rather than
/// panicking, so a host test can collect the log and fail with the message.
struct HostServices<'a> {
    host: HostSharedHandle<'a>,
    log: Option<HostLog>,
    thread_check: Option<HostThreadCheck>,
}

impl<'a> HostServices<'a> {
    fn new(host: HostSharedHandle<'a>) -> Self {
        Self {
            log: host.get_extension(),
            thread_check: host.get_extension(),
            host,
        }
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
