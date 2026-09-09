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

## Step 02 — console summing

Landed on `feat/console-summing` (2026-09-09). Any channel or bus can be
switched to sum into its destination through a non-linear encode, decoded
there together with everything else that opted in. Off by default and
bit-identical to a linear mixer while it is off.

### The requirement that shaped it: the bus is invisible

Adam, on how the Airwindows pair is actually used and what he wanted
differently: *"you put a 'Channel' plugin on the individual tracks ... then
you bus that ... to a single buss where you have the 'Buss' plugin ... i think
what i was trying to communicate is that i want that buss to be invisible."*

So the mechanism is the plugin pair's and the gesture is not. **Every summing
point decodes**, and the master is already a summing point, so two channels
switched on glue with nothing created and nothing placed in a chain. The whole
of it on the engine side is one extra accumulator per bus that actually has an
encoded feed: decode that, add the linear one. That is Adam's *"the decode
stage is mixed with master to pick up any channels that don't have it switched
on"*, generalised from the master to every bus.

Nesting fell out for free, as the plan predicted, and there is a test that says
so — a console-on bus encodes at its own output and whatever it feeds decodes
it, with no parallel decoder and no special case. That is what makes step 04's
routing free: a track several others feed is a decode point, with nothing
added.

### Two measurements changed the design

**The pair is unscaled.** The plan implied `asin`'s domain bounded the decode
at 0 dBFS; it does not, it bounds the *input*, and the decode's range is
`PI/2` = **+3.92 dBFS**. The obvious fix — normalize to `sin(x * PI/2)` and
`asin(y) / (PI/2)` so the knee lands on full scale — was built and rejected on
two numbers. It clips two sources at -8 dBFS, which is a limiter rather than a
summing law. And its round trip is worst exactly where music is loudest: the
conditioning is `1 / cos`, the normalized curve's `cos` goes to zero at full
scale, and `decode(encode(0.999))` came back wrong in the fifth decimal place,
so the null test could not hold. The unscaled pair nulls to **1.5e-8 against a
0.251 peak (about -156 dBFS)** across the whole range.

The correction is *more* headroom than Adam agreed to, not less, so it needed
no second ruling. `GAIN_STRUCTURE.md` now carries the ceiling as a deliberate
exception to "nothing bounds a sample in the live path", with the reason: the
bound is the effect.

**Which fader is which is a test, not a note.** A bus's fader is after its
decode in the block order, so pulling a bus down is level (measured: 0.0
departure from a pure scale) and pulling its feeders down is drive (0.0064
against a 0.508 peak). Nothing was built to make that true — it falls out of
where the decode goes — but Adam's stated way of using it depends on it, so it
is asserted.

### The test that was wrong, and what it taught

`a_console_strip_and_a_linear_one_share_a_summing_point` was written to assert
that one console strip beside a linear one changes the mix. It failed, and it
was the test that was wrong: a lone console strip is alone in the encoded
accumulator, so it nulls *whether or not* linear strips share its summing
point. That is the design working, and it is now two tests — one saying a lone
switch is transparent (the case a user meets first, where "nothing happened"
is the right answer), and one stating the real property as **superposition
between the two groups**: two console strips plus a linear one must equal the
two console strips alone plus the linear one alone. Measured at 3e-8.

That second form is the one that catches the plausible wrong implementation —
a single accumulator decoded at the summing point, which would pass every
other test in the file while putting a strip whose switch is *off* through an
`asin` it never opted into.

### What was deferred, and why it costs nothing

**`ConsoleMode` on the project.** The plan wanted an enum naming the algorithm
so a second curve would be a defaulted field rather than a format change.
Adding it *later* is the same no-op migration, so shipping a saved field with
one possible value and no control behind it would be capacity without a
decision. Recorded in `02-console-summing.md`.

### Adam's mockup arrived mid-step, and two things changed

He drew the finished strip while this was being built
([`THE-STRIP.md`](THE-STRIP.md), `img/strip-mockup.png`). Two things in it
apply to step 02 rather than to a later one, and both were adopted the same
day:

- **The feature is called "analog sum"**, not "console summing". That is the
  interface word from here; `console` stays the code word, because it is the
  technique's name and what `mooloop_dsp::console` and every document about it
  are written around. `ConsoleButton`'s doc comment records the split so
  nobody reconciles it in the wrong direction.
- **It goes at the foot of the strip, set apart from mute**, not beside the
  destination picker where it was first put. Set apart is the honest place: it
  is the last thing that happens on the way out, after the fader, and it is
  not an output control the way mute and pan are.

The rest of the mockup reshapes steps 03, 05 and 06 rather than this one, and
is written up in `THE-STRIP.md`.

### Where the controls went

A `ConsoleButton` at the foot of the mixer strip, set apart from mute, where
Adam's mockup puts it.

**A channel briefly had one too, and it was removed the same day.** Adam:
*"the summing thing for now is tracks-only."* That is the right reading of the
model this is after -- the console puts its Channel stage on a mixer strip,
and mooloop's mixer strip is a track -- and it resolves the discomfort that
had already been noted here without its cause: the switch had gone on a
channel's *rack row*, which is the instrument's room rather than the console's,
because the mixer draws no channel tracks for it to live on.

Several channels on one track now reach it linearly and the track encodes
their sum, which is what a desk does with a group. The engine tests were
rewritten around tracks and every measured number came back identical, which
is the useful confirmation: the mechanism did not change, only where the
switch lives. It draws the curve rather than
wearing a word: a straight line when off, a sine when on, which is
`reference/ADAM.md`'s "controls should communicate behaviour visually" and its
explicit dislike of tiny low-contrast labels.

The master has no switch, because it feeds nothing and an encode there would
go into a sum nothing decodes. Refused in the engine as well as absent from
the face, so a hand-edited file cannot make the master inaudible.


## Step 04 — the mixer is tracks (model half)

Landed on `feat/dynamic-tracks` (2026-09-09). The track bank stops being a
fixed seventeen: `default_buses()` returns the master alone, tracks are made,
renamed and removed, and the engine materialises a strip per track as a
project loads instead of building all seventeen whether or not a song has
them.

### The default song is in, as far as it goes

`Project::starter_kit` now opens with `Drums` and `Bass`, its four drum
channels grouped onto the first. That is Adam's sketch minus the reverb send,
which waits on step 05 because a send is what would feed it. Grouping needed
no new concept at all: it is the `bus` field several channels share.

### Two things found by doing it

**A short bank could silently drop tracks.** The bus render loop was
`for slot in 0..self.buses.len()`, indexing `render_order` — which is a
permutation over the *whole* address space, so a track's position in it has
nothing to do with how many tracks exist. Walking a prefix visits an arbitrary
subset. It now walks the whole order and skips absent entries. This was
invisible while the bank was always full, and would have been a track that
simply made no sound.

**A channel could name a track that is not there.** `clamp_bus` bounds by the
address space, which is not the same as the bank being that long. Such a
channel now feeds the master; the alternative is a channel that is silently
unheard, which is the worst of the available answers.

### Capacity, measured rather than assumed

`docs/CAPACITY_POLICY.md` forbids small product caps, and seventeen is one. It
also warns that the expensive mistake is dimensioning by a ceiling, so the
ceiling was measured before being raised — `block_cost::track_memory`:

- making strips arrive with the project **saved 2.00 MB** of pure floor;
- a track costs **128 KB** when it exists;
- the ceiling costs **48.2 KB per bus** whatever the song holds, so raising
  `MAX_BUSES` to the `u8` space would add **11.25 MB** before anybody makes
  anything.

Almost all of that is `DeviceMeters`'s spectrum array, which is dimensioned by
two ceilings multiplied together for analyzers that are individually gated and
almost never on. **The recommendation is to fix that first**, after which the
ceiling is free — raising it before would be paying to keep a mistake.

So the cap is unchanged at seventeen and is now written down with the
measurement needed to lift it, rather than being a number nobody had priced.
