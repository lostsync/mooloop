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


## Step 05 — sends

Landed on `feat/sends` (2026-09-09). A track routes a copy of itself to
another track; the source gains a fader for it; the track at the far end is an
effects return by virtue of being sent to and by nothing else. The starter
kit's last third arrives with it: `Reverb`, fully wet, fed post-fader by
`Drums` and `Bass`.

### The step doc was wrong in four places, and three of them made it look bigger

`05-sends-and-returns.md` was written before `docs/TERMINOLOGY.md` and Adam
corrected it on sight. It is rewritten as [`05-sends.md`](05-sends.md), which
keeps the original at the bottom. The correction worth carrying:

**`CompiledBusGraph::destinations` did not have to be replaced.** The doc said
it must be replaced rather than widened, and that claim is what made this the
step "that forces the compiler change" and the reason it was scheduled last. A
track still has exactly one *output* — sends are extra edges, not extra
outputs — so the `[u8; MAX_BUSES]` permutation was never in question. What
died was one *edge* per node, which lives in `compile_latency`, in `reaches`,
and in the block loop.

The other two: `SetCompensation` did not need an edge id, because a producer's
main edge is still one per producer; and `OutletTap::Output` / `TapIsLate` are
not this feature's vocabulary at all. `TapIsLate` is a rule about the
*consumer* — an aux-in edge lands pre-chain, where there is nowhere to put a
delay — and a send carries its own compensation, so it never asks. Two edge
systems wearing one word, and the word was doing the arguing.

### Adam's four rulings

1. **Four is not a limit.** *"i drew 4 sends bc that's how many fit in my
   drawing… we're not limiting to 4."* The area draws the sends that exist and
   scrolls. `THE-STRIP.md` is amended rather than quietly contradicted.
2. **A send is a route to a track, not a kind of track.** Reaper's model, and
   already what `TERMINOLOGY.md` said.
3. **No send device.** The tap point is chosen in the interface. Post-fader
   and pre-fader are built; the drill-down into a device's output or one of
   its declared outlets is stage 2, and `05-sends.md` says what it needs.
4. **Returns are not a thing.** *"i dont really know why we need it. i dont
   think we do?"* Nothing was built for them. A send that would feed back is a
   cycle and is refused under the same rule an output is, and the answer to
   "process the dry and the wet together" is to route both to a third track.

### The measurement that split the work cleanly

**Pre-fader and post-fader arrive at the same time.** Both are after the
chain, and a fader declares no latency. So the two taps Adam wanted first
needed no new latency arithmetic beyond a per-edge delay, and the taps that do
need chain prefix sums — after a named device, or at one of its outlets — fall
out as a separately shippable stage rather than as a thing to cut.

### What the block loop actually gained

Two capture points and one emission point per loop, and a strip with no sends
walks a zero-length slice. A project with no sends holds no bank, no rings and
none of the three scratch buffers, which is a test rather than a claim.

`EngineCommand::InstallBusGraph` is retired: routing stopped being POD the
moment a send carried a compensation ring, so the graph and its sends are
derived and diffed once a pump tick as `StructuralCommand::SetTrackGraph`,
beside the three reconciles already there. Level, tap and enable stay POD, or
a fader drag would rebuild every ring in the plan sixty times a second.

### The back face is cramped at the pane's default height

Found by rendering it. The mixer pane gets whatever height the views around
it leave -- about 200px in the default arrangement -- and the turned-over
strip spends that on the name, a shortened meter and fader, the pan, and two
rows of page tabs, which leaves the page itself about 40px. It **scrolls**
rather than clipping or shrinking its knobs, which is the same answer the
sends area gives, and the dock divider is right there; but the back face is
made for a taller pane than the one it opens in, and the zoomed console is
the drawing that fixes it properly.

Two smaller things the rendering settled. The fader has a *second* floor for
when the strip is turned over -- 48px against 96 -- because on the back face
it is there to be watched rather than dragged, and without that the page got
what was left of 200px after a full-height fader. And the four page tabs do
not fit across 84px of content: a quarter of it is 21px, which carries
neither `COMP` nor `SENDS`, so they are two rows of two with the words
intact. `THE-STRIP.md` sketched them as one row, which is the sketch being a
sketch.

### What is not in it

Named here rather than left to be discovered.

**The drill-down tap points.** Adam asked for a menu that reaches past
pre-fader into "individual signal outs if those exist, or the output of any
device in the chain", with a mark in the chain where the send taps. Stage 1
ships the top two entries of that menu and neither the deeper ones nor the
mark — there is no device to mark yet, because both taps this step has are the
strip's own output. `05-sends.md` says what stage 2 needs: a chain that can
emit a copy after slot *k*, and a prefix variant of `chain_latency` so the tap
has an arrival.

**Sends from a channel.** The engine is strip-level: `SendBank` is keyed by
`EffectTarget`, the channel block loop walks its run, and `compile_latency`
compiles a channel's send. Nothing authors one, because the mixer draws no
channel strips for the control to live on. That is where the drill-down has
anything to drill into, so the two are one decision and it is Adam's.

**A send has no pan.** Level and a tap, which is what the mockup drew.

**Sends are not undoable**, because routing was not. A send edit marks the
document dirty and reaches audio, like `set_bus_output` beside it; a track
*add* still goes through the project-edit path and is. Worth unifying, and not
here.

### The verification pass, 2026-09-10

The step landed and was then gone over against its own acceptance list, the
tooltip rules in `UI_DESIGN.md`, and every document the feature could have
invalidated. The engine half needed nothing: all seven acceptance cases were
already asserted, including the alignment null at three block sizes with the
send disabled. Four things were open, and three of them were in the interface
or in the documents rather than in the audio.

**The target picker said "Send to" in the three places that are not sends.**
`BusPicker` is one component behind four controls — the mixer strip's
destination, the track face's `Output`, `main.slint`'s channel destination,
and the send picker — and every row in its popup was tooltipped `"Send to " +
name`. That string predates this step and was harmless until it landed: what
step 05 changed is that *send* became a specific thing three of those four
controls are not, which is the same correction the step already made one level
up when it labelled the track face `Output` rather than `Sends to`. The row
tooltip is the track's name now.

Its refused row keeps its sentence, and that is deliberate rather than an
oversight against the tooltip rule. `ToolButton` publishes `hint` from its
`TouchArea`, which is disabled on a refused row, so the status bar can never
hear it — where Slint lowers `Tooltip` to a `TooltipArea` with its own hover
detection, which still fires. It is the one place in the interface where a
sentence in a tooltip is the only mechanism that works, and it now says so in
a comment so the next audit does not "fix" it.

**Two acceptance cases were true but untested, both on the half a test would
not notice.** The send row's remove button had no test, although the row's own
padding comment claims a test found it sitting under the scroll bar — which is
the failure mode where every control to its left keeps working, so nothing
looks wrong. And the loop refusal was asserted in `mooloop-core`, where the
rule is, and in `mooloop-session` only for an *output*: `add_send` builds its
own `RoutingLoop` rather than sharing `set_bus_output`'s, so the two could name
different tracks and only one would be caught. Both are tests now, plus the
degenerate self-send a user reaches first by clicking their own row.

**Four documents still said sends were absent.** `CURRENT.md` in two places
(its aux-in entry and its mixer-gaps entry, the second of which also still
said tracks could not be renamed), `AUDIO_ARCHITECTURE.md`'s migration step 6,
and `JOURNAL.md`'s open threads. The correction worth carrying out of them is
the one step 6 had backwards: parallel sends were written there as the thing
the typed-edge model existed to grow into, and they did not extend it at all.
A send is a producer-side edge carrying its own compensation, so it never asks
`compile_audio_graph` anything. Two edge systems, one vocabulary — which is
recorded in all three now rather than resolved.

`UI_DESIGN.md` also gained the rule this step's sends area is the first
instance of: a list of things a user makes draws exactly the ones that exist
and scrolls, rather than reserving bays for a number somebody drew once. The
scroll-bar-over-the-viewport trap is recorded beside it, because that is what
makes the rule cost a test rather than nothing.

## Step 03 — the channel strip

Landed on `feat/channel-strip` (2026-09-11). Four sections on every track --
an input stage, a four-band EQ, a compressor and a polarity switch -- under
one strip-wide voicing, all out by default, drawn on the mixer strip's new
back face and on a pinned row in the track's device rack.

### The step doc was a stack of corrections, and one of them was wrong

`03-the-channel-strip-device.md` opened by proposing a composite
`EffectKind` you insert, and then carried two rounds of amendment saying it
is not one. It is rewritten as a work order; the retired premise is here
rather than at the bottom of that file, because a plan you have to read
backwards is one nobody reads.

The claim that had to be replaced rather than amended: the old file had a
voicing selecting *"knee, ratio law, attack and release curves, EQ band
frequencies and Q"*. **A voicing that moves a band's frequency means the
3 kHz on the face is not the frequency being boosted**, and four voicings are
then four sets of lying knobs. So the rule this step is built on is that **a
voicing selects laws, never values**: the harmonic profile, the tilt, the
slew limit, the Q law, the curve above the knee and the programme dependence
-- every one of which is either invisible or *drawn*, by `EqResponseDisplay`
and `DynamicsCurveDisplay`. The Q law is the one case where it touches a
knob's meaning, and it is precedent rather than an exception: `EqQProfile`
already does exactly that to the seven-band EQ.

Two of the voicing's three thirds are new. `Punch` *eases* its ratio above
the knee, which is the mechanism behind "louder at the same level" -- a
transient over the knee is compressed less than the passage under it -- and
`Iron` steepens toward a vari-mu. One signed number does both, equal to the
knob's ratio at the threshold whatever the bend, and `a_bent_ratio_stays_
monotone` walks 90 dB in tenths because a gain computer whose output falls as
its input rises inverts transients.

### The pin is one statement, and now a testable one

`THE-STRIP.md` left open where the strip sits in a track's chain.
**Decided: at the head** -- the drive stage is an input stage, a track's rack
is glue and post, and the device people put last on a track is a limiter.
`mooloop_core::mixer::STRIP_PIN` is that decision, read by the bus block loop
*and* by the rack to place the pinned row, so the drawing cannot say one
thing while the audio does another.

`RenderState` holds it as a field initialized from the constant, for the
reason `skip_idle` is a field: a test renders the same project both ways and
says what the difference is. The proof needs a device that does not commute
with the strip, which is a 36 dB/oct low-pass where the compressor is
looking -- at the head it detects the whole signal and clamps hard, at the
tail it detects what the filter left.

It costs the latency compiler nothing, as the plan predicted: biquads, a
detector and a memoryless shaper declare no latency and there is no
oversampler, so `chain_latency` is unchanged.

### No control on the faces has a number to drift from

Every other device face mirrors its descriptor's range in the markup, and
`slint_face_agreement.rs` exists because that second copy drifts. The strip's
faces mirror nothing: `install_strip_spec` hands the markup the whole
descriptor table and every parameter id once at startup, as the `StripSpec`
global, so `StripSpec.drive-db` *is* `STRIP_DRIVE_DB` and a knob's minimum
*is* its descriptor's.

That makes this feature's agreement test a different shape and a stronger
one. `tests/strip_face.rs` reads the table back out of the window and holds
it to `StripParams::descriptors()`, checks the band arithmetic the markup
does lands on the ids `strip_band_param` mints, and **fails if a bound is
ever spelled on a control in `strip.slint`** -- which is how the second copy
of a range gets written in the first place, one knob at a time, to make it
clearer.

The two numbers the markup does hold are `EqResponseDisplay`'s axes, which
`StripEqPage` inverts to turn a dragged point back into hertz and decibels
(`gain * 36 - 18`, as `eq-device.slint` writes it). Those are the display's
convention rather than a parameter's, and the frequency one is deliberately
wider than any single band -- but the gain one *coincides* with the bands'
gain range, and widening that range in the descriptor alone would leave a
dragged point unable to reach the top of the knob. Found reviewing the step;
`the_response_plots_gain_axis_is_the_bands_gain_range` is the assertion.

The same principle settled the compressor's curve. `DynamicsCurveDisplay`
holds a gain computer of its own and could draw a threshold, a ratio and a
knee unaided -- but not the voicing's bend, and teaching the markup the bend
would be two formulas under one name. It now takes an optional `curve-db`
and plots what Rust sampled with the same two functions the audio path calls.

### Three duplications closed on the way past

**The shared biquad**, which is `docs/plans/archive/adopt-shared-biquad-in-eq/`
absorbed: `effects/eq.rs` had kept its private copy of the RBJ cookbook for a
fortnight after the shared one was promoted out of it, so the strip would
have been the third. The shared one gained `is_at_rest` (which the private
copy had) and `shelf_slope`, which is new -- a band's `q` is its Q as a bell
and its **slope** as a shelf, so a shelf with no slope parameter would be a
dead knob.

**`eq_effective_q`** is in `mooloop-core` now. The proportional-Q law had one
caller; it now has three in two crates -- the seven-band EQ, the strip's bank,
and every display that has to plot the curve that is *running* rather than
the one the knobs imply -- and the copy that drifts is the one deciding what
is heard.

**`preamp_voicing`** moved from `effects/preamp.rs` into `preamp.rs`. The
device and every track's strip now read one mapping from the persisted choice
to the measured table, with a test that the two agree: a strip on `Iron` and
a `Preamp` on `Iron` have to be the same stage, or the voicing means two
things.

### What the doing found

**The programme-dependent release had to be timed off the release, not the
attack.** The first version charged its second detector at twelve times the
*attack* -- 12 ms at a 1 ms attack -- which fills on a single transient, and a
stage that fills instantly is a slower release with extra arithmetic: a short
burst and a long passage released within 3% of each other. Both constants are
multiples of the knob's release now, so "a long passage" means long by the
standard the user set.

**And the test that found it was measuring the wrong quantity.** It compared
the RMS of a quiet tail, which separated the two cases by three percent even
after the mechanism was right. What programme dependence changes is a
*time*, so it counts blocks to recovery instead: a factor of two, with `Moo`
as the control at 1%.

**Pan is on the back face.** Adam's mockup draws mute / solo / polarity in
the column beside the fader and `meter vol pan` on the back, so the front
face has no pan for the first time. It is on the back and on the track's rack
face, which satisfies the reachability rule; recorded here because it is the
one place the built strip will surprise someone who knows the old 62px one.

**A section switched *in* clears its own state first.** Not tidiness: a
filter bank and a detector last fed audio before the section was switched out
would otherwise put that audio into the first block back, which is the one
artefact "free while it is out" could plausibly produce.

### What is not in it

Named here rather than left to be discovered.

- **The zoomed console**, which `THE-STRIP.md` settles as the third drawing.
  The paned back face and the pinned rack row already satisfy the
  reachability rule, so it is comfort rather than capability, and it needs
  nothing this step did not build: `StripSections` is the full-format
  arrangement and takes the room it is given.
- **Automation and modulation of strip parameters.** A lane's target is an
  `EffectTarget` plus a slot plus a param id, and a strip is not a slot. The
  ids are ready for it: they start at 16 because
  `modulation::STRIP_PARAM_VOLUME` and `STRIP_PARAM_PAN` are 0 and 1 of what
  is conceptually the same strip -- `ParamOwner::Strip`, already addressable
  by a route -- so the two tables can later become one table without
  renumbering an id automation has persisted.
- **A live gain-reduction meter on the COMP page.** `Strip::dynamics_frame`
  exists and reports the block's extremes; what is missing is a carrier for
  it, since the strip is not a device and `DeviceMeters` is addressed by
  device. The page draws the static curve, which is what says what the
  voicing is doing.
- **A strip preset.** The preset system's unit is a device and a strip is not
  one. The voicing is the thing worth recalling and it is one value.
- **The voicing's own EQ curve.** `06-preamp-modelling.md` asks for a broad
  presence lift on `Iron` and a top-end bump and rolloff. Not built, for one
  reason: it would make the strip's drive and `EffectKind::Preamp` two
  different stages under one voicing name. When somebody measures a curve it
  belongs in `PreampVoicing`, where both get it.
- **Undo.** A strip edit marks the document dirty and reaches audio as a
  command, exactly like `set_bus_output` and a send level; it does not go
  through the project-edit path, so it is not undoable. The same gap step 05
  recorded for sends, with the same fix -- unifying the two paths -- and the
  same reason for not doing it here.
- **Solo**, which is a monitor tap rather than a control and is the largest
  unbuilt thing on the mockup. `LOOSE_ENDS.md` carries it and `MIXER_PLAN.md`
  specifies it. A dead solo button is not drawn.

### The review pass, 2026-09-11

The step landed and was then gone over against its own claims, `AGENTS.md`'s
duplication standard, and every document it touched. The audio was sound; six
things were open, and **four of them were tests that were green while
guarding nothing**, which is the failure mode `AGENTS.md` opens on and is
worth reading as a pattern rather than as four items.

**The Q law was written twice, in two crates, with nothing comparing them.**
`StripVoicing::proportional_q` is what the bank is designed against.
`StripParams::proportional_q` is the same rule spelled again in
`mooloop-core`, because `strip_row` plots the running Q and `mooloop-core`
cannot see the DSP table. `only_two_voicings_narrow_a_boosted_band` checks
the second against a hard-coded list of four booleans and therefore does not
check it against the first. Flip one and the response display draws a curve
the audio is not running -- which is *the* failure "a voicing selects laws,
never values" was adopted to prevent, since for the Q law the plot is the
only place the law is visible at all. `a_voicings_q_law_is_the_one_the_
display_plots` holds the two together now.

**`the_id_space_starts_after_the_faders_and_has_no_holes` sorted the ids
before checking them**, and a sorted copy cannot see the property the markup
depends on. `StripSpec.spec(id)` reads `params[id - first]`: the descriptor
behind a knob is found by arithmetic on its *position* in `DESCRIPTORS`, so
swapping two rows to read better gives every knob below the swap its
neighbour's range, name and unit, with no Rust change to blame. It walks the
table's own order now.

**`a_flat_band_is_bit_identical_to_not_having_it` would have passed with the
skip removed.** A cookbook peak or shelf at 0 dB has `b == a` term for term,
so its normalized coefficients come out exactly `Biquad::identity`'s --
running the stage produces the same samples, and bit-identity cannot tell the
two apart. The claim the code makes is that the stage is *not run*, and
`active_len` is the only place that is visible.

**`a_track_reused_by_a_shorter_project_loses_its_strip` did not cover the fix
it sits beside.** It reloads a project, so it goes through the arm that
installs parameters and would pass whether or not `ed83c58`'s reset were
there. `a_document_arriving_clears_the_strip_it_lands_in` asks the strip
directly instead, with both sections in on both sides, because the whole of
what the reset adds is state and the only block it shows up in is the first.

Two things in the code itself:

**A section switched in glided up from whichever values it last ran.** A
smoother only advances while its own section runs, so every knob turned while
a section was out is a value that smoother has never seen. Switching in then
spent five milliseconds at the old setting -- the same artefact "a section
switched *in* clears its own state first" exists to prevent, one field over.
The switch-in path snaps the section's smoothed values to its knobs now.

**The faces spell two numbers after all**, which three documents and the
markup's own header said they did not. They are `EqResponseDisplay`'s axes,
which `StripEqPage` inverts to turn a dragged point back into hertz and
decibels -- the display's convention rather than a parameter's, and
`eq-device.slint` writes them the same way, so they are not the fault the
claim was about. But the gain one *coincides* with the bands' gain range, and
the compressor curve is sampled in Rust over a floor the display holds a
private second copy of; move either and the drawing is stretched rather than
merely wrong at an edge. Both are asserted in `tests/strip_face.rs` now, and
the claim is narrowed to what is true: no *control* on the faces declares a
bound.

One thing was left rather than fixed, and is in `LOOSE_ENDS.md`: the
compressor curve is drawn at `w/d mix` 1 and `in trim` 0, both of which move
the line and both of which are knobs on the same page as the plot. The
arithmetic is not the obstacle -- the audio path computes exactly this per
sample -- it is that changing what the drawing means is Adam's call and not a
reviewer's.

## Step 06 — preamp modelling

Closed with step 03, having landed ahead of it: `mooloop_dsp::preamp` was
built 2026-09-09, re-based on 106 measured units 2026-09-10
(`spikes/preamp-measure/`), and given `EffectKind::Preamp` the same day so
that a rack had somewhere to automate gain in the middle of a chain. Step 03
is what finally put it where the plan always said it went -- the strip's own
input stage, under the voicing selector that governs all three sections.

What remains in `06-preamp-modelling.md` is **authoring and measurement
rather than building**: supply sag and hysteresis (priced, and the second is
a per-sample ODE solve), a measured fit for `tilt_db`, `tilt_hz` and `slew`
where only the harmonic half is fitted, and Adam's decision about whether
`Grip` gets the drive-dependent low shelf the real SSL turns out to have
instead of a tilt. None of those is a step; each is a row of a table and a
pair of ears.
