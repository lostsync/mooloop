//! JACK adapter around the allocation-free shared render state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use jack::ProcessHandler;
use jack::{AudioOut, Client, Control, Port, ProcessScope};
use mooloop_core::EngineEvent;
use mooloop_dsp::MAX_BLOCK_SIZE;
use rtrb::Consumer;

use crate::render::RenderState;
use crate::{RealtimeCommand, StructuralReclaim};

pub(crate) struct GraphIo {
    pub out_l: Port<AudioOut>,
    pub out_r: Port<AudioOut>,
    pub cmd_rx: Consumer<RealtimeCommand>,
    pub evt_tx: rtrb::Producer<EngineEvent>,
    pub reclaim_tx: rtrb::Producer<StructuralReclaim>,
}

pub(crate) struct Graph {
    render: Box<RenderState>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    cmd_rx: Consumer<RealtimeCommand>,
    evt_tx: rtrb::Producer<EngineEvent>,
    reclaim_tx: rtrb::Producer<StructuralReclaim>,
    /// A command popped from the ordered stream while reclamation is
    /// backpressured. No later command may pass it.
    pending_command: Option<RealtimeCommand>,
    xrun_count: Arc<AtomicU64>,
    last_seen_xruns: u64,
}

impl Graph {
    pub(crate) fn new(io: GraphIo, render: Box<RenderState>, xrun_count: Arc<AtomicU64>) -> Self {
        Self {
            render,
            out_l: io.out_l,
            out_r: io.out_r,
            cmd_rx: io.cmd_rx,
            evt_tx: io.evt_tx,
            reclaim_tx: io.reclaim_tx,
            pending_command: None,
            xrun_count,
            last_seen_xruns: 0,
        }
    }
}

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        let frames = (scope.n_frames() as usize).min(MAX_BLOCK_SIZE);
        // Value edits, structural ownership transfers, and prepared projects
        // share one ordered stream. Only apply an ownership-changing command
        // when its displaced object can immediately leave through the reclaim
        // ring; otherwise retain it and let no later command cross the
        // generation boundary.
        loop {
            let command = match self.pending_command.take() {
                Some(command) => command,
                None => match self.cmd_rx.pop() {
                    Ok(command) => command,
                    Err(_) => break,
                },
            };
            let prepared = match command {
                RealtimeCommand::Engine(command) => {
                    self.render.apply_command(command);
                    continue;
                }
                RealtimeCommand::Structural(command) => {
                    if self.reclaim_tx.slots() == 0 {
                        self.pending_command = Some(RealtimeCommand::Structural(command));
                        break;
                    }
                    if let Some(displaced) = self.render.apply_structural(command) {
                        match self.reclaim_tx.push(StructuralReclaim::Effect(displaced)) {
                            Ok(()) => {}
                            Err(_) => {
                                unreachable!("reclaim capacity checked before structural edit")
                            }
                        }
                    }
                    continue;
                }
                RealtimeCommand::InstallProject(prepared) => prepared,
            };
            if self.reclaim_tx.slots() == 0 {
                self.pending_command = Some(RealtimeCommand::InstallProject(prepared));
                break;
            }
            let retired = std::mem::replace(&mut self.render, prepared.render);
            match self
                .reclaim_tx
                .push(StructuralReclaim::RenderState(retired))
            {
                Ok(()) => {}
                Err(_) => unreachable!("reclaim capacity checked before project swap"),
            }
            let _ = self.evt_tx.push(EngineEvent::ProjectInstalled {
                generation: prepared.generation,
            });
        }

        let report = self.render.process_block(frames);
        let master = self.render.master();
        let buffer_l = self.out_l.as_mut_slice(scope);
        let buffer_r = self.out_r.as_mut_slice(scope);
        buffer_l[..frames].copy_from_slice(&master.l[..frames]);
        buffer_r[..frames].copy_from_slice(&master.r[..frames]);
        buffer_l[frames..].fill(0.0);
        buffer_r[frames..].fill(0.0);

        let _ = self.evt_tx.push(EngineEvent::Position {
            tick: report.position_tick,
            beat_in_bar: report.beat_in_bar,
            playing: report.playing,
        });
        let _ = self.evt_tx.push(EngineEvent::Metering {
            peak_l: report.peak_l,
            peak_r: report.peak_r,
        });
        let xruns = self.xrun_count.load(Ordering::Relaxed);
        if xruns != self.last_seen_xruns {
            self.last_seen_xruns = xruns;
            let _ = self.evt_tx.push(EngineEvent::Xrun);
        }
        Control::Continue
    }
}

pub(crate) struct Notifications {
    pub xrun_count: Arc<AtomicU64>,
}

impl jack::NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.xrun_count.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }
}

pub(crate) type AsyncClient = jack::AsyncClient<Notifications, Graph>;
