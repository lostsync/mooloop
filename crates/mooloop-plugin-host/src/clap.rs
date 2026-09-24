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
//! [`ClapProcessor::process`] allocates, locks or blocks: the input and
//! output buffers, the event buffer and the ring for the plugin's own
//! parameter output are all sized at activation, and events arrive in
//! order, so the event buffer is never sorted. What the plugin itself does
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

use std::ffi::CString;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::{Duration, Instant, SystemTime};

use clack_extensions::audio_ports::{AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::latency::{HostLatency, HostLatencyImpl, PluginLatency};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamInfoBuffer,
    ParamInfoFlags, ParamRescanFlags, PluginParams,
};
use clack_extensions::state::{HostState, HostStateImpl, PluginState as ClapStateExt};
use clack_extensions::tail::PluginTail;
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_host::events::event_types::{ParamModEvent, ParamValueEvent, TransportEvent, TransportFlags};
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::EventFlags;
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};
use mooloop_core::{PluginParamInfo, PluginRef, PluginState, PluginStateChunk};
use mooloop_dsp::node::{Discontinuity, SILENCE_PEAK};
use mooloop_dsp::{AudioNode, Event, EventList, HostedParam, ProcessContext, StereoBus, MAX_BLOCK_SIZE};

pub use crate::instance::PluginParamEvent;
use crate::instance::{AudioConfig, HostError, HostedInstance, Lifeline, PluginOpener};
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
            .register::<HostParams>();
    }
}

/// What a plugin may reach from any thread. Every callback only raises bits
/// or bumps a counter: nothing locks and nothing allocates, because any of
/// them may arrive on the audio thread.
pub struct ClapShared {
    main_thread: ThreadId,
    requests: Arc<RequestFlags>,
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

/// What a plugin may reach only from the main thread.
pub struct ClapMainThread {
    requests: Arc<RequestFlags>,
}

impl<'a> MainThreadHandler<'a> for ClapMainThread {}

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
    instance: PluginInstance<ClapHost>,
}

impl ClapInstance {
    /// Load `plugin` from the library at `path`, create it, and load `state`
    /// into it. It is not activated until [`HostedInstance::build_processor`].
    ///
    /// Refused as [`HostError::Incompatible`] unless its audio ports are one
    /// stereo input and one stereo output. Sidechains and other layouts are
    /// in the plan's "Deliberately not" list.
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
        let mut instance = PluginInstance::<ClapHost>::new(
            move |_| ClapShared {
                main_thread: std::thread::current().id(),
                requests: shared_requests,
                unlogged: AtomicU64::new(0),
                misbehaviour: AtomicU64::new(0),
            },
            move |_| ClapMainThread {
                requests: main_requests,
            },
            &entry,
            &id,
            &crate::host_info(),
        )
        .map_err(|error| HostError::Plugin(format!("{} could not be created: {error}", plugin.id)))?;
        check_ports(&mut instance)?;
        let mut hosted = Self {
            plugin: plugin.clone(),
            params: Vec::new(),
            latency: 0,
            config,
            requests,
            flags: Arc::new(ProcessorFlags::default()),
            events_out: None,
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

/// Refuse any layout but one stereo input and one stereo output.
fn check_ports(instance: &mut PluginInstance<ClapHost>) -> Result<(), HostError> {
    let Some(ports) = instance
        .plugin_shared_handle()
        .get_extension::<PluginAudioPorts>()
    else {
        return Err(HostError::Incompatible("it declares no audio ports".into()));
    };
    let handle = instance.plugin_handle();
    for is_input in [true, false] {
        let side = if is_input { "input" } else { "output" };
        let count = ports.count(&handle, is_input);
        if count != 1 {
            return Err(HostError::Incompatible(format!(
                "it has {count} audio {side} ports; only one stereo {side} is hosted"
            )));
        }
        let mut buffer = AudioPortInfoBuffer::new();
        let channels = ports
            .get(&handle, 0, is_input, &mut buffer)
            .map(|info| info.channel_count)
            .unwrap_or(0);
        if channels != 2 {
            return Err(HostError::Incompatible(format!(
                "its main {side} has {channels} channels, not 2"
            )));
        }
    }
    Ok(())
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        // The rack drops an instance only once its processors are gone, so
        // this succeeds; if it cannot, `clack-host` leaks the plugin rather
        // than destroy it under a running processor.
        let _ = self.deactivate();
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

    fn set_audio_config(&mut self, config: AudioConfig) {
        self.config = config;
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
        Ok(Box::new(ClapProcessor {
            params: params.into_boxed_slice(),
            modulated: [0; MODULATED],
            modulated_count: 0,
            driven: false,
            processor: Some(PluginAudioProcessor::from(stopped)),
            tail,
            tail_frames: 0,
            latency: self.latency,
            in_l: vec![0.0; frames],
            in_r: vec![0.0; frames],
            out_l: vec![0.0; frames],
            out_r: vec![0.0; frames],
            inputs: AudioPorts::with_capacity(2, 1),
            outputs: AudioPorts::with_capacity(2, 1),
            events: EventBuffer::with_capacity(EVENTS_IN),
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
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
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
            // Notes and everything else an effect might emit are not routed
            // anywhere (the plan's "Deliberately not").
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
        let frames = ctx.frames.min(self.in_l.len()).min(bus.l.len()).min(bus.r.len());
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

        self.in_l[..frames].copy_from_slice(&bus.l[..frames]);
        self.in_r[..frames].copy_from_slice(&bus.r[..frames]);
        let input_peak = peak(&self.in_l[..frames]).max(peak(&self.in_r[..frames]));

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

        // **The block is cut at every frame a parameter changes on**, and
        // each piece is its own process call. CLAP lets a plugin apply its
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
            let is_param = matches!(timed.event, Event::ParamValue { .. } | Event::ParamMod { .. });
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
            let (in_l, in_r) = (&mut self.in_l[start..end], &mut self.in_r[start..end]);
            let (out_l, out_r) = (&mut self.out_l[start..end], &mut self.out_r[start..end]);
            let inputs = self.inputs.with_input_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_input_only(
                    [InputChannel::variable(in_l), InputChannel::variable(in_r)].into_iter(),
                ),
            }]);
            let mut outputs = self.outputs.with_output_buffers([AudioPortBuffer {
                latency: 0,
                channels: AudioPortBufferType::f32_output_only([out_l, out_r].into_iter()),
            }]);
            let mut out = ParamOut {
                ring: &mut self.events_out,
                dropped: &self.flags.dropped_events,
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
        let (out_l, out_r) = (&self.out_l[..frames], &self.out_r[..frames]);
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
        if found.audio_inputs != [2] || found.audio_outputs != [2] {
            return Err(HostError::Incompatible(format!(
                "its audio ports are {:?} in and {:?} out; only one stereo input and one stereo output are hosted",
                found.audio_inputs, found.audio_outputs
            )));
        }
        let path = found.path.clone();
        Ok(Box::new(ClapInstance::open(&path, plugin, state, config)?))
    }
}

/// The largest block the engine will ever hand a processor: what a plugin
/// is activated for. The driver's own buffer is at most this, and the
/// executor never passes more (`mooloop_dsp::MAX_BLOCK_SIZE`).
pub const MAX_FRAMES: u32 = MAX_BLOCK_SIZE as u32;
