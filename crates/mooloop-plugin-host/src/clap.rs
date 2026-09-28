//! The CLAP adapter: a hosted CLAP effect as the rest of mooloop sees one
//! (`docs/plans/plugin-hosting/06-a-headless-clap-effect.md`).
//!
//! [`ClapInstance`] is the control-thread half ([`HostedInstance`]): it owns
//! the loaded library and the plugin instance, reads the parameter list,
//! saves and loads state, and activates the plugin to build a processor.
//! [`ClapProcessor`] is the audio-thread half, an [`AudioNode`] that crosses
//! to the engine through the ordinary structural commands.
//!
//! **CLAP allows one processor per instance at a time.** `activate` hands
//! out the processor, and `deactivate` wants it back. So an instance never
//! builds a second processor while the first is out: the rack pulls the old
//! one back (a placeholder swapped into its slot) and builds the next one
//! once the lifeline says the old one has been dropped. A restart, a sample
//! rate change and a carry that did not carry all go that way.
//!
//! **What the processor promises the callback.** Nothing in
//! [`ClapProcessor::process`] allocates, locks or blocks: the buffers for
//! every audio port the plugin declares, the event buffer and the ring for
//! the plugin's own parameter output are all sized at activation, and events
//! arrive in order, so the event buffer is never sorted.
//!
//! **Every audio port runs, and only the main ones are wired** (MOO-306,
//! MOO-308). CLAP wants a buffer for every port a plugin declares, so a
//! sidechain or a second bus gets one: each extra input is fed silence and
//! each extra output is scratch that nothing reads. The main ports are the
//! ones the plugin flags `CLAP_AUDIO_PORT_IS_MAIN` (port 0 when it flags
//! none), found the way the scan finds them ([`crate::scan::read_audio_ports`]),
//! and they need not be port 0. What the plugin itself does
//! inside its `process` is its own affair; mooloop cannot vouch for it.
//!
//! **One deviation from CLAP's threading rules, and why.** CLAP says
//! `stop_processing` is called on the audio thread. A processor that is
//! removed from a chain never gets another call on the audio thread -- the
//! engine has no "you are about to be removed" hook, and adding one to
//! `AudioNode` for this is out of proportion -- so a started processor is
//! stopped by `deactivate` on the main thread, which is what `clack-host`
//! does for a processor dropped while started. No plugin in the test set
//! objects.

use std::cell::RefCell;
use std::ffi::CString;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::{Duration, Instant, SystemTime};

use clack_extensions::audio_ports::PluginAudioPorts;
use clack_extensions::gui::{
    GuiApiType, GuiConfiguration, GuiSize as ClapGuiSize, HostGui, HostGuiImpl, PluginGui, Window,
};
#[cfg(unix)]
use clack_extensions::posix_fd::{FdFlags, HostPosixFd, HostPosixFdImpl, PluginPosixFd};
use clack_extensions::timer::{HostTimer, HostTimerImpl, PluginTimer, TimerId};
use clack_host::host::HostError as ClapHostError;
use clack_extensions::latency::{HostLatency, HostLatencyImpl, PluginLatency};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{NoteDialects, NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer,
    ParamInfoFlags, ParamRescanFlags, PluginParams,
};
use clack_extensions::state::{HostState, HostStateImpl, PluginState as ClapStateExt};
use clack_extensions::tail::PluginTail;
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_host::events::event_types::{
    MidiEvent, NoteOffEvent, NoteOnEvent, ParamModEvent, ParamValueEvent, TransportEvent,
    TransportFlags,
};
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::EventFlags;
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};
use mooloop_core::{PluginParamInfo, PluginRef, PluginState, PluginStateChunk};
use mooloop_dsp::node::{Discontinuity, SILENCE_PEAK};
use mooloop_dsp::{AudioNode, Event, EventList, HostedParam, ProcessContext, StereoBus, MAX_BLOCK_SIZE};

use crate::gui::{
    GuiApi, GuiConfig, GuiError, GuiRequest, GuiSize, HostedGui, IoActivity, IoRegistrations,
    NativeWindow,
};
use crate::host_io::HostIo;
pub use crate::instance::PluginParamEvent;
use crate::instance::{AudioConfig, HostError, HostedInstance, Lifeline, PluginOpener};
use crate::notes::{HeldNote, NoteTable, NOTE_ROWS};
use crate::scan::PluginCache;
use crate::{RequestFlags, Requests};

/// The tag a CLAP plugin's one state chunk is saved under
/// (`PluginStateChunk::tag`).
pub const STATE_TAG: &str = "clap";

/// How many parameter events a block can carry into a plugin. The engine's
/// own event list holds 256 (`mooloop_dsp::EventList`), and only its
/// `ParamValue`s cross, so this is never the limit.
const EVENTS_IN: usize = 256;

/// How many parameters a processor tracks as offset by a route at once:
/// a channel's whole route table.
const MODULATED: usize = mooloop_core::MAX_MOD_ROUTES_PER_CHANNEL;

/// How many of the plugin's own parameter changes and gestures wait for the
/// control thread (step 07 reads them). A plugin that sends more between two
/// pump ticks has the rest dropped and counted.
const EVENTS_OUT: usize = 1024;

/// The [`HostHandlers`] a hosted CLAP runs under.
pub struct ClapHost;

impl HostHandlers for ClapHost {
    type Shared<'a> = ClapShared;
    type MainThread<'a> = ClapMainThread;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostLog>()
            .register::<HostThreadCheck>()
            .register::<HostLatency>()
            .register::<HostState>()
            .register::<HostParams>()
            // Step 11 (MOO-300): the GUI, and the event loop it runs on.
            .register::<HostGui>()
            .register::<HostTimer>();
        #[cfg(unix)]
        builder.register::<HostPosixFd>();
    }
}

/// The GUI requests a plugin raises from any thread, as bits in one word
/// ([`ClapShared::gui_requests`]), in the order the pump carries them out.
mod gui_bits {
    pub const RESIZE: u32 = 1 << 0;
    pub const SHOW: u32 = 1 << 1;
    pub const HIDE: u32 = 1 << 2;
    pub const HINTS: u32 = 1 << 3;
    pub const CLOSED: u32 = 1 << 4;
    pub const DESTROYED: u32 = 1 << 5;
}

/// What a plugin may reach from any thread. Every callback only raises bits
/// or bumps a counter: nothing locks and nothing allocates, because any of
/// them may arrive on the audio thread.
pub struct ClapShared {
    main_thread: ThreadId,
    requests: Arc<RequestFlags>,
    /// The GUI's requests of its window ([`gui_bits`]). Kept apart from
    /// `requests`, which the rack drains every tick for other work.
    gui_requests: RequestFlags,
    /// The last size the GUI asked for, packed as CLAP packs it.
    gui_size: AtomicU64,
    /// Log lines the plugin sent from a thread other than the main one,
    /// which are counted rather than written: writing a log line allocates.
    unlogged: AtomicU64,
    /// Calls the plugin itself reported as the host misbehaving.
    misbehaviour: AtomicU64,
}

impl<'a> SharedHandler<'a> for ClapShared {
    fn request_restart(&self) {
        self.requests.raise(Requests::RESTART);
    }

    /// Ignored: the engine calls every processor every block it is not
    /// skipping, so there is nothing to wake.
    fn request_process(&self) {}

    fn request_callback(&self) {
        self.requests.raise(Requests::CALLBACK);
    }
}

impl HostLogImpl for ClapShared {
    fn log(&self, severity: LogSeverity, message: &str) {
        if matches!(severity, LogSeverity::HostMisbehaving) {
            self.misbehaviour.fetch_add(1, Ordering::Relaxed);
        }
        if std::thread::current().id() != self.main_thread {
            self.unlogged.fetch_add(1, Ordering::Relaxed);
            return;
        }
        match severity {
            LogSeverity::Error | LogSeverity::Fatal | LogSeverity::HostMisbehaving => {
                mooloop_core::log_warn!("plugin", "{message}");
            }
            _ => mooloop_core::log_info!("plugin", "{message}"),
        }
    }
}

impl HostThreadCheckImpl for ClapShared {
    fn is_main_thread(&self) -> bool {
        std::thread::current().id() == self.main_thread
    }

    /// Any thread but the one that created the instance. The engine's
    /// callback thread belongs to the driver and an export renders on a
    /// worker, so there is no single audio thread to compare against.
    fn is_audio_thread(&self) -> bool {
        !self.is_main_thread()
    }
}

impl HostParamsImplShared for ClapShared {
    /// Ignored: the engine calls the processor every block, which is when a
    /// flush would happen anyway.
    fn request_flush(&self) {}
}

/// The GUI's requests of its window: bits and one packed size, so a request
/// from the plugin's own GUI thread neither locks nor allocates.
impl HostGuiImpl for ClapShared {
    fn resize_hints_changed(&self) {
        self.gui_requests.raise(gui_bits::HINTS);
    }

    fn request_resize(&self, new_size: ClapGuiSize) -> Result<(), ClapHostError> {
        self.gui_size.store(new_size.pack_to_u64(), Ordering::Release);
        self.gui_requests.raise(gui_bits::RESIZE);
        Ok(())
    }

    fn request_show(&self) -> Result<(), ClapHostError> {
        self.gui_requests.raise(gui_bits::SHOW);
        Ok(())
    }

    fn request_hide(&self) -> Result<(), ClapHostError> {
        self.gui_requests.raise(gui_bits::HIDE);
        Ok(())
    }

    fn closed(&self, was_destroyed: bool) {
        let bits = if was_destroyed {
            gui_bits::CLOSED | gui_bits::DESTROYED
        } else {
            gui_bits::CLOSED
        };
        self.gui_requests.raise(bits);
    }
}

/// What a plugin may reach only from the main thread.
pub struct ClapMainThread {
    requests: Arc<RequestFlags>,
    main_thread: ThreadId,
    /// The timers and fds the plugin registered (step 11). A `RefCell`
    /// because the handlers take `&self`; it is borrowed only for a moment,
    /// never across a call into the plugin, which may register or
    /// unregister from inside its own callback.
    io: RefCell<HostIo>,
}

impl ClapMainThread {
    /// The registration table, or a refusal off the main thread: CLAP says
    /// these are main-thread calls, and the table is not shared.
    fn io(&self) -> Result<std::cell::RefMut<'_, HostIo>, ClapHostError> {
        if std::thread::current().id() != self.main_thread {
            return Err(ClapHostError::Message("called off the main thread"));
        }
        self.io
            .try_borrow_mut()
            .map_err(|_| ClapHostError::Message("the host's event table is busy"))
    }
}

impl<'a> MainThreadHandler<'a> for ClapMainThread {}

impl HostTimerImpl for ClapMainThread {
    fn register_timer(&self, period_ms: u32) -> Result<TimerId, ClapHostError> {
        Ok(TimerId(self.io()?.register_timer(period_ms, Instant::now())))
    }

    fn unregister_timer(&self, timer_id: TimerId) -> Result<(), ClapHostError> {
        if self.io()?.unregister_timer(timer_id.0) {
            Ok(())
        } else {
            Err(ClapHostError::Message("no such timer"))
        }
    }
}

#[cfg(unix)]
impl HostPosixFdImpl for ClapMainThread {
    fn register_fd(&self, fd: std::os::unix::io::RawFd, flags: FdFlags) -> Result<(), ClapHostError> {
        if self.io()?.register_fd(fd, flags.bits()) {
            Ok(())
        } else {
            Err(ClapHostError::Message("that fd is already registered, or not an fd"))
        }
    }

    fn modify_fd(&self, fd: std::os::unix::io::RawFd, flags: FdFlags) -> Result<(), ClapHostError> {
        if self.io()?.modify_fd(fd, flags.bits()) {
            Ok(())
        } else {
            Err(ClapHostError::Message("no such fd"))
        }
    }

    fn unregister_fd(&self, fd: std::os::unix::io::RawFd) -> Result<(), ClapHostError> {
        if self.io()?.unregister_fd(fd) {
            Ok(())
        } else {
            Err(ClapHostError::Message("no such fd"))
        }
    }
}

impl HostLatencyImpl for ClapMainThread {
    fn changed(&self) {
        self.requests.raise(Requests::LATENCY_CHANGED);
    }
}

impl HostStateImpl for ClapMainThread {
    fn mark_dirty(&self) {
        self.requests.raise(Requests::STATE_DIRTY);
    }
}

impl HostParamsImplMainThread for ClapMainThread {
    fn rescan(&self, _flags: ParamRescanFlags) {
        self.requests.raise(Requests::PARAMS_RESCAN);
    }

    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

/// What the processor and the instance both see.
#[derive(Debug, Default)]
struct ProcessorFlags {
    /// Set by the processor when the plugin failed; it passes audio through
    /// from then on.
    failed: AtomicBool,
    /// Parameter events from the plugin the ring had no room for.
    dropped_events: AtomicU64,
    /// Note-ons and note-offs the plugin sent of its own (step 10: counted,
    /// not routed).
    generated_notes: AtomicU64,
}

/// How a plugin takes notes, from its first note input port: CLAP's own
/// note events where it supports them, MIDI bytes where it supports only
/// those (step 10's order of preference), or not at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Notes {
    None,
    Clap,
    Midi,
}

/// The ports a plugin was accepted with, and where it may go (MOO-85): the
/// places are [`crate::scan::main_port_effect_refusal`] and
/// `main_port_source_refusal` on its own features and main ports, the same
/// rule the browser reads from the scan (MOO-307).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Layout {
    /// Channels of each audio input port, in the plugin's order (0 for a
    /// port it would not describe, so indices stay the plugin's). Every one
    /// gets a buffer.
    inputs: Box<[u32]>,
    /// Channels of each audio output port, as `inputs`.
    outputs: Box<[u32]>,
    /// Which input is main; past the end when there are none. As an effect
    /// it is fed the chain's signal, a mono one `(L + R) / 2` (MOO-266); as
    /// a source, and every other input always, silence.
    main_input: u32,
    /// Which output is main: one or two channels, by the refusal rules. A
    /// mono one goes to both sides. Every other output is thrown away.
    main_output: u32,
    notes: Notes,
    effect: bool,
    source: bool,
}

impl Layout {
    fn main_input_channels(&self) -> Option<u32> {
        crate::scan::main_channels(&self.inputs, self.main_input)
    }

    fn main_output_channels(&self) -> Option<u32> {
        crate::scan::main_channels(&self.outputs, self.main_output)
    }
}

/// One buffer of `frames` for each channel of each port, port after port.
fn port_buffers(ports: &[u32], frames: usize) -> Box<[Box<[f32]>]> {
    let channels: usize = ports.iter().map(|&channels| channels as usize).sum();
    (0..channels).map(|_| vec![0.0; frames].into_boxed_slice()).collect()
}

/// Where port `port`'s first channel sits in [`port_buffers`]' list.
fn first_channel(ports: &[u32], port: u32) -> usize {
    ports
        .iter()
        .take(port as usize)
        .map(|&channels| channels as usize)
        .sum()
}

/// A hosted CLAP plugin's control-thread half.
pub struct ClapInstance {
    plugin: PluginRef,
    params: Vec<PluginParamInfo>,
    latency: u32,
    config: AudioConfig,
    requests: Arc<RequestFlags>,
    flags: Arc<ProcessorFlags>,
    events_out: Option<rtrb::Consumer<PluginParamEvent>>,
    layout: Layout,
    /// The thread the instance was opened on: every GUI call must be made
    /// there.
    main_thread: ThreadId,
    /// The plugin's side of the GUI, timer and fd extensions, where it has
    /// them.
    gui_ext: Option<PluginGui>,
    timer_ext: Option<PluginTimer>,
    #[cfg(unix)]
    fd_ext: Option<PluginPosixFd>,
    /// The configuration the GUI is open in.
    gui_open: Option<GuiConfig>,
    gui_visible: bool,
    /// Reused by [`HostedInstance::service_io`], so a tick allocates nothing
    /// once these have grown.
    io_scratch: IoScratch,
    instance: PluginInstance<ClapHost>,
}

#[derive(Default)]
struct IoScratch {
    timers: Vec<u32>,
    fds: Vec<(crate::host_io::Fd, u32)>,
    ready: Vec<(crate::host_io::Fd, u32)>,
    poll: Vec<crate::host_io::sys::PollFd>,
}

impl ClapInstance {
    /// Load `plugin` from the library at `path`, create it, and load `state`
    /// into it. It is not activated until [`HostedInstance::build_processor`].
    ///
    /// Refused as [`HostError::Incompatible`] unless its main ports are an
    /// effect's (a main input and a main output of one or two channels each)
    /// or an instrument's (a main output of one or two channels and a note
    /// input). Any other port is run and not wired: a sidechain hears
    /// silence, a second output is thrown away (MOO-308). Where each may go
    /// is the session's call.
    pub fn open(
        path: &Path,
        plugin: &PluginRef,
        state: &PluginState,
        config: AudioConfig,
    ) -> Result<Self, HostError> {
        // SAFETY: the file is one the scanner loaded in a child process
        // first, and the caller named it by the scan cache's path.
        let entry = unsafe { crate::load_entry(path) }
            .map_err(|error| HostError::Plugin(format!("{} did not load: {error}", path.display())))?;
        let id = CString::new(plugin.id.clone())
            .map_err(|_| HostError::Incompatible("its id holds a NUL".into()))?;
        let requests = Arc::new(RequestFlags::new());
        let (shared_requests, main_requests) = (requests.clone(), requests.clone());
        let main_thread = std::thread::current().id();
        let mut instance = PluginInstance::<ClapHost>::new(
            move |_| ClapShared {
                main_thread,
                requests: shared_requests,
                gui_requests: RequestFlags::new(),
                gui_size: AtomicU64::new(0),
                unlogged: AtomicU64::new(0),
                misbehaviour: AtomicU64::new(0),
            },
            move |_| ClapMainThread {
                requests: main_requests,
                main_thread,
                io: RefCell::new(HostIo::default()),
            },
            &entry,
            &id,
            &crate::host_info(),
        )
        .map_err(|error| HostError::Plugin(format!("{} could not be created: {error}", plugin.id)))?;
        let features = crate::load_features(&entry, &id);
        let layout = check_ports(&mut instance, &features)?;
        let shared = instance.plugin_shared_handle();
        let (gui_ext, timer_ext) = (shared.get_extension::<PluginGui>(), shared.get_extension::<PluginTimer>());
        #[cfg(unix)]
        let fd_ext = shared.get_extension::<PluginPosixFd>();
        let mut hosted = Self {
            plugin: plugin.clone(),
            params: Vec::new(),
            latency: 0,
            config,
            requests,
            flags: Arc::new(ProcessorFlags::default()),
            events_out: None,
            layout,
            main_thread,
            gui_ext,
            timer_ext,
            #[cfg(unix)]
            fd_ext,
            gui_open: None,
            gui_visible: false,
            io_scratch: IoScratch::default(),
            instance,
        };
        if !state.is_empty() {
            hosted.load_state(state)?;
        }
        hosted.params = hosted.read_params();
        Ok(hosted)
    }

    fn read_params(&mut self) -> Vec<PluginParamInfo> {
        let Some(params) = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginParams>()
        else {
            return Vec::new();
        };
        let handle = self.instance.plugin_handle();
        (0..params.count(&handle))
            .filter_map(|index| {
                let mut buffer = ParamInfoBuffer::new();
                let info = params.get_info(&handle, index, &mut buffer)?;
                let stepped = info.flags.contains(ParamInfoFlags::IS_STEPPED).then(|| {
                    ((info.max_value - info.min_value).round().max(0.0) as u64 + 1).min(u64::from(u16::MAX))
                        as u16
                });
                Some(PluginParamInfo {
                    id: info.id.get(),
                    name: String::from_utf8_lossy(info.name).into_owned(),
                    module: String::from_utf8_lossy(info.module).into_owned(),
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    stepped,
                    automatable: info.flags.contains(ParamInfoFlags::IS_AUTOMATABLE),
                    modulatable: info.flags.contains(ParamInfoFlags::IS_MODULATABLE),
                    hidden: info.flags.contains(ParamInfoFlags::IS_HIDDEN),
                })
            })
            .collect()
    }

    /// How many times the plugin reported the host misbehaving.
    pub fn misbehaviour(&self) -> u64 {
        self.instance
            .access_shared_handler(|shared| shared.misbehaviour.load(Ordering::Relaxed))
    }

    /// Deactivate, if a processor was ever built. Fails while the last
    /// processor is still out: CLAP wants it back first.
    fn deactivate(&mut self) -> Result<(), HostError> {
        if !self.instance.is_active() {
            return Ok(());
        }
        self.instance
            .try_deactivate()
            .map_err(|_| HostError::Plugin("its processor has not come back yet".into()))
    }
}

/// Accept an effect's main ports or an instrument's (see [`Layout`]), and
/// refuse everything else. [`crate::scan::ScannedPlugin::effect_refusal`]
/// and `source_refusal` say the same from the scan, without loading
/// anything, and both find the main ports through
/// [`crate::scan::read_audio_ports`].
fn check_ports(instance: &mut PluginInstance<ClapHost>, features: &[String]) -> Result<Layout, HostError> {
    let Some(ports) = instance
        .plugin_shared_handle()
        .get_extension::<PluginAudioPorts>()
    else {
        return Err(HostError::Incompatible("it declares no audio ports".into()));
    };
    let note_ports = instance
        .plugin_shared_handle()
        .get_extension::<PluginNotePorts>();
    let handle = instance.plugin_handle();
    // Counted before the first is asked for (MOO-311): an effect that
    // declares the extension with no ports (Surge XT Effects) reports an
    // out-of-bounds `get` as host misbehaviour, and a strict plugin could
    // terminate on it.
    let notes = note_ports
        .filter(|note_ports| note_ports.count(&handle, true) > 0)
        .and_then(|note_ports| {
            let mut buffer = NotePortInfoBuffer::new();
            note_ports
                .get(&handle, 0, true, &mut buffer)
                .map(|info| info.supported_dialects)
        })
        .map_or(Notes::None, |dialects| {
            if dialects.contains(NoteDialects::CLAP) {
                Notes::Clap
            } else if dialects.contains(NoteDialects::MIDI) {
                Notes::Midi
            } else {
                Notes::None
            }
        });
    let (inputs, main_input) = crate::scan::read_audio_ports(&ports, &handle, true);
    let (outputs, main_output) = crate::scan::read_audio_ports(&ports, &handle, false);
    let layout = Layout {
        inputs: inputs.into_boxed_slice(),
        outputs: outputs.into_boxed_slice(),
        main_input,
        main_output,
        notes,
        effect: false,
        source: false,
    };
    // The note input the host wires is one it can speak to.
    let note_inputs = u32::from(notes != Notes::None);
    let (main_in, main_out) = (layout.main_input_channels(), layout.main_output_channels());
    let as_effect = crate::scan::main_port_effect_refusal(features, main_in, main_out);
    let as_source = crate::scan::main_port_source_refusal(features, main_out, note_inputs);
    if let (Some(effect), Some(source)) = (&as_effect, &as_source) {
        return Err(HostError::Incompatible(format!("as an effect, {effect}; as a source, {source}")));
    }
    Ok(Layout {
        effect: as_effect.is_none(),
        source: as_source.is_none(),
        ..layout
    })
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        // Step 04's order: the GUI, then the processor, then the instance.
        // The rack destroys the GUI before it retires an instance; this is
        // the backstop for any other owner, and says so when it is needed.
        if self.gui_open.is_some() {
            mooloop_core::log_warn!("plugin", "{}: its GUI was still open when it was dropped", self.plugin.name);
            HostedGui::destroy(self);
        }
        // The rack drops an instance only once its processors are gone, so
        // this succeeds; if it cannot, `clack-host` leaks the plugin rather
        // than destroy it under a running processor.
        let _ = self.deactivate();
    }
}

fn clap_config(config: GuiConfig) -> GuiConfiguration<'static> {
    GuiConfiguration {
        api_type: match config.api {
            GuiApi::X11 => GuiApiType::X11,
        },
        is_floating: config.floating,
    }
}

fn clap_window(window: NativeWindow) -> Result<Window<'static, 'static>, GuiError> {
    match window.api {
        GuiApi::X11 => std::ffi::c_ulong::try_from(window.id)
            .map(Window::from_x11_handle)
            .map_err(|_| GuiError::Refused("take an X11 window id this wide")),
    }
}

impl ClapInstance {
    /// The plugin's GUI extension, on the control thread only.
    fn gui_ext(&self) -> Result<PluginGui, GuiError> {
        if std::thread::current().id() != self.main_thread {
            return Err(GuiError::WrongThread);
        }
        self.gui_ext.ok_or(GuiError::NoGui)
    }

    /// The extension, and the GUI open.
    fn open_gui_ext(&self) -> Result<PluginGui, GuiError> {
        let gui = self.gui_ext()?;
        if self.gui_open.is_none() {
            return Err(GuiError::NotOpen);
        }
        Ok(gui)
    }
}

impl HostedGui for ClapInstance {
    fn is_api_supported(&mut self, config: GuiConfig) -> bool {
        let Ok(gui) = self.gui_ext() else {
            return false;
        };
        gui.is_api_supported(&self.instance.plugin_handle(), clap_config(config))
    }

    fn preferred_api(&mut self) -> Option<GuiConfig> {
        let gui = self.gui_ext().ok()?;
        let preferred = gui.get_preferred_api(&self.instance.plugin_handle())?;
        (preferred.api_type == GuiApiType::X11).then_some(GuiConfig {
            api: GuiApi::X11,
            floating: preferred.is_floating,
        })
    }

    fn open_config(&self) -> Option<GuiConfig> {
        self.gui_open
    }

    fn is_visible(&self) -> bool {
        self.gui_visible
    }

    fn create(&mut self, config: GuiConfig) -> Result<(), GuiError> {
        let gui = self.gui_ext()?;
        if self.gui_open.is_some() {
            return Err(GuiError::AlreadyOpen);
        }
        if !gui.is_api_supported(&self.instance.plugin_handle(), clap_config(config)) {
            return Err(GuiError::Unsupported(config));
        }
        // Anything raised while no GUI was open is stale.
        self.instance.access_shared_handler(|shared| shared.gui_requests.take());
        gui.create(&self.instance.plugin_handle(), clap_config(config))
            .map_err(|_| GuiError::Refused("open"))?;
        self.gui_open = Some(config);
        self.gui_visible = false;
        Ok(())
    }

    fn set_scale(&mut self, scale: f64) -> Result<(), GuiError> {
        let gui = self.open_gui_ext()?;
        gui.set_scale(&self.instance.plugin_handle(), scale)
            .map_err(|_| GuiError::Refused("take the window's scale"))
    }

    fn size(&mut self) -> Option<GuiSize> {
        let gui = self.open_gui_ext().ok()?;
        gui.get_size(&self.instance.plugin_handle())
            .map(|size| GuiSize {
                width: size.width,
                height: size.height,
            })
    }

    fn can_resize(&mut self) -> bool {
        let Ok(gui) = self.open_gui_ext() else {
            return false;
        };
        gui.can_resize(&self.instance.plugin_handle())
    }

    fn adjust_size(&mut self, size: GuiSize) -> Option<GuiSize> {
        let gui = self.open_gui_ext().ok()?;
        gui.adjust_size(
            &self.instance.plugin_handle(),
            ClapGuiSize {
                width: size.width,
                height: size.height,
            },
        )
        .map(|size| GuiSize {
            width: size.width,
            height: size.height,
        })
    }

    fn set_size(&mut self, size: GuiSize) -> Result<(), GuiError> {
        let gui = self.open_gui_ext()?;
        gui.set_size(
            &self.instance.plugin_handle(),
            ClapGuiSize {
                width: size.width,
                height: size.height,
            },
        )
        .map_err(|_| GuiError::Refused("take that size"))
    }

    fn set_parent(&mut self, window: NativeWindow) -> Result<(), GuiError> {
        let gui = self.open_gui_ext()?;
        let window = clap_window(window)?;
        // SAFETY: `NativeWindow`'s contract, which the caller keeps: the
        // window outlives the GUI in it. An X11 id is a number, not a
        // pointer, so a stale one is an X error in the plugin, not memory
        // this process could corrupt.
        unsafe { gui.set_parent(&self.instance.plugin_handle(), window) }
            .map_err(|_| GuiError::Refused("embed in the window"))
    }

    fn set_transient(&mut self, window: NativeWindow) -> Result<(), GuiError> {
        let gui = self.open_gui_ext()?;
        let window = clap_window(window)?;
        // SAFETY: as for `set_parent`.
        unsafe { gui.set_transient(&self.instance.plugin_handle(), window) }
            .map_err(|_| GuiError::Refused("stay above the window"))
    }

    fn suggest_title(&mut self, title: &str) {
        let Ok(gui) = self.open_gui_ext() else {
            return;
        };
        let title: String = title.chars().filter(|&c| c != '\0').collect();
        if let Ok(title) = CString::new(title) {
            gui.suggest_title(&self.instance.plugin_handle(), &title);
        }
    }

    fn show(&mut self) -> Result<(), GuiError> {
        let gui = self.open_gui_ext()?;
        gui.show(&self.instance.plugin_handle())
            .map_err(|_| GuiError::Refused("show"))?;
        self.gui_visible = true;
        Ok(())
    }

    fn hide(&mut self) -> Result<(), GuiError> {
        let gui = self.open_gui_ext()?;
        gui.hide(&self.instance.plugin_handle())
            .map_err(|_| GuiError::Refused("hide"))?;
        self.gui_visible = false;
        Ok(())
    }

    fn destroy(&mut self) {
        let Ok(gui) = self.open_gui_ext() else {
            return;
        };
        gui.destroy(&self.instance.plugin_handle());
        self.gui_open = None;
        self.gui_visible = false;
        self.instance.access_shared_handler(|shared| shared.gui_requests.take());
    }

    fn take_requests(&mut self, sink: &mut dyn FnMut(GuiRequest)) {
        let (bits, packed) = self.instance.access_shared_handler(|shared| {
            let bits = shared.gui_requests.take();
            (bits, shared.gui_size.load(Ordering::Acquire))
        });
        if self.gui_open.is_none() || bits.is_empty() {
            return;
        }
        if bits.has(gui_bits::HINTS) {
            sink(GuiRequest::ResizeHintsChanged);
        }
        if bits.has(gui_bits::RESIZE) {
            let size = ClapGuiSize::unpack_from_u64(packed);
            sink(GuiRequest::Resize(GuiSize {
                width: size.width,
                height: size.height,
            }));
        }
        // A show and a hide in one tick: the later one cannot be told, so
        // the window ends up shown, which the user can undo and a lost show
        // could not be.
        if bits.has(gui_bits::HIDE) && !bits.has(gui_bits::SHOW) {
            sink(GuiRequest::Hide);
        }
        if bits.has(gui_bits::SHOW) {
            sink(GuiRequest::Show);
        }
        if bits.has(gui_bits::CLOSED) {
            sink(GuiRequest::Closed {
                destroyed: bits.has(gui_bits::DESTROYED),
            });
        }
    }
}

impl ClapInstance {
    /// Is timer `id` still registered? A callback may unregister another
    /// timer, or an fd, before its own turn comes.
    fn has_timer(&self, id: u32) -> bool {
        self.instance
            .access_handler(|main| main.io.try_borrow().is_ok_and(|io| io.has_timer(id)))
    }

    #[cfg(unix)]
    fn has_fd(&self, fd: crate::host_io::Fd) -> bool {
        self.instance
            .access_handler(|main| main.io.try_borrow().is_ok_and(|io| io.has_fd(fd)))
    }
}

impl HostedInstance for ClapInstance {
    fn plugin(&self) -> &PluginRef {
        &self.plugin
    }

    fn params(&self) -> &[PluginParamInfo] {
        &self.params
    }

    fn latency_frames(&self) -> u32 {
        self.latency
    }

    fn save_state(&mut self) -> Result<PluginState, HostError> {
        let Some(state) = self
            .instance
            .plugin_shared_handle()
            .get_extension::<ClapStateExt>()
        else {
            return Ok(PluginState::default());
        };
        let mut data = Vec::new();
        state
            .save(&self.instance.plugin_handle(), &mut data)
            .map_err(|error| HostError::Plugin(format!("its state did not save: {error}")))?;
        Ok(PluginState {
            chunks: vec![PluginStateChunk {
                tag: STATE_TAG.to_owned(),
                data,
            }],
        })
    }

    fn load_state(&mut self, saved: &PluginState) -> Result<(), HostError> {
        let Some(chunk) = saved.chunks.iter().find(|chunk| chunk.tag == STATE_TAG) else {
            return Ok(());
        };
        let Some(state) = self
            .instance
            .plugin_shared_handle()
            .get_extension::<ClapStateExt>()
        else {
            return Err(HostError::Plugin("it has saved state but no way to load it".into()));
        };
        state
            .load(&self.instance.plugin_handle(), &mut chunk.data.as_slice())
            .map_err(|error| HostError::Plugin(format!("its state did not load: {error}")))
    }

    fn value_text(&mut self, id: u32, value: f64) -> Option<String> {
        let params = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginParams>()?;
        let mut text = [0u8; 256];
        let shown = params
            .value_to_text(&self.instance.plugin_handle(), ClapId::new(id), value, &mut text)
            .ok()?;
        Some(String::from_utf8_lossy(shown).into_owned())
    }

    fn take_requests(&self) -> Requests {
        self.requests.take()
    }

    fn on_main_thread(&mut self) {
        self.instance.call_on_main_thread_callback();
    }

    fn refresh_params(&mut self) {
        self.params = self.read_params();
    }

    fn param_value(&mut self, id: u32) -> Option<f64> {
        let params = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginParams>()?;
        params.get_value(&self.instance.plugin_handle(), ClapId::new(id))
    }

    /// Every parameter event the running processor's plugin reported, oldest
    /// first, off the bounded ring the processor fills (step 06).
    fn drain_param_events(&mut self, sink: &mut dyn FnMut(PluginParamEvent)) {
        if let Some(events) = self.events_out.as_mut() {
            while let Ok(event) = events.pop() {
                sink(event);
            }
        }
    }

    /// Counted by the processor as it had no room, and read here. It starts
    /// again at zero with each processor.
    fn dropped_param_events(&self) -> u64 {
        self.flags.dropped_events.load(Ordering::Relaxed)
    }

    fn failed(&self) -> bool {
        self.flags.failed.load(Ordering::Relaxed)
    }

    fn fits_effect(&self) -> bool {
        self.layout.effect
    }

    fn fits_source(&self) -> bool {
        self.layout.source
    }

    fn generated_notes(&self) -> u64 {
        self.flags.generated_notes.load(Ordering::Relaxed)
    }

    fn set_audio_config(&mut self, config: AudioConfig) {
        self.config = config;
    }

    fn gui(&mut self) -> Option<&mut dyn HostedGui> {
        if self.gui_ext.is_some() {
            Some(self)
        } else {
            None
        }
    }

    fn service_io(&mut self, now: Instant) -> IoActivity {
        let mut activity = IoActivity::default();
        if std::thread::current().id() != self.main_thread {
            return activity;
        }
        let mut scratch = std::mem::take(&mut self.io_scratch);
        scratch.timers.clear();
        scratch.fds.clear();
        scratch.ready.clear();
        let empty = self.instance.access_handler(|main| {
            let Ok(mut io) = main.io.try_borrow_mut() else {
                return true;
            };
            io.take_due_timers(now, &mut scratch.timers);
            io.fds(&mut scratch.fds);
            io.is_empty()
        });
        if !empty {
            if let Some(timer) = self.timer_ext {
                for &id in &scratch.timers {
                    if self.has_timer(id) {
                        timer.on_timer(&self.instance.plugin_handle(), TimerId(id));
                        activity.timers_fired += 1;
                    }
                }
            }
            #[cfg(unix)]
            if let Some(fd_ext) = self.fd_ext {
                crate::host_io::poll_ready(&scratch.fds, &mut scratch.poll, &mut scratch.ready);
                for &(fd, flags) in &scratch.ready {
                    if self.has_fd(fd) {
                        fd_ext.on_fd(&self.instance.plugin_handle(), fd, FdFlags::from_bits_truncate(flags));
                        activity.fds_fired += 1;
                    }
                }
            }
        }
        self.io_scratch = scratch;
        activity
    }

    fn io_registrations(&self) -> IoRegistrations {
        self.instance.access_handler(|main| {
            main.io
                .try_borrow()
                .map(|io| IoRegistrations {
                    timers: io.timer_count(),
                    fds: io.fd_count(),
                })
                .unwrap_or_default()
        })
    }

    fn build_processor(&mut self, lifeline: Lifeline) -> Result<Box<dyn AudioNode + Send>, HostError> {
        self.deactivate()?;
        let AudioConfig {
            sample_rate,
            max_frames,
        } = self.config;
        let stopped = self
            .instance
            .activate(
                |_, _| (),
                PluginAudioConfiguration {
                    sample_rate: f64::from(sample_rate),
                    min_frames_count: 1,
                    max_frames_count: max_frames.max(1),
                },
            )
            .map_err(|error| HostError::Plugin(format!("it did not activate: {error}")))?;
        self.latency = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginLatency>()
            .map(|latency| latency.get(&self.instance.plugin_handle()))
            .unwrap_or(0);
        let tail = self
            .instance
            .plugin_shared_handle()
            .get_extension::<PluginTail>();
        let flags = Arc::new(ProcessorFlags::default());
        self.flags = flags.clone();
        let (producer, consumer) = rtrb::RingBuffer::new(EVENTS_OUT);
        self.events_out = Some(consumer);
        let frames = max_frames.max(1) as usize;
        let mut params: Vec<(u32, HostedParam)> = self
            .params
            .iter()
            .map(|info| {
                (
                    info.id,
                    HostedParam {
                        min: info.min,
                        max: info.max,
                        steps: info.stepped,
                        automatable: info.automatable,
                        modulatable: info.modulatable,
                    },
                )
            })
            .collect();
        params.sort_by_key(|&(id, _)| id);
        params.dedup_by_key(|&mut (id, _)| id);
        let layout = self.layout.clone();
        let main_out = match layout.main_output_channels() {
            Some(channels @ (1 | 2)) => channels as usize,
            // `check_ports` accepted it with a main output of one or two
            // channels, in either place.
            _ => return Err(HostError::Incompatible("it has no main output of 1 or 2 channels".into())),
        };
        let main_in = layout.main_input_channels().unwrap_or(0) as usize;
        let in_count: usize = layout.inputs.iter().map(|&channels| channels as usize).sum();
        let out_count: usize = layout.outputs.iter().map(|&channels| channels as usize).sum();
        Ok(Box::new(ClapProcessor {
            params: params.into_boxed_slice(),
            modulated: [0; MODULATED],
            modulated_count: 0,
            driven: false,
            processor: Some(PluginAudioProcessor::from(stopped)),
            tail,
            tail_frames: 0,
            latency: self.latency,
            notes: NoteTable::new(),
            max_frames: frames,
            ins: port_buffers(&layout.inputs, frames),
            outs: port_buffers(&layout.outputs, frames),
            main_in: (first_channel(&layout.inputs, layout.main_input), main_in),
            main_out: (first_channel(&layout.outputs, layout.main_output), main_out),
            inputs: AudioPorts::with_capacity(in_count, layout.inputs.len()),
            outputs: AudioPorts::with_capacity(out_count, layout.outputs.len()),
            layout,
            events: EventBuffer::with_capacity(EVENTS_IN + NOTE_ROWS),
            events_out: producer,
            flags,
            at_rest: false,
            steady_time: 0,
            _lifeline: lifeline,
        }))
    }
}

/// A hosted CLAP plugin's audio-thread half.
pub struct ClapProcessor {
    /// The plugin's parameters as it listed them when this processor was
    /// built, sorted by id, for [`AudioNode::hosted_param`]. A plugin's list
    /// can only change its ranges across a restart, which builds a new
    /// processor, so this never goes stale while it runs.
    params: Box<[(u32, HostedParam)]>,
    /// Ids carrying a nonzero offset from a route as of the last block, so
    /// that an offset whose route has gone is set back to zero: CLAP's
    /// parameter modulation holds until it is changed.
    modulated: [u32; MODULATED],
    modulated_count: usize,
    /// The last block this processor ran carried a parameter change: a lane
    /// or a route is moving the plugin. A driven plugin is never at rest,
    /// because the blocks the host skipped would never deliver their values,
    /// and a plugin that glides between values would wake somewhere that
    /// depends on how long it slept, which depends on the callback's size.
    driven: bool,
    processor: Option<PluginAudioProcessor<ClapHost>>,
    tail: Option<PluginTail>,
    tail_frames: u32,
    latency: u32,
    /// The ports it was accepted with: every port to hand a buffer, which
    /// are main, how it takes notes.
    layout: Layout,
    /// The notes it is holding, by mooloop's id and its own (MOO-85).
    notes: NoteTable,
    /// The longest block the buffers hold.
    max_frames: usize,
    /// One buffer per channel of every input port, port after port in the
    /// plugin's order ([`port_buffers`]), and the same for outputs. All are
    /// sized at activation; `process` only writes into them.
    ins: Box<[Box<[f32]>]>,
    outs: Box<[Box<[f32]>]>,
    /// The main input's first channel in `ins` and its width (0 for none),
    /// and the main output's in `outs` (one or two channels).
    main_in: (usize, usize),
    main_out: (usize, usize),
    /// clack's per-call port lists, sized for every port at activation, so
    /// that filling them never grows them.
    inputs: AudioPorts,
    outputs: AudioPorts,
    events: EventBuffer,
    events_out: rtrb::Producer<PluginParamEvent>,
    flags: Arc<ProcessorFlags>,
    at_rest: bool,
    steady_time: u64,
    _lifeline: Lifeline,
}

/// The plugin's output events, into the bounded ring, never growing.
struct ParamOut<'a> {
    ring: &'a mut rtrb::Producer<PluginParamEvent>,
    dropped: &'a AtomicU64,
    /// The notes the processor holds: a plugin's `note_end` frees its row.
    notes: &'a mut NoteTable,
    generated: &'a AtomicU64,
}

impl OutputEventBuffer for ParamOut<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let item = match event.as_core_event() {
            Some(CoreEventSpace::ParamValue(event)) => event
                .param_id()
                .map(|id| PluginParamEvent::Value {
                    id: id.get(),
                    value: event.value(),
                }),
            Some(CoreEventSpace::ParamGestureBegin(event)) => event
                .param_id()
                .map(|id| PluginParamEvent::GestureBegin { id: id.get() }),
            Some(CoreEventSpace::ParamGestureEnd(event)) => event
                .param_id()
                .map(|id| PluginParamEvent::GestureEnd { id: id.get() }),
            // A voice the plugin finished on its own: its row is free, and a
            // later note-off for it is stale.
            Some(CoreEventSpace::NoteEnd(event)) => {
                if let clack_host::events::Match::Specific(note_id) = event.pckn().note_id {
                    self.notes.ended(note_id);
                }
                None
            }
            // Notes the plugin plays of its own are counted, not routed
            // (the plan's "Deliberately not"), and so is everything else.
            Some(CoreEventSpace::NoteOn(_) | CoreEventSpace::NoteOff(_) | CoreEventSpace::Midi(_)) => {
                self.generated.fetch_add(1, Ordering::Relaxed);
                None
            }
            _ => None,
        };
        let Some(item) = item else {
            return Ok(());
        };
        self.ring.push(item).map_err(|_| {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            TryPushError::new()
        })
    }
}

impl ClapProcessor {
    fn transport(ctx: &ProcessContext) -> TransportEvent {
        let ppq = f64::from(mooloop_core::time::DEFAULT_PPQ);
        let beats = ctx.position_ticks / ppq;
        let per_bar = f64::from(mooloop_core::time::BEATS_PER_BAR);
        let bar = (beats / per_bar).floor();
        let seconds = if ctx.sample_rate == 0 {
            0.0
        } else {
            ctx.position_frames as f64 / f64::from(ctx.sample_rate)
        };
        let mut flags = TransportFlags::HAS_TEMPO
            | TransportFlags::HAS_BEATS_TIMELINE
            | TransportFlags::HAS_SECONDS_TIMELINE
            | TransportFlags::HAS_TIME_SIGNATURE;
        if ctx.playing {
            flags |= TransportFlags::IS_PLAYING;
        }
        TransportEvent {
            header: EventHeader::new_core(0, EventFlags::empty()),
            flags,
            song_pos_beats: BeatTime::from_float(beats),
            song_pos_seconds: SecondsTime::from_float(seconds),
            tempo: ctx.bpm,
            tempo_inc: 0.0,
            loop_start_beats: BeatTime::from_int(0),
            loop_end_beats: BeatTime::from_int(0),
            loop_start_seconds: SecondsTime::from_int(0),
            loop_end_seconds: SecondsTime::from_int(0),
            bar_start: BeatTime::from_float(bar * per_bar),
            bar_number: bar as i32,
            time_signature_numerator: mooloop_core::time::BEATS_PER_BAR as u16,
            time_signature_denominator: 4,
        }
    }

    fn fail(&mut self) {
        self.flags.failed.store(true, Ordering::Relaxed);
    }
}

/// Push `event`, a note, onto `events` at `at` in the plugin's dialect,
/// through the note table: a note-on gets the plugin's id for it (and a
/// full table first releases its oldest note), a note-off only reaches a
/// note the plugin is holding, and a choke releases every one. Returns how
/// many events it pushed; never more than `room`, so the buffer never grows.
fn push_note(
    events: &mut EventBuffer,
    notes: &mut NoteTable,
    dialect: Notes,
    at: u32,
    event: Event,
    room: usize,
) -> usize {
    let mut pushed = 0;
    let off = |events: &mut EventBuffer, note: HeldNote, pushed: &mut usize| {
        if *pushed >= room {
            return;
        }
        match dialect {
            Notes::Clap => events.push(&NoteOffEvent::new(
                at,
                Pckn::new(0u16, 0u16, u16::from(note.key), note.note_id),
                0.0,
            )),
            Notes::Midi => events.push(&MidiEvent::new(at, 0, [0x80, note.key & 0x7f, 0])),
            Notes::None => return,
        }
        *pushed += 1;
    };
    match event {
        Event::NoteOn { id, note, velocity } => {
            let (held, evicted) = notes.start(id, note);
            if let Some(evicted) = evicted {
                off(events, evicted, &mut pushed);
            }
            if pushed < room {
                match dialect {
                    Notes::Clap => events.push(&NoteOnEvent::new(
                        at,
                        Pckn::new(0u16, 0u16, u16::from(note), held.note_id),
                        f64::from(velocity.min(127)) / 127.0,
                    )),
                    Notes::Midi => events.push(&MidiEvent::new(
                        at,
                        0,
                        [0x90, note & 0x7f, velocity.clamp(1, 127)],
                    )),
                    Notes::None => return pushed,
                }
                pushed += 1;
            }
        }
        // A note-off for a note the plugin is not holding is dropped, never
        // sent as a wildcard that would release every voice on its key.
        Event::NoteOff { id, .. } => {
            if let Some(held) = notes.stop(id) {
                off(events, held, &mut pushed);
            }
        }
        Event::Choke => notes.release_all(|held| off(events, held, &mut pushed)),
        _ => {}
    }
    pushed
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

impl AudioNode for ClapProcessor {
    /// The plugin's own tail, as it last reported it; `u32::MAX` for one
    /// that reports none, so the host never sleeps it on a guess.
    fn tail_frames(&self) -> u32 {
        if self.driven {
            return u32::MAX;
        }
        if self.tail.is_some() {
            self.tail_frames
        } else {
            u32::MAX
        }
    }

    /// The plugin said it may sleep: it returned `Sleep`, or it returned
    /// "continue if not quiet" over a block that was quiet in and out. A
    /// failed plugin is a pass-through, which is always at rest.
    fn is_at_rest(&self) -> bool {
        self.flags.failed.load(Ordering::Relaxed) || (self.at_rest && !self.driven)
    }

    fn latency_frames(&self) -> u32 {
        self.latency
    }

    /// From the table built at activation: a binary search, no allocation.
    fn hosted_param(&self, id: u32) -> Option<HostedParam> {
        self.params
            .binary_search_by_key(&id, |&(id, _)| id)
            .ok()
            .map(|index| self.params[index].1)
    }

    /// A block the host skipped because the plugin was asleep still passes:
    /// CLAP's `steady_time` counts every sample, called or not.
    fn skip_block(&mut self, ctx: &ProcessContext) {
        self.steady_time = self.steady_time.wrapping_add(ctx.frames as u64);
    }

    /// A seek or a stop resets the plugin: CLAP's `reset`, on the audio
    /// thread, which clears what it holds without deactivating it.
    fn on_discontinuity(&mut self, kind: Discontinuity) {
        if kind.invalidates_tails() {
            if let Some(PluginAudioProcessor::Started(processor)) = self.processor.as_mut() {
                processor.reset();
            }
            // A reset plugin holds no voices, so a note-off for one it held
            // is stale from here.
            self.notes.clear();
            self.at_rest = false;
        }
    }

    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        if self.flags.failed.load(Ordering::Relaxed) {
            return;
        }
        let frames = ctx.frames.min(self.max_frames).min(bus.l.len()).min(bus.r.len());
        if frames == 0 {
            return;
        }
        let Some(processor) = self.processor.as_mut() else {
            return;
        };
        let started = match processor.ensure_processing_started() {
            Ok(started) => started,
            Err(_) => {
                self.flags.failed.store(true, Ordering::Relaxed);
                return;
            }
        };

        // The bus is the input: an effect's signal, or, as a channel's
        // source, the silence `HostedSource` cleared it to -- a source has
        // nothing upstream of it.
        let input_peak = peak(&bus.l[..frames]).max(peak(&bus.r[..frames]));
        // Only an effect's main input hears the bus; a source's, and every
        // extra input (a sidechain, a second bus), is silence (MOO-308).
        // They are cleared every block, not once, in case a plugin wrote
        // into a buffer it was only meant to read.
        let (main_at, main_width) = self.main_in;
        let fed = if self.layout.effect { main_width.min(2) } else { 0 };
        for (index, channel) in self.ins.iter_mut().enumerate() {
            if !(main_at..main_at + fed).contains(&index) {
                channel[..frames].fill(0.0);
            }
        }
        if fed == 1 {
            // A mono input hears the sum at -6 dB (MOO-266): a centred
            // signal (L = R) passes at unity and a hard-panned one 6 dB
            // down. The -3 dB sum would raise a centred signal by 3 dB.
            let (left, right) = (&bus.l[..frames], &bus.r[..frames]);
            for ((mono, &l), &r) in self.ins[main_at][..frames].iter_mut().zip(left).zip(right) {
                *mono = (l + r) * 0.5;
            }
        } else if fed == 2 {
            self.ins[main_at][..frames].copy_from_slice(&bus.l[..frames]);
            self.ins[main_at + 1][..frames].copy_from_slice(&bus.r[..frames]);
        }

        // A route that was offsetting a parameter last block and names it no
        // more leaves the plugin's offset where it was, because CLAP's
        // modulation holds until it is changed: set it back to zero first,
        // at the block's first frame, ahead of everything that follows.
        let mut still = [0u32; MODULATED];
        let mut still_count = 0;
        for timed in events_in.iter() {
            if let Event::ParamMod { id, .. } = timed.event {
                if !still[..still_count].contains(&id) && still_count < MODULATED {
                    still[still_count] = id;
                    still_count += 1;
                }
            }
        }
        let mut resets = [0u32; MODULATED];
        let mut reset_count = 0;
        for &id in &self.modulated[..self.modulated_count] {
            if !still[..still_count].contains(&id) {
                resets[reset_count] = id;
                reset_count += 1;
            }
        }
        self.modulated[..still_count].copy_from_slice(&still[..still_count]);
        self.modulated_count = still_count;

        // **The block is cut at every frame a parameter changes on, and at
        // every note** (MOO-85), and each piece is its own process call. CLAP lets a plugin apply its
        // events at their frames, and many read them once a call instead
        // (LSP's do): cut this way, such a plugin hears each value from its
        // own frame, so what it plays no longer depends on the callback's
        // size, and an export at 512 frames matches playback at 64 (MOO-82).
        // The engine's control ticks fall on a grid every block size shares,
        // so the pieces are the same pieces whatever the block. A block with
        // no changes past its first frame is one call, as before.
        self.driven = reset_count > 0
            || events_in
                .iter()
                .any(|timed| matches!(timed.event, Event::ParamValue { .. } | Event::ParamMod { .. }));
        let mut cuts = [0u32; EVENTS_IN + 1];
        let mut cut_count = 1;
        for timed in events_in.iter() {
            // Notes too: a plugin that reads its events once a call would
            // otherwise start a note at the top of whatever block it fell
            // in, and the note's frame would depend on the callback.
            let is_param = matches!(
                timed.event,
                Event::ParamValue { .. }
                    | Event::ParamMod { .. }
                    | Event::NoteOn { .. }
                    | Event::NoteOff { .. }
                    | Event::Choke
            );
            if is_param
                && (timed.offset as usize) < frames
                && timed.offset > cuts[cut_count - 1]
                && cut_count < cuts.len()
            {
                cuts[cut_count] = timed.offset;
                cut_count += 1;
            }
        }
        let ticks_per_frame = if ctx.sample_rate == 0 {
            0.0
        } else {
            ctx.bpm * f64::from(mooloop_core::time::DEFAULT_PPQ) / 60.0 / f64::from(ctx.sample_rate)
        };

        let mut status = ProcessStatus::Continue;
        // Events are in order, so each piece's start where the last ended.
        let mut next_event = 0;
        for piece in 0..cut_count {
            let start = cuts[piece] as usize;
            let end = if piece + 1 < cut_count {
                cuts[piece + 1] as usize
            } else {
                frames
            };
            // In order already (the engine sorts its list), so the buffer is
            // never sorted here: `EventBuffer::sort` allocates.
            self.events.clear();
            let mut pushed = 0;
            if piece == 0 {
                for &id in &resets[..reset_count] {
                    self.events.push(&ParamModEvent::new(0, ClapId::new(id), Pckn::match_all(), 0.0));
                    pushed += 1;
                }
            }
            for timed in events_in.iter().skip(next_event) {
                let offset = timed.offset as usize;
                if offset >= end {
                    break;
                }
                next_event += 1;
                if offset < start || pushed == EVENTS_IN {
                    continue;
                }
                let at = (offset - start) as u32;
                match timed.event {
                    Event::ParamValue { id, value } => self.events.push(&ParamValueEvent::new(
                        at,
                        ClapId::new(id),
                        Pckn::match_all(),
                        f64::from(value),
                    )),
                    Event::ParamMod { id, amount } => self.events.push(&ParamModEvent::new(
                        at,
                        ClapId::new(id),
                        Pckn::match_all(),
                        f64::from(amount),
                    )),
                    event @ (Event::NoteOn { .. } | Event::NoteOff { .. } | Event::Choke) => {
                        pushed += push_note(
                            &mut self.events,
                            &mut self.notes,
                            self.layout.notes,
                            at,
                            event,
                            EVENTS_IN - pushed,
                        );
                        continue;
                    }
                    _ => continue,
                }
                pushed += 1;
            }

            let piece_ctx = ProcessContext {
                frames: end - start,
                position_ticks: ctx.position_ticks + start as f64 * ticks_per_frame,
                position_frames: ctx.position_frames + start as u64,
                ..*ctx
            };
            let transport = Self::transport(&piece_ctx);
            // Every port the plugin declared, in its order, each channel this
            // piece's frames of its buffer. `rest` walks the flat list one
            // port's width at a time; nothing here allocates, because
            // `inputs` and `outputs` were sized for every port. A plugin with
            // no input port gets no input buffers.
            let mut rest: &mut [Box<[f32]>] = &mut self.ins;
            let inputs = self.inputs.with_input_buffers(self.layout.inputs.iter().map(|&width| {
                let (port, tail) = std::mem::take(&mut rest).split_at_mut(width as usize);
                rest = tail;
                AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_input_only(
                        port.iter_mut()
                            .map(move |channel| InputChannel::variable(&mut channel[start..end])),
                    ),
                }
            }));
            let mut rest: &mut [Box<[f32]>] = &mut self.outs;
            let mut outputs = self.outputs.with_output_buffers(self.layout.outputs.iter().map(|&width| {
                let (port, tail) = std::mem::take(&mut rest).split_at_mut(width as usize);
                rest = tail;
                AudioPortBuffer {
                    latency: 0,
                    channels: AudioPortBufferType::f32_output_only(
                        port.iter_mut().map(move |channel| &mut channel[start..end]),
                    ),
                }
            }));
            let mut out = ParamOut {
                ring: &mut self.events_out,
                dropped: &self.flags.dropped_events,
                notes: &mut self.notes,
                generated: &self.flags.generated_notes,
            };
            let events = &self.events;
            let steady_time = self.steady_time;
            let result = catch_unwind(AssertUnwindSafe(|| {
                started.process(
                    &inputs,
                    &mut outputs,
                    &events.as_input(),
                    &mut OutputEvents::from_buffer(&mut out),
                    Some(steady_time),
                    Some(&transport),
                )
            }));
            self.steady_time = self.steady_time.wrapping_add((end - start) as u64);
            status = match result {
                Ok(Ok(status)) => status,
                // The plugin said it failed, or panicked inside Rust code:
                // pass the block through untouched and every block after it.
                Ok(Err(_)) | Err(_) => {
                    self.fail();
                    return;
                }
            };
        }
        // The main output is the bus; a mono one goes to both sides, and
        // every other output is left where the plugin wrote it.
        let (main_at, main_width) = self.main_out;
        let (out_l, out_r) = (
            &self.outs[main_at][..frames],
            &self.outs[main_at + main_width - 1][..frames],
        );
        if out_l.iter().chain(out_r).any(|sample| !sample.is_finite()) {
            self.fail();
            return;
        }
        bus.l[..frames].copy_from_slice(out_l);
        bus.r[..frames].copy_from_slice(out_r);

        if let (Some(tail), Some(PluginAudioProcessor::Started(processor))) =
            (self.tail, self.processor.as_mut())
        {
            let length = tail.get(&processor.plugin_handle());
            self.tail_frames = if length.is_infinite() {
                u32::MAX
            } else {
                length.to_raw()
            };
        }
        let quiet = || input_peak < SILENCE_PEAK && peak(out_l).max(peak(out_r)) < SILENCE_PEAK;
        self.at_rest = match status {
            ProcessStatus::Sleep => true,
            ProcessStatus::ContinueIfNotQuiet | ProcessStatus::Tail => quiet(),
            ProcessStatus::Continue => false,
        };
    }
}

/// Opens a CLAP plugin by its [`PluginRef`] through the scanner's cache
/// (`<config>/plugins.toml`, MOO-80), which it reads again whenever the file
/// has changed: a song opened while the startup scan is still running finds
/// its plugins once the scan writes the cache.
pub struct ClapOpener {
    cache_path: PathBuf,
    cache: PluginCache,
    /// The cache file's modification time when it was last read.
    read_at: Option<SystemTime>,
    last_check: Option<Instant>,
    generation: u64,
}

impl ClapOpener {
    /// How often the cache file is checked for a newer copy.
    const RECHECK: Duration = Duration::from_secs(1);

    pub fn new(cache_path: PathBuf) -> Self {
        let mut opener = Self {
            cache_path,
            cache: PluginCache::default(),
            read_at: None,
            last_check: None,
            generation: 0,
        };
        opener.reload_if_changed();
        opener
    }

    /// An opener over a cache already in memory, for tests and tools that
    /// have no cache file.
    pub fn with_cache(cache: PluginCache) -> Self {
        Self {
            cache_path: PathBuf::new(),
            cache,
            read_at: None,
            last_check: None,
            generation: 1,
        }
    }

    fn reload_if_changed(&mut self) {
        if self.cache_path.as_os_str().is_empty() {
            return;
        }
        let modified = std::fs::metadata(&self.cache_path)
            .and_then(|meta| meta.modified())
            .ok();
        if modified.is_some() && modified != self.read_at {
            self.cache = PluginCache::load(&self.cache_path);
            self.read_at = modified;
            self.generation += 1;
        }
    }
}

impl PluginOpener for ClapOpener {
    fn refresh(&mut self) -> u64 {
        let due = self
            .last_check
            .is_none_or(|checked| checked.elapsed() >= Self::RECHECK);
        if due {
            self.last_check = Some(Instant::now());
            self.reload_if_changed();
        }
        self.generation
    }

    fn open(
        &mut self,
        plugin: &PluginRef,
        state: &PluginState,
        config: AudioConfig,
    ) -> Result<Box<dyn HostedInstance>, HostError> {
        if plugin.format != mooloop_core::PluginFormat::Clap {
            return Err(HostError::Missing);
        }
        let Some(found) = self.cache.resolve(plugin) else {
            return Err(HostError::Missing);
        };
        // Where it may go is the session's call; here it must fit one.
        if let (Some(_), Some(as_source)) = (found.effect_refusal(), found.source_refusal()) {
            return Err(HostError::Incompatible(as_source));
        }
        let path = found.path.clone();
        Ok(Box::new(ClapInstance::open(&path, plugin, state, config)?))
    }
}

/// The largest block the engine will ever hand a processor: what a plugin
/// is activated for. The driver's own buffer is at most this, and the
/// executor never passes more (`mooloop_dsp::MAX_BLOCK_SIZE`).
pub const MAX_FRAMES: u32 = MAX_BLOCK_SIZE as u32;
