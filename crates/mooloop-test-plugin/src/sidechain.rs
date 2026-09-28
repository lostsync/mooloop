//! `mooloop.test.sidechain`: a unity effect with a sidechain input and two
//! extra outputs (MOO-306, MOO-308), and probes that say what reached the
//! sidechain.
//!
//! Its ports are laid out so that the main one is never where a host that
//! assumed port 0 would look, and not at the same index in both directions:
//!
//! | direction | 0 | 1 | 2 |
//! | --- | --- | --- | --- |
//! | inputs | `sidechain`, mono | **`main`**, stereo | |
//! | outputs | `aux`, stereo | `aux mono`, mono | **`main`**, stereo |
//!
//! The main output is the main input, unchanged. Every extra output is
//! filled with [`AUX_LEVEL`], so a host that hands an extra output's buffer
//! on as the main one is heard. A block whose ports are not these, or not
//! these widths, fails.
//!
//! The probes, read through `params.get_value` like the GUI's
//! (`crate::gui::PROBE_IDS`), and never listed as parameters:
//! [`PROBE_SIDECHAIN_BLOCKS`] counts the process calls that delivered the
//! sidechain, [`PROBE_SIDECHAIN_LOUD`] the samples on it that were not zero.

use crate::HostServices;
use clack_extensions::audio_ports::{
    AudioPortFlags, AudioPortInfo, AudioPortInfoWriter, AudioPortType, PluginAudioPorts,
    PluginAudioPortsImpl,
};
use clack_extensions::params::{
    ParamDisplayWriter, ParamInfoWriter, PluginAudioProcessorParams, PluginMainThreadParams,
    PluginParams,
};
use clack_plugin::prelude::*;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering};

/// What the plugin writes into every extra output.
pub const AUX_LEVEL: f32 = 1.0;
/// Process calls that handed the plugin its sidechain port.
pub const PROBE_SIDECHAIN_BLOCKS: u32 = 0xFFFF_0020;
/// Samples on the sidechain that were not zero, over every call.
pub const PROBE_SIDECHAIN_LOUD: u32 = 0xFFFF_0021;

/// Channels of each input port, in the plugin's order.
const INPUTS: [u32; 2] = [1, 2];
/// Channels of each output port, in the plugin's order.
const OUTPUTS: [u32; 3] = [2, 1, 2];
/// Which input and which output are main.
const MAIN_IN: usize = 1;
const MAIN_OUT: usize = 2;
const SIDECHAIN: usize = 0;

pub struct SidechainPlugin;

impl Plugin for SidechainPlugin {
    type AudioProcessor<'a> = SidechainProcessor<'a>;
    type Shared<'a> = SidechainShared<'a>;
    type MainThread<'a> = SidechainMain<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&SidechainShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginParams>();
    }
}

/// The host's services and the probes.
pub struct SidechainShared<'a> {
    services: HostServices<'a>,
    blocks: AtomicU64,
    loud: AtomicU64,
}

impl<'a> SidechainShared<'a> {
    pub(crate) fn new(host: HostSharedHandle<'a>) -> Result<Self, PluginError> {
        Ok(Self {
            services: HostServices::new(host),
            blocks: AtomicU64::new(0),
            loud: AtomicU64::new(0),
        })
    }
}

impl<'a> PluginShared<'a> for SidechainShared<'a> {}

/// The main-thread half.
pub struct SidechainMain<'a> {
    shared: &'a SidechainShared<'a>,
}

impl<'a> SidechainMain<'a> {
    pub(crate) fn new(
        _host: HostMainThreadHandle<'a>,
        shared: &'a SidechainShared<'a>,
    ) -> Result<Self, PluginError> {
        Ok(Self { shared })
    }
}

impl<'a> PluginMainThread<'a, SidechainShared<'a>> for SidechainMain<'a> {}

/// The audio-thread half. The main input is copied here before the main
/// output is written, because a plugin cannot hold an input port and an
/// output port at once; both are sized at activation.
pub struct SidechainProcessor<'a> {
    shared: &'a SidechainShared<'a>,
    main: [Vec<f32>; 2],
}

impl<'a> PluginAudioProcessor<'a, SidechainShared<'a>, SidechainMain<'a>> for SidechainProcessor<'a> {
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        _main_thread: &SidechainMain<'a>,
        shared: &'a SidechainShared<'a>,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        shared
            .services
            .expect_main_thread(c"sidechain: activate called off the main thread");
        let frames = audio_config.max_frames_count as usize;
        Ok(Self {
            shared,
            main: [vec![0.0; frames], vec![0.0; frames]],
        })
    }

    fn process(
        &mut self,
        _process: Process,
        mut audio: Audio,
        _events: Events,
    ) -> Result<ProcessStatus, PluginError> {
        self.shared
            .services
            .expect_audio_thread(c"sidechain: process called off an audio thread");
        let frames = audio.frames_count() as usize;
        if frames > self.main[0].len() {
            return Err(PluginError::Message("sidechain: a block over max_frames"));
        }
        if audio.input_port_count() != INPUTS.len() || audio.output_port_count() != OUTPUTS.len() {
            return Err(PluginError::Message("sidechain: the host did not pass every port"));
        }
        for (index, &channels) in INPUTS.iter().enumerate() {
            if audio.input_port(index).map(|port| port.channel_count()) != Some(channels) {
                return Err(PluginError::Message("sidechain: an input port is the wrong width"));
            }
        }
        for (index, &channels) in OUTPUTS.iter().enumerate() {
            if audio.output_port(index).map(|port| port.channel_count()) != Some(channels) {
                return Err(PluginError::Message("sidechain: an output port is the wrong width"));
            }
        }

        {
            let port = audio
                .input_port(SIDECHAIN)
                .ok_or(PluginError::Message("sidechain: no sidechain"))?;
            let channels = port
                .channels()?
                .into_f32()
                .ok_or(PluginError::Message("sidechain: expected f32 buffers"))?;
            let loud = channels
                .iter()
                .flat_map(|channel| channel.iter())
                .filter(|&&sample| sample != 0.0)
                .count();
            self.shared.blocks.fetch_add(1, Ordering::Relaxed);
            self.shared.loud.fetch_add(loud as u64, Ordering::Relaxed);
        }
        {
            let port = audio
                .input_port(MAIN_IN)
                .ok_or(PluginError::Message("sidechain: no main input"))?;
            let channels = port
                .channels()?
                .into_f32()
                .ok_or(PluginError::Message("sidechain: expected f32 buffers"))?;
            for (index, copy) in self.main.iter_mut().enumerate() {
                let channel = channels
                    .channel(index as u32)
                    .ok_or(PluginError::Message("sidechain: a main input channel is missing"))?;
                copy[..frames].copy_from_slice(&channel[..frames]);
            }
        }
        for index in 0..OUTPUTS.len() {
            let mut port = audio
                .output_port(index)
                .ok_or(PluginError::Message("sidechain: an output is missing"))?;
            let mut channels = port
                .channels()?
                .into_f32()
                .ok_or(PluginError::Message("sidechain: expected f32 buffers"))?;
            for (channel, output) in channels.iter_mut().enumerate() {
                if index == MAIN_OUT {
                    output.copy_from_slice(&self.main[channel][..frames]);
                } else {
                    output.fill(AUX_LEVEL);
                }
            }
        }
        Ok(ProcessStatus::ContinueIfNotQuiet)
    }
}

impl PluginAudioProcessorParams for SidechainProcessor<'_> {
    fn flush(&mut self, _input: &InputEvents, _output: &mut OutputEvents) {}
}

impl PluginAudioPortsImpl for SidechainMain<'_> {
    fn count(&self, is_input: bool) -> u32 {
        if is_input { INPUTS.len() as u32 } else { OUTPUTS.len() as u32 }
    }

    fn get(&self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        let (widths, main, names): (&[u32], usize, &[&[u8]]) = if is_input {
            (&INPUTS, MAIN_IN, &[b"sidechain", b"main"])
        } else {
            (&OUTPUTS, MAIN_OUT, &[b"aux", b"aux mono", b"main"])
        };
        let index = index as usize;
        let Some(&channel_count) = widths.get(index) else {
            return;
        };
        writer.set(&AudioPortInfo {
            id: ClapId::new(index as u32),
            name: names[index],
            channel_count,
            flags: if index == main { AudioPortFlags::IS_MAIN } else { AudioPortFlags::empty() },
            port_type: Some(if channel_count == 1 {
                AudioPortType::MONO
            } else {
                AudioPortType::STEREO
            }),
            in_place_pair: None,
        });
    }
}

/// No parameters: only the probes, which no list names.
impl PluginMainThreadParams for SidechainMain<'_> {
    fn count(&self) -> u32 {
        0
    }

    fn get_info(&self, _index: u32, _info: &mut ParamInfoWriter) {}

    fn get_value(&self, id: ClapId) -> Option<f64> {
        match id.get() {
            PROBE_SIDECHAIN_BLOCKS => Some(self.shared.blocks.load(Ordering::Relaxed) as f64),
            PROBE_SIDECHAIN_LOUD => Some(self.shared.loud.load(Ordering::Relaxed) as f64),
            _ => None,
        }
    }

    fn value_to_text(
        &self,
        _id: ClapId,
        _value: f64,
        _writer: &mut ParamDisplayWriter,
    ) -> std::fmt::Result {
        Err(std::fmt::Error)
    }

    fn text_to_value(&self, _id: ClapId, _text: &CStr) -> Option<f64> {
        None
    }

    fn flush(&self, _input: &InputEvents, _output: &mut OutputEvents) {}
}
