//! The realtime audio graph and its JACK `ProcessHandler` entry point.
//!
//! All state here is touched only by the JACK realtime thread. Communication
//! with the outside world is exclusively via the two lock-free queues owned by
//! `Graph` (`cmd_rx` in, `evt_tx` out) and the shared sample slot.

use jack::ProcessHandler;
use jack::{AudioOut, Client, Control, Port, ProcessScope};
use mooloop_core::{EngineCommand, EngineEvent, Pattern, SamplerParams};
use mooloop_dsp::{Device, ProcessContext, SampleData, Sampler};
use rtrb::Consumer;

use crate::sequencer::Sequencer;
use crate::transport::Transport;
use std::sync::Arc;

pub(crate) struct Graph {
    transport: Transport,
    sequencer: Sequencer,
    devices: Vec<Sampler>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    cmd_rx: Consumer<EngineCommand>,
    evt_tx: rtrb::Producer<EngineEvent>,
    sample_rate: u32,
}

impl Graph {
    pub(crate) fn new(
        sample_rate: u32,
        out_l: Port<AudioOut>,
        out_r: Port<AudioOut>,
        cmd_rx: Consumer<EngineCommand>,
        evt_tx: rtrb::Producer<EngineEvent>,
        sample_slot: Arc<arc_swap::ArcSwapOption<SampleData>>,
        initial_params: SamplerParams,
    ) -> Self {
        let num_channels = 1;
        let sequencer = Sequencer::new(Pattern::new(num_channels), mooloop_core::Ppq::DEFAULT);
        let devices = (0..num_channels)
            .map(|_| Sampler::new(sample_slot.clone(), initial_params, sample_rate))
            .collect();
        Self {
            transport: Transport::new(sample_rate),
            sequencer,
            devices,
            out_l,
            out_r,
            cmd_rx,
            evt_tx,
            sample_rate,
        }
    }

    fn apply_command(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play => self.transport.play(),
            EngineCommand::Pause => self.transport.pause(),
            EngineCommand::Stop => self.transport.stop(),
            EngineCommand::SetTempo(bpm) => self.transport.set_tempo(bpm),
            EngineCommand::SetStep {
                channel,
                step,
                on,
                velocity,
            } => {
                self.sequencer
                    .set_step(channel as usize, step as usize, on, velocity);
            }
            EngineCommand::SetSamplerParams(params) => {
                if let Some(dev) = self.devices.first_mut() {
                    dev.set_params(params);
                }
            }
        }
    }
}

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        let frames = scope.n_frames() as usize;

        // 1. Drain commands first — disjoint from the port buffer borrows.
        while let Ok(cmd) = self.cmd_rx.pop() {
            self.apply_command(cmd);
        }

        // 2. Advance transport; capture the tick interval for the sequencer.
        let tps = self.transport.ticks_per_sample();
        let (start_tick, end_tick) = self.transport.advance(frames);

        // 3. Borrow output buffers and zero them.
        let buf_l = self.out_l.as_mut_slice(scope);
        let buf_r = self.out_r.as_mut_slice(scope);
        for s in buf_l.iter_mut() {
            *s = 0.0;
        }
        for s in buf_r.iter_mut() {
            *s = 0.0;
        }

        // 4. Schedule note-ons for any step boundaries inside this block.
        if self.transport.playing {
            self.sequencer
                .schedule(start_tick, end_tick, frames, tps, &mut self.devices);
        }

        // 5. Render devices (one sampler for Phase 1).
        let ctx = ProcessContext {
            sample_rate: self.sample_rate,
            frames,
        };
        for dev in self.devices.iter_mut() {
            dev.process(ctx, buf_l, buf_r);
        }

        // 6. Meter + push transport position.
        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        for i in 0..frames {
            peak_l = peak_l.max(buf_l[i].abs());
            peak_r = peak_r.max(buf_r[i].abs());
        }
        let _ = self.evt_tx.push(EngineEvent::Position {
            tick: self.transport.position_ticks as u64,
            beat_in_bar: self.transport.beat_in_bar(),
            playing: self.transport.playing,
        });
        let _ = self.evt_tx.push(EngineEvent::Metering {
            peak_l,
            peak_r,
        });

        Control::Continue
    }
}

/// JACK notifications handler. No-op for Phase 1.
pub(crate) struct Notifications;

impl jack::NotificationHandler for Notifications {}

pub(crate) type AsyncClient = jack::AsyncClient<Notifications, Graph>;
