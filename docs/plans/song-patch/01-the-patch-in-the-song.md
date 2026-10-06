# 01 — the patch in the song

The song's modulation set becomes a patch: boxes with a place on a canvas,
and wires between them. The document, the save file and the session learn
it; nothing in the engine or the pane changes yet (step 02 and step 03).
Headless, tested without a window.

## The type

In `core/src/modulation.rs`, `SongModulation` (`:2865`) keeps its name and
its place on `Project.modulation`, and grows:

- **`SongModule`** (`:2748`) gains `at: CanvasPoint` (whole canvas units,
  `i32` x and y) and `open: bool` (its face shown in place, step 05). Keep
  `id`, `name`, `seed` and `params`. **`input` goes**: wires replace it.
  `rack`, the home seat song modulation kept for naming and channel
  presets, stays until step 09 replaces what presets do with it; a new box
  is named by its kind and a number and gets no seat.
- **`wires: Vec<Wire>`**, where a `Wire` is
  `{ from: Jack, to: Jack, bend: Option<Bend> }` and a `Jack` is
  `{ box: ModSourceId, port: u8 }`. `bend` is step 05's; add the field now
  so the format does not change twice.
- **An inlet takes at most one wire; an outlet feeds any number.** Two wires
  into one inlet are refused by the session and repaired by integrity (keep
  the first). Summing is a `+` box's job, so a patch reads the way it runs.
- **Tags are boxes.** Add `ModulatorParams` variants for the tag kinds now,
  with no DSP yet, so the format is settled before step 06 fills them in:
  `Inlet { bind: Option<InletSource> }`, `NotesIn { channel:
  Option<ChannelId>, take: bool }`, `NotesOut { channel: Option<ChannelId>
  }`. `None` is the empty `[ ]` slot. `InletSource` starts as
  `Gate(ChannelId)`; step 06 adds the transport sources.

**Every kind declares its jacks.** A `ports()` on `ModulatorKind` (beside
`descriptors`, `:1597`) names each inlet and outlet and its sort, control or
note. Today's five:

| Kind | Inlets | Outlets |
| --- | --- | --- |
| `lfo` | `rate`, `retrigger` | `out` |
| `env` | `gate` | `out` |
| `step` | `advance`, `reset` | `out` |
| `random` | `trigger` | `out` |
| `math` | `in` | `out` |

The jack table is the one place a port's name and sort live. The canvas's
labels, the session's refusals and the engine's inlet buffer all read it.

**Routes name an outlet.** `ModSourceRef::Id` (`mod_metadata.rs:317`)
gains a port: `Id { id, port: u8 }`, port 0 for every route that exists.
Outlet and performance sources stay as they are, and step 06 turns them
into inlet tags on the canvas. A route also gains `at: Option<CanvasPoint>`,
where the canvas draws its assignment tag (step 03).

## Converting a song saved by song modulation

On load, before `resolve_math_inputs` (`modulation.rs:3023`, which goes):

- each module whose `input` is `ChannelNotes(c)` gets an `Inlet` tag bound
  to `Gate(c)`, placed left of it, wired to the inlet that input fed: the
  LFO's `retrigger`, the Envelope's `gate`, the Step's `advance`, the
  Random's `trigger`. **One tag per channel**, shared by every module that
  read it, which is how Adam drew it (*"two patches can share one gate
  tag"*);
- a Math module's `Module(m)` becomes a wire from `m`'s `out` to its `in`;
- modules with no `at` are laid out in a grid in their list order, so the
  first open of a converted song looks like the grid it had;
- the old list order is kept as the order boxes are listed in the file, and
  step 02 checks that the compiled order evaluates every converted song the
  same as list order did.

A song with no modulation still writes no `modulation` table.
`FORMAT_VERSION` does not move (`PROJECT_FORMAT.md:406`).

## Session verbs

In `session/src/modulation.rs`, keeping the existing verbs:
- `add_box(kind, at)`, `move_box(id, at)`, `remove_box(id)` (drops its wires
  and its routes), `connect(from, to)` and `disconnect(to)`. `connect`
  refuses a control outlet into a note inlet and the reverse, an inlet that
  already has a wire (the canvas replaces by disconnecting first, in one
  undo step), and a box into itself. A wire that closes a loop is allowed
  (step 02 says how it runs).
- `set_module_input` (`:424`) and `module_input_options` (`:450`) become
  `connect` and a list of the outlets that can feed an inlet.
- `arm` takes a `Jack` (an outlet), not a module.
- Every verb is one undo step through `with_gesture_history`; a drag that
  moves several boxes is one.

## Done when

- `SongModule` has `at` and `open` and no `input`; `SongModulation`
  has `wires`; the five kinds and the three tag kinds declare their jacks.
- Every song under `project/tests/fixtures/songs/` and the converted 0.1.6
  fixture in `release_corpus.rs` loads, converts, saves and loads again
  with the same boxes, wires and routes.
- A song saved by `main` before this step, with an Envelope gated by one
  channel, an LFO retriggered by the same channel and a Math reading the
  LFO, converts to one shared gate tag, three wires, and the same routes
  (a unit test).
- Integrity repairs a wire naming a missing box or port, a second wire into
  one inlet, and a wire of the wrong sort; each has a test.
- `PROJECT_FORMAT.md` documents the boxes, wires, tags and the conversion.
