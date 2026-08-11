//! The realtime audio graph and its JACK `ProcessHandler` entry point.
//!
//! All state here is touched only by the JACK realtime thread. Communication
//! with the outside world is exclusively via the two lock-free queues owned by
//! `Graph` (`cmd_rx` in, `evt_tx` out) and the immutable `sample_rate`.

use jack::{AudioOut, Client, Control, Port, ProcessScope};
use jack::ProcessHandler;
use mooloop_core::{EngineCommand, EngineEvent};
use mooloop_dsp::{Device, Metronome, ProcessContext};
use rtrb::Consumer;

use crate::transport::Transport;

pub(crate) struct Graph {
    transport: Transport,
    metronome: Metronome,
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
    ) -> Self {
        Self {
            transport: Transport::new(sample_rate),
            metronome: Metronome::new(sample_rate),
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
            EngineCommand::SetMetronomeVolume(v) => self.metronome.set_volume(v),
        }
    }
}

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        let frames = scope.n_frames() as usize;

        // 1. Drain any commands queued by the UI since the last block. Done
        //    before touching the port buffers so the disjoint field borrows
        //    below don't conflict with the `&mut self` apply_command needs.
        while let Ok(cmd) = self.cmd_rx.pop() {
            self.apply_command(cmd);
        }

        // 2. Advance transport; this schedules metronome clicks for any beat
        //    boundaries falling inside this block. Only touches transport and
        //    metronome, not the ports.
        self.transport.advance(frames, &mut self.metronome);

        // 3. Borrow the output buffers and zero them. Devices add into them.
        let buf_l = self.out_l.as_mut_slice(scope);
        let buf_r = self.out_r.as_mut_slice(scope);
        for s in buf_l.iter_mut() {
            *s = 0.0;
        }
        for s in buf_r.iter_mut() {
            *s = 0.0;
        }

        // 4. Render the graph (metronome only, for Phase 0). buf_l/buf_r borrow
        //    disjoint fields from metronome, so this composes fine.
        let ctx = ProcessContext {
            sample_rate: self.sample_rate,
            frames,
        };
        self.metronome.process(ctx, buf_l, buf_r);

        // 5. Meter (instantaneous block peak) + push transport position.
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
        let _ = self.evt_tx.push(EngineEvent::Metering { peak_l, peak_r });

        Control::Continue
    }
}

/// JACK notifications handler. No-op for Phase 0.
pub(crate) struct Notifications;

impl jack::NotificationHandler for Notifications {}

pub(crate) type AsyncClient = jack::AsyncClient<Notifications, Graph>;
