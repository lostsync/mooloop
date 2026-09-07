# 02 — A device can be copied

This is the step that spends `docs/plans/containers/` step 01. A rack device
became a durable identity on 2026-09-06, `SlotRemap` was deleted outright, and
the status document's headline is that **reordering, inserting and deleting
rack rows rewrite no address anywhere**. Nothing has used that yet. Copying a
device between channels is what it was for.

Today there is no way to copy a device at all. `grep` finds no
`copy_device`, `duplicate_effect` or `clone_effect` anywhere in the tree. To
get the same delay onto a second channel you add a delay and set every knob
again, or you save a preset and load it — which works, and which is the
workaround this step retires.

## Why it is small

Both halves already exist and were built for something else.

**The payload is `EffectRun`** (`crates/mooloop-core/src/effect.rs:1912`):

```rust
pub struct EffectRun {
    pub effects: Vec<EffectSlotState>,
}
```

That is exactly a copied selection: one device is a run of one, and a
container plus its children is a run whose head is the container. It was built
so a container could save as one preset, and it is already the thing
`load_effect_run` accepts.

**The paste is `load_effect_run`** (`crates/mooloop-session/src/effects.rs:221`).
It already mints fresh `DeviceId`s on the way in — `lib.rs:482` says so
outright, and `document.rs:350` records that a preset is written with every
identity stripped for the same reason. A pasted device must not share an
identity with the device it was copied from, and the code that guarantees that
is written and tested.

**The clipboard has a home.** `Clipboards` (`crates/mooloop-session/src/command.rs:17`)
already holds `channel_clipboard: Option<ChannelClipboard>` and
`note_clipboard: Vec<NoteEvent>`. A `device_clipboard: Option<EffectRun>` sits
beside them and follows the same lifetime rules.

So the work is one field, four session verbs, four registry actions, and the
rack surface to reach them.

## The shape

- `Session::copy_device(slot)` lifts the row — and, if it is a container, its
  whole run — into the clipboard, identity stripped, the way
  `take_preset_save` already does.
- `Session::cut_device(slot)` is that plus the existing `remove_effect_at`,
  which already calls `forget_device` and is already undoable.
- `Session::paste_device(slot)` is `load_effect_run` at an insert position
  rather than over an existing row. **This is the one genuinely new
  behaviour**: `load_effect_run` today replaces a run in place, and paste
  inserts. Check whether that is a parameter or a second entry point before
  assuming either.
- `Session::duplicate_device(slot)` is copy-then-paste-after, without
  disturbing the clipboard — the same relationship `channel.clone` has to
  `channel.copy` / `channel.paste`.

All four are undoable, because every other rack edit already is: "effect
add/move/remove became undoable" is recorded in `CONTRIBUTORS.md` against the
structural-addressing work.

## What crosses a channel boundary, and what does not

A device pasted onto a *different* channel cannot bring its modulation with
it. This is not a new limitation and it must not be re-solved here: it is
precisely the one thing `docs/plans/containers/` recorded as **blocked rather
than done** — a container preset cannot carry the modulation that drives it,
because a route's source lives in the channel's rack, and fixing it means
deciding whether a modulator can live in a container, which the containers
brief reserves.

So this step inherits that answer rather than making one. Paste carries
parameters and structure; it does not carry routes. **Say so in the
interface**, the way the container already says what it cannot carry — a
silent drop is how a musician learns not to trust a gesture.

Automation lanes are the same question and get the same answer.

## The actions

The registry has thirty-nine actions and not one of them is device-level.
Adding these is also the first real test of whether `Ctrl+C` can mean two
things depending on what is focused — today it is `edit.copy-channel`,
unconditionally. Resolve that deliberately:

- The honest option is context-sensitive `Ctrl+C`/`Ctrl+X`/`Ctrl+V`, dispatched
  on what has focus, which is the REAPER-shaped answer Adam has asked for and
  which step 04 is building the focus vocabulary for.
- The cheap option is separate chords for device copy/paste.

**Prefer the cheap option in this step** and leave the dispatcher question to
step 04. A context-sensitive clipboard designed before the focus model is
settled will be designed twice.

## Do not

- **Do not add a cross-channel drag.** Dragging a device between rack rows
  exists (`device-rack.slint:503`); dragging between *channels* means a drag
  that survives a channel switch, which is a gesture problem, not a clipboard
  one. A clipboard is the keyboard-shaped answer and it is the one that works
  without solving that.
- **Do not put the clipboard on the system clipboard.** It holds an
  `EffectRun`, not text, and a serialisable form of it already exists as a
  preset bundle if that is ever wanted.

## Done when

A device — or a container and everything in it — can be copied from one
channel and pasted onto another, with fresh identities, undoably, from the
keyboard and from the rack row's own controls; the routes that could not come
with it are named rather than dropped in silence; and
`a_reorder_does_not_change_one_byte_of_the_saved_addresses` still passes.
