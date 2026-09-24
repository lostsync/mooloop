//! Loads and runs third-party plugins (`docs/plans/plugin-hosting/`).
//!
//! **No plugin format's types leave this crate.** Core, project, session,
//! engine and UI will see only the neutral types step 02 defines; only this
//! crate's `Cargo.toml` names `clack-*`.
//!
//! What is here is step 01's spike: the smallest CLAP host that
//! `clack-host` 0.2 can express, which the spike's tests
//! (`tests/spike.rs`) drive against the in-repo test plugin. It is a host in
//! the CLAP sense -- the callbacks a plugin may make, and the extensions it
//! may ask the host for -- and deliberately nothing more. [`instance`] is
//! step 04's neutral half, [`scan`] is step 05's scanner, which loads each
//! library only in a child process, and [`clap`] is step 06's adapter: the
//! host a real CLAP effect runs under in a chain.
//!
//! The one `unsafe` block is [`load_entry`]: loading a shared library runs
//! its initialisers, which no Rust type can vouch for. Everything the spike
//! needed after that is safe `clack-host` API (`00-status.md`, "Step 01").

#![deny(unsafe_op_in_unsafe_fn)]

pub mod clap;
pub mod instance;
pub mod scan;

pub use instance::{
    AudioConfig, HostError, HostedInstance, Lifeline, PluginOpener, PluginParamEvent, RequestFlags,
    Requests,
};

use std::ffi::CStr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread::ThreadId;

use clack_extensions::latency::{HostLatency, HostLatencyImpl};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags,
    ParamRescanFlags,
};
use clack_extensions::state::{HostState, HostStateImpl};
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_host::prelude::*;

pub use clack_host;
pub use clack_extensions;

/// The host's name, as a plugin sees it.
pub const HOST_NAME: &str = "mooloop";

/// What mooloop tells a plugin about itself.
///
/// # Panics
///
/// Never: the strings are constants with no interior NUL.
pub fn host_info() -> HostInfo {
    HostInfo::new(
        HOST_NAME,
        "mooloop",
        "https://github.com/lostsync/mooloop",
        env!("CARGO_PKG_VERSION"),
    )
    .expect("the host's own strings have no NUL")
}

/// Load the CLAP library at `path` and initialise its entry.
///
/// # Safety
///
/// Loading a shared library runs whatever initialisation code it carries, and
/// a CLAP entry is trusted to honour the ABI it declares. The caller vouches
/// for the file, which the scanner (step 05) will do by loading it in a child
/// process first.
pub unsafe fn load_entry(path: &Path) -> Result<PluginEntry, clack_host::entry::PluginEntryError> {
    // SAFETY: the caller's contract, above.
    unsafe { PluginEntry::load(path) }
}

/// The [`HostHandlers`] every spike instance runs under.
pub struct SpikeHost;

impl HostHandlers for SpikeHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = HostMainThread;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostLog>()
            .register::<HostThreadCheck>()
            .register::<HostLatency>()
            .register::<HostState>()
            .register::<HostParams>();
    }
}

/// What a plugin may reach from any thread: its requests, its log, and the
/// question of which thread it is on.
pub struct HostShared {
    main_thread: ThreadId,
    restart_requested: AtomicBool,
    process_requested: AtomicBool,
    callback_requested: AtomicBool,
    flush_requested: AtomicBool,
    /// Messages the plugin logged, in order. A spike-only convenience: the
    /// real host must not lock on a plugin's audio thread.
    log: Mutex<Vec<(LogSeverity, String)>>,
    /// Calls the plugin itself reported as made on the wrong thread.
    misbehaviour: AtomicU32,
}

impl HostShared {
    /// A host whose main thread is the calling thread.
    pub fn new() -> Self {
        Self {
            main_thread: std::thread::current().id(),
            restart_requested: AtomicBool::new(false),
            process_requested: AtomicBool::new(false),
            callback_requested: AtomicBool::new(false),
            flush_requested: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
            misbehaviour: AtomicU32::new(0),
        }
    }

    /// Whether the plugin asked for a restart since the last call.
    pub fn take_restart_request(&self) -> bool {
        self.restart_requested.swap(false, Ordering::AcqRel)
    }

    /// Whether the plugin asked for a main-thread callback since the last
    /// call.
    pub fn take_callback_request(&self) -> bool {
        self.callback_requested.swap(false, Ordering::AcqRel)
    }

    /// Everything the plugin has logged.
    pub fn log(&self) -> Vec<(LogSeverity, String)> {
        self.log.lock().map(|log| log.clone()).unwrap_or_default()
    }

    /// How many calls the plugin reported as host misbehaviour.
    pub fn misbehaviour(&self) -> u32 {
        self.misbehaviour.load(Ordering::Acquire)
    }
}

impl Default for HostShared {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> SharedHandler<'a> for HostShared {
    fn request_restart(&self) {
        self.restart_requested.store(true, Ordering::Release);
    }

    fn request_process(&self) {
        self.process_requested.store(true, Ordering::Release);
    }

    fn request_callback(&self) {
        self.callback_requested.store(true, Ordering::Release);
    }
}

impl HostLogImpl for HostShared {
    fn log(&self, severity: LogSeverity, message: &str) {
        if matches!(
            severity,
            LogSeverity::HostMisbehaving | LogSeverity::PluginMisbehaving
        ) {
            self.misbehaviour.fetch_add(1, Ordering::AcqRel);
        }
        if let Ok(mut log) = self.log.lock() {
            log.push((severity, message.to_owned()));
        }
    }
}

impl HostThreadCheckImpl for HostShared {
    fn is_main_thread(&self) -> bool {
        std::thread::current().id() == self.main_thread
    }

    /// Any thread but the main one. The spike processes on a thread of its
    /// own for exactly this reason; the engine's real answer is the driver's
    /// callback thread.
    fn is_audio_thread(&self) -> bool {
        !self.is_main_thread()
    }
}

impl HostParamsImplShared for HostShared {
    fn request_flush(&self) {
        self.flush_requested.store(true, Ordering::Release);
    }
}

/// What a plugin may reach only from the main thread.
#[derive(Default)]
pub struct HostMainThread {
    latency_changed: AtomicBool,
    state_dirty: AtomicBool,
    params_rescanned: AtomicBool,
}

impl HostMainThread {
    /// Whether the plugin said its latency changed since the last call.
    pub fn take_latency_changed(&self) -> bool {
        self.latency_changed.swap(false, Ordering::AcqRel)
    }

    /// Whether the plugin marked its state dirty since the last call.
    pub fn take_state_dirty(&self) -> bool {
        self.state_dirty.swap(false, Ordering::AcqRel)
    }

    /// Whether the plugin asked for its parameters to be rescanned.
    pub fn take_params_rescanned(&self) -> bool {
        self.params_rescanned.swap(false, Ordering::AcqRel)
    }
}

impl<'a> MainThreadHandler<'a> for HostMainThread {}

impl HostLatencyImpl for HostMainThread {
    fn changed(&self) {
        self.latency_changed.store(true, Ordering::Release);
    }
}

impl HostStateImpl for HostMainThread {
    fn mark_dirty(&self) {
        self.state_dirty.store(true, Ordering::Release);
    }
}

impl HostParamsImplMainThread for HostMainThread {
    fn rescan(&self, _flags: ParamRescanFlags) {
        self.params_rescanned.store(true, Ordering::Release);
    }

    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

/// Create an instance of `id` from `entry` under [`SpikeHost`].
pub fn instantiate(
    entry: &PluginEntry,
    id: &CStr,
) -> Result<PluginInstance<SpikeHost>, PluginInstanceError> {
    PluginInstance::<SpikeHost>::new(
        |_| HostShared::new(),
        |_| HostMainThread::default(),
        entry,
        id,
        &host_info(),
    )
}
