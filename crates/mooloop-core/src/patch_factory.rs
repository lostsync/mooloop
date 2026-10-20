//! The patch presets mooloop ships (song patch boxes step 08): two pieces of
//! the song patch that show what the note boxes do together. Each is a
//! fragment as [`SongModulation::fragment`] lifts one, its tags unbound and
//! hinted, so it lands in any song and asks only for its tags to be bound.

use crate::box_text::parse;
use crate::modulation::{SongModulation, InputSource};
use crate::patch::{CanvasPoint, InletSource, Jack, TagHint, TagKind};
use crate::ModSourceId;

/// One shipped patch preset: its name and browser category, and the
/// fragment it lands.
#[derive(Debug, Clone)]
pub struct FactoryPatch {
    pub name: &'static str,
    pub category: &'static str,
    pub tags: &'static [&'static str],
    pub fragment: SongModulation,
}

/// The bank, in the order the browser lists it.
pub fn patches() -> Vec<FactoryPatch> {
    vec![left_hand(), arp_from_parts()]
}

/// A fragment being built: boxes typed as on the canvas, tags unbound with
/// the hint of what to bind them to, and wires between them.
struct Sketch(SongModulation);

impl Sketch {
    fn new() -> Self {
        Self(SongModulation::default())
    }

    fn typed(&mut self, text: &str, x: i32, y: i32) -> ModSourceId {
        let params = parse(text).unwrap_or_else(|| panic!("{text:?} is not a box"));
        let id = self.0.add_module(params, InputSource::None, "Patch");
        self.0.move_node(id, CanvasPoint::new(x, y));
        id
    }

    fn tag(&mut self, kind: TagKind, hint: &str, x: i32, y: i32) -> ModSourceId {
        let id = self.0.add_tag(kind.unbound(), CanvasPoint::new(x, y));
        self.0.hints.push(TagHint {
            tag: id,
            text: hint.into(),
        });
        id
    }

    fn notes_in(&mut self, x: i32, y: i32) -> ModSourceId {
        self.tag(TagKind::NotesIn { channel: None, take: false }, "Notes in", x, y)
    }

    fn notes_out(&mut self, x: i32, y: i32) -> ModSourceId {
        self.tag(TagKind::NotesOut { channel: None }, "Notes out", x, y)
    }

    fn wire(&mut self, from: (ModSourceId, u8), to: (ModSourceId, u8)) {
        self.0
            .connect(Jack::new(from.0, from.1), Jack::new(to.0, to.1))
            .unwrap_or_else(|refusal| panic!("{from:?} to {to:?}: {refusal:?}"));
    }
}

/// The chord as played, and its lowest note an octave under it: the left
/// hand. Take the notes in from a channel and send them out to the same
/// one, or to another.
fn left_hand() -> FactoryPatch {
    let mut patch = Sketch::new();
    let input = patch.notes_in(0, 0);
    let pick = patch.typed("pick low", 0, 80);
    let down = patch.typed("transpose -12", 0, 140);
    let merge = patch.typed("merge 2", 120, 200);
    let output = patch.notes_out(120, 260);
    patch.wire((input, 0), (pick, 0));
    patch.wire((pick, 0), (down, 0));
    patch.wire((input, 0), (merge, 0));
    patch.wire((down, 0), (merge, 1));
    patch.wire((merge, 0), (output, 0));
    FactoryPatch {
        name: "Left hand",
        category: "Notes",
        tags: &["pick", "transpose", "merge"],
        fragment: patch.0,
    }
}

/// An arp built from parts: each held chord unpacked into four held boxes,
/// a counter on the beat picking one of their pitches, and a note box
/// playing it on each beat.
fn arp_from_parts() -> FactoryPatch {
    let mut patch = Sketch::new();
    let input = patch.notes_in(0, 0);
    let beat = patch.tag(TagKind::Inlet { bind: Some(InletSource::Beat) }, "Beat", 400, 0);
    let unpack = patch.typed("unpack 4", 0, 80);
    let counter = patch.typed("counter 4", 400, 80);
    let select = patch.typed("select 4", 200, 220);
    let note = patch.typed("note", 200, 300);
    let output = patch.notes_out(200, 380);
    patch.wire((input, 0), (unpack, 0));
    patch.wire((beat, 0), (counter, 0));
    patch.wire((counter, 0), (select, 0));
    for at in 0..4u8 {
        let held = patch.typed("held", i32::from(at) * 90, 150);
        patch.wire((unpack, at), (held, 0));
        patch.wire((held, 1), (select, at + 1));
    }
    patch.wire((select, 0), (note, 0));
    patch.wire((beat, 0), (note, 2));
    patch.wire((note, 0), (output, 0));
    FactoryPatch {
        name: "Arp from parts",
        category: "Notes",
        tags: &["arp", "unpack", "held", "note"],
        fragment: patch.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every shipped patch is whole: each wire one the fragment holds, every
    /// tag unbound and hinted.
    #[test]
    fn every_factory_patch_is_whole_and_unbound() {
        for patch in patches() {
            let fragment = &patch.fragment;
            assert!(!fragment.wires.is_empty(), "{}", patch.name);
            for wire in &fragment.wires {
                assert_eq!(fragment.check_wire(wire.from, wire.to), Ok(()), "{}: {wire:?}", patch.name);
            }
            for tag in &fragment.tags {
                assert_eq!(tag.kind, tag.kind.unbound(), "{}", patch.name);
                assert!(fragment.hint(tag.id).is_some(), "{}", patch.name);
            }
        }
    }
}
