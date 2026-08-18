//! The realtime audio graph and its JACK `ProcessHandler` entry point.
//!
//! ## Architecture
//!
//! ```text
//!   commands ──> ┌──────────┐   tick interval   ┌───────────┐
//!   (rtrb)       │Transport │ ────────────────> │ Sequencer │──┐
//!                └──────────┘                   └───────────┘  │ EventList/ch
//!                                                              ▼
//!   ┌─────────────────────────── per ChannelStrip ───────────────────────┐
//!   │  own StereoBus (cleared) ─> instrument ─> effects ─> gain/pan/mute │
//!   └───────────────────────────┬────────────────────────────────────────┘
//!                               ▼  sum
//!                          master bus ──> JACK out_l/out_r
//! ```
//!
//! Everything is preallocated to pool size (channels, patterns, buses to
//! [`MAX_BLOCK_SIZE`]) so the realtime thread never allocates or locks.
//!
//! ## Routing headroom
//!
//! Channel strips own their buffers; the graph decides what flows where.
//! Adding send/return buses, submixes or sidechains later means teaching
//! `Graph` to hand different buses to different nodes — the node trait
//! ([`AudioNode`]) doesn't change. Channel gain/pan/mute are already applied
//! at the strip's output stage.

use jack::ProcessHandler;
use jack::{AudioOut, Client, Control, Port, ProcessScope};
use mooloop_core::{EngineCommand, EngineEvent, SamplerParams, MAX_CHANNELS, MAX_PATTERNS};
use mooloop_dsp::{
    pan_gains, AudioNode, Event, EventList, ProcessContext, SampleData, Sampler, StereoBus,
    TimedEvent, MAX_BLOCK_SIZE,
};
use rtrb::Consumer;

use crate::sequencer::Sequencer;
use crate::transport::Transport;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const INITIAL_STEPS: usize = mooloop_core::DEFAULT_STEPS as usize;
const INITIAL_CHANNELS: usize = 1;

fn inject_choke_events(choke_groups: &[u8], events: &mut [EventList]) {
    let active = choke_groups.len().min(events.len());
    for source in 0..active {
        let group = choke_groups[source];
        if group == 0 {
            continue;
        }
        for target in 0..active {
            if source == target || choke_groups[target] != group {
                continue;
            }
            let (source_events, target_events) = if source < target {
                let (left, right) = events.split_at_mut(target);
                (&left[source], &mut right[0])
            } else {
                let (left, right) = events.split_at_mut(source);
                (&right[0], &mut left[target])
            };
            for event in source_events.iter() {
                if matches!(event.event, Event::NoteOn { .. }) {
                    target_events.push_ordered(TimedEvent {
                        offset: event.offset,
                        event: Event::Choke,
                    });
                }
            }
        }
    }
}

/// Ports and queues handed to the graph at construction.
pub(crate) struct GraphIo {
    pub out_l: Port<AudioOut>,
    pub out_r: Port<AudioOut>,
    pub cmd_rx: Consumer<EngineCommand>,
    pub evt_tx: rtrb::Producer<EngineEvent>,
}

/// One channel's signal chain: instrument, effect chain, output stage.
/// Buffers and nodes are preallocated; `effects` starts empty (Phase 3).
struct ChannelStrip {
    instrument: Sampler,
    effects: Vec<Box<dyn AudioNode + Send>>,
    bus: StereoBus,
    /// Linear output gain (0..1 UI scale applied as-is for now).
    gain: f32,
    /// Pan in [-1, 1]; 0 = centre.
    pan: f32,
    muted: bool,
}

impl ChannelStrip {
    fn new(instrument: Sampler) -> Self {
        Self {
            instrument,
            effects: Vec::new(),
            bus: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            gain: 0.8,
            pan: 0.0,
            muted: false,
        }
    }

    fn set_volume(&mut self, volume: f32) {
        self.gain = volume.clamp(0.0, 1.0);
    }

    fn set_pan(&mut self, pan: f32) {
        self.pan = pan.clamp(-1.0, 1.0);
    }
}

pub(crate) struct Graph {
    transport: Transport,
    sequencer: Sequencer,
    strips: Vec<ChannelStrip>,
    /// Per-channel event scratch, refilled by the sequencer each block.
    events: Vec<EventList>,
    /// Reused empty list for effect stages (they take no note input yet).
    empty_events: EventList,
    master: StereoBus,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    cmd_rx: Consumer<EngineCommand>,
    evt_tx: rtrb::Producer<EngineEvent>,
    sample_rate: u32,
    /// Shared with the notification handler; xruns are reported to the UI.
    xrun_count: Arc<AtomicU64>,
    last_seen_xruns: u64,
}

impl Graph {
    pub(crate) fn new(
        sample_rate: u32,
        io: GraphIo,
        sample_slots: Arc<Vec<Arc<arc_swap::ArcSwapOption<SampleData>>>>,
        initial_params: SamplerParams,
        xrun_count: Arc<AtomicU64>,
    ) -> Self {
        let sequencer = Sequencer::new(
            INITIAL_CHANNELS,
            MAX_PATTERNS,
            INITIAL_STEPS,
            mooloop_core::Ppq::DEFAULT,
        );
        let strips = sample_slots
            .iter()
            .map(|slot| ChannelStrip::new(Sampler::new(slot.clone(), initial_params, sample_rate)))
            .collect();
        Self {
            transport: Transport::new(sample_rate),
            sequencer,
            strips,
            events: (0..MAX_CHANNELS).map(|_| EventList::empty()).collect(),
            empty_events: EventList::empty(),
            master: StereoBus::with_capacity(MAX_BLOCK_SIZE),
            out_l: io.out_l,
            out_r: io.out_r,
            cmd_rx: io.cmd_rx,
            evt_tx: io.evt_tx,
            sample_rate,
            xrun_count,
            last_seen_xruns: 0,
        }
    }

    fn apply_command(&mut self, cmd: EngineCommand) {
        match cmd {
            EngineCommand::Play => self.transport.play(),
            EngineCommand::Pause => self.transport.pause(),
            EngineCommand::Stop => self.transport.stop(),
            EngineCommand::SetTempo(bpm) => self.transport.set_tempo(bpm),
            EngineCommand::SetCurrentPattern(p) => self.sequencer.set_current_pattern(p as usize),
            EngineCommand::SetPatternLength {
                pattern,
                length_steps,
            } => self
                .sequencer
                .set_pattern_length(pattern as usize, length_steps as usize),
            EngineCommand::AddChannel => {
                let n = self.sequencer.active_channels() + 1;
                self.sequencer.set_active_channels(n);
            }
            EngineCommand::RemoveChannel => {
                let n = self.sequencer.active_channels().saturating_sub(1);
                self.sequencer.set_active_channels(n);
            }
            EngineCommand::SetChannelMuted { channel, muted } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.muted = muted;
                }
            }
            EngineCommand::SetChannelVolume { channel, volume } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_volume(volume);
                }
            }
            EngineCommand::SetChannelPan { channel, pan } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.set_pan(pan);
                }
            }
            EngineCommand::SetStep {
                pattern,
                channel,
                step,
                on,
                note,
                velocity,
            } => self.sequencer.set_step(
                pattern as usize,
                channel as usize,
                step as usize,
                on,
                note,
                velocity,
            ),
            EngineCommand::UpsertNote {
                pattern,
                channel,
                note,
            } => {
                self.sequencer
                    .upsert_note(pattern as usize, channel as usize, note);
            }
            EngineCommand::RemoveNote {
                pattern,
                channel,
                id,
            } => {
                self.sequencer
                    .remove_note(pattern as usize, channel as usize, id);
            }
            EngineCommand::SetChannelSamplerParams { channel, params } => {
                if let Some(strip) = self.strips.get_mut(channel as usize) {
                    strip.instrument.set_params(params);
                }
            }
        }
    }
}

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        // Clamp cycles larger than our preallocated buses (PipeWire lets the
        // quantum be raised; past MAX_BLOCK_SIZE we render what we can and
        // zero the remainder rather than allocating on the RT thread).
        let frames = (scope.n_frames() as usize).min(MAX_BLOCK_SIZE);

        // 1. Drain commands first — disjoint from the port buffer borrows.
        while let Ok(cmd) = self.cmd_rx.pop() {
            self.apply_command(cmd);
        }

        // 2. Advance transport; capture the tick interval for the sequencer.
        let tps = self.transport.ticks_per_sample();
        let position_frames = self.transport.frames_played();
        let (start_tick, end_tick) = self.transport.advance(frames);

        // 3. Schedule note events into per-channel lists.
        if self.transport.playing {
            for ev in self.events.iter_mut() {
                ev.clear();
            }
            self.sequencer
                .schedule(start_tick, end_tick, frames, tps, &mut self.events);
            let mut choke_groups = [0; MAX_CHANNELS];
            for (index, strip) in self
                .strips
                .iter()
                .enumerate()
                .take(self.sequencer.active_channels())
            {
                if !strip.muted {
                    choke_groups[index] = strip.instrument.choke_group();
                }
            }
            inject_choke_events(
                &choke_groups[..self.sequencer.active_channels()],
                &mut self.events,
            );
        } else {
            for ev in self.events.iter_mut() {
                ev.clear();
            }
        }

        // 4. Render each active channel strip into its own bus, then mix.
        let ctx = ProcessContext {
            sample_rate: self.sample_rate,
            frames,
            playing: self.transport.playing,
            bpm: self.transport.bpm,
            position_ticks: start_tick,
            position_frames,
        };
        self.master.clear(frames);
        let active = self.sequencer.active_channels();
        for (i, strip) in self.strips.iter_mut().enumerate().take(active) {
            if strip.muted {
                continue;
            }
            strip.bus.clear(frames);
            strip
                .instrument
                .process(&ctx, &mut strip.bus, &self.events[i], None);
            for fx in strip.effects.iter_mut() {
                fx.process(&ctx, &mut strip.bus, &self.empty_events, None);
            }
            let (pan_l, pan_r) = pan_gains(strip.pan);
            strip
                .bus
                .apply_stereo_gain(strip.gain * pan_l, strip.gain * pan_r, frames);
            self.master.add_from(&strip.bus, frames);
        }

        // 5. Copy the master bus to the JACK ports; zero beyond our capacity
        //    if the cycle was oversized.
        let buf_l = self.out_l.as_mut_slice(scope);
        let buf_r = self.out_r.as_mut_slice(scope);
        buf_l[..frames].copy_from_slice(&self.master.l[..frames]);
        buf_r[..frames].copy_from_slice(&self.master.r[..frames]);
        for s in &mut buf_l[frames..] {
            *s = 0.0;
        }
        for s in &mut buf_r[frames..] {
            *s = 0.0;
        }

        // 6. Events: transport position, master metering, xruns.
        let (peak_l, peak_r) = self.master.peak(frames);
        let _ = self.evt_tx.push(EngineEvent::Position {
            tick: self.transport.position_ticks as u64,
            beat_in_bar: self.transport.beat_in_bar(),
            playing: self.transport.playing,
        });
        let _ = self.evt_tx.push(EngineEvent::Metering { peak_l, peak_r });
        let xruns = self.xrun_count.load(Ordering::Relaxed);
        if xruns != self.last_seen_xruns {
            self.last_seen_xruns = xruns;
            let _ = self.evt_tx.push(EngineEvent::Xrun);
        }

        Control::Continue
    }
}

/// JACK notifications handler. Xruns are counted on an atomic shared with
/// the graph, which forwards them to the UI (the notification callback runs
/// on JACK's control thread, not the RT thread, but staying lock-free keeps
/// the coupling trivial).
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_strip() -> ChannelStrip {
        let slot = Arc::new(arc_swap::ArcSwapOption::empty());
        ChannelStrip::new(Sampler::new(slot, SamplerParams::default(), 48_000))
    }

    #[test]
    fn channel_output_controls_are_bounded() {
        let mut strip = test_strip();
        strip.set_volume(2.0);
        strip.set_pan(-2.0);
        assert_eq!(strip.gain, 1.0);
        assert_eq!(strip.pan, -1.0);

        strip.set_volume(-1.0);
        strip.set_pan(2.0);
        assert_eq!(strip.gain, 0.0);
        assert_eq!(strip.pan, 1.0);
    }

    #[test]
    fn matching_choke_group_receives_sample_timed_choke() {
        let mut events = [EventList::empty(), EventList::empty(), EventList::empty()];
        events[0].push(TimedEvent {
            offset: 37,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        });

        inject_choke_events(&[2, 2, 3], &mut events);

        assert_eq!(events[0].len(), 1, "a channel does not choke itself");
        let target = events[1].iter().next().unwrap();
        assert_eq!(target.offset, 37);
        assert_eq!(target.event, Event::Choke);
        assert!(events[2].is_empty());
    }

    #[test]
    fn choke_is_ordered_before_a_simultaneous_note_on() {
        let mut events = [EventList::empty(), EventList::empty()];
        for (channel, id) in events.iter_mut().zip([1, 2]) {
            channel.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id,
                    note: 60,
                    velocity: 100,
                },
            });
        }

        inject_choke_events(&[1, 1], &mut events);

        for channel in &events {
            assert!(matches!(channel.iter().next().unwrap().event, Event::Choke));
            assert!(matches!(
                channel.iter().nth(1).unwrap().event,
                Event::NoteOn { .. }
            ));
        }
    }
}
