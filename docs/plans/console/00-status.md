# Console pass status

## Step 01 — a channel can be moved

Landed on `feat/channel-reorder` (2026-09-09). A rack row's name plate is
dragged to another row; the rows between the grab and the landing slide aside
and the gap that opens is the drop indicator; the move is one undoable edit.

### The edit that could not be composed

`ChannelEdit` had `Removed` and `Inserted` and a reorder is not either of them
or both. `Removed` **drops the departing channel's own lanes and routes by
design** — that is the variant's whole job, so that a lane left on index 3
does not start automating whichever channel slid into the seat — and a move
has to keep them. So `Moved { from, to }` is a third variant, and it is the
only one whose `channel()` never returns `None`: a move loses nobody.

Everything downstream came free, because `AuxInParams::rescope`,
`ModRack::rescope_channels` and `rescope_lanes` all ask `edit.channel()` /
`edit.address()` rather than matching the variant. One new arm, three
renumberings.

### What the step actually found: the session half was never rescoped at all

`Project::rescope_after` covers what the *song* holds. Six things the
**session** holds are keyed by a channel index or by an `EffectTarget`
containing one, and none of them was in any walk:

`selected_device`, `selected_source`, `automation_target`,
`effect_preset_names`, `source_preset_names`, `pending_preset_save`.

**This is not a bug the reorder introduced.** An insert and a delete move
every channel past them too, and have been mis-keying all six for as long as
they have existed — a channel deleted above your open device left the rack's
selection and its preset label pointing at a stranger. The reorder is only the
first edit that makes it *visible*, because it is the first one performed
while looking at the thing the labels belong to.

`Session::rescope_after(edit)` fixes them together, and insert and delete now
carry their edits through it as well. The mechanism is one new field on
`ProjectEdit` — `channel_edit: Option<ChannelEdit>` — because the snapshot
that crosses to the pump is a `Project` and none of this is in a `Project`.
Undo and redo carry `None`: they restore a whole document rather than applying
an edit to one, and `Session::effect_preset_name` already documented that
these labels do not survive undo.

### The one departure from the plan, and why

The plan asked for a `StructuralCommand::MoveChannel` rotating
`RenderState::strips` in place, on the argument that a remove-then-add would
kill voices and tails. **The argument is right and the premise was wrong.**
Channel structural edits do not go through incremental commands today:
`queue_channel_delete` and `queue_channel_insert` build a whole `Project` and
the pump calls `EngineHandle::install_project`, which constructs a new
`RenderState` and swaps it. Every voice and tail in the song already stops on
a channel delete or a paste.

So a move through the same path is *consistent with a paste* rather than a
regression, and it is what the acceptance case needs anyway, since undo is the
snapshot path. An incremental rotate is still worth having — it would stop a
paste cutting every tail in the song, which is where its value is — but it is
its own change: `strips` is addressed by index by the sequencer, the meter
cells and the audio-tap plan, and rotating it without also rotating
`EngineHandle`'s `sample_slots` and `slice_slots` would hand the moved channel
its neighbour's audio.

### The gesture, and one Slint constraint

Drag pattern A, on the `y` axis: `ChannelDrag` is `RackDrag` with `y` where
the `x` is, and lives in a new `ui/channel-rack.slint` for the two reasons
`device-rack.slint` exists — the grab and the landing are in elements that do
not contain each other, and a test harness has to import it without pulling in
the whole of `main.slint`.

The row now keeps its seat in the layout and its *contents* slide. That is not
a stylistic echo of the device rack: `hot` is answered from
`absolute-position`, so the element that answers it must not be the element
that moves.

**Slint 1.17 requires `z` to be a number literal**, so it cannot be
conditional and the dragged row cannot be lifted over the rows it passes. The
device rack has the same constraint and answers it the same way — the held
thing is marked out by its own chrome (here a shadow under the name plate)
rather than by stacking order, and the gap that opens under it is the drop
indicator either way.
