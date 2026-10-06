# 06 — song inlets

Everything the song can send into the patch comes in as an inlet tag: a
channel's gate, a generator's control outlet, the mod wheel and aftertouch,
and the transport (beat, bar, where it is in the pattern, which pattern).
After this step the kick pump can be made from the kick's gate with no
audio sidechain, which is the case Adam opened with.

## Making a tag

Tags are made from the canvas, not typed: a right-click on empty canvas
(or the pane's toolbar) offers **Inlet**, **Notes in** and **Notes out**,
as the prototype's *Song ports* buttons do. A new tag is unbound (`[ ]`);
clicking it opens the list of what can feed it, grouped by channel, with the
transport first. Notes in and Notes out do nothing until step 07.

## The sources

`InletSource` (step 01) grows:

| Source | Value | Fires | When it is read |
| --- | --- | --- | --- |
| `Gate(channel)` | 1 while a note is held on the channel | each NoteOn | this block (step 02) |
| `Outlet(channel, outlet)` | the generator's control outlet (velocity, gate and the rest, `PERFORMANCE_DESCRIPTORS` and the outlet table) | when the outlet is a gate and opens | **one block late**, as routes read them today (`render.rs:10334`) |
| `Performance(channel, source)` | the mod wheel or aftertouch | never | this block |
| `Beat` | a ramp 0 to 1 across each beat | each beat | this block |
| `Bar` | a ramp 0 to 1 across each bar | each bar | this block |
| `PatternPosition` | a ramp 0 to 1 across the playing pattern's length | each time the pattern starts | this block |
| `Pattern` | the playing pattern's number, as 0 to 1 across the song's patterns | each change | this block |

**A tag that reads a block late says so**, with a small mark on its arrow,
so a patch that depends on it is not a surprise.

**Routes from outlets become tags.** Today a route's source can be a
generator outlet or a performance source directly (`ModSourceRef`,
`mod_metadata.rs:317`). On load, each such route gets an inlet tag bound to
that source (one per source, shared), and the route's source becomes the
tag's outlet. `ModSourceRef` is then only ever a box's outlet. The engine's
`ChannelSources` reads (`render.rs:1279-1373`) move into the tag boxes.
The outlet band in the shelf (`OutletBand`, `modulation-shelf.slint:108`)
goes: the tag list replaces it.

## The transport

Modules see `song_beats` today (`dsp/modulator.rs:879`), not the bar or
the pattern. The render loop already knows both:
`Sequencer::current_pattern` (`sequencer.rs:687`), `pattern_length_ticks`
(`:512`) and, in Song mode, `covering_pattern_at` (`:726`). Fill a
per-tick transport table beside the gate table (`render.rs:10276`) and pass
it to `tick_block`. It must follow loop folds and cuts exactly as
`song_beats_for` does (`render.rs:991`), and read nothing while stopped
(the ramps hold, nothing fires).

**Which pattern, in Song mode.** Several placements can play at once. The
tag reads the placement on the topmost playlist row that covers the tick,
and says so in its picker. A tag bound to one playlist row is a later
variant if Adam wants it.

## Done when

- Each source above can be bound to a tag, saved and reopened, and has an
  engine test of its value and its fires over a looped pattern and a song
  with a cut.
- Routes from outlets and performance sources convert to tags, and the
  null test (step 02) still passes.
- `LISTENING.md` has the kick pump: the kick's gate tag into an `env`, then
  `* -1`, assigned to a bass channel's volume; and a `Bar` ramp into an
  `lfo`'s `retrigger`.
