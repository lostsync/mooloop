# Devices

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## The device rack and its host

- **A track's rack reads in signal order**: its head (name, routing,
  polarity, then its channel strip's drive/EQ/comp, in one row), its own
  devices as inserts, and its fader last.

- The source device is a picker rather than a chip per instrument.
- **Changing a channel's device leaves what was aimed at the old one inert,
  and keeps it.** A lane, a modulation route or a MIDI binding on the
  sampler's Cutoff does not start moving whatever the new device calls the
  same id (the drum synth's snare tone). It stays in the song and is saved
  with it: the shelf lists the route as an unavailable destination, and the
  mapping list shows the binding as an unavailable parameter. Switching the
  channel back makes all three work again. The new device still arrives at
  its defaults (MOO-192). An inert lane still counts toward the channel's
  eight lanes per pattern, and the cap stays (MOO-270). The lane picker lists
  a pattern's inert lanes as missing rows (italic, thin) after the device's
  own rows, under the device they were drawn on. Picking one opens the lane,
  and Remove lane takes it out as one undo step, which frees its slot. No new
  lane can be made on a device the channel no longer runs.

- A horizontal lower device rack with one fixed-height 3U source face followed
  by a chainable effect chain (slots are added by kind from the rack's add
  slot, bypassed or removed from their shared host header, and reordered by
  dragging a header). **A drag shows itself**: the dragged face lifts off the
  rack with a shadow and follows the pointer, the row it came from stays as
  an outline, and the rows between the grab and the landing slide aside to
  open a gap exactly one row wide where it will drop -- inside a container's
  box when that is where it lands. The landing is whichever row the pointer
  is over, measured from that row's own bounds, so a drag across a four-unit
  device counts it as four units wide. Sampler, drum synth, v1 mono synth, ML-M1, and
  poly synth faces share the same rack chrome and preserve their dimensions at
  narrow widths through horizontal scrolling. Sampler controls are divided
  into Sample, Voice, and Tone pages; the v1 mono controls into Osc,
  Amp/Filter, and Mod pages; and poly controls add a VOICE page for polyphony,
  stereo spread and **mono mode**. Mono mode is not `Voices = 1`: a pool of one
  voice steals from itself, so releasing the newer of two held notes leaves it
  on the note that is no longer down. Mono mode gives the voice a held-note
  stack, a note priority of Last, Low or High, and a Retrig/Legato switch over
  whether an overlapping note restarts the envelope -- and a release falls back
  to whatever is still held as a pitch change rather than as a new note, in
  either switch position, which is what makes a trill work. `Voices` greys out
  while it is on. Overlapping notes glide and a note landing on a release tail
  jumps; that is one fixed rule rather than a second glide control. The ML-M1 is a distinct mono filter/performance instrument:
  Osc, Amp/Filter, and Perf pages expose separate amplitude and filter ADSRs,
  three low-pass filter characters, pre-filter drive, keytracking, a held-note
  priority stack, legato/retrigger and glide modes, and velocity Accent. The
  ML-P8 face is five pages -- OSC, NETWORK, FILTER, AMP and ML-P8 MOD, whose
  name distinguishes it from the frame's MOD button, which opens the channel
  shelf. Sixty-nine parameters on one four-unit screen were unreadable on a
  laptop, so the face spends a click per group and every control is a 34px
  `KnobStack` with its value typed into. NETWORK is the
  source-by-destination grid with a page to itself; its four columns divide
  the face's width. Each cell is a **horizontal** slider -- the bar and the
  drag both run along the cell's long axis. A cell
  draws its modulation state the way a knob's ring does: an armed source marks
  every legal cell and shows the excursion its route would produce about the
  authored value, an unarmed one shows where the running sources have actually
  put the parameter, and a dot per incoming route. AMP carries the amp envelope beside allocation and
  character: Unison and Chorus as selectors under a fixed `VOICES 8` and the
  note count Unison leaves, with Detune, Spread, Drift and Glide as knobs. The
  face stays four rack units. The DS-01 face is six pages at four rack units
  on the same argument -- VOICE, TONE, NOISE, BODY, AMP and DS-01 MOD -- with
  every one of its ninety-two parameters on exactly one of them, at a 34px
  dial with its value typed into. It reads in the units a drum patch is
  written in: a time under a second is milliseconds, a frequency over a
  kilohertz is kilohertz, and a matrix route's depth is a signed percentage.
  A typed value means the unit it is written with, and with none written it
  means whatever the field was showing. A page is its layer's controls beside that
  layer's scope: the amplitude envelope carries the rendered hit inside its
  own contour, the pitch envelope is drawn over a quiet amplitude one for
  scale, and the eight-row matrix has a page to itself. The scopes are
  displays, not editors; their shared span is stated once in the page bar and
  follows the patch, so a 5 ms hat and a 4 s ride both read. The v1
  drum face keeps family, character, shared shaping, and voice-specific controls
  visible together. The Aux In face is two rack units and one page, because a
  source, an outlet and a level is the whole device: two pickers, a Level
  knob, and a line saying where the signal is tapped from or why the
  subscription was refused. Replacing a source does not change the channel's notes or
  mixer state. The starter kit's closed and open hats share a choke group, so
  the closed hat cuts the open one.
- Every insert runs inside a shared device host. The host owns bypass, a
  generic dry/wet blend, independent input and output trims, insertion/removal actions, and separate
  held input/output peaks; its dry path is preallocated and runs after the
  device DSP, so parallel processing works even when an effect itself has no
  mix parameter. At full wet the blend is exactly the device, sample for
  sample. The dry path is delayed by the device's declared dry-path
  alignment latency before the blend, so latency-introducing effects do not
  comb-filter their own dry copy; wet-only returns may retain their own
  intentional pre-delay. **Every host move ramps**:
  wet/dry, both trims and a container's Mix follow their controls through
  the mixer's 5 ms one-pole per sample, and bypass is a crossfade between
  the device's output and the bypassed path while the device keeps running,
  taken out of the path once the fade is 60 dB down (about 35 ms). A
  bypassed container fades its Mix to dry the same way before its run stops
  being called. A device coming back from bypass is told its held audio is
  stale, so an un-bypassed delay starts empty rather than playing the
  repeats it held when it went out. Knobs turned while a device is not being
  processed (bypassed, asleep, or on a muted channel) all land when it runs
  again, however many were turned; a hosted plugin, effect or instrument,
  takes them at once through CLAP's `params.flush`. Removing a device fades
  it out of the path first: the executor holds the removal, and the edits
  queued behind it, until the fade has run, or 100 ms at most for a chain
  that is not being processed. Installing is the same in reverse:
  an added device fades in along the bypass crossfade, and one installed over
  another waits for that one to fade out first. Loading an effect preset,
  or a container preset over a run, works that way too: the old device or
  run fades out and the loaded one fades in, and the channel is not rebuilt,
  so its voices and tails carry on. The device keeps its identity, so its
  routes and lanes stay attached. A single-row preset loaded onto a
  container's own row changes the box's values -- Mix, Level, its branch mute
  and solo, bypass, wet/dry and trims -- the way its controls would, and the
  box and everything in it keep sounding. Buses meter their effect slots the
  same way channels do: the rack polls whichever chain it shows, and a bus's
  head face reads its summed input and post-chain peak. Sources have a blank
  input meter because they generate rather than receive audio.
- Every gain trim — device input/output, the rack-row volume knob, the source
  output trim — is the same dB knob class: −60 dB (−∞) to +12 dB from unity,
  double-click to 0 dB. Project files and the engine wire keep linear gain.
  A new channel starts at 0 dB however it is made, and the strip's volume
  descriptor defaults there too.
- The generator at the head of a chain is selectable, by clicking its header
  the way a device row is selected, and wears the same border. It is the one
  rack row a click could not name. What it does not do is take part in
  copy, cut, duplicate or paste: those move rows of an effect chain, and a
  generator is not one -- moving a patch between channels is what its
  presets are for.
- Every effect face inherits one shared shell (`EffectDeviceShell`): the
  identity header and drag-to-reorder live there, so a face file holds only
  its working controls and a new effect kind adds no chrome of its own.
- Source-device oscillator, lo-fi, and filter plots respond to their live
  parameters. Drum plots are generated by the production voice renderer;
  filter response geometry is reusable for LPF, BPF, and HPF modes.

## Effects

- Each channel runs a full 256-slot addressable effect chain after its
  generator, and knob changes arrive as sample-timed `ParamValue` events.
  Effect chains persist in song files (`ChannelSetup.effects`,
  serde-defaulted for older manifests).
- Structural edits keep every address honest. An effect is addressed by its
  slot and a channel by its index, so adding, moving, or removing a device --
  or deleting or pasting a channel -- is stated once as a permutation
  (`mooloop_core::structure`) and run over everything that names a position:
  the modulation matrix, every automation lane in every pattern, and the
  lane the editor is showing. The UI's model and the engine's mirror apply
  the same table for the same command, so a route or lane keeps meaning the
  device it was drawn on; a removed device takes its routes and lanes with
  it. Add, move and remove are undoable edits. The modulator grid follows the
  same rule one level down: a route aimed at a modulator's own parameter
  moves with that module and is dropped when its slot is emptied. On load,
  the integrity pass points a route or lane stranded on another channel's
  index back at its own channel and drops one that names a device or control
  that is not there, leaving addresses on a generator that has no descriptor
  table yet untouched.
- Fourteen effect kinds ship: a low-pass/high-pass filter, a drive/saturation
  with four curves at 2x oversampling whose Drive changes character rather
  than level -- a signal at the -12 dBFS operating level keeps its peak at
  any drive on any curve, and a hotter one is held down toward it -- a preamp carrying the channel strip's
  four voicings -- Moo, Grip, Punch and Iron, the last three measured from
  real units rather than picked -- over a Drive, Mix and Output in dB. `Moo`
  is the default and is bit-identical to no device at all, which makes the
  preamp the way to put an automatable gain stage in the middle of a chain;
  it is deliberately not oversampled, so it adds no latency and can sit
  anywhere. Its band display draws what the stage *added* (two analyzers,
  dry against wet), and **it analyzes only while the window is drawing it**:
  the saved switch means "show it when I look", so a song saved with
  displays on costs nothing headless, in an export, or unsubscribed. Every
  spectrum analyzer (this one and the EQ's) spreads its band bank across its
  hop rather than running it in one callback, so no callback pays for a
  whole bank. A bitcrush that is deliberately not oversampled either, a stereo delay with damped cross-feedable feedback and
  digital/tape/reverse responses to a moving delay time. Its Time control is
  a knob with a sync lamp: dark, it sweeps free milliseconds; lit, it steps
  the same twenty-one-entry musical grid the modulators use, `4/1` down to
  `1/64T` with dotted and triplet entries throughout. While synced, every
  project BPM change immediately recalculates and sends the ordinary ms
  parameter to the audio engine, clamped to the two seconds the delay line
  can serve -- a division asking for longer reads in amber. It persists the
  division, not just its current ms result. It is joined by a gate,
  compressor, and limiter sharing one detector and gain-computer module; a
  seven-band parametric EQ with optional bounded spectrum telemetry, **whose
  every band and both pass filters carry their own stable parameter ids** --
  so an automation lane on band 3's frequency means band 3 forever, whatever
  the face happens to be showing, and the band selector is a view control
  rather than an automatable parameter that decided what every other EQ lane
  meant. Its bands run the same two laws the channel strip does, from the same
  function: **a band's Q is its slope while that band is a shelf** and its Q
  while it is a bell, and a proportional bell narrows as it is pushed.
  **Its response plot draws the filter that is running**, not a
  shape resembling it: Rust designs the same coefficients the audio path
  designs and evaluates their magnitude response, so the drawn curve is
  within a tenth of a decibel of what a sine measures through the bank. Both
  pass filters are on that curve at their real slopes with grabbable corners,
  a shelf's drawn slope follows its Q knob, and the same plot draws the
  channel strip's four bands the same way. The band buttons read 1 to 7 --
  the numbering their parameters use -- and the pass-slope buttons read
  12/24/36/48/72 dB per octave, which is what the bank rolls off at. **All
  seven bands start on and spread across the band** at the seven-band
  graphic EQ's own centres -- 63, 160, 400, 1k, 2.5k, 6.3k and 16k -- with a
  **low shelf at band 1 and a high shelf at band 7**. Every band rests flat,
  and a bell at 0 dB is the identity filter, so a fresh EQ still passes the
  signal through untouched. The target row is drawn in the order the plot
  reads -- the high-pass, the seven bands, then the low-pass -- and **a
  target that has a shape of its own draws it**: the two shelves and the two
  pass filters are line-art glyphs, a plain bell is its number, and the glyph
  follows the band's live kind rather than a fixed picture of the opening
  arrangement. The analyzer's switch sits in the plot's own top corner
  instead of a third button in that row, and the selected target's ON sits
  beside the three knobs it switches on. Double-clicking a knob returns to
  the *selected* band's resting value. A feedback-delay-network hall reverb;
  and one five-mode modulation processor
  (chorus, flange, phaser, ensemble, and ADT) whose Rate carries the same
  sync lamp the delay does, over the same grid, clamped to the 12 Hz its LFO
  runs to. Its delay-based modes share a
  bounded fractional stereo ring; Phaser uses a stereo all-pass cascade
  whose coefficients are worked out every 8 samples and followed in a
  straight line between.
  Past Feedback 75% (either sign) every mode's wet output is trimmed by
  `min(1, 4 * (1 - |fb|))`, so a feedback resonance peaks at +12 dB instead
  of the +22 dB a loop at 92% reaches; the loop itself is not
  touched, so a flanger at full feedback rings as long as it did. Width
  scales the wet signal's side against its mid, from both
  voices folded to the centre at 0% to the mode's own image at 100%, the
  default, where it changes nothing; the face's Mix knob is the slot's own
  wet/dry rather than a second blend. The face's knob panel is five a row.
  The generic host supplies their dry/wet blend, so the DSP returns the
  processed signal only. The reverb (`REVERB.md`) is eight modulated,
  diffused delay lines whose tail blooms into a dense wash rather than
  ringing on eight sparse modes, at a fixed per-sample cost independent of
  decay time and with no reported latency; Size, Decay, Damp, Pre, Diffuse, Width and Mod
  are all ordinary event-driven parameters, so every one of them is a
  working modulation destination. Beside it is a cheaper
  plate: eight parallel Freeverb-tuned combs into four series allpasses per
  channel, with Size, Decay, Damp, and Width, for material that does not need
  the hall. The thirteenth kind is the retained-audio Buffer described below,
  which is an ordinary insert in the same picker. The fourteenth is the Bus
  Comp, the master section's compressor offered as an insert
  ([engine.md](engine.md)). Device faces are width-quantized in rack units,
  declared once in `effect_kind_units`: filter, drive, preamp, bitcrush, and
  limiter take 1U; gate, compressor, plate, EQ, Mod, and Buffer take 2U;
  delay, reverb, and Bus Comp take 3U.
- Gate, compressor, and limiter share one transfer-curve display with a
  draggable threshold handle. Its live dot is fed by the device's own gain
  computer rather than by the surrounding peak meters: the audio thread
  reports the level its sidechain detector reached and the gain reduction it
  applied, held per block the way the peak meters are, and the display plots
  the dot at that detector level against the level actually leaving the
  device. So the dot moves with the attack and release the device is running,
  and rides above the static curve for as long as a slow release is still
  holding the gain down. Gain reduction is also read out three ways: a number,
  a rail down the right edge, and a warm glow over the whole plot whose
  strength tracks it. All three rest when nothing is coming in, so a gate shut
  on a silent channel does not sit lit up.
- The dynamics effects detect on the louder of the two channels and apply one
  gain to both, so compression cannot walk the stereo image around.
  **The limiter looks ahead and limits true peaks.** It holds its audio back
  96 frames (2 ms at 48 kHz) and declares that latency, so the mixer
  compensates it like any other -- which means **every Limiter adds 96
  frames of latency** to its channel, and every other channel is delayed to
  meet it. Its gain computer finds each frame's 4x-interpolated true peak,
  holds the deepest need across the lookahead, and ramps into it so the gain
  has arrived when the peak comes out. Nothing leaves above the ceiling,
  between samples included; the hard clamp is only a backstop. It sounds
  cleaner on transients: they come down smoothly instead of being squared
  off, and there's no clipping distortion.
  **The gate has two thresholds**: it opens at the knob and shuts only once
  the level falls 6 dB under it (`GATE_HYSTERESIS_DB`), judged on a level
  detector that holds across a waveform's troughs, so material sitting on
  the threshold does not flick it open and shut. It sounds steadier on
  sustained material near the line, and a sparse hit rings a few
  milliseconds longer before the gate shuts. **The compressor has its own
  Mix**, a linear parallel balance like the channel strip's `w/d mix` (0 is
  the input exactly), because the header's equal-power Wet runs a compressor
  and its own input 3 dB hot at 50%. Its gain computer is the strip's under
  the `Moo` voicing (the same detector and curve); the voicings' programme
  dependence and ratio bend stay the strip's own.
- Each kind publishes a static `ParamDescriptor` table
  (range, curve, unit, default) in `mooloop-core`, which is the single source
  of truth for normalization and clamping (`MODULATION.md`). The pre-tag
  untagged filter shape still loads.

## Containers and layers

- A rack device may be a **container**: `EffectKind::Chain` holds an ordered
  run of the devices after it, appears in the rack exactly where a device
  would, and is designed to nest four deep -- though nothing refuses a fifth,
  and a box past the cap stops blending (it still bypasses; see
  `docs/LOOSE_ENDS.md`). It is made either from the insert menu, like any
  other device, or by wrapping a device that is already there (a button on
  its left rail). **A device is added from the arrow between two devices**:
  the `→` turns into a `+` under the pointer and opens
  the insert menu, and the device lands in that gap -- inside a box when the
  arrow is inside it, after the box when the arrow leads out of it. An empty
  box draws an arrow of its own inside it, which adds into the box. **A Chain
  holding devices draws an arrow after its last one, inside the box and
  before its rail, which adds at the end of the Chain**; the arrow past its
  rail still adds after it. The same arrow
  takes **Plugin…** and a dropped preset, and so does an empty Chain's own
  arrow; a layer's shown Chain branch has one, a layer does not. The arrow
  after a layer's head adds nothing: a new branch is the layer face's `+`.
  There is no `+` on a device's rail and no add slot after the chain; the
  last arrow adds at the end. A preset dragged out of the browser and dropped
  on an arrow lands there too.
  The end of a box is named rather than indexed, because the index past a
  run means "after the container" -- for an empty box, "just inside" and
  "just after" are the same position, so which one is meant has to come from
  the arrow pressed rather than from the index. Its one control is a dry/wet mix across the
  whole run, delayed to match that run's latency — the wet/dry that a
  *single* device has always had, applied to a group. It really is the one
  control: the shell's own dry/wet is not offered on a container row, because
  a box has no node to be wet with. **Its two trims are heard**: the input
  trim on what enters the box, so on both sides of its Mix, and the output
  trim on what leaves it, after the blend, as a device's are. Both ramp, and
  a bypassed box is its input with neither on it; a bypass fades them with
  the Mix. Songs keep their saved trims, so one that saved a box trimmed away
  from unity reopens at that level. Bypassing a container skips its run
  without moving the channel in time. **In the interface**: a device's left
  rail wraps it in a container, a container's
  right rail unwraps it, and dragging a device onto a row already inside a box
  puts it in that box. **Containers nest four deep and the wrap button goes
  out at the fourth**, because the engine preallocates one dry buffer per open
  box and a fifth one would have an inert Mix and no chrome. A *device* inside
  the fourth box is fine: the cap counts boxes, not rows. And **dropping onto
  an emptied box puts it back
  inside** -- an empty container's span covers no index, so its own row is
  the only thing there is to aim at and a drop on it means "into this".
  Dropping on a container that still holds something keeps meaning "before
  it". **The run is drawn as a box, and the box is the container's own
  chrome**: its input rail stands at the head, its *output* rail stands past
  the last device it holds and meters what leaves the run rather than what
  entered it, and the recessed space between them is what its devices sit
  in. Both rails and that space are one colour, darker than a
  device, so the container reads as the thing the faces are inside of; the
  devices inside keep their outline but not their fill, so they stay unified
  chunks sitting in something rather than cards floating on it. Every device
  wears the same 1px perimeter, drawn over its own rails and header rather
  than under them. The border hugs the faces rather than
  standing clear of them, and nesting reads from the stacked border lines
  rather than a colour per level. An empty container caps itself, so an empty
  box still looks like a box. A container wider than the viewport has no
  collapsed form yet.
- **A device folds to its header, on its side**: the
  `<` at the top of every insert's left rail, where the insert `+` was,
  collapses it to a strip one rail wide -- `>` to open it, its colour chip,
  its name and kind reading top to bottom, and its on/off, wet/dry and
  remove still working at the foot. A folded device sounds exactly as it did,
  and it drags to reorder like any row. A folded Chain or Layer hides
  everything inside it and brings it back as it was, folds included. **The
  fold is saved with the song and is not an undo step**; undo and redo keep
  whatever is folded now, so no Ctrl+Z unfolds anything (whether it should
  be saved at all is Adam's call: MOO-219).
- A container's face draws the **parallel split** the box cannot show: the
  signal entering, a dry lane straight across, a wet lane through a chip per
  device in the run, and the sum taken between them at a node that rides to
  the blend position as the mix knob moves. The run itself is not listed on
  the face -- it is the box immediately to the right of it. A container **saves and loads as one preset** — the box
  and everything in it, from the same rail every other device's presets live
  on. The modulation driving a run does not travel with it, because a route's
  source lives in the channel's rack rather than in the container; see
  `docs/plans/archive/containers/00-status.md`.
- A second container kind, **Layer**, sits under Chain in the insert menu and
  saves and reloads as its own kind. It holds a run, nests, wraps, bypasses
  and mixes as a chain does, but **it splits its input across its direct
  children and sums them**: each device it holds directly -- or each Chain
  it holds, with whatever that chain holds -- is one parallel branch fed the
  layer's input. Branches sum at unity (two identical branches are exactly
  6 dB up), a branch holding a Drive is matched by delaying the others to it
  so the sum does not comb, and the layer declares its longest branch as its
  latency. Its Mix blends the sum against its input; bypassing it passes the
  input, delayed by that latency. A layer of one branch is a chain.
- **A layer draws like Bitwig's FX Layer** (laid out by Adam's ruling;
  `docs/UI_DESIGN.md`, *Device Rack Layout*). Its face is a list of its
  branches, each row with a name, a
  `BYP` badge when the branch is bypassed, **S**, **M** and a level meter,
  a `+` under the list that adds an empty branch at the end, and beside the
  list the **selected branch's controls**: its Level, Mix, bypass, input and
  output trim, and preset **Save** and **Load** (a branch saves and loads as
  a Chain preset). Only the selected branch's devices are drawn in the rack,
  to the right of the layer under an accent bracket, and the layer's own
  **Gain** and **Mix** stand at the far end of its box, after those devices
  and before its output rail. Clicking another row shows that branch instead
  and selects its Chain as the rack's device, so copy, duplicate, delete and
  the sidebar's parameter list act on the branch; which branch is shown is
  not saved and is not an undo step. **Dragging a row along the list moves
  the branch** among its siblings, one undo step ("Branch moved"). The face
  is as wide as what it holds, not a count of rack units. Every branch the
  `+` makes is a Chain. Mute takes a branch out of the sum and Solo keeps only
  the soloed branches of that one layer; both ramp, and both are undoable
  and saved.
- **Every branch of a layer is a Chain.** A layer is
  chains in parallel. Wrap in Layer, the layer's `+` and a drop into a layer
  all put the device inside a branch's Chain: a drop on the layer, or on the
  gap between two branches, lands first in a branch's Chain, and a device put
  beside one already in a branch joins that branch. Only `+` makes a branch;
  a layer with no branch takes no device until it has one. A paste or a move
  follows the same rule, a container preset for a whole Layer will not
  replace a branch, unwrapping a branch's Chain that holds a device is
  refused, and a wrap the nesting cap leaves no room for both boxes of is
  refused rather than leaving a Layer straight round a device. **A song with
  a bare device directly in a layer gets that device wrapped in a fresh Chain
  when it opens** (`load_bundle` does it, for a song's channels and buses, a
  kit, a channel and an effect run, through `normalize_layer_branches` in
  `core/src/effect.rs`): the file format is unchanged, every existing device
  keeps its id, the new Chain is transparent (default Level, Mix, Mute and
  Solo), and it is part of loading rather than an undo step. The engine still
  plays a bare device in a layer if one is ever found. **The rack does not
  draw a layer's shown branch's Chain**. Its devices are drawn
  straight under the layer's bracket, one box deep (`RowView.draw_depth`); the
  layer's box holds the append join the Chain's would have, wired to that
  Chain, and a layer showing an empty branch closes on its own row with the
  join inside it wired to the branch. Its controls are the layer face's (the
  entry above); a branch does not fold, and a branch Chain folded in an older
  song is shown open.
- **Making and emptying a layer.** The rail's wrap button
  opens a menu, **Chain** or **Layer**. Wrapping in a layer makes a layer of
  one branch, a Chain holding what was wrapped, so the branch has its S, M
  and Level from the start. A right-click on a branch in the list offers
  **Remove branch**, which takes the branch and everything in it. The last
  branch can go too, leaving an empty layer that passes its input. Each of
  these is one undo step.
- **A layer saves as a preset**, the box and every branch in it, and lists
  on the layer's own preset rail. It ships with a bank of three: Parallel
  Drum Compression, Clean and Distorted, and Three-Way Split (a low-pass, a
  band-pass and a high-pass on three branches).

## The Buffer

- **The retained-audio Buffer is a ring that is always recording, and four
  ways to hear it instead of the input.**
  **Three held gestures, each owning its own settings.** JUMP plays forward
  from `Jump Back` behind now; REVERSE plays backward from now; STUTTER
  repeats the last `Stutter` length. Each holds the ring still while it is
  down — the press is a freeze — and letting go returns to live audio and
  restarts the writer. JUMP and REVERSE wrap round the ring rather than
  running out, so a held button never lets go on its own. None of them
  borrows another's setting: the stutter's length is its own knob. The three
  are **gates** rather than triggers — `Jump`, `Reverse` and `Stutter Gate`
  are high for exactly as long as the gesture lasts — so a finger on the
  button, a held MIDI note and a block drawn in a lane are one mechanism.
  Last one pressed wins.
  **`Position` is a playhead, heard only while it moves.** A static Position is
  a setting nobody is playing, and the device falls through to live audio.
  The head *is* the position rather than chasing it, so the playback speed is
  the position's own speed: a one-bar saw from the modulator rack over a
  one-bar ring plays at unity, half the period is an octave up, and a
  descending ramp is reverse. Nothing on the face names a rate. A move too
  large to sweep (past four times speed — a saw's wrap) cuts under the
  crossfade instead of zipping. At the default full `Span` it addresses **the
  ring in the ring's own coordinates**, the same map the waveform is drawn in,
  so dragging across the picture scrubs exactly what is under the pointer; a
  shortened `Span` is the most recent that much and follows the writer.
  **Frozen with nothing else playing, the ring plays** — forward, round and
  round — so a freeze leaves a loop rather than a still. Priority is fixed and
  short: a held gesture, then a moving playhead, then a frozen ring, then the
  input. Every change of source crossfades by `Crossfade`.
  **`Quantize` delays presses, never releases.** A gesture and a freeze wait
  for the next `Quant Start` boundary (one bar by default); letting go before
  it lands takes the press back, and the face shows ARMED meanwhile. `Quant
  Start` is independent of every length on the device — starting on the
  quarter while stuttering a thirty-second is the ordinary case. With the
  transport stopped there is no grid, so a press lands at once. A saved
  freeze is restored rather than quantized. A waiting freeze and a waiting
  gesture are independent — both land, the freeze first, so the gesture
  plays over the frozen ring — and a gesture still waiting outranks nothing
  until it lands. The wait is measured from the press's own frame, and a
  setting written on the same tick as a press (`Quant Start`, a length)
  applies before it.
  **Every length is on the shared musical grid** — the twenty-one
  `ModTimeDivision` entries the modulator racks and the delay use — and `Span`
  adds one position past them for the whole ring, which is its default.
  **A fresh Buffer keeps two bars of history**, adjustable from 1 to 64 on the
  face's HISTORY stepper. `bars` is not a descriptor parameter and cannot
  become one: resizing reallocates, so the replacement is built on the control
  thread and swapped in at a block boundary, down the road a tempo change
  travels. **The replacement takes over the history**: the most
  recent frames the old ring held, as many as the new one has room for, so a
  tempo change or a HISTORY change does not empty an unfrozen Buffer. A
  frozen buffer refuses that swap rather than losing what is playing, which
  also means **changing HISTORY while frozen does nothing to the running
  ring** until it thaws and something resizes it again. An undo, a redo or
  any other whole-project install keeps a Buffer whose own settings did not
  change, with its ring, and keeps the channel it sits on sounding.
  **A freeze is not saved** (Adam: *"freeze is temporary"*), and
  neither is the ring's audio. A reopened song, a preset, a pasted device or
  a pasted channel arrives unfrozen and recording; an undo keeps a freeze and
  its ring. A Buffer that is rebuilt with Freeze on and an empty ring anyway
  -- a Freeze lane, say -- records with the freeze armed (the face shows
  ARMED), and the freeze lands once the ring holds a full history. A
  document saved with a gesture held reopens holding it.
  The face's SEAMS readout counts wraps and cuts — a stutter's repeats, a
  reverse head lapping the ring — which is the number that says whether the
  head is doing what the picture claims.
  **Retired parameters.** `Rate`, `Length` and `Loop`, from the turntable
  model the Buffer replaced, are retired, and ids 3, 4 and 5 are spent
  alongside `Offset`'s 0. Their saved values are dropped on load, and **an
  automation lane or modulation route that names one of them stays in the
  document but drives nothing**. `Quant Grid` is now `Quant Start` on the
  same id, and a saved `quant_grid` key loads into it. A project saved before
  either change still opens with its `Offset` converted to `Position`, lanes
  and routes included.
  The `BufferMidiMap` event path still builds its own head from a
  `BufferEvent`'s geometry, and is the one path that can ask for a speed other
  than ±1; it still has no caller outside its own tests.

## Generators

- **Every generator is descriptor-addressed** through `GeneratorParams`, so
  their parameters automate and modulate like an effect's. The
  three-oscillator synths reserve ten parameter ids per oscillator, starting
  at 100; ML-P8's, DS-01's and the v1 drum synth's ids are each their own
  namespace starting at zero, because none of them is that voice with a
  different count. `docs/MODULATION.md` records the approved design.
- The **v1** drum synth has sixteen continuous controls and four selectors
  under ids of its own, and a modulation route and an automation lane both
  reach them. Each id means one thing forever -- `kick_start_hz` is the kick
  sweep start whatever Mode says, which is exactly why the other modes' knobs
  are *retained* across a mode change rather than reset.
  **What Mode selects is audibility, not meaning.** A route onto a kick
  control does nothing while the device is in Snare mode: the value is still
  written and still the one the patch authored, it is simply not heard. That
  is the same situation as a route onto a bypassed effect, which the
  application permits everywhere, so it is documented rather than
  special-cased -- suppressing it would be a second rule about when a
  parameter exists. Mode itself is automatable and is latched on a voice at
  its trigger, so a lane moving it changes the *next* hit rather than
  reshaping the one that is playing.
  The device has three modes, and old projects load exactly as before. DS-01
  remains the better instrument and the reason the v1 device does not need to
  grow.
- DS-01 is a second drum instrument, not a rewrite of the first: one universal
  percussion voice with no drum-type mode, three layers — a morphing tone with
  a partial bank and FM, a four-colour noise generator through a morphing
  state-variable filter, and three tuned resonators that ring — into a shape
  stage with four drive characters. Four AHD envelopes with a curve control
  and an optional gate; a burst that fires up to eight impulses from one
  trigger inside one voice; and its own eight-row modulation matrix whose
  sources are per hit. It ships a factory bank of seventeen patches --
  three kicks, three snares including a velocity-shaped ghost, rim, clap, one
  tom at three tunings, both hats sharing a choke group, a gated ride,
  cowbell, clave and a zap -- seeded once into `presets/generators/ds01/` as
  generator presets, since a DS-01 patch's modulation is inside its own voice
  and has no channel rack to re-scope. Those are the same patches the DSP
  acceptance test asserts, so what ships is what is checked. The bank is
  there to prove the architecture reaches a kit from the controls rather than
  to be a curated bank. It publishes six control outlets — `Amp Envelope`,
  `Mod Envelope`, `Velocity`, `Note`, `Gate` and `Trigger` — reduced through
  the hit created by the most recent trigger, which stays the focus for its
  whole life and falls to zero rather than stepping backward onto an older
  hit that is still ringing. `Trigger` is the one a drum channel wants: one
  publication wide per hit, so a kick can duck a bass, open a gate, or fire
  an envelope on another device with no sidechain graph. `Gate` is honest
  rather than useful here — it answers "any hit is still waiting on its
  note-off", which is low for the one-shot patches most of the kit uses. Its
  four audio outlets (`Tone`, `Noise`, `Body`, `Pre-Shape`) are declared with
  frozen ids and tap points, and an Aux In channel can read any of them.

- **A channel can play another channel's published audio outlet.** `Aux In`
  is a generator kind whose sound is one subscription: a source channel and
  one of its declared audio outlets, at a Level. The samples arrive in the
  block they were made in, not the one after, because the channel loop walks
  a compiled order that puts a producer before its consumers —
  `mooloop_core::compile_audio_graph`, beside the bus graph and the
  compensation plan. A block-sized delay was ruled out on purpose: its length
  would be the host's buffer size, so the same project would render
  differently at 128 and 512 frames.

  The useful case is the surprising one. An audio outlet declares where in the
  device it is tapped, and ML-P8's five source outlets are tapped *before*
  each source's own Level — so an oscillator turned down to silence in ML-P8's
  own mix still publishes, and an Aux In can play it while it stays absent
  from the producer's output. The face says `pre-level` rather than leaving
  that to be discovered. A muted producer publishes too: mute is a decision
  about what reaches the bus.

  **A tap exists only while somebody is subscribed to it.** ML-P8 declares
  seven stereo outlets and materialising them all would be 448 KB a channel;
  the compiler names the distinct (producer, outlet) pairs somebody reads, the
  buffers are allocated on the control thread and installed with the schedule
  they belong to, and a project that has never authored an edge holds none.
  Two channels reading the same outlet share one buffer.

  An edge that cannot resolve is refused and **kept**: a subscription naming a
  departed channel, a device that publishes no audio, an outlet that is not
  declared, a control outlet, an outlet tapped after its channel's effects, or
  a ring of subscriptions. The face says which, and a user who builds a cycle
  and then breaks it gets the edge back rather than authoring it again. Aux In
  publishes its own output, so Aux Ins chain — and that is what makes a ring
  constructible at all.

  It is not a send: the producing channel does not know it is being read and
  its own routing does not move. It is not a router either — one subscription,
  one channel, one outlet. **Parallel sends exist**, but they are
  a second edge system rather than this one: a send is a producer-side edge that
  carries its own compensation, where an aux-in subscription lands pre-chain in
  the consumer and has nowhere to put a delay, which is why it is refused when
  its tap is late instead. Unifying the two is recorded and not done. Sidechain
  key inputs are still absent.
- The ML-P8 has a device output stage: Volume and Pan, before the channel
  strip's own. They exist to be the base its per-voice `VcaLevel` and `Pan`
  modulation destinations offset from, so a Velocity route on Pan swings
  around wherever the patch put the device, and Spread widens around that
  rather than around the middle. Volume is one-pole smoothed over 5 ms like
  the device's other levels, so modulating it (a kick-gated envelope pumping
  a pad) or dragging it glides instead of stepping once per 32-frame control
  tick.
- The ML-P8 allocates its eight physical voices as *groups*. Unison at 1x, 2x,
  4x and 8x spends the pool rather than growing it, leaving 8, 4, 2 and 1 notes
  of polyphony; a note allocates a complete group and steals complete older
  groups, and a slot stolen by a smaller group leaves through the same short
  de-click transition rather than stopping. Changing Unison releases the
  sounding groups and applies the new topology to the next note; it never
  resizes a group in place. Detune and Spread place a group's members
  symmetrically about the note that was played, and at 1x Spread places notes
  by their stable slot positions so a chord occupies the field the same way on
  every render. Drift is one control over stable per-slot offsets to
  oscillator pitch, cutoff, the envelopes' attack, decay and release times, and
  oscillator start phase — never sustain, and never from runtime entropy, so
  Drift 0 renders bit-for-bit what the patch authored. A finishing chorus with
  four fixed policies (OFF, I, II, Ensemble) reuses the rack's modulation
  effect over ML-P8's own scratch buses, never the channel's; OFF is a true
  bypass and a mode change crosses through a silent wet rather than stepping.
  A unison group shares one note's level between its members:
  each plays at `N^-(1/2 + c/2)`, where the coherence `c` falls from 1 for
  identical members to 0 as Detune and Drift pull them apart. A held note at
  any Unison count stays within a couple of decibels of 1x, and within
  0.7 dB from Detune 50% up. Chords still sum honestly: this is the one
  normalization in the device, and it is by group, not by note count. The
  law and its measurements are in `docs/GAIN_STRUCTURE.md`.
- The ML-P8 and DS-01 are the two generators with modulation of their own. It owns an
  audio-rate LFO and a list of internal routes reading six per-voice sources
  — the LFO, both envelopes, velocity, key, and gate — into thirty-one
  continuous destinations, resolved per sample as authored base plus offset and
  clamped through the destination's own descriptor. This is deliberately not
  the channel shelf: the shelf's sources are per channel, and a polysynth needs
  values that differ between two notes held at once. A route's amount is
  automatable through `ParamOwner::SourceRoute`, addressed by the route's
  durable id rather than by a parameter of the device.
- The ML-P8 ships a factory bank of eight patches -- Init Saw, Crosswire
  Brass, Furnace Stab, Cold Metal, Sub Pressure, Servo Pad, Broken Choir and
  Wide Machine -- seeded once into `presets/generators/mlp8/` as generator
  presets, since an ML-P8 patch's modulation is its own routes and its own LFO
  and has no channel rack to re-scope. Seven of the eight run at Unison 1x
  with the chorus off, and five leave Drift at 0: the bank's job is to show
  that the *network* reaches eight sounds, so a patch that needed a duplicator
  to be interesting would not have proved it. Init Saw is the device default
  unchanged, because the gain contract is calibrated against exactly that
  signal. Those are the same patches the DSP acceptance test plays, so what
  ships is what is checked. The bar it is held to is Adam's: enough to prove
  the architecture reaches its range from the controls, not a curated bank.

## Instrument DSP

- The ML-M1's Ladder and Acid filters put their corner in the same place at
  every sample rate: their stages are cornered by solving the stage's own
  response rather than by the impulse-invariant pole, so one Cutoff is one
  sound at 44.1, 48, 96 and 192 kHz. `scale::cutoff_hz_from_normalized` is
  the one cutoff-knob law: 20 Hz to 20 kHz whatever the rate, with the filter
  primitives' own `0.45 x sample rate` clamp the only place the rate enters.
- Every filtered instrument (v1 mono and poly, the sampler, ML-M1, ML-P8)
  maps Cutoff through that law, so a knob position is one frequency at every
  rate and matches the face's readout. They share one voice-cutoff block
  (`voice_filter::VoiceCutoff`: six octaves of envelope, keytracking from
  middle C). ML-P8's feedback DC blocker is specified in Hz.
- Glide slides linearly in pitch and arrives in the Glide time: an octave up
  and an octave down are mirror images, and a 100 ms glide is on the new note
  after 100 ms.
- Resonance tapers exponentially on every SVF-based filter (the v1 synths,
  the sampler, ML-P8, the Filter effect, ML-M1's clean model): the peak grows
  by about 2.6 dB for each tenth of the knob up to +20 dB. A 24 dB slope is
  one shared compensated cascade (`filter::SvfCascade`): its corner lands
  where the 12 dB one's does and it peaks about as hard, so the Filter
  effect's 24 dB mode does not reach +40 dB, and that effect bends its output
  under the voice ceiling. Delay feedback saturates in the loop above half
  scale, so a loud input into 0.98 feedback settles at twice full scale at
  most rather than fifty times.
- **A song saved before 2026-09-23 sounds slightly different** from when it
  was saved, with every stored value unchanged (they are normalized, and the
  curves under them moved): a Cutoff knob at 0.75 is 3.56 kHz where it was
  3.77 kHz at 48 kHz (about a semitone down; more at 96 and 192 kHz, where
  it was far brighter); mid-knob Resonance is less peaky with the same top; a
  glide arrives in its Glide time, about 4.6 times sooner than before; ML-P8's
  LP24 and the Filter effect's 24 dB mode peak harder or softer to match
  their 12 dB modes; and delay repeats above half scale saturate. There is no
  migration.
- The v1 mono synth's LFO is one shape (sine, triangle, saw, square, or sample and
  hold) with a depth per destination: pitch, filter cutoff, pulse width, and
  tremolo. It free-runs across notes and silence unless set to retrigger.
- Mono synth amplitude is continuous by construction: the amp envelope attacks
  from its current level rather than restarting at zero, and velocity,
  oscillator levels, cutoff, and drive are one-pole smoothed over 5 ms so
  neither a retrigger nor a knob turn steps the waveform.
