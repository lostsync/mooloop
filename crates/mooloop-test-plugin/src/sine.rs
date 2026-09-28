//! `mooloop.test.sine`: one sine voice per note, with a release tail.
//!
//! A note-on starts a voice at the key's equal-tempered pitch and
//! [`SINE_AMPLITUDE`] times its velocity, on both output channels. A note-off
//! starts a linear release of [`RELEASE_SECONDS`]; when it reaches zero the
//! voice ends and the plugin sends a CLAP `note_end` event at that frame. The
//! `tail` extension reports the release length, and `process` returns
//! `Sleep` once no voice is left, so a host can measure the tail three ways.
//!
//! One parameter, `level` ([`PARAM_LEVEL`]): plain decibels,
//! [`LEVEL_DB_MIN`]..=[`LEVEL_DB_MAX`], at [`LEVEL_DB_DEFAULT`], applied to
//! the sum of the voices. It exists so a host test can drive an instrument's
//! parameter (MOO-314): like the gain's, a change lands on the exact frame
//! of its event with no smoothing, a CLAP `ParamMod` on it is an offset in dB
//! clamped with the value into the range and held until the next one, and
//! `get_value` reports the value without the offset. At the default the
//! voices are multiplied by exactly 1, so a song that never touches it
//! renders what it did before the parameter existed.

use crate::gui::{TestGui, impl_test_gui, register_test_gui};
use crate::{AtomicF64, HostServices};
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfo, ParamInfoFlags, ParamInfoWriter, PluginAudioProcessorParams,
    PluginMainThreadParams, PluginParams,
};
use clack_extensions::tail::{PluginTail, PluginTailImpl, TailLength};
use clack_plugin::events::event_types::NoteEndEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use std::f64::consts::TAU;
use std::ffi::CStr;
use std::fmt::Write as _;

/// Peak amplitude of a voice struck at full velocity.
pub const SINE_AMPLITUDE: f32 = 0.25;
/// Length of a voice's linear release, in seconds.
pub const RELEASE_SECONDS: f64 = 0.05;
/// Voices that can sound at once. A note beyond this is ignored rather than
/// allocating on the audio thread.
const MAX_VOICES: usize = 64;

/// The level parameter's id: plain dB. Sparse, as every id here is.
pub const PARAM_LEVEL: u32 = 50;
/// Lowest level, in dB.
pub const LEVEL_DB_MIN: f64 = -48.0;
/// Highest level, in dB.
pub const LEVEL_DB_MAX: f64 = 12.0;
/// Level at creation, in dB: the voices exactly as they were.
pub const LEVEL_DB_DEFAULT: f64 = 0.0;

/// The sine plugin, with (`GUI = true`) or without the `gui` extension.
pub struct SinePlugin<const GUI: bool>;

impl<const GUI: bool> Plugin for SinePlugin<GUI> {
    type AudioProcessor<'a> = SineProcessor<'a>;
    type Shared<'a> = SineShared<'a>;
    type MainThread<'a> = SineMain<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&SineShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginTail>();
        if GUI {
            register_test_gui!(builder);
        }
    }
}

/// What both threads share: the host's services and the level.
pub struct SineShared<'a> {
    services: HostServices<'a>,
    level_db: AtomicF64,
    /// The host's modulation offset on the level, in dB.
    level_mod_db: AtomicF64,
}

impl<'a> SineShared<'a> {
    pub(crate) fn new(host: HostSharedHandle<'a>) -> Result<Self, PluginError> {
        Ok(Self {
            services: HostServices::new(host),
            level_db: AtomicF64::new(LEVEL_DB_DEFAULT),
            level_mod_db: AtomicF64::new(0.0),
        })
    }

    /// A parameter event, from either thread's queue.
    fn handle_param_event(&self, event: &UnknownEvent) {
        match event.as_core_event() {
            Some(CoreEventSpace::ParamValue(event))
                if event.param_id().map(ClapId::get) == Some(PARAM_LEVEL) && event.value().is_finite() =>
            {
                self.level_db.store(event.value().clamp(LEVEL_DB_MIN, LEVEL_DB_MAX));
            }
            Some(CoreEventSpace::ParamMod(event))
                if event.param_id().map(ClapId::get) == Some(PARAM_LEVEL) && event.amount().is_finite() =>
            {
                self.level_mod_db.store(event.amount());
            }
            _ => {}
        }
    }

    /// What the voices are multiplied by: the value and the host's offset
    /// over it. Exactly 1 at 0 dB.
    fn heard_level(&self) -> f32 {
        let db = (self.level_db.load() + self.level_mod_db.load()).clamp(LEVEL_DB_MIN, LEVEL_DB_MAX);
        if db == 0.0 { 1.0 } else { 10f64.powf(db / 20.0) as f32 }
    }
}

impl<'a> PluginShared<'a> for SineShared<'a> {}

/// The main-thread half.
pub struct SineMain<'a> {
    host: HostMainThreadHandle<'a>,
    shared: &'a SineShared<'a>,
    gui: TestGui,
}

impl<'a> SineMain<'a> {
    pub(crate) fn new(
        host: HostMainThreadHandle<'a>,
        shared: &'a SineShared<'a>,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            host,
            shared,
            gui: TestGui::new(),
        })
    }
}

impl<'a> PluginMainThread<'a, SineShared<'a>> for SineMain<'a> {}

impl_test_gui!(SineMain);

struct Voice {
    /// The note-on's port, channel, key and note id, for matching its
    /// note-off and for its `note_end`.
    pckn: Pckn,
    phase: f64,
    phase_step: f64,
    amplitude: f32,
    /// `None` while the note is held; frames of release left after.
    release_left: Option<u32>,
}

/// The audio-thread half.
pub struct SineProcessor<'a> {
    shared: &'a SineShared<'a>,
    voices: Vec<Voice>,
    sample_rate: f64,
    release_frames: u32,
}

impl<'a> PluginAudioProcessor<'a, SineShared<'a>, SineMain<'a>> for SineProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &SineMain<'a>,
        shared: &'a SineShared<'a>,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        shared
            .services
            .expect_main_thread(c"sine: activate called off the main thread");
        Ok(Self {
            shared,
            voices: Vec::with_capacity(MAX_VOICES),
            sample_rate: audio_config.sample_rate,
            release_frames: (RELEASE_SECONDS * audio_config.sample_rate).ceil() as u32,
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        self.shared
            .services
            .expect_audio_thread(c"sine: process called off an audio thread");
        self.shared.services.strict_process();

        let mut port = audio
            .output_port(0)
            .ok_or(PluginError::Message("sine: no output port"))?;
        let mut channels = port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("sine: expected f32 buffers"))?;
        for channel in channels.iter_mut() {
            channel.fill(0.0);
        }
        let frames = channels.frames_count() as usize;

        let mut frame = 0;
        for batch in events.input.batch() {
            for event in batch.events() {
                self.handle_event(event);
                self.shared.handle_param_event(event);
            }
            let level = self.shared.heard_level();
            let end = batch.next_batch_first_sample().unwrap_or(frames).min(frames);
            // Sample-major, so the `note_end` events come out in time order,
            // which CLAP requires of a plugin's output queue.
            while frame < end {
                let mut sum = 0.0;
                let mut index = 0;
                while index < self.voices.len() {
                    let (sample, ended) = self.voices[index].next(self.release_frames);
                    sum += sample;
                    if ended {
                        let voice = self.voices.swap_remove(index);
                        let _ = events
                            .output
                            .try_push(NoteEndEvent::new(frame as u32, voice.pckn));
                    } else {
                        index += 1;
                    }
                }
                for channel in channels.iter_mut() {
                    channel[frame] = sum * level;
                }
                frame += 1;
            }
        }

        Ok(if self.voices.is_empty() {
            ProcessStatus::Sleep
        } else {
            ProcessStatus::Continue
        })
    }

    fn start_processing(&mut self) -> Result<(), PluginError> {
        self.shared.services.strict_start("sine");
        Ok(())
    }

    fn stop_processing(&mut self) {
        self.shared.services.strict_stop("sine");
        self.voices.clear();
    }

    fn reset(&mut self) {
        self.voices.clear();
    }
}

impl SineProcessor<'_> {
    fn handle_event(&mut self, event: &UnknownEvent) {
        match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(note)) => {
                let Some(key) = note.pckn().key.into_specific() else {
                    return;
                };
                if self.voices.len() == MAX_VOICES {
                    return;
                }
                let hz = 440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0);
                self.voices.push(Voice {
                    pckn: note.pckn(),
                    phase: 0.0,
                    phase_step: TAU * hz / self.sample_rate,
                    amplitude: SINE_AMPLITUDE * note.velocity().clamp(0.0, 1.0) as f32,
                    release_left: None,
                });
            }
            Some(CoreEventSpace::NoteOff(note)) => {
                let off = note.pckn();
                for voice in &mut self.voices {
                    if voice.release_left.is_none() && off.matches(&voice.pckn) {
                        voice.release_left = Some(self.release_frames);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Voice {
    /// The next sample, and whether the voice has just finished.
    fn next(&mut self, release_frames: u32) -> (f32, bool) {
        let envelope = match self.release_left {
            None => 1.0,
            Some(0) => return (0.0, true),
            Some(left) => {
                self.release_left = Some(left - 1);
                left as f32 / release_frames.max(1) as f32
            }
        };
        let sample = self.phase.sin() as f32 * self.amplitude * envelope;
        self.phase = (self.phase + self.phase_step) % TAU;
        (sample, false)
    }
}

impl PluginAudioProcessorParams for SineProcessor<'_> {
    fn flush(&mut self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.handle_param_event(event);
        }
    }
}

impl PluginMainThreadParams for SineMain<'_> {
    fn count(&self) -> u32 {
        1
    }

    fn get_info(&self, index: u32, info: &mut ParamInfoWriter) {
        self.shared
            .services
            .expect_main_thread(c"sine: params.get_info called off the main thread");
        if index != 0 {
            return;
        }
        info.set(&ParamInfo {
            id: ClapId::new(PARAM_LEVEL),
            flags: ParamInfoFlags::IS_AUTOMATABLE | ParamInfoFlags::IS_MODULATABLE,
            cookie: Default::default(),
            name: b"Level",
            module: b"",
            min_value: LEVEL_DB_MIN,
            max_value: LEVEL_DB_MAX,
            default_value: LEVEL_DB_DEFAULT,
        });
    }

    /// The level, or a GUI probe's value (`crate::gui::PROBE_IDS`).
    fn get_value(&self, id: ClapId) -> Option<f64> {
        if id.get() == PARAM_LEVEL {
            return Some(self.shared.level_db.load());
        }
        self.gui.probe(id.get())
    }

    fn value_to_text(
        &self,
        id: ClapId,
        value: f64,
        writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        match id.get() {
            PARAM_LEVEL => write!(writer, "{value:.1} dB"),
            _ => Err(std::fmt::Error),
        }
    }

    fn text_to_value(&self, id: ClapId, text: &CStr) -> Option<f64> {
        let text = text.to_str().ok()?.trim();
        match id.get() {
            PARAM_LEVEL => text.trim_end_matches("dB").trim().parse().ok(),
            _ => None,
        }
    }

    fn flush(&self, input: &InputEvents, _output: &mut OutputEvents) {
        for event in input {
            self.shared.handle_param_event(event);
        }
    }
}

impl PluginTailImpl for SineProcessor<'_> {
    fn get(&self) -> TailLength {
        TailLength::Finite(self.release_frames)
    }
}

impl PluginAudioPortsImpl for SineMain<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { 0 } else { 1 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        if !is_input && index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(0),
                name: b"main",
                channel_count: 2,
                flags: AudioPortFlags::IS_MAIN,
                port_type: Some(AudioPortType::STEREO),
                in_place_pair: None,
            });
        }
    }
}

impl PluginNotePortsImpl for SineMain<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { 1 } else { 0 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        self.shared
            .services
            .strict_note_port("sine", index, PluginNotePortsImpl::count(self, is_input));
        if is_input && index == 0 {
            writer.set(&NotePortInfo {
                id: ClapId::new(0),
                name: b"notes",
                preferred_dialect: Some(NoteDialect::Clap),
                supported_dialects: NoteDialects::CLAP,
            });
        }
    }
}
