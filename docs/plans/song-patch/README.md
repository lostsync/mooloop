# Song patch

The Modulation pane becomes one patching canvas for the whole song. Boxes
are typed as text (`lfo`, `* -0.5`, `select 6`, `chord min7`), joined by
two kinds of wire: control wires that carry a value every control tick, and
note wires that carry notes, each NoteOff following its NoteOn through every
box. The song's sources (a channel's gate, the transport, a channel's notes)
come in as tags on the canvas; assignments and note outputs go out the same
way. Planned 2026-10-06, after song modulation (`archive/song-modulation/`)
landed and Adam tried it. Linear: project **Song patch**, one issue per step
(`00-status.md` has the list).

The prototype is the design: https://claude.ai/artifact/BBYu543x1WZ8VAf2MrGCUY
(*Mooloop Song Patch*, 2026-09-27). Read it before any UI step. What it
leaves open, this plan decides, and `00-status.md` lists each decision so
Adam can overturn it.

## Why, in Adam's words

**2026-09-23**, on what the modulation rack was meant to be:

> generating modulation from LFOs and step seqs is fine, but what's truly
> interesting is when you can start letting signals modify other signals, or
> trigger them, or invert them, or whatever, and doing it across device
> lines is how you end up with crazy cool syncopation and stuff like that.
> [...] i could take the gate signal from my kick device and just invert it,
> and apply that to the volume. now im processing 0 samples of DSP to get
> the same effect.

> what i am actually picturing is like if bitwig's grid were data-only and
> global scope [...] i want the functionality of a node based patching
> environment built into the thing. [...] if you wanna do node based
> processing, maybe just draw some damn nodes like a grownup.

**2026-09-27**, the patches it should make, and the feel:

> 1. chord inversions
> 2. base note to modal chord
> 3. note probability filter
> 4. lfo seq - so like there are n lfo slots and they can advance with
>    transport or note on, sorta like our step mod but for LFOs
> 5. probably something to play the buffer device sorta in auto/algo mode.

> notes: yeah, definitely
>
> patterns: [...] it should probably be able to know the pattern number and
> maybe its length.
>
> feel: lean grid, i'd say. less busy to look at.

> it's a patch environment for the whole song. [...] yeah im pretty much
> just describing max or pd but inside fruityloops with bitwig devices.

and on the prototype:

> i really like this. [...] i'd make select take an argument so you arent
> stuck with 4. [...] i'd want a way to manually position the angles in the
> cables. and this blinking is a bit loud. we'd have to tune it, and maybe
> optionally make it tunable

**2026-10-06**, after trying song modulation: *"everything worked as
intended. you can go ahead and do the plan."*

## What Adam agreed (2026-09-27)

- one canvas for the whole song, in its own pane;
- a curated set of box kinds typed as text, not an open language. Pd itself
  may come later as one box that runs a `.pd` file;
- two kinds of wire, control and note;
- a box's face opens in place on the canvas;
- song inlets and outlets are tags. A preset keeps them as empty slots
  (*"kick goes here [ ]"*);
- depth is set with today's Assign drag; math boxes do the scaling;
- cable bends can be dragged, and cable activity is a setting.

**Mock-ups illustrate; they don't specify.** Adam, on the tracker that an
earlier mock-up showed: *"once that agent saw the mockup it was dead set on
delivering a literal reality but i was just kinda trying to show it what i
was thinking."* The prototype's layout, colours and vocabulary sidebar are a
sketch to build toward with mooloop's own parts (`MiniKnob`, `KnobField`,
the theme tokens), not a spec to copy.

## What it reverses

- **`SCOPE.md`, *Out, explicitly*: "the node-based patcher [is] out".** It
  is in, on Adam's word. This plan's PR edits that line and `FOCUS.md`.
- **`ModulationShelf`'s "does not introduce patch cords"**
  (`ui/modulation-shelf.slint:641`). Step 03 replaces the grid it describes.
- **Modules run in list order** (`dsp/modulator.rs:870`,
  `PROJECT_FORMAT.md:712`). The song-modulation plan claimed the order was
  not part of the save format; it is, implicitly, because the array order
  is the evaluation order. Step 02 makes the order a compiled topological
  sort, so it stops being.

**Kept:**
- **Base plus offset.** An assignment adds an offset around the knob's value
  or lane; it never replaces it.
- **The control rate.** Control wires tick every 32 frames.
- **Durable identity.** A box is named by its `ModSourceId`, never by its
  place in a list or on the canvas.
- **Device-local modulation.** Per-voice envelopes and ML-P8's internal LFOs
  stay inside their device. Control on the canvas is monophonic.
- **Every saved song sounds the same.** A song saved by 0.1.7's song
  modulation opens as a patch and plays exactly as before.

## What exists (survey, 2026-10-06, `main` at `b7ee1fe`)

**The model** (`core/src/modulation.rs`):
- `SongModulation` (`:2865`) is `{ modules, routes, next_source_id }`, on
  `Project.modulation` (`core/src/project.rs:941`). No count cap.
- `SongModule` (`:2748`): `id`, `name`, `seed`, `input: InputSource`,
  `rack: Option<RackSeat>` (a home seat used for naming and presets), and
  `params: ModulatorParams` (`:1367`, one of five kinds).
- `InputSource` (`:2695`) is `None`, `ChannelNotes(ChannelId)` or
  `Module(ModSourceId)`. Its doc calls it "the song patch canvas's inlet in
  its first form, so it is an enum to grow". **Each module has one input.**
  Only Math reads another module.
- `ModRoute` (`:1626`): a source (`ModSourceRef`: a module, a channel's
  generator outlet, or its mod wheel or aftertouch), a `ParamAddr`
  destination anywhere in the song, depth and polarity.
- The save table is hand-written (`SavedSongModulation`, `:2874`); module
  order in the array is the evaluation order.

**The engine:**
- `SongModulator` (`engine/src/song_modulation.rs:30`) is built whole off the
  audio thread from a `CompiledModulation` (`core/src/modulation_plan.rs:95`)
  and swapped in by `StructuralCommand::SetModulation`. State carries by id.
- `ModulatorSet::tick` (`dsp/modulator.rs:879`) runs every module in list
  order each 32-frame tick. A Math module reads `outputs[reads]`: this
  tick's value if listed earlier, otherwise last tick's. That is the only
  cycle handling.
- Notes reach modules only as **per-tick counts** for one channel
  (`NoteGateEvents { note_ons, note_offs, choke }`, `:190`), built from
  every channel's event list before anything renders (`render.rs:10276`).
- Modules see `bpm`, `sample_rate` and `song_beats` (position in beats,
  interpolated per tick). **Not bar, pattern number or pattern length**;
  those live on the `Sequencer` (`sequencer.rs:687`, `:512`, `:726`).

**The note path** (`engine/src/render.rs`, `process_block_inner` `:10109`):
- `Event::NoteOn { id: u64, note, velocity }` and `NoteOff { id, note }`
  (`dsp/src/event.rs:19`), in one fixed `EventList` per channel (256
  events, never allocates, `push_ordered` by offset and priority).
- Per block: owed releases, then `Sequencer::schedule` per span, then
  choke groups, then `observe` into the strip's `SequencedVoices`
  (`voices.rs`), then keyboard and audition notes (`dispatch_auditions`),
  then the modulation gate table, then each channel renders.
- **Nothing transforms notes between the sequencer and the instrument**
  except swing and choke groups. `AudioNode::process` has an `events_out`
  documented for "arpeggiators, MIDI-generating effects" that nothing uses
  (`dsp/src/node.rs:567`).
- Stop, pause, seek, pattern switch, mute and panic release through
  `SequencedVoices` and `release_all_voices` (`render.rs:8844-9214`).
  Sequencing & Time owns that lifecycle (`TEAMS.md:57`).

**The pane** (`ui/modulation-shelf.slint`, 1687 lines): a module grid, an
outlet band, an Add list, the selected module's surface with its input
picker and Assign, and its routes. Its comment at `:790` says the canvas is
meant to replace the grid. No Slint view in the repo draws cables; the
nearest are the automation lane's per-segment `Path`s (`main.slint:7519`)
and the piano roll, whose one `TouchArea` does every gesture with hit
testing in Rust (`ui/piano-grid.slint:335-344`). The canvas follows the
piano roll.

## The shape

**A box is a module.** `SongModule` grows a place on the canvas and a
spelling, and keeps its id, name, seed and params. The five kinds stay, and
step 04 adds the rest of the vocabulary. A module's single `input` becomes
**wires**: the song's patch has `wires: Vec<Wire>`, each from one outlet of
a box or tag to one inlet of a box.

**A tag is a box with no face.** Song inlets (a channel's gate, a channel's
notes, the transport) and outlets (notes to a channel) are boxes of their
own kinds, bound to something in the song or left unbound (`[ ]`).

**An assignment is a route.** `ModRoute` stays: a source, a `ParamAddr`,
depth and polarity. Its source gains an outlet index, so any box outlet can
drive a knob. The canvas draws each route as an assignment tag wired from
its outlet and reading `ML-M1 9 › Cutoff +40%`. Arming an outlet and
dragging a knob works as it does today.

**The engine compiles a graph.** Off the audio thread, the session compiles
boxes and wires into an evaluation order (topological; a wire that closes a
loop reads the previous tick, and the canvas marks it). The audio thread
runs the order every control tick and never sees the graph.

**Notes run before any channel renders.** Note boxes run once per block,
after the sequencer and keyboard have filled every channel's event list and
before the first channel renders, so a note sent from one channel to
another arrives in the same block, sample for sample.

## Steps

| # | Step | Team | Milestone |
| --- | --- | --- | --- |
| 01 | The patch in the song | Document & Session | Boxes and wires |
| 02 | The engine runs a graph | Parameters & Control | Boxes and wires |
| 03 | The canvas | Interface | Boxes and wires |
| 04 | Typing a box | Parameters & Control | Boxes and wires |
| 05 | Faces, bends and cable activity | Interface | Boxes and wires |
| 06 | Song inlets | Parameters & Control | Boxes and wires |
| 07 | Notes through the patch | Sequencing & Time | Note wires |
| 08 | Note boxes | Sequencing & Time | Note wires |
| 09 | Patches you can keep | Document & Session | Patches you can keep |
| 10 | The contract, rewritten | Parameters & Control | Patches you can keep |

Each step is blocked by the one before it, except that 09 needs only 06.

**What is audible when.** After 02 a song from 0.1.7 plays exactly as
before (the null test). After 04 Adam's patch 4, the LFO sequence
(`counter` advancing a `select 4` over four `lfo`s), can be made and heard.
After 06 the kick pump can be made from the kick's gate with no audio
sidechain. After 08 patches 1 to 3: chord inversions, a root note to a
modal chord, and a note probability filter.

## Not in this plan

- **Patch 5, the algorithmic Buffer.** It needs the Buffer to take actions
  (play, jump the playhead), not just parameters. That is a device change,
  and belongs with the devices push. The canvas can already drive Buffer's
  parameters.
- **Patterns played on conditions.** Adam: *"being able to have patterns
  play based on conditions could be cool."* The graph can read the pattern
  number and length (step 06); choosing what plays is a Sequencing question
  with Song mode in the way, and is not this plan.
- **A Pd box** that runs a `.pd` file. Agreed as a later escape hatch.
- **MIDI out.** A `notes to` tag reaches a channel, not a port. MIDI out is
  0.2.0 and not next (`FOCUS.md`).
- **Modulators inside containers** (MOO-160), **audio-rate wires**, and
  **per-voice control**.
- **Reason's rack flip** (Tab to the back of the rack) as a second view of
  the same cables. Raised in September, not decided.
- **Durable-id destinations.** Assignments stay `ParamAddr`, rescoped on
  every channel and track edit, as song modulation left them.
