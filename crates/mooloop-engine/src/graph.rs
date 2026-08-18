//! JACK adapter around the allocation-free shared render state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use jack::ProcessHandler;
use jack::{AudioOut, Client, Control, Port, ProcessScope};
use mooloop_core::{EngineCommand, EngineEvent, Project, SamplerParams};
use mooloop_dsp::{SampleData, MAX_BLOCK_SIZE};
use rtrb::Consumer;

use crate::render::RenderState;

pub(crate) struct GraphIo {
    pub out_l: Port<AudioOut>,
    pub out_r: Port<AudioOut>,
    pub cmd_rx: Consumer<EngineCommand>,
    pub evt_tx: rtrb::Producer<EngineEvent>,
}

pub(crate) struct Graph {
    render: RenderState,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    cmd_rx: Consumer<EngineCommand>,
    evt_tx: rtrb::Producer<EngineEvent>,
    project_slot: Arc<ArcSwapOption<Project>>,
    xrun_count: Arc<AtomicU64>,
    last_seen_xruns: u64,
}

impl Graph {
    pub(crate) fn new(
        sample_rate: u32,
        io: GraphIo,
        sample_slots: Arc<Vec<Arc<ArcSwapOption<SampleData>>>>,
        project_slot: Arc<ArcSwapOption<Project>>,
        initial_params: SamplerParams,
        xrun_count: Arc<AtomicU64>,
    ) -> Self {
        Self {
            render: RenderState::new(sample_rate, sample_slots, initial_params),
            out_l: io.out_l,
            out_r: io.out_r,
            cmd_rx: io.cmd_rx,
            evt_tx: io.evt_tx,
            project_slot,
            xrun_count,
            last_seen_xruns: 0,
        }
    }
}

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        let frames = (scope.n_frames() as usize).min(MAX_BLOCK_SIZE);
        while let Ok(command) = self.cmd_rx.pop() {
            match command {
                EngineCommand::InstallProject { generation } => {
                    if let Some(project) = self.project_slot.load_full() {
                        self.render.load_project(&project);
                        drop(project);
                    }
                    let _ = self
                        .evt_tx
                        .push(EngineEvent::ProjectInstalled { generation });
                }
                other => self.render.apply_command(other),
            }
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
