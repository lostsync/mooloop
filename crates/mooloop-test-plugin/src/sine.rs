//! `mooloop.test.sine`: one sine voice per note, with a release tail.
//!
//! A note-on starts a voice at the key's equal-tempered pitch and
//! [`SINE_AMPLITUDE`] times its velocity, on both output channels. A note-off
//! starts a linear release of [`RELEASE_SECONDS`]; when it reaches zero the
//! voice ends and the plugin sends a CLAP `note_end` event at that frame. The
//! `tail` extension reports the release length, and `process` returns
//! `Sleep` once no voice is left, so a host can measure the tail three ways.

use crate::HostServices;
use crate::gui::{TestGui, impl_test_gui};
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::gui::PluginGui;
use clack_extensions::note_ports::{
    NoteDialect, NoteDialects, NotePortInfo, NotePortInfoWriter, PluginNotePorts,
    PluginNotePortsImpl,
};
use clack_extensions::tail::{PluginTail, PluginTailImpl, TailLength};
use clack_plugin::events::event_types::NoteEndEvent;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use std::f64::consts::TAU;

/// Peak amplitude of a voice struck at full velocity.
pub const SINE_AMPLITUDE: f32 = 0.25;
/// Length of a voice's linear release, in seconds.
pub const RELEASE_SECONDS: f64 = 0.05;
/// Voices that can sound at once. A note beyond this is ignored rather than
/// allocating on the audio thread.
const MAX_VOICES: usize = 64;

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
            .register::<PluginTail>();
        if GUI {
            builder.register::<PluginGui>();
        }
    }
}

/// What both threads share: only the host's services.
pub struct SineShared<'a> {
    services: HostServices<'a>,
}

impl<'a> SineShared<'a> {
    pub(crate) fn new(host: HostSharedHandle<'a>) -> Result<Self, PluginError> {
        Ok(Self {
            services: HostServices::new(host),
        })
    }
}

impl<'a> PluginShared<'a> for SineShared<'a> {}

/// The main-thread half.
pub struct SineMain<'a> {
    _shared: &'a SineShared<'a>,
    gui: TestGui,
}

impl<'a> SineMain<'a> {
    pub(crate) fn new(
        _host: HostMainThreadHandle<'a>,
        shared: &'a SineShared<'a>,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            _shared: shared,
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
            }
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
                    channel[frame] = sum;
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

    fn stop_processing(&mut self) {
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
