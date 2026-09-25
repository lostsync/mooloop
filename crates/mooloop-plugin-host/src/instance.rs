//! The main-thread half of a hosted plugin, as the rest of mooloop sees it
//! (`docs/plans/plugin-hosting/04-the-plugin-rack.md`).
//!
//! A plugin comes in two halves, the way `clack-host` already splits one: an
//! **instance** that lives on the control thread (the Slint event-loop
//! thread, where the pump runs) and owns the library, the parameter list, the
//! state and the GUI, and a **processor** -- an `AudioNode + Send` -- that
//! crosses to the audio thread through the ordinary structural commands.
//! [`HostedInstance`] is the first half with every plugin format's types
//! taken out: the CLAP adapter (step 06) implements it, and so does the test
//! double the rack's tests use.
//!
//! [`Lifeline`] is what keeps the two halves in order. Every processor an
//! instance builds holds one end, and the rack holds the other, so the rack
//! can tell -- without the engine telling it -- when the last processor has
//! been dropped. A processor is only ever dropped on the control thread
//! (`EngineHandle::poll` drains the reclaim ring there, and a retired
//! project goes the same way), so the instance is dropped strictly after its
//! processor and never from the callback.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use mooloop_core::{PluginParamInfo, PluginRef, PluginState};
use mooloop_dsp::AudioNode;

/// Work a plugin asked the main thread for, as bits in one word.
///
/// A plugin may ask from any thread, the audio thread included, so the host
/// callbacks only ever set bits ([`RequestFlags::raise`]): nothing blocks and
/// nothing allocates. The pump drains them once a tick through
/// [`HostedInstance::take_requests`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Requests(pub u32);

impl Requests {
    /// CLAP `request_callback`: run [`HostedInstance::on_main_thread`].
    pub const CALLBACK: u32 = 1 << 0;
    /// CLAP `request_restart`: pull the processor back, deactivate,
    /// reactivate, and swap in a new one ([`HostedInstance::build_processor`]).
    pub const RESTART: u32 = 1 << 1;
    /// CLAP `latency.changed`: compensation has to be derived again.
    pub const LATENCY_CHANGED: u32 = 1 << 2;
    /// CLAP `params.rescan`: the parameter list changed.
    pub const PARAMS_RESCAN: u32 = 1 << 3;
    /// CLAP `state.mark_dirty`: the song has changed.
    pub const STATE_DIRTY: u32 = 1 << 4;

    pub const fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// The flag word a plugin's host callbacks raise and the pump drains.
#[derive(Debug, Default)]
pub struct RequestFlags(AtomicU32);

impl RequestFlags {
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// Raise `bits`. Safe from any thread, the audio thread included.
    pub fn raise(&self, bits: u32) {
        self.0.fetch_or(bits, Ordering::AcqRel);
    }

    /// Everything raised since the last call, cleared.
    pub fn take(&self) -> Requests {
        Requests(self.0.swap(0, Ordering::AcqRel))
    }
}

/// One of the plugin's own parameter changes, as its processor reported
/// it: a GUI drag, a preset it loaded itself, or a value it moved on its
/// own. Read on the control thread ([`HostedInstance::drain_param_events`]).
/// These update what the session knows and never send anything back to the
/// plugin, which is what keeps them from looping (MOO-82).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PluginParamEvent {
    Value { id: u32, value: f64 },
    GestureBegin { id: u32 },
    GestureEnd { id: u32 },
}

/// Why a plugin could not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The plugin reported failure, with whatever it said.
    Plugin(String),
    /// The plugin's ports or format are not ones this host runs (step 06).
    Incompatible(String),
    /// The plugin is not loaded: missing, or failed to instantiate.
    Missing,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plugin(message) => write!(f, "the plugin failed: {message}"),
            Self::Incompatible(message) => write!(f, "the plugin cannot run here: {message}"),
            Self::Missing => write!(f, "the plugin is not loaded"),
        }
    }
}

impl std::error::Error for HostError {}

/// One end of the thread between an instance and the processors it built.
///
/// Cloned into every processor ([`Lifeline::tie`]); the rack keeps the
/// original. [`Lifeline::is_alone`] is true once every processor holding a
/// clone has been dropped.
#[derive(Debug, Clone, Default)]
pub struct Lifeline(Arc<()>);

impl Lifeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// A clone for a processor to hold for as long as it exists.
    pub fn tie(&self) -> Self {
        self.clone()
    }

    /// Whether nothing but this end is left: every processor tied to it has
    /// been dropped.
    pub fn is_alone(&self) -> bool {
        Arc::strong_count(&self.0) == 1
    }
}

/// A hosted plugin's control-thread half, with no format's types in it.
///
/// Every method runs on the control thread. Nothing here may be called from
/// the audio thread, and nothing it returns is shared with it except the
/// processor [`Self::build_processor`] hands over.
pub trait HostedInstance {
    /// What the song remembers this plugin as.
    fn plugin(&self) -> &PluginRef;

    /// The parameters as the plugin reports them now.
    fn params(&self) -> &[PluginParamInfo];

    /// Base-rate frames the plugin currently adds. Known only once it is
    /// active, and it can change while it runs ([`Requests::LATENCY_CHANGED`]).
    fn latency_frames(&self) -> u32;

    fn save_state(&mut self) -> Result<PluginState, HostError>;

    fn load_state(&mut self, state: &PluginState) -> Result<(), HostError>;

    /// The plugin's own text for `value` of parameter `id`, if it has any.
    fn value_text(&mut self, id: u32, value: f64) -> Option<String>;

    /// Drain the requests raised since the last call.
    fn take_requests(&self) -> Requests;

    /// Run the main-thread work the plugin asked for with
    /// [`Requests::CALLBACK`].
    fn on_main_thread(&mut self);

    /// Read the parameter list again, for [`Requests::PARAMS_RESCAN`].
    fn refresh_params(&mut self) {}

    /// The value parameter `id` holds now, in the plugin's plain units, or
    /// `None` when the plugin cannot say.
    fn param_value(&mut self, id: u32) -> Option<f64> {
        let _ = id;
        None
    }

    /// Hand every parameter change the plugin reported since the last call
    /// to `sink`, oldest first.
    fn drain_param_events(&mut self, sink: &mut dyn FnMut(PluginParamEvent)) {
        let _ = sink;
    }

    /// How many of the plugin's own parameter changes its processor had no
    /// room to report. A number that grows is a lost gesture, not a guess.
    fn dropped_param_events(&self) -> u64 {
        0
    }

    /// Whether the running processor has given up on the plugin: it
    /// reported an error, produced a non-finite sample or panicked, and
    /// passes audio through from then on. A new processor starts clean.
    fn failed(&self) -> bool {
        false
    }

    /// Whether it may be a device on a chain: [`crate::scan::effect_refusal`]
    /// on what it declared when it opened (MOO-85). The session refuses a
    /// plugin in a place it does not fit.
    fn fits_effect(&self) -> bool {
        true
    }

    /// Whether it may be a channel's source: [`crate::scan::source_refusal`]
    /// on what it declared when it opened (MOO-85).
    fn fits_source(&self) -> bool {
        false
    }

    /// How many notes the plugin sent out of its own. They are counted, not
    /// routed (plugin-hosting step 10, the plan's "Deliberately not").
    fn generated_notes(&self) -> u64 {
        0
    }

    /// The rate and block ceiling the next processor is built for. The one
    /// that is out keeps its own until it comes back.
    fn set_audio_config(&mut self, config: AudioConfig) {
        let _ = config;
    }

    /// A processor for the audio thread, tied to `lifeline`.
    ///
    /// **One at a time.** A plugin format may allow only one processor per
    /// instance (CLAP does), so this may refuse while an earlier processor
    /// is still out; the rack only asks once the lifeline is alone. It is
    /// also how a restart happens: pull the old processor back, then build.
    fn build_processor(&mut self, lifeline: Lifeline) -> Result<Box<dyn AudioNode + Send>, HostError>;
}

/// The audio configuration a processor is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    pub sample_rate: u32,
    /// The largest block the processor will ever be handed.
    pub max_frames: u32,
}

/// Finds and opens a plugin a song names (step 06).
///
/// The app's is the CLAP opener over the scanner's cache
/// (`crate::clap::ClapOpener`); a test hands the rack whatever it likes.
pub trait PluginOpener {
    /// Create `plugin`, with `state` loaded into it, ready to build a
    /// processor for `config`. [`HostError::Missing`] when it is not
    /// installed.
    fn open(
        &mut self,
        plugin: &PluginRef,
        state: &PluginState,
        config: AudioConfig,
    ) -> Result<Box<dyn HostedInstance>, HostError>;

    /// A number that grows whenever what this opener can find may have
    /// changed (a rescan wrote the cache). A slot that failed to open is
    /// tried again when it moves. Called once a pump tick, so it must be
    /// cheap.
    fn refresh(&mut self) -> u64 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_accumulate_until_taken_and_then_clear() {
        let flags = RequestFlags::new();
        flags.raise(Requests::RESTART);
        flags.raise(Requests::PARAMS_RESCAN);
        let taken = flags.take();
        assert!(taken.has(Requests::RESTART) && taken.has(Requests::PARAMS_RESCAN));
        assert!(!taken.has(Requests::CALLBACK));
        assert!(flags.take().is_empty());
    }

    #[test]
    fn a_lifeline_is_alone_once_every_tie_is_dropped() {
        let lifeline = Lifeline::new();
        assert!(lifeline.is_alone());
        let first = lifeline.tie();
        let second = lifeline.tie();
        assert!(!lifeline.is_alone());
        drop(first);
        assert!(!lifeline.is_alone());
        drop(second);
        assert!(lifeline.is_alone());
    }
}
