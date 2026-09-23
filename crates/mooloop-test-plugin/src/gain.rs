//! `mooloop.test.gain`: a stereo gain with a latency switch and a way to fail.
//!
//! - `gain` ([`PARAM_GAIN`]): plain decibels, [`GAIN_DB_MIN`]..=[`GAIN_DB_MAX`].
//!   A change lands on the exact frame of its event; there is no smoothing,
//!   so a host test can see the frame.
//! - `latency` ([`PARAM_LATENCY`]): stepped, an index into [`LATENCY_STEPS`].
//!   The delay is real (the output is the input that many frames late), it
//!   is fixed at activation, and changing the parameter while active asks the
//!   host for a restart, as CLAP requires.
//! - `fail` ([`PARAM_FAIL`]): stepped off/on. While on, `process` returns an
//!   error.

use crate::gui::{TestGui, impl_test_gui};
use crate::{AtomicF64, HostServices};
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::PluginGui;
use clack_extensions::latency::{HostLatency, PluginLatency, PluginLatencyImpl};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::state::{PluginState, PluginStateImpl};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The gain parameter's id: plain dB.
pub const PARAM_GAIN: u32 = 10;
/// The latency parameter's id: an index into [`LATENCY_STEPS`].
pub const PARAM_LATENCY: u32 = 20;
/// The fail parameter's id: 0 off, 1 on.
pub const PARAM_FAIL: u32 = 30;

/// Lowest gain, in dB.
pub const GAIN_DB_MIN: f64 = -60.0;
/// Highest gain, in dB.
pub const GAIN_DB_MAX: f64 = 12.0;
/// Gain at creation, in dB.
pub const GAIN_DB_DEFAULT: f64 = 0.0;

/// The latencies, in frames, that the `latency` parameter selects between.
pub const LATENCY_STEPS: [u32; 3] = [0, 64, 512];

/// The first four bytes of a saved state; the fourth is the format version.
pub const STATE_MAGIC: [u8; 4] = *b"MTG\x01";

/// The gain plugin, with (`GUI = true`) or without the `gui` extension.
pub struct GainPlugin<const GUI: bool>;

impl<const GUI: bool> Plugin for GainPlugin<GUI> {
    type AudioProcessor<'a> = GainProcessor<'a>;
    type Shared<'a> = GainShared<'a>;
    type MainThread<'a> = GainMain<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&GainShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginParams>()
            .register::<PluginState>()
            .register::<PluginLatency>();
        if GUI {
            builder.register::<PluginGui>();
        }
    }
}

/// Parameter values, shared by the main thread and the audio thread.
pub struct GainShared<'a> {
    services: HostServices<'a>,
    gain_db: AtomicF64,
    latency_step: AtomicU32,
    fail: AtomicBool,
    /// Whether an audio processor exists; a latency change only needs a
    /// restart while one does.
    active: AtomicBool,
    /// The latency step the running processor was built with.
    active_latency_step: AtomicU32,
}

impl<'a> GainShared<'a> {
    pub(crate) fn new(host: HostSharedHandle<'a>) -> Result<Self, PluginError> {
        Ok(Self {
            services: HostServices::new(host),
            gain_db: AtomicF64::new(GAIN_DB_DEFAULT),
            latency_step: AtomicU32::new(0),
            fail: AtomicBool::new(false),
            active: AtomicBool::new(false),
            active_latency_step: AtomicU32::new(0),
        })
    }

    fn set_param(&self, id: u32, value: f64) {
        if !value.is_finite() {
            return;
        }
        match id {
            PARAM_GAIN => self.gain_db.store(value.clamp(GAIN_DB_MIN, GAIN_DB_MAX)),
            PARAM_LATENCY => self.set_latency_step(value.round() as i64),
            PARAM_FAIL => self.fail.store(value >= 0.5, Ordering::Relaxed),
            _ => {}
        }
    }

    fn set_latency_step(&self, step: i64) {
        let step = step.clamp(0, LATENCY_STEPS.len() as i64 - 1) as u32;
        self.latency_step.store(step, Ordering::Relaxed);
        if self.active.load(Ordering::Relaxed)
            && step != self.active_latency_step.load(Ordering::Relaxed)
        {
            self.services.host.request_restart();
        }
    }

    fn param(&self, id: u32) -> Option<f64> {
        match id {
            PARAM_GAIN => Some(self.gain_db.load()),
            PARAM_LATENCY => Some(f64::from(self.latency_step.load(Ordering::Relaxed))),
            PARAM_FAIL => Some(if self.fail.load(Ordering::Relaxed) { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    fn handle_event(&self, event: &UnknownEvent) {
        if let Some(CoreEventSpace::ParamValue(event)) = event.as_core_event() {
            if let Some(id) = event.param_id() {
                self.set_param(id.get(), event.value());
            }
        }
    }
}

impl<'a> PluginShared<'a> for GainShared<'a> {}

/// The main-thread half.
pub struct GainMain<'a> {
    host: HostMainThreadHandle<'a>,
    shared: &'a GainShared<'a>,
    host_latency: Option<HostLatency>,
    gui: TestGui,
}

impl<'a> GainMain<'a> {
    pub(crate) fn new(
        host: HostMainThreadHandle<'a>,
        shared: &'a GainShared<'a>,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            host_latency: host.get_extension(),
            host,
            shared,
            gui: TestGui::new(),
        })
    }
}

impl<'a> PluginMainThread<'a, GainShared<'a>> for GainMain<'a> {}

impl_test_gui!(GainMain);

/// The audio-thread half.
pub struct GainProcessor<'a> {
    shared: &'a GainShared<'a>,
    delays: [DelayLine; 2],
}

impl<'a> PluginAudioProcessor<'a, GainShared<'a>, GainMain<'a>> for GainProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        main_thread: &GainMain<'a>,
        shared: &'a GainShared<'a>,
        _audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        shared
            .services
            .expect_main_thread(c"gain: activate called off the main thread");
        let step = shared.latency_step.load(Ordering::Relaxed);
        let previous = shared.active_latency_step.swap(step, Ordering::Relaxed);
        shared.active.store(true, Ordering::Relaxed);
        // CLAP: the latency may only change during activate, and the plugin
        // says so from inside it.
        if previous != step {
            if let Some(latency) = main_thread.host_latency {
                latency.changed(&main_thread.host);
            }
        }
        let frames = LATENCY_STEPS[step as usize] as usize;
        Ok(Self {
            shared,
            delays: [DelayLine::new(frames), DelayLine::new(frames)],
        })
    }

    fn deactivate(self, _main_thread: &GainMain<'a>) {
        self.shared.active.store(false, Ordering::Relaxed);
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        self.shared
            .services
            .expect_audio_thread(c"gain: process called off an audio thread");

        let mut port = audio
            .port_pair(0)
            .ok_or(PluginError::Message("gain: no main port pair"))?;
        let mut channels = port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("gain: expected f32 buffers"))?;

        let mut buffers: [Option<&mut [f32]>; 2] = [None, None];
        for (pair, slot) in channels.iter_mut().zip(buffers.iter_mut()) {
            *slot = match pair {
                ChannelPair::InPlace(buffer) => Some(buffer),
                ChannelPair::InputOutput(input, output) => {
                    output.copy_from_slice(input);
                    Some(output)
                }
                ChannelPair::InputOnly(_) | ChannelPair::OutputOnly(_) => None,
            };
        }

        for batch in events.input.batch() {
            for event in batch.events() {
                self.shared.handle_event(event);
            }
            if self.shared.fail.load(Ordering::Relaxed) {
                return Err(PluginError::Message("gain: the fail parameter is on"));
            }
            let gain = db_to_gain(self.shared.gain_db.load()) as f32;
            for buffer in buffers.iter_mut().flatten() {
                for sample in &mut buffer[batch.sample_bounds()] {
                    *sample *= gain;
                }
            }
        }

        for (buffer, delay) in buffers.iter_mut().zip(self.delays.iter_mut()) {
            if let Some(buffer) = buffer {
                delay.process(buffer);
            }
        }

        Ok(ProcessStatus::ContinueIfNotQuiet)
    }
}

impl PluginAudioProcessorParams for GainProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.handle_event(event);
        }
    }
}

impl PluginAudioPortsImpl for GainMain<'_> {
    fn count(&self, _is_input: bool) -> u32 {
        1
    }

    fn get(&self, index: u32, _is_input: bool, writer: &mut AudioPortInfoWriter) {
        if index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: Some(ClapId::new(0)),
            });
        }
    }
}

impl PluginLatencyImpl for GainMain<'_> {
    fn get(&self) -> u32 {
        LATENCY_STEPS[self.shared.active_latency_step.load(Ordering::Relaxed) as usize]
    }
}

impl PluginMainThreadParams for GainMain<'_> {
    fn count(&self) -> u32 {
        3
    }

    fn get_info(&self, index: u32, info: &mut ParamInfoWriter) {
        self.shared
            .services
            .expect_main_thread(c"gain: params.get_info called off the main thread");
        let stepped = ParamInfoFlags::IS_AUTOMATABLE | ParamInfoFlags::IS_STEPPED;
        let (id, flags, name, min, max, default): (u32, _, &[u8], f64, f64, f64) = match index {
            0 => (
                PARAM_GAIN,
                ParamInfoFlags::IS_AUTOMATABLE | ParamInfoFlags::IS_MODULATABLE,
                b"Gain",
                GAIN_DB_MIN,
                GAIN_DB_MAX,
                GAIN_DB_DEFAULT,
            ),
            1 => (
                PARAM_LATENCY,
                stepped,
                b"Latency",
                0.0,
                (LATENCY_STEPS.len() - 1) as f64,
                0.0,
            ),
            2 => (PARAM_FAIL, stepped, b"Fail", 0.0, 1.0, 0.0),
            _ => return,
        };
        info.set(&ParamInfo {
            id: ClapId::new(id),
            flags,
            cookie: Default::default(),
            name,
            module: b"",
            min_value: min,
            max_value: max,
            default_value: default,
        });
    }

    fn get_value(&self, id: ClapId) -> Option<f64> {
        self.shared.param(id.get())
    }

    fn value_to_text(
        &self,
        id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        match id.get() {
            PARAM_GAIN => write!(writer, "{value:.1} dB"),
            PARAM_LATENCY => {
                let step = (value.round().max(0.0) as usize).min(LATENCY_STEPS.len() - 1);
                write!(writer, "{} frames", LATENCY_STEPS[step])
            }
            PARAM_FAIL => writer.write_str(if value >= 0.5 { "on" } else { "off" }),
            _ => Err(std::fmt::Error),
        }
    }

    fn text_to_value(&self, id: ClapId, text: &CStr) -> Option<f64> {
        let text = text.to_str().ok()?.trim();
        match id.get() {
            PARAM_GAIN => text.trim_end_matches("dB").trim().parse().ok(),
            PARAM_LATENCY => {
                let frames: u32 = text.trim_end_matches("frames").trim().parse().ok()?;
                LATENCY_STEPS
                    .iter()
                    .position(|&f| f == frames)
                    .map(|step| step as f64)
            }
            PARAM_FAIL => match text {
                "on" => Some(1.0),
                "off" => Some(0.0),
                _ => None,
            },
            _ => None,
        }
    }

    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.handle_event(event);
        }
    }
}

impl PluginStateImpl for GainMain<'_> {
    fn save(&self, output: &mut OutputStream) -> Result<(), PluginError> {
        self.shared
            .services
            .expect_main_thread(c"gain: state.save called off the main thread");
        output.write_all(&STATE_MAGIC)?;
        output.write_all(&self.shared.gain_db.load().to_le_bytes())?;
        output.write_all(&self.shared.latency_step.load(Ordering::Relaxed).to_le_bytes())?;
        output.write_all(&[u8::from(self.shared.fail.load(Ordering::Relaxed))])?;
        Ok(())
    }

    fn load(&self, input: &mut InputStream) -> Result<(), PluginError> {
        self.shared
            .services
            .expect_main_thread(c"gain: state.load called off the main thread");
        let mut magic = [0; 4];
        input.read_exact(&mut magic)?;
        if magic != STATE_MAGIC {
            return Err(PluginError::Message("gain: not a test-gain state"));
        }
        let mut gain = [0; 8];
        input.read_exact(&mut gain)?;
        let mut step = [0; 4];
        input.read_exact(&mut step)?;
        let mut fail = [0; 1];
        input.read_exact(&mut fail)?;
        self.shared
            .set_param(PARAM_GAIN, f64::from_le_bytes(gain));
        self.shared
            .set_param(PARAM_LATENCY, f64::from(u32::from_le_bytes(step)));
        self.shared.set_param(PARAM_FAIL, f64::from(fail[0]));
        Ok(())
    }
}

fn db_to_gain(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// A fixed delay, sized at activation. Zero frames is a pass-through.
struct DelayLine {
    ring: Vec<f32>,
    position: usize,
}

impl DelayLine {
    fn new(frames: usize) -> Self {
        Self {
            ring: vec![0.0; frames],
            position: 0,
        }
    }

    fn process(&mut self, buffer: &mut [f32]) {
        if self.ring.is_empty() {
            return;
        }
        for sample in buffer {
            std::mem::swap(sample, &mut self.ring[self.position]);
            self.position = (self.position + 1) % self.ring.len();
        }
    }
}
