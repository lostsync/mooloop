//! The one ordered stream, and the undo history that rides on it.
//!
//! The six typed senders exist so that boxed structural edits and POD
//! commands cannot enter separate queues and lose their relative order. A
//! parameter applied to a node that does not exist yet is silently dropped by
//! the engine, so the ordering is not a nicety.

use mooloop_core::{EffectTarget, EngineCommand, Project};
use mooloop_session::engine::{
    EngineCommandSender, PendingEngineMessage, PreviewSender, StructuralCommandSender,
};
use mooloop_session::history::{Entry, History};
use mooloop_session::project::ProjectSnapshot;

fn wire() -> (
    EngineCommandSender,
    StructuralCommandSender,
    PreviewSender,
    std::sync::mpsc::Receiver<PendingEngineMessage>,
) {
    let (tx, rx) = std::sync::mpsc::channel();
    (
        EngineCommandSender(tx.clone()),
        StructuralCommandSender(tx.clone()),
        PreviewSender(tx),
        rx,
    )
}

/// A structural install followed by a parameter change for the same slot has
/// to arrive in that order; the reverse applies a value to a node that is not
/// there.
#[test]
fn a_structural_install_leads_the_parameter_change_that_follows_it() {
    let (commands, structural, preview, rx) = wire();

    assert!(structural.add_channel(1, mooloop_core::DeviceKind::Sampler));
    assert!(commands.send(EngineCommand::SetChannelVolume {
        channel: 1,
        volume: 0.5,
    }));
    assert!(preview.send_gain(0.25));
    assert!(commands.resize_buffers(140.0));

    let drained: Vec<PendingEngineMessage> = rx.try_iter().collect();
    assert!(
        matches!(
            drained.as_slice(),
            [
                PendingEngineMessage::AddChannel { channel: 1, .. },
                PendingEngineMessage::Command(EngineCommand::SetChannelVolume { channel: 1, .. }),
                PendingEngineMessage::PreviewGain(_),
                PendingEngineMessage::ResizeBuffers { .. },
            ]
        ),
        "the senders did not share one ordered queue"
    );
}

/// Different senders, one queue: interleaving them must not reorder anything.
#[test]
fn interleaving_the_senders_preserves_the_order_they_were_called_in() {
    let (commands, structural, _preview, rx) = wire();

    for slot in 0..8u8 {
        if slot % 2 == 0 {
            assert!(structural.send(mooloop_engine::StructuralCommand::RemoveEffect {
                target: EffectTarget::Channel(0),
                slot,
            }));
        } else {
            assert!(commands.send(EngineCommand::SetEffectBypassed {
                target: EffectTarget::Channel(0),
                slot,
                bypassed: true,
            }));
        }
    }

    let slots: Vec<u8> = rx
        .try_iter()
        .map(|message| match message {
            PendingEngineMessage::Structural(mooloop_engine::StructuralCommand::RemoveEffect {
                slot,
                ..
            }) => slot,
            PendingEngineMessage::Command(EngineCommand::SetEffectBypassed { slot, .. }) => slot,
            other => panic!("unexpected message: {}", std::any::type_name_of_val(&other)),
        })
        .collect();
    assert_eq!(slots, (0..8).collect::<Vec<u8>>());
}

fn snapshot(bpm: u16) -> ProjectSnapshot {
    ProjectSnapshot {
        project: Project {
            bpm,
            ..Project::default()
        },
        samples: Vec::new(),
    }
}

/// Undo then redo is the identity, on the type the application actually
/// stores rather than the integers `history.rs` exercises it with.
#[test]
fn undo_then_redo_puts_the_document_back() {
    let mut history: History<ProjectSnapshot> = History::default();
    history.record(Entry {
        before: snapshot(120),
        after: snapshot(140),
        label: "tempo",
        gesture: None,
    });

    let undone = history.undo_target().expect("one entry to undo").clone();
    assert_eq!(undone.before.project.bpm, 120);
    history.commit_undo();

    let redone = history.redo_target().expect("one entry to redo").clone();
    assert_eq!(redone.after.project.bpm, 140);
    history.commit_redo();

    assert!(history.can_undo());
    assert!(!history.can_redo());
}

/// A drag reports an edit per move frame. Stamping them with one token is
/// what makes the whole drag a single undo step; losing it turns a note drag
/// back into twenty undos, and a build will not say so.
#[test]
fn a_drags_frames_collapse_into_one_document_level_entry() {
    let mut history: History<ProjectSnapshot> = History::default();
    for (index, bpm) in [130u16, 140, 150].into_iter().enumerate() {
        history.record(Entry {
            before: snapshot(120 + index as u16 * 10),
            after: snapshot(bpm),
            label: "drag",
            gesture: Some(7),
        });
    }

    let undone = history.undo_target().expect("the drag is one entry");
    assert_eq!(
        (undone.before.project.bpm, undone.after.project.bpm),
        (120, 150),
        "the collapsed entry does not span the whole drag"
    );
    history.commit_undo();
    assert!(!history.can_undo(), "the drag left more than one entry");

    // A second drag, after the release, is its own step.
    history.commit_redo();
    history.record(Entry {
        before: snapshot(150),
        after: snapshot(160),
        label: "drag",
        gesture: Some(8),
    });
    history.commit_undo();
    assert!(history.can_undo(), "two drags collapsed into one");
}
