//! **Test support, not API.** The MIDI capture path, driven block by block
//! with no driver, so a check outside this crate can hold what the engine
//! reports while recording against what the session writes from it
//! (MOO-234).
//!
//! The order of what a block reports is the executor's: the notes whose
//! keys came up, then the block's own `Position`. Nothing in the application
//! calls it; compiled only for this crate's tests and behind the
//! `test-support` feature, like [`crate::live_check`].

use mooloop_core::{
    EngineCommand, EngineEvent, MidiChannelFilter, MidiInputRoute, MidiKind, MidiMessage,
    MidiPortId, MidiRouteSource, Project,
};

use crate::render::{MidiRouting, RenderState};

/// A renderer that takes MIDI on every channel's first route from every
/// port, as a keyboard played into channel 0 does.
pub struct RecordRig {
    render: RenderState,
}

impl RecordRig {
    /// `project` at `sample_rate`, armed and stopped, with channel 0 taking
    /// every port's notes.
    pub fn new(project: &Project, sample_rate: u32) -> Self {
        let mut render = RenderState::from_project(sample_rate, project, &[]);
        let _ = render.set_midi_routing(Box::new(MidiRouting {
            routes: vec![MidiInputRoute {
                source: MidiRouteSource::AllPorts,
                channel: MidiChannelFilter::Omni,
            }],
        }));
        render.set_record_armed(true);
        Self { render }
    }

    /// Anything the session would send: play, a seek, a pattern switch.
    pub fn command(&mut self, command: EngineCommand) {
        self.render.apply_command(command);
    }

    pub fn play(&mut self) {
        self.render.play();
    }

    /// Frames per tick at the song's tempo, for placing a key on a tick.
    pub fn ticks_per_sample(&self) -> f64 {
        self.render.ticks_per_sample()
    }

    /// One block of `frames`, with `keys` -- `(offset, note, down)` --
    /// arriving in it. Returns what the executor would hand the pump, in its
    /// order.
    pub fn block(&mut self, frames: usize, keys: &[(u32, u8, bool)]) -> Vec<EngineEvent> {
        let messages: Vec<MidiMessage> = keys
            .iter()
            .map(|&(offset, note, down)| MidiMessage {
                offset,
                port: MidiPortId::FIRST,
                channel: 0,
                kind: if down {
                    MidiKind::NoteOn { note, velocity: 100 }
                } else {
                    MidiKind::NoteOff { note }
                },
            })
            .collect();
        if !messages.is_empty() {
            self.render.apply_midi(&messages);
        }
        let report = self.render.process_block(frames);
        let mut events: Vec<EngineEvent> =
            std::iter::from_fn(|| self.render.pop_outgoing()).collect();
        events.push(EngineEvent::Position {
            tick: report.position_tick,
            beat_in_bar: report.beat_in_bar,
            playing: report.playing,
        });
        events
    }
}
