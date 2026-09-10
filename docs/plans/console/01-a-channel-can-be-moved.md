# 01 — A channel can be moved

The warm-up, and the only item on Adam's list with no dependencies. It is
also the half of step 04 that can be built and listened to before the
structural half exists: reordering is a drag, and so is routing a channel
to a track by hand.

## The one thing that is genuinely missing

`ChannelEdit` has two variants, `Removed(u8)` and `Inserted(u8)`, and a
reorder **cannot be composed from them**. `Removed` drops the moved channel's
own lanes by design -- that is the whole point of the variant, so that a lane
left pointing at index 3 does not start automating whichever channel slid into
the seat -- and a move must keep them. So the edit is a third variant, not a
pair.

```rust
pub enum ChannelEdit {
    Removed(u8),
    Inserted(u8),
    Moved { from: u8, to: u8 },
}
```

`ChannelEdit::channel(old)` for `Moved` is the standard remove-then-insert
renumbering: `from` becomes `to`; anything strictly between them shifts by one
towards `from`; everything outside the range is untouched. Nothing is dropped,
which is the difference from both other variants and the reason it returns
`Some` for every input.

## What follows from it

- **`Project::move_channel(from, to)`**, beside `insert_channel` and
  `remove_channel`, running the same `rescope_after` walk. That walk already
  covers the three durable channel-addressed things: `AuxInParams`
  subscriptions, `ModRoute` destinations, and `AutomationLane.target`.
- **The session-side referrers that `rescope_after` does not reach.**
  `Session` holds six things keyed by a channel index or by an `EffectTarget`
  containing one: `selected_device`, `selected_source`, `automation_target`,
  `effect_preset_names`, `source_preset_names`, and `pending_preset_save`. A
  move breaks all six today, and so does an insert or a delete -- this is not
  a bug the move introduces, it is one the move makes visible. One
  `Session::rescope_after(edit)` fixes them together.
- **The `samples` sidecar.** `ProjectSnapshot`/`ProjectEdit` carry
  `samples: Vec<Option<Arc<SampleData>>>` parallel to `project.channels`, and
  it is hand-maintained: `queue_channel_insert` does `samples.insert(index,
  ...)` and `queue_channel_delete` does `samples.remove(index)`. A move has to
  do the matching rotate or every sampler past the move plays the wrong file.

## The engine, and one deviation from the brief

The brief asked for a `StructuralCommand::MoveChannel` rotating
`RenderState::strips` in place, on the argument that a remove-then-add would
kill voices and tails. **That argument is right and the premise is not:
today's channel structural edits do not go through incremental commands at
all.** `queue_channel_delete` and `queue_channel_insert` build a whole
`Project`, hand it to the pump, and the pump calls `EngineHandle::
install_project`, which constructs a complete new `RenderState` and swaps it.
Every voice and tail in the song already stops on a channel delete or paste.

Two things follow. First, a move through the same path is *consistent* with
every other channel edit rather than a regression. Second, an incremental
`MoveChannel` cannot simply be added: strips are addressed by index by the
sequencer, the meter cells, the audio-tap plan and -- the sharp one -- by
`EngineHandle::sample_slots` / `slice_slots`, which are `Vec<Arc<...>>` shared
with the strips and written by index from `load_sample`. Rotating `strips`
without rotating those hands the moved channel its neighbour's audio.

So this step **uses the snapshot path**, and the incremental rotate is
recorded as a separate improvement that would cover insert and delete too --
which is where its value is, since it would then stop a paste from cutting
every tail in the song.

`EngineCommand::MoveEffect` is still the model for what that rotate would look
like: it is a POD bridge command, not a `StructuralCommand`, and it is
realtime-safe precisely because rotating `Vec<Box<_>>` entries moves pointers.

## The gesture

Copy **drag pattern A** -- the `RackDrag` global (`device-rack.slint`) plus
per-row `hot`/`shift` landing (`main.slint`). Grab the 88px name plate on the
rack row.

Use `absolute-position + mouse-x`, never `mouse-x - pressed-x`; the reason is
already documented twice in `device-rack.slint` and is that a `TouchArea`'s
`mouse-x` is relative to an element that is *itself moving* under the shift.

Undoable through the existing snapshot path; `queue_channel_delete` is the
model, including setting `project.selected_channel` so the rack follows the
channel rather than the seat.

## Acceptance

A channel with notes, an automation lane, a modulation route and an Aux In
subscription pointing at it is dragged three rows up; every one of those still
means the same channel, and undo puts it back.

## Verification

- `cargo test -p mooloop-core -p mooloop-session`.
- Extend `channel_edits_renumber_every_address_that_named_a_channel`
  (`project.rs`) rather than adding a parallel test: it is already the place
  that asserts every address follows an edit.
- A new UI test modelled on `tests/rack_reorder.rs`.
