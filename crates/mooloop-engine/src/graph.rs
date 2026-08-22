//! JACK adapter around the allocation-free shared render state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use jack::ProcessHandler;
use jack::{AudioOut, Client, Control, Port, ProcessScope};
use mooloop_core::{EngineCommand, EngineEvent, Project};
use mooloop_dsp::{SampleData, MAX_BLOCK_SIZE};
use rtrb::Consumer;

use crate::render::RenderState;
use crate::{StructuralCommand, StructuralReclaim};

pub(crate) struct GraphIo {
    pub out_l: Port<AudioOut>,
    pub out_r: Port<AudioOut>,
    pub cmd_rx: Consumer<EngineCommand>,
    pub evt_tx: rtrb::Producer<EngineEvent>,
    pub structural_rx: Consumer<StructuralCommand>,
    pub reclaim_tx: rtrb::Producer<StructuralReclaim>,
}

pub(crate) struct Graph {
    render: RenderState,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    cmd_rx: Consumer<EngineCommand>,
    evt_tx: rtrb::Producer<EngineEvent>,
    structural_rx: Consumer<StructuralCommand>,
    reclaim_tx: rtrb::Producer<StructuralReclaim>,
    /// Reclaim nodes that did not fit the reclaim ring last block; retried
    /// each block so a full ring never forces a drop on this thread.
    reclaim_spill: Vec<StructuralReclaim>,
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
        xrun_count: Arc<AtomicU64>,
    ) -> Self {
        Self {
            render: RenderState::new(sample_rate, sample_slots),
            out_l: io.out_l,
            out_r: io.out_r,
            cmd_rx: io.cmd_rx,
            evt_tx: io.evt_tx,
            structural_rx: io.structural_rx,
            reclaim_tx: io.reclaim_tx,
            reclaim_spill: Vec::new(),
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
        while let Ok(command) = self.structural_rx.pop() {
            self.render.apply_structural(command);
        }
        // Hand displaced effect nodes back to the GUI thread for disposal.
        // Anything that does not fit this block is retried next block,
        // reusing the spill buffer's capacity.
        let mut outgoing = std::mem::take(&mut self.reclaim_spill);
        outgoing.extend(
            self.render
                .take_reclaim()
                .into_iter()
                .map(StructuralReclaim::Node),
        );
        for node in outgoing.drain(..) {
            if let Err(rtrb::PushError::Full(node)) = self.reclaim_tx.push(node) {
                self.reclaim_spill.push(node);
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
