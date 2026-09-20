//! What a reconciler does when the command ring will not take its command.
//!
//! The defect these are written against is not "a command was lost". It is
//! "a command was lost and the session stopped believing anything was wrong":
//! every reconciler here is diff-based, so one that advances its `_sent`
//! mirror after a refused send will never resend it — the difference that
//! would have found it has already been satisfied. The routing or
//! compensation that resulted is then wrong, and stays wrong, for as long as
//! the document is open.
//!
//! The ring is a `Ring` rather than the engine's own, because an
//! `EngineHandle` cannot be built without opening an audio driver. What is
//! under test is the reconciler's handling of a refusal, and a refusal is a
//! `false`; `rtrb`'s own bound is `rtrb`'s to keep.

use mooloop_core::{DeviceKind, EffectKind, EffectTarget, EngineCommand, MusicalEdge};
use mooloop_engine::{CommandSink, StructuralCommand};
use mooloop_session::session::Session;

/// A command sink with a capacity and nothing draining it.
///
/// `room` is how many more commands it will take. Setting it to zero is a
/// full ring; raising it is the audio thread having caught up.
struct Ring {
    room: usize,
    engine: Vec<EngineCommand>,
    structural: Vec<StructuralCommand>,
}

impl Ring {
    fn full() -> Self {
        Self {
            room: 0,
            engine: Vec::new(),
            structural: Vec::new(),
        }
    }

    fn with_room(room: usize) -> Self {
        Self {
            room,
            ..Self::full()
        }
    }

    fn take_room(&mut self) -> bool {
        if self.room == 0 {
            return false;
        }
        self.room -= 1;
        true
    }

    fn sent(&self) -> usize {
        self.engine.len() + self.structural.len()
    }

    /// Which targets a `SetCompensation` was accepted for, in order.
    fn compensated(&self) -> Vec<EffectTarget> {
        self.structural
            .iter()
            .filter_map(|command| match command {
                StructuralCommand::SetCompensation { target, .. } => Some(*target),
                _ => None,
            })
            .collect()
    }

    fn drain(&mut self, room: usize) {
        self.engine.clear();
        self.structural.clear();
        self.room = room;
    }
}

impl CommandSink for Ring {
    fn send(&mut self, cmd: EngineCommand) -> bool {
        if !self.take_room() {
            return false;
        }
        self.engine.push(cmd);
        true
    }

    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        if !self.take_room() {
            return false;
        }
        self.structural.push(cmd);
        true
    }

    /// Recorded on the same list as an immediate command: nothing in
    /// `mooloop-session` defers anything yet, and these tests are about what
    /// reaches the ring rather than when the engine applies it.
    fn send_deferred(&mut self, cmd: EngineCommand, _when: MusicalEdge) -> bool {
        self.send(cmd)
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }
}

/// Three channels with a Drive on the first, which is the only device that
/// declares a latency. Everything that sums downstream of it then owes that
/// latency, so the compensation plan has many entries to deliver and each is
/// its own command.
fn session_owing_compensation() -> Session {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);
    session.add_channel(DeviceKind::Sampler);
    session.selected = 0;
    session.effect_target = EffectTarget::Channel(0);
    session
        .insert_effect_at(EffectKind::Drive, 0)
        .expect("an empty chain has room");
    session
}

/// The whole point of the step, on the simplest reconciler that has one: a
/// refused command must leave the mirror where it was, so the next tick sends
/// it again.
#[test]
fn a_refused_solo_is_sent_again_on_the_next_tick() {
    // Two tracks, one soloed: a solo silences what it beats, so a bank with
    // nothing to beat derives all-false and this would test nothing.
    let mut session = Session::default();
    session.add_track().expect("a default project has room for a track");
    session.add_track().expect("a default project has room for a track");
    session.buses[1].bus.solo = true;
    let wanted = mooloop_core::mixer::solo_silenced(&session.buses);
    assert_ne!(
        wanted, session.solo_silenced_sent,
        "the fixture has to want a send, or this test passes for the wrong reason"
    );

    let mut ring = Ring::full();
    session.sync_solo(&mut ring);
    assert_eq!(ring.sent(), 0, "a full ring accepted a command");
    assert_ne!(
        session.solo_silenced_sent, wanted,
        "the mirror advanced past a command that was never delivered, so no \
         later diff can find it"
    );

    ring.drain(64);
    session.sync_solo(&mut ring);
    assert!(
        ring.engine.iter().any(|command| matches!(
            command,
            EngineCommand::SetTrackSoloSilenced { .. }
        )),
        "the retry did not resend what the refusal dropped"
    );
    assert_eq!(
        session.solo_silenced_sent, wanted,
        "a delivered command did not advance the mirror"
    );
}

/// `sync_compensation` sends one command per target inside a loop, so its
/// mirror cannot be one flag at the bottom: a ring with room for one of two
/// must leave the other outstanding, and must not resend the one that landed.
#[test]
fn a_partly_delivered_compensation_plan_resends_only_what_was_refused() {
    let mut session = session_owing_compensation();

    // How many targets the plan owes, taken from a run against a ring that
    // refuses nothing. Derived rather than written down: what a Drive makes
    // the rest of the graph wait is `compile_latency`'s answer, and a literal
    // here would be a second copy of it.
    let owed = {
        let mut reference = session_owing_compensation();
        let mut ring = Ring::with_room(usize::MAX);
        reference.sync_compensation(&mut ring);
        ring.compensated()
    };
    assert!(
        owed.len() > 1,
        "the fixture must owe more than one target for this to be a partial \
         delivery; it owed {owed:?}"
    );

    let mut ring = Ring::with_room(1);
    session.sync_compensation(&mut ring);
    let first = ring.compensated();
    assert_eq!(first.len(), 1, "a ring with room for one took {first:?}");

    // The refused targets are still outstanding; the delivered one is not.
    ring.drain(usize::MAX);
    session.sync_compensation(&mut ring);
    let second = ring.compensated();
    assert!(
        !second.contains(&first[0]),
        "the second tick resent {:?}, which had already landed",
        first[0]
    );
    let mut delivered = first.clone();
    delivered.extend(second);
    delivered.sort_by_key(|target| format!("{target:?}"));
    let mut expected = owed.clone();
    expected.sort_by_key(|target| format!("{target:?}"));
    assert_eq!(
        delivered, expected,
        "two ticks did not between them deliver the whole plan exactly once"
    );

    // And now the plan is fully delivered, so a third tick is silent.
    ring.drain(usize::MAX);
    session.sync_compensation(&mut ring);
    assert_eq!(
        ring.sent(),
        0,
        "a fully delivered plan still had something to say"
    );
}

/// The all-or-nothing reconcilers get the same guarantee: one command, and
/// the mirror follows it rather than leading it.
#[test]
fn a_refused_track_graph_leaves_the_mirror_alone() {
    let mut session = Session::default();
    session.add_track().expect("a default project has room for a track");
    session.add_channel(DeviceKind::Sampler);
    let before = session.track_graph_sent.clone();

    let mut ring = Ring::full();
    session.sync_track_graph(&mut ring);
    assert_eq!(ring.sent(), 0, "a full ring accepted a command");
    assert_eq!(
        session.track_graph_sent, before,
        "the mirror advanced past a refused send"
    );

    ring.drain(64);
    session.sync_track_graph(&mut ring);
    assert_eq!(
        ring.structural.len(),
        1,
        "the retry did not resend the graph"
    );
    assert_ne!(
        session.track_graph_sent, before,
        "a delivered graph did not advance the mirror"
    );
}

/// Two decodes into one channel are ordered by which one *finishes*, and
/// finishing is decode time — file size and page cache. Load a long file,
/// change your mind and load a short one, and the short one can land first
/// and be overwritten by the long one: the user clicks sample B and hears
/// sample A.
///
/// `source_revision` cannot separate them, because it is a property of the
/// project rather than of a request, and neither load changed the project.
/// The token can.
#[test]
fn an_older_sample_load_cannot_overwrite_a_newer_one() {
    let mut session = Session::default();

    // Request A for channel 0, then request B for the same channel — the
    // user changing their mind before A's decode finished.
    let a = session.next_sample_request(0);
    let b = session.next_sample_request(0);
    assert_ne!(a, b, "two dispatches must not share a token");

    assert!(
        session.sample_request_is_current(0, b),
        "the most recent request is the one the channel is waiting for"
    );
    assert!(
        !session.sample_request_is_current(0, a),
        "a superseded request must be refused however it is ordered against \
         the one that replaced it"
    );

    // Delivery order does not matter, which is the whole point: B arriving
    // first does not make A acceptable afterwards.
    assert!(session.sample_request_is_current(0, b));
    assert!(!session.sample_request_is_current(0, a));

    // A different channel is unaffected by either.
    assert!(
        !session.sample_request_is_current(1, b),
        "a token is per channel; channel 1 asked for nothing"
    );
}

/// **A completion must not be honoured by whoever slid into its channel's
/// seat.** A load is dispatched for a seat, because a seat is all the
/// completion carries; the token it is checked against is keyed by the
/// channel, so the two disagree exactly when they should.
///
/// The map used to be a `Vec` indexed by seat, rewritten by
/// `Session::rescope_after` on every structural edit. It is keyed by
/// `ChannelId` since `channel-identity/03`, so there is nothing to rewrite --
/// but the claim this test makes is the same one, and it is the claim that
/// matters.
#[test]
fn a_channel_removal_does_not_hand_its_load_to_its_successor() {
    let mut session = Session::default();
    session.add_channel(DeviceKind::Sampler);
    session.add_channel(DeviceKind::Sampler);

    let second = session.next_sample_request(1);
    let third = session.next_sample_request(2);

    // The install is what removes the channel; the rescope follows it.
    session.channels.remove(1);
    session.rescope_after(mooloop_core::ChannelEdit::Removed(1));

    assert!(
        session.sample_request_is_current(1, third),
        "the channel that was at 2 is at 1 now, and is still waiting for its \
         own load"
    );
    assert!(
        !session.sample_request_is_current(1, second),
        "the removed channel's token must not be honoured at the seat its \
         successor now occupies"
    );
}
