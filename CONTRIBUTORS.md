# Contributors

Mooloop's code has been written almost entirely by AI coding agents working
under Adam's direction, across whatever tools happen to be in rotation
(Claude Code, Codex, opencode, others). This file is a sign-in sheet, not a
changelog — a place for each distinct model+harness combination to leave a
mark, so Adam has a real trail to work from later if he wants to credit
people^Wmodels properly.

## How to sign in

If you're an agent reading this after doing work in this repo: add or update
your own entry below.

- One entry per **model + harness** pair (e.g. "Claude Sonnet 5 — Claude
  Code" is distinct from "Claude Sonnet 5 — Codex", and distinct from
  "Claude Opus 5 — Claude Code"). If you don't know your exact model ID,
  use the most specific name you're aware of.
- If an entry for your exact pair already exists, just bump `Last seen` and
  `Sessions` — don't create a duplicate.
- If it doesn't exist yet, add one, alphabetized by model name, using the
  template below.
- Keep it to the template's fields. This file is a roster, not a diary —
  save war stories for commit messages, not here.

```
### <Model name> — <Harness>
- First seen: YYYY-MM-DD
- Last seen: YYYY-MM-DD
- Sessions: N
- Notes: optional, one line
```

`Sessions` is a rough count, not an audit — increment it once per distinct
work session you can recall being part of, and don't stress over precision.

## Roster

### Claude Opus 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-09-02
- Sessions: 35
- Notes: Parameter descriptors, the modulation design, seven effects, the
  mixer bus graph, and the near-term focus sequence. Buffer device stage 1
  follow-up: collision telemetry, debug trigger surface, and the remaining
  block-size and crossfade acceptance coverage. Clip automation end to end:
  breakpoint lanes on `ParamAddr`, engine resolution composed with the
  modulation matrix, and the piano roll's velocity and automation lanes, then
  the buffer's own offset/crossfade parameters so a lane can move its read
  head. Rebuilt Preferences > Appearance on three color seeds with derived
  palettes, saveable schemes, and live roundness/contrast scalars. Audited gain and
  summing end to end and wrote the `docs/plans/archive/gain-structure/` plan: a
  console fader taper, a -12 dBFS operating level, energy-normalized reverb
  IRs, and IEC 60268-18 metering. Turned the synth v2 direction spec into two
  plans that split Mono and Poly apart: `docs/plans/mono-synth-v2/` (ladder and
  acid filter models, pre-filter drive, a held-note stack with priority and
  legato, velocity accent) and `docs/plans/poly-synth-v2/` (deterministic
  per-voice drift, a multimode filter, grouped unison, an internal chorus).
  Added `scripts/antibox`, which runs builds, tests, and headless UI snapshots
  on the remote build box instead of the laptop. Traced the reported
  level-dependent summing to a shared nonlinear stage rather than the mixer:
  added superposition and ducking tests to `gain_structure_tests.rs`, and
  freed the bus faders' top +6 dB, which the UI was clamping at unity. Made
  the modulator system's destination metadata load-bearing: the declared
  policy now gates and clamps every route, and the channel strip's fader and
  pan became real modulation destinations resolved in control-rate segments.
  Then fixed up the modulation shelf: gave it a real layout instead of
  overlapping manual offsets, and replaced its hardcoded filter-cutoff routing
  with descriptor-id-indexed depth and legality arrays, so every eligible knob
  on a face is assignable through one addressed path. Gave `scripts/antibox` a
  `--release-bin` flag that strips a release build back to
  `bin/mooloop-test`. Tried one shared remote target directory across checkouts
  and reverted it: same-named workspace packages collided, so a branch linked
  against another checkout's stale `mooloop-core` and failed to compile code
  that was correct on disk. Dependencies are shared through sccache instead,
  where a differing source is a miss rather than a wrong artifact. Ran down
  the "reverb is still too loud at 1-4% mix" report that an earlier round of
  fixes had measured clean: the mechanism was right but `IR_TONAL_CEILING`
  allowed +4.9 dB, and every existing probe (whole-render peak and RMS)
  structurally flatters a reverb, so a wet branch +4.7 dB hot through the
  sustain passed. Measured mid-sustain instead, tightened the reverb ceiling
  and the plate's output reference, and moved the broadband assertions off
  peak onto energy. Finished the modulator destination surface: every eligible
  knob on every effect and generator face is now assignable, with the
  assignment contract declared once on `EffectDeviceShell` and the oscillator
  strip deriving its ids from a `param-base` rather than repeating them per
  oscillator. Then gave the knob a modulation indicator with four states --
  value, assigning, dialled-in depth, and live -- backed by a new
  `ModulatorMeters` that publishes each channel's four modulator outputs so
  the UI can resolve per-destination offsets itself instead of the engine
  shipping a value per parameter. Replaced the generated-room convolution
  reverb with an eight-line feedback delay network rather than amortizing its
  partition spike as planned: the spike was only one of its three problems,
  since a convolution node also cannot accept a parameter event — so every
  modulation route aimed at a reverb knob was a silent no-op — and a static
  image-source IR rings rather than blooms. Worst-case per-block cost fell
  31x and stopped depending on decay length at all. Started the v2 mono synth
  as a third device rather than an edit of the v1 one: its own params struct
  with `#[serde(default)]` from the start, a descriptor table built from a
  shared core block instead of inherited from Mono's, and a voice with
  separate amplitude and filter envelopes plus cutoff keytracking read off the
  gliding frequency. Then gave it a real held-note stack in its own module,
  with note priority and independent legato and glide-mode switches, and a
  face whose third page is PERF rather than MOD. Named it the ML-1 and gave it its
  filter: a nonlinear four-pole ladder with the saturation moved ahead of the
  filter, so the oscillator mixer is a tone control, then a three-pole
  asymmetric one beside it and the clean SVF as a third character. Added its
  Accent knob, which turned out to need no new state: velocity is the carrier
  the plan asks for, and riding the smoothed velocity the VCA already uses
  gives per-note capture, the priority fallback's winning-note velocity, and
  the legato slide for free. Then wrote its factory bank — six patches defined
  as data so the DSP tests and the preset seeder share one source of truth,
  seeded onto disk on first run because the project had no factory-content
  mechanism at all. Building it surfaced that modulation routes name their
  destination channel absolutely, so any channel preset with a rack modulated
  whatever channel it was saved from; the bank could not ship without
  rescoping on load. The listening pass the step exists for is still open,
  since voicing by ear is not something I can do. Opened the sampler v2 push
  on GitHub issues instead of `docs/plans/`, filing the loop-focused gaps the
  existing set missed, and started its first slice: control-side zero-crossing
  snapping for sample and loop markers, where preference is a tier order
  (rising crossings, then same-direction ones, then quiet points) rather than
  a weight vector, and a channel that is still stepping disqualifies the
  other channel's crossing. Measured `EngineCommand` before believing
  `large_enum_variant` about it: the width is the rack the preallocated ring
  is sized for, so the variant kept its size and gained the reasoning in a
  comment. Replaced the fail-fast document validator with a repair pass that
  detects and corrects in one traversal, so a save clamps, resquares, and
  reissues rather than refusing, loading does the same instead of leaving a
  song unopenable, and what genuinely cannot be fixed without deleting music
  is reported with its channel, pattern, and the count to delete, next to a
  copyable machine report. Then gave the app a diagnostic log, since the
  documents that failed had all been unsaved ones with nothing left behind to
  look at: levelled records to stderr from every crate, optionally mirrored to
  a file from a Developer preference, and a refused song now parked under the
  config directory with its report beside it rather than dropped. Cleared the
  bench before the next push: pruned nine merged worktrees and a forgotten
  `main` stash whose contents had already landed, and gave the v1 poly's
  mono/legato toggles — decided during the ML-1 restructure but recorded only
  as a consequence note — their own plan, since they are what blocks deleting
  the v1 mono synth. Then corrected the ML-M1's name, which had shipped as
  "ML-1" because an agent misread it: a mechanical source and UI rename, with
  the serialized `ml1` tag, the preset directory, and the factory marker file
  deliberately frozen at the old spelling, since a serialized name is an
  on-disk identifier and Adam already had songs and a seeded bank written
  under it. Added the backward-compatibility test the existing round-trip
  could not provide, because renaming both ends at once still passes it. Then
  chased Adam's listening-pass note that the filter models differed in apparent
  loudness: measured 10.8 dB between them, traced it to the two nonlinear
  models compressing as resonance drives them while the linear one gains from
  its peak, and matched them to 2.4 dB with a makeup shaped on each model's own
  feedback gain. Found on the way that Acid's corner sits three quarters of an
  octave below the other two, and that correcting it breaks the filter's
  resonance taper outright -- so the miscalibration is load-bearing, and it is
  recorded rather than fixed. Then captured a design conversation without
  building any of it: Adam wants node-based patching because he likes it, not
  because a workflow demanded it, so it went to `docs/NODE_MODEL.md` as a
  recorded direction — devices staying opinionated externals with objects
  wired around them in the rack, and a note/control/audio cost table showing
  the audio case needs delay compensation first. What that conversation did
  surface as real is the preset system: device-level presets were asked for
  and never delivered, effect presets do not exist, and the ML-M1 bank already
  paid for it. Queued in `docs/plans/preset-system/`. The part adopted now is
  three cheap habits in `COMPOSABLE_DEVICE_UNITS.md` that keep a node view
  reachable without betting on one.
  Then gave the sampler its own output trim. Two things only the code could
  settle: the builtin kick looked like it would be quietened by a trimmed
  default until it turned out to be reachable only through the legacy
  `Builtin` reference, and the trim had to be -9 dB rather than the -12 the
  issue specified, because `REFERENCE_PEAK_DBFS` is measured after the
  equal-power pan law and a generator's own output peaks 3 dB above it --
  at -12 every loaded sample would have sat under every synth. One lagged
  gain serves every voice: each copies the smoother and walks its own copy,
  the original is caught up once per segment by a closed-form `advance_by`,
  and installing a patch onto a silent device snaps rather than ramps, since
  the ramp was otherwise paid for by the first note's attack. Then replaced
  the sampler's linear interpolation with a band-limited reader, as its own
  unit: one windowed-sinc prototype read at rate-dependent spacing, so
  pitching up narrows the cutoff instead of folding back, and unity rate
  stays sample-exact because sinc is zero at every non-zero integer. The part
  that needed designing was not the kernel but the region -- a 16-tap kernel
  overhangs a loop point the read head has not reached, so `Region` says what
  is on the other side, and a forward loop reads what it is about to wrap
  into rather than silence.
  Then split the sampler's filter envelope off the amplitude one. The
  migration is what shaped that design: "copy the amp ADSR into the missing
  filter ADSR" cannot be a serde default, because a default cannot see its
  siblings, so absence had to be representable -- `Option<EnvTimes>`, where
  `None` means "follow amp" and reproduces an old patch's filter motion
  exactly rather than approximately. Editing one stage materializes all four,
  seeded from where the envelope was reading, so the other three do not jump.
  Started the piano roll's mouse-editing pass by lifting the note canvas out
  of `main.slint` into its own `PianoGrid`: the one hit area that owns every
  grid gesture was sitting at 48 columns of indent, which is not somewhere
  marquee, tool modes, and scale handles can be added legibly. Then gave the
  undo history a gesture token, because a drag records an edit per move frame
  and each was becoming its own undo entry -- moving a note across the grid
  cost twenty undos to take back. Frames sharing a token collapse into one
  entry spanning the whole drag; tokens rather than labels, so two separate
  drags of the same kind stay two steps. Then made the roll's drag modifiers
  a registry rather than literals in the pointer handler, so Preferences can
  remap them when a window manager claims a chord -- Alt is offered but never
  a default for exactly that reason. They cross into `.slint` as booleans
  rather than a bitmask, since Slint's expression language has no bitwise
  operators, and the grid tests them by implication so the roles compose.
  Then gave the roll pointer tools -- select, draw, paint, slice/join, erase --
  a snap toggle whose override modifier inverts rather than only defeating,
  and marquee selection with additive and subtractive bands. Two things that
  only showed up in the doing: Slint's `pressed` is a left-button notion, so
  a right-drag erase sweep stalled after one frame until the gesture flags
  became the only record of a drag in flight; and the new header controls
  propagated a window minimum width that cost the sidebar most of its resize
  range, so the row clips and leads with the tool selector instead. Finished
  the selection as an editable object: left-edge resize, length adjust across
  the whole selection, and a frame with a grab handle at each time edge that
  scales the selection in time -- double its span and an eighth becomes a
  quarter. The scale caches its pre-drag geometry in Rust and applies each
  frame to that, because scaling the live notes compounds its own rounding
  and a slow drag walks the selection away from where the pointer says it is.
  Left one standard gesture out on purpose and said so in `ENHANCEMENTS.md`:
  axis-constrained drag has no conventional binding that is not Alt. Adam
  then found the real faults in it: a plain press collapsed the selection
  before the drag it was starting, so a group could be neither dragged nor
  resized; Shift both added to the selection and defeated snap, so a
  Shift-drag deselected the note it was moving and looked like an accidental
  clone; and the note rectangle floored its drawn width at one snap step, so
  changing the grid appeared to rewrite every note's length. Dropped the
  selection frame for plain tinting, moved stretch onto Alt+edge-drag with a
  grab cursor, gave notes keyboard delete/nudge/copy/paste, and replaced the
  PPQ-tick length field with musical divisions.
  Then fixed the dynamics display's jumpy signal dot, which turned out not to
  be a smoothing problem: the dot was being fed the surrounding peak meters,
  through a twelve-segment change filter that quantized its travel into five
  decibel steps. Gate, compressor, and limiter now report their own gain
  computer's state -- the level their sidechain detector reached and the
  reduction they applied, as block extremes on the `AudioNode` trait beside
  `buffer_collisions` -- so the dot rides the attack and release the device is
  actually running, and sits at the level really leaving the device rather
  than on the static curve, which makes a slow release visible as the dot
  floating above the curve. Added the gain-reduction readout that was missing
  entirely: a number, a rail, and a glow whose strength tracks it. The glow
  needed one rule the audio does not imply -- it rests when nothing is coming
  in, because a gate shut on a silent channel is truthfully holding 80 dB down
  and would otherwise sit at full brightness for as long as nobody played.
  Then took the modulator-modules plan's second step: step sequencer,
  random, and math modules, which cost a descriptor table and a tick apiece
  now that step 01 gave modulators the effect param contract. The math
  module needed the one new rule the plan asks for -- modules evaluate in
  slot order, so reading a lower slot sees this tick and reading itself or a
  higher one sees the last -- and it turned out to need no machinery at all:
  the outputs array already holds last tick's value everywhere the pass has
  not reached, so self-reference is bounded by the module's own output clamp
  rather than by a cycle check.
  Then the grid: the shelf's tile row became a real grid of modules beside
  the selected module's full surface, and a module's input -- an envelope's
  gate channel, a math module's source slot -- became a labelled jack rather
  than one more knob in the row. Lifting the header and the jack strip out of
  the five kind editors deleted five copies of each. Grew the rack from four
  slots to eight after measuring what it costs rather than assuming: the whole
  rack rides the command ring by value, so a slot is 72 bytes on every
  preallocated entry and eight slots take the ring from 552 KiB to 840 KiB.
  Pinned that arithmetic in a test, since the number is the thing a future
  capacity change has to argue with.
  Then made routes stop meaning "slot 2": a rack slot now carries a durable
  `ModSourceId` that routes persist instead of a slot number, so reordering
  the grid moves modules without touching what any route means. The part that
  needed care was not the routes -- they resolve by identity -- but the math
  module's input, which is a slot reference the user never sees; a reorder
  remaps it through the permutation, because a hidden pointer is exactly the
  thing that would have broken silently. Legacy projects decode through the
  `ModSourceRef` adapter `mod_metadata.rs` had been shipping unconsumed since
  the spec landed, taking each slot number as its own id so an old route
  keeps pointing at what it always did.
  Then measured what modulator capacity actually costs before planning to
  grow it, and the answer moved the plan: the command ring that `bridge.rs`
  warns about is not the big line -- control outputs are, at twice the ring,
  because every per-channel array is preallocated for `MAX_CHANNELS = 256`
  whether or not those channels exist. So capacity is cheap and dimensioning
  by the ceiling is expensive, which is now `docs/plans/archive/modulator-capacity/`.
  Built its first step: the three places that still quietly capped the number
  after step 03 claimed they did not -- a literal row count, a shelf that
  could not scroll, and a per-slot segment bank -- with a test that renders
  the same shelf at eight and sixteen so a regression is visible rather than
  theoretical.
  Then went to size the engine's per-channel arrays by the live channel
  count and found the plan I had just written was wrong about where the
  memory was. It said 3.1 MiB and named the modulator arrays, because the
  measurement behind it had only tallied what the modulator work touched.
  The render graph actually reserves 42.8 MiB, of which modulation is 433
  KiB -- one percent -- and 37.7 MiB is the channel strip, almost all of it
  `EffectChain`. The cause is that `MAX_CHANNELS` and
  `MAX_EFFECTS_PER_CHANNEL` are both the u8 index space, so the graph
  reserves their product: 65,536 effect slots, each with a 320-byte pending
  queue. No individual definition looks unreasonable; the number only exists
  when they are multiplied, which is why it went unnoticed and why the fix
  is a pinned test rather than a paragraph. Stopped short of the refactor
  and rewrote the plan against the real numbers instead.
  Then took the first half of the fix Adam asked for -- allocate none of them
  at launch, allow more up to a high limit -- on the effect chain, where the
  weight was. An effect slot's host and control state became one boxed
  struct, so an addressable-but-empty slot costs a pointer rather than the
  496 bytes it used to reserve. The chain fell from 141 KiB to 20 KiB and the
  graph from 42.8 MiB to 11.6 MiB, with both ceilings untouched. The subtlety
  worth recording is that host controls belong to the slot and not to the
  device in it, so `install` carries wet/dry and the trims across a
  replacement rather than letting a fresh box reset what the user dialled.
  Then the other half: channels are materialized from a project rather than
  reserved 256-deep, which took a sixteen-channel project from 42.8 MiB of
  startup reservation to 1.1 MiB. Two things only the doing settled. The
  per-channel vectors had to stay separate rather than becoming one struct,
  because the block loop borrows strips, events and control outputs with
  different mutabilities at once and bundling them turns that into a borrow
  conflict. And `AddChannel` stopped being a POD command: it allocates now,
  so leaving it on the realtime ring would have made it silently do nothing
  exactly when a channel needed creating. The bug that surfaced -- the
  sequencer claiming a channel the graph had no storage for -- is why there
  is a `live_channels` accessor rather than two counts that agree by
  convention.
  Then finished the plan by taking the rack off the command ring: modulation
  edits name one fact each -- a parameter, a slot, a route -- so an entry is
  136 bytes rather than 936 and stops moving when capacity does. What the
  step turned out to be about was the diff, not the width: the whole-rack
  path was what restored a destination's base when a route vanished, so
  every narrow command runs through one helper that keeps the before-and-
  after rack and hands back any destination that lost its last route. Two
  departures. The wide command was deleted rather than kept for presets and
  undo, because those already rebuild the renderer through `install_project`
  -- and a dead wide variant would have held the ring at 936 bytes, which is
  the whole thing the step was for. And reordering the grid needed its own
  narrow verb, or it would have been the one gesture still sending
  everything. Started the ML-P8 as a fourth device: its own params struct and
  its own parameter id space starting at zero, since it is the one generator
  that is not the shared three-oscillator voice with a different count. Its
  block is the widest a command carries now, so the modulation ring's cost
  test moved from 136 to 152 bytes an entry -- recorded rather than worked
  around, because twelve network amounts and three sync selectors are what
  the instrument is. Then built the network itself: six directed
  cross-modulation routes all reading the previous sample, so the graph is
  causal and order-independent; noise into every phase input whether or not it
  is audible; a derived sub that follows its source's pitch and sync but not
  its cross-modulation; and hard sync with a band-limited reset. The sync
  correction made aliasing *worse* until two things were fixed -- the step
  height has to be measured on the naive waveform rather than through the
  PolyBLEP that has already corrected it once, and the oscillator's own
  cycle-boundary residual has to stand down for the sample after a reset,
  where it would otherwise correct a wrap that did not happen at a height that
  is not the one it stepped by. Neither shows up in a test that hunts for a
  high band, because a synced oscillator is exactly periodic at its master's
  rate and every alias folds onto the master's own harmonic grid; the test
  compares harmonic magnitudes against an eight-times-oversampled render
  instead. The plan's "skip an oscillator nothing reads" rule also needed a
  caveat it could not have known: the skip reads target levels and levels are
  smoothed, so a knob reaching zero un-needs a source while its ramp is still
  running, replacing the ramp with the step the smoother exists to prevent. Then
  Adam pushed back on the face -- three pages of knob rows, the same synth UI
  again, and twelve identical bipolar knobs for the one thing that makes this
  device different from the last one. Rebuilt it as a matrix: rows are
  sources, columns destinations, the diagonal is an oscillator on itself, and
  sync is the row underneath, so the topology is a picture instead of twelve
  labels to read. The cell is a `ParameterKnob` with a new `show-dial: false`
  rather than a second draggable control, because arming a modulation source
  changes what every gesture means and two implementations of that would
  drift. Laying it out with layouts was a binding loop -- a knob sizes its
  insides from its width, so asking a cell how wide it wants to be depends on
  the answer -- which is the same wall `KnobField` hit and solved by placing
  its parts by hand. Adam then marked up a screenshot in GIMP,
  purple for space used and not needed, orange for space awkwardly empty, and
  said it still read as a stereotypical Linux UI. Both marks had one cause:
  the layout spread space evenly instead of spending it on what carried
  information. The fix was an idea rather than a nudge -- a mix level is a
  route too, source into the output, so it became a MIX column of the grid.
  That filled the matrix's empty half with real information, emptied the knob
  columns that were crowded, and let the grid state the whole truth about
  where every source goes. The three bordered cards became one surface split
  by hairlines along the signal path, since three identical rounded panels is
  the look of a settings dialog. Then step 03: two envelopes, four filter
  modes off one shared SVF, keytracking read off the gliding frequency, both
  velocity depths, and a feedback loop with the drive inside it, which is what
  makes feedback change the tone rather than only the gain. Two findings. The
  loop was being cleared in the fresh-slot restart, but stealing a sounding
  voice deliberately keeps its oscillator phases and was keeping the loop with
  them -- exactly the tail the plan says a reassigned slot must not emit; it
  is its own method now, run wherever a slot changes hands. And the device
  outgrew its row: `face-height` is a global constant and "3U" is a label, so
  the answer had to be structural. Making the source column's five tabs the
  same list as the network grid's five rows absorbed the leftover sub and
  noise panel and freed the whole right side for the filter and the amplifier
  as one region. Then made good on a note left in step 03: the ML-P8's
  parameter block had widened the engine's command ring twice, and the comment
  said the next move should be the fix rather than a bigger number. It was.
  Every entry in a fixed ring is as wide as its widest variant, so shipping a
  whole 200-byte struct to move one knob made every unrelated command pay for
  the largest device; ML-P8 edits go through a narrow
  `SetChannelGeneratorParam` now and the ring is back to 136 bytes. The test
  pins the property rather than the number. It made the UI smaller too --
  addressing controls by descriptor id instead of by field path deleted a
  closure-per-control macro and got the knobs the same clamping the typed
  values already had. Landing step 04 on `main` turned up one thing the
  branch's own doc comment had already promised and the derive had not
  delivered: `MlP8Routes` said a saved patch carries the routes it has and no
  more, while serializing all sixteen slots -- and TOML cannot write a `None`
  element, so no ML-P8 patch could be saved at all. It writes the occupied
  routes now, and trusts them over a `next_id` that disagrees, because a
  counter that has fallen behind is how a durable id gets handed out twice.
  Investigated the `spike/sampler-time-stretch` spike (#32),
  found its "build vs. buy" conclusion under-argued, and merged it to `main`
  once Adam decided to own the build regardless. Built the sampler's #13
  time-stretch on top of it: a WSOLA unit with a grain mode that leaves the
  similarity search off on purpose, because the phase-discontinuity rattle
  it produces at extreme ratios is the target sound (NIN *Year Zero*
  territory), not an artifact to suppress. Composed the stretcher with the
  existing transposing reader so pitch and rate are independent controls,
  moved its ~1.6 MB per-voice state to one device-level pool provisioned off
  the realtime thread instead of paying it per voice, and added a
  fit-to-tempo mode that derives the stretch ratio from bars/tempo/playback
  rate so a sample's loop locks to the grid however it's transposed.
  Then slice mode, which had to resolve the rule the stretch work inherited
  from the spike -- WSOLA runs forwards only, so reverse was disabled whenever
  stretch was on, and a slicer that loses reverse the moment a loop is
  tempo-fitted is not a groovebox. Two findings settled the design. Slicing
  does not depend on the stretcher at all: ReCycle/REX fitted breaks to tempo
  by retriggering markers, with no resynthesis, so the three time strategies
  stay independent rather than layered. And committing a stretch is a render,
  not a new engine -- `SampleData` is already an immutable `Arc` published
  through an `ArcSwapOption`, so freezing a stretched region is the existing
  `Stretcher::next_frame` driven by a plain loop on the control thread. That
  collapses every "disable X while stretching" case to one rule: stretch is
  live for forward pitched playback, and committed for reverse or slice.
  Reviewing that work found two things the tests had been too weak to see. The
  offline renderer assembles a channel's audio in its own second place, so an
  export shipped no slice maps and no committed buffers -- a sliced channel
  rendered silent exactly where the app was not. And auditioning a slice with
  the transport stopped was cut off a block after it began, because the
  sampler released every voice on every stopped block rather than on the
  transition; nothing had noticed because until auditions existed there was no
  way to sound a note while stopped. Both now have tests confirmed to fail
  without their fix, the second measured as level rather than as playhead
  position -- a releasing voice is still an active voice and its head keeps
  advancing, so the obvious check passed straight through the bug.
  Then the face: a play-mode selector, numbered boundaries draggable over the
  waveform with press-to-audition and right-click-to-remove, a
  base-note/count/DIVIDE/CLEAR row that takes the loop fields' place rather
  than adding a row a 3U face has no room for, and a commit row that becomes a
  baked-ratio badge and REVERT. Slice edits are the first undoable sampler
  edits -- there were none -- so they follow the modulator-param precedent and
  collapse a drag through the same gesture token the piano roll uses. Two
  things the snapshot caught that reading the diff had not: the loop region
  band was still being drawn in slice mode, where the loop is the slice and
  the global loop points describe nothing, and only one of the two loop
  markers had been greyed.
  Then promoted the mockup tool from a toy to something worth reaching for.
  The blocker was never the features, it was that adding one widget meant
  editing five ordered lists across three files, so nobody did: one catalog in
  `mockup-catalog.slint` now feeds the palette, the render switch, the default
  sizes and the drag behaviour, and both copies of the Rust half collapsed
  into `src/mockup.rs`. Saved layouts key on component name rather than
  palette index, so reordering the catalog stops corrupting them. `build.rs`
  scans `ui/` for exported components and the tool subtracts its own catalog
  from that, which is the part that keeps paying: the gap shows up in the
  palette as an UNCATALOGUED group rather than in someone's memory. Then
  filled the palette from that group -- 29 kinds, near enough doubling it,
  leaving only the two composite editors nobody has factored -- and wrote
  `docs/WIDGET_INVENTORY.md` for the converse gap: patterns that recur with no
  component behind them at all. Two of its eleven entries turned out to be
  live bugs rather than duplication, both caused by the missing component:
  `DisplayPrefs.smooth-curves` reaches only 4 of the 17 hand-rolled plots, and
  the mono and poly LFO glyphs are the same hardcoded cubic sitting under a
  waveform selector neither of them reads.
  Measured where `mooloop-ui`'s build time actually goes: `build.rs` spends
  14s compiling Slint, and rustc then spends 336s on the 488k lines it emits.
  A quarter of that module was developer-only surfaces `main.slint` re-exports
  into the same root document, so `cargo run -p mooloop-app` compiled them
  too. Dropped `gallery.slint`, the largest at 16.5%: the mockup tool's
  catalog now covers every widget the gallery hand-listed, so it was a second
  list to keep in step for no coverage the audit does not already give. Then
  dropped `widget-sheet.toml` behind it: proposing it as the gallery's
  replacement was the wrong instinct, since it was scaffolding an agent had
  left that afternoon, unread by any test and unknown to Adam. `rack-row.toml`
  went with it, emptying `tests/fixtures/` entirely. Then added
  `scripts/slint-sketch`, which was the real lesson from measuring that build:
  the four minutes were pricing out the look-and-adjust loop that visual work
  runs on, so `.slint` got written defensively from memory instead of checked.
  `slint-viewer` interprets the real widgets in ~0.05s, which closes the loop.
  Then planned the drum synth's v2 as `docs/plans/drum-synth-v2/`, after Adam
  reported that mod-source assignment could not be enabled for it the way it
  was for the sampler and the synths. The cause is not the missing descriptor
  table `generator.rs` calls mechanical work: `DrumSynthParams` is a
  mode-union, so two thirds of it is inert at any moment and a parameter id's
  meaning depends on a discrete selector that must not itself be a modulation
  destination. Giving v1 a table is therefore a different instrument, which is
  what DS-01 is -- one universal percussion voice on Microtonic's argument
  rather than Tattoo's selectable engines, since per-engine namespaces would
  reintroduce the same union with more code in it. Reading v1 turned up two
  things the plan had to answer that nobody had decided: `render_range`
  re-reads params per range while `trigger` latches envelope coefficients and
  cutoffs, so what a knob does to a hit already sounding is an artifact of
  where the code happened to read the struct -- DS-01 publishes that split as
  a table instead. And because `trigger` snapshots, a route aimed at a hit
  lands on the next one unless parameter events precede note-ons at the same
  offset, which is a renderer contract no descriptor-addressed generator has
  needed before.
  Then mocked the face up in `slint-sketch` before anyone builds it, which
  cost 0.2s a look instead of the four-minute build, and the second concept
  falsified the step I had just written: three source columns over one display
  carrying every envelope overlaid is soup at ninety pixels tall, needs a
  legend to say which curve is which, and arrives at twenty-six near-identical
  knobs in a grid -- the same "pages of knob rows" that was rejected on
  ML-P8's first face, from a different direction. Rewrote it as lanes, where
  each layer's controls and its contour share a row on one time axis, so the
  noise envelope being shorter than the tone envelope is visible rather than
  two numbers to compare. Both concepts are checked in under
  `mockups/`, against `AGENT_OPERATIONS.md`'s rule that sketches stay in
  `$TMPDIR`, because these are the argument for a decision rather than notes
  from making one.
  Adam then asked what if each scope sat directly under its own generator,
  which is a better idea than the lanes and for a reason I had missed: the
  three layers are parallel in the signal path, so drawing them as parallel
  columns is truer than stacking them as rows, which implies an order they do
  not have. Building it surfaced the thing all three concepts had been
  hiding -- roughly 55 controls do not fit on a 268px face, and every layout
  so far had quietly been showing about two thirds of them. A scope directly
  under its controls is what fixes it, because it can carry that envelope's
  handles: attack, hold and decay come off the knob rows and onto the curve,
  which is 13 cells back and takes the face from short to 61 cells for 55
  controls. So the scope being an editor rather than a readout is load-bearing
  rather than a nicety, and `08-the-face.md` says to build the handles in the
  same step as the scopes. Swept the project records after three days of
  parallel work had outrun them: rewrote `docs/FOCUS.md`, whose sequence still
  named a finished step and omitted the two workstreams actually consuming
  time; wrote nine missing days into `docs/JOURNAL.md` and refreshed its open
  threads, four of six of which had quietly closed; archived the four complete
  plans; corrected `CURRENT.md` and `ROADMAP.md` where they described a
  smaller system than exists; and added `docs/plans/README.md` so plan state
  is one file rather than nine. Archiving turned out to be the drift's own
  cause -- it had already stranded a dozen `docs/plans/<name>/` references in
  source comments, so those were repointed at `archive/` and a check confirms
  every plan path in the tree resolves.

### Claude Fable 5 — Claude Code
- First seen: 2026-08-31
- Last seen: 2026-08-31
- Sessions: 1
- Notes: Wrote `docs/plans/archive/modulator-modules/` — the modulator-grid plan —
  and started step 01: modulator params join the descriptor system.

### Claude Fable 5.1 — Claude Code
- First seen: 2026-09-02
- Last seen: 2026-09-02
- Sessions: 2
- Notes: Sanity pass over the sampler slice/commit push (commit bakes the
  stored ratio, revert re-provisions the stretch pool, slice-mode seed and
  rate fixes, slice note-offs, one-click re-bake). Then device ordering:
  one permutation per structural edit, run over routes and lanes on both
  the UI and engine sides, so modulation and automation follow a moved
  effect and die with a removed one; channel delete/paste renumber every
  channel-scoped address; effect add/move/remove became undoable; the
  integrity pass repairs stranded and dangling addresses.

### Claude Sonnet 5 — Claude Code
- First seen: 2026-08-21
- Last seen: 2026-09-01
- Sessions: 18
- Notes: Rounded out the UI mockup tool's palette with the remaining real
  controls (meters, mute/solo, trim knob, device chassis), fixed its
  selection tab and click-vs-drag handling, and wired a launcher into
  Preferences > Developer. Set up this file at Adam's request. Refreshed the README
  screenshot. Sampler UI overhaul: waveform zoom/scroll, sample-accurate
  trim/loop fields, compact tuning knobs with a note/frequency readout, a
  per-voice playhead, and no more auto-loaded kick on a new channel. Audio
  preferences: driver/output-device/buffer-size/auto-reconnect controls for
  JACK, behind a per-driver control surface so ALSA can slot in later.
  Diagnosed general CPU jankiness to unguarded denormal floats in recursive
  DSP state; added an MXCSR FTZ/DAZ guard on the realtime thread plus
  snap-to-zero epsilons in the parameter smoother and envelope follower.
  Assignable keyboard shortcuts: the action registry (`actions.rs`,
  `docs/ACTIONS.md`), a generic key dispatcher replacing the old hardcoded
  chain, pane switching and piano-roll zoom shortcuts, undoable pattern
  clone/remove, and the Preferences > Shortcuts page. Closed out FOCUS.md's
  command-layer step: piano-roll multi-select (Shift/Ctrl-click, Select
  All, bulk delete), Clear Pattern, and a pattern right-click context menu.
  Menu-popup positioning pass: add-channel and add-effect popups now open
  next to the button that triggered them instead of a fixed spot; File/Edit
  menu-bar titles switch on hover (worked around Slint 1.17 only chaining
  mouse-move for the built-in Menu widget kind, not a hand-rolled
  PopupWindow); the add-effect type list is de-duplicated into one
  left-aligned, content-width component shared by every insert trigger.
  Effects-feedback pass: removed a stale UI-side 8-effect cap so the rack
  matches the backend's real 256-effect ceiling, and reworked ParameterKnob
  to put the label above the knob and a bright monospace value readout below
  it, sized to its own content and bounded to its knob's column. EQ
  selection/layout: shrank the EQ face to 2U with a SelectorBank band strip,
  fixed the response curve's Q falloff, made coincident band points
  separately clickable, and fixed a drag-test harness bug where a fixed
  Window width/height literal silently ignored `set_size()` in tests.
  Sampler bugfix pass: fixed a browser-load race where a channel's own
  default-sample reset could overwrite the file the user had just picked,
  reworked the pitch-and-speed group's layout after a knob/field composition
  bug clipped its own controls off the face, and made a sampler's tuning
  live -- it was baked into a voice's playback rate once at trigger and
  never revisited, so retuning a held or looping note (by hand or by
  modulation) silently did nothing until the next note-on. Added an opt-out
  toggle for the old per-trigger behavior, defaulting to live.

### GLM 5.3 Flash (glm-5.3-flash) — opencode
- First seen: 2026-08-23
- Last seen: 2026-09-01
- Sessions: 17
- Notes: Effect-container refactor: latency-aligned dry path, one dB trim
  knob everywhere, bus-effect metering, and the shared effect-device shell.
  Extracted the shared DraggablePoint handle and gave the EQ band points
  and the Filter cutoff/resonance point a common drag + wheel interaction.
  Docked the transient hover/status overlay as an always-visible bottom
  status bar. Made the piano roll's dock resizable via a draggable
  splitter, with a moving-origin drag integrator and a snapshot-tested
  clamp/restore contract. Added the browser sidebar shell: right-docked
  column in flow with the work area, status-bar toggle chip, and an
  ew-resize grip on the same integrator. Filled the sidebar with the
  sample browser: locations persisted in settings.toml, zenity folder
  picker through the pump, VS Code-style tree with expand/collapse,
  wav-only listing, and right-click location removal. Browser pass two:
  playable-children filtering behind a format predicate, an autoplay arm
  and preview-volume trim knob, and a header-stats info pane fed by a
  dedicated engine preview voice with live shared gain. Started the
  gain-structure plan: characterization tests pinning today's source
  peaks, summing, reverb wet-path gain, and fader travel identity, then
  the shared gain module (`mooloop-core/src/gain.rs` + `GainMath` in
  `gain.slint`) with the fader taper and its cross-boundary agreement
  test, then the fader taper and dB readouts across mixer strips, the
  bus output stage, and oscillator level knobs, then the -12 dBFS
  operating level: calibrated every generator against it, set channels
  genuinely at unity, and wrote docs/GAIN_STRUCTURE.md as the standing
  reference, then pinned the per-oscillator unity reference and made
  drive level-compensated (reference-anchored saturation shared by every
  drive stage), then energy-normalized the reverb IR, level-matched the
  plate, and switched the host wet/dry blend to equal-power, then put
   the meters on IEC 60268-18 with the warning threshold at -10 and
   pixel-verified colour transitions, completing the gain-structure plan.
   Opened the modulator-system branch and laid its metadata groundwork:
   `mooloop-core/src/mod_metadata.rs` with durable `ModSourceId` refs and
   source descriptors (shape, rate, latency, trigger), a legacy local-slot
   LFO decode, and `ModDestinationDescriptor` defaults that derive from each
   `ParamCurve` so stepped targets refuse modulation until they opt in.
   Added the bitcrush style row: a `BitcrushStyle` param (crush, TPDF
   dither, µ-law companding, interpolated hold) threading core descriptors,
   the DSP branch, and the device-face SelectorBank, with per-style DSP
   tests pinning the signal behaviors the styles exist for.

### GPT-5 — Codex
- First seen: 2026-08-21
- Last seen: 2026-09-02
- Sessions: 74
- Notes: Audio-core architecture, realtime project swaps, compiled bus graphs,
  latency/gain hardening, device-host controls, command-history foundation, and
  realtime capacity policy, mixer signal-slot design, CI, packaging, and the
  retained-audio buffer device/event path and off-realtime tempo/config ring
  replacement; release README revisions. Refined the channel modulation shelf
  into compact source, source-editor/input, and destination modules, with an
  explicit assignment mode separate from source selection. Augmented the
  channel LFO with free/synced rate and fade-in, clickable sync LEDs, smoothing,
  pulse width, and note-triggered realtime reset. Added the first second source
  type: a tempo-syncable ADSR envelope with explicit cross-channel piano-roll
  gate input, unipolar route defaults, realtime note-gate handling, and its
  compact shelf editor. Made the compact and expanded modulator faces follow
  the configured ADSR and LFO signal shapes instead of generic source icons.
  Refocused the active work on distinct Mono and Poly identities followed by
  the Buffer's ordinary composition workflow. Integrated the composable-device
  unit contract with the existing realtime and modulation architecture.
  Reconciled and merged the local ML-P8 work with the newer sampler
  time-stretch history on GitHub main.

### GPT-5.6 Terra — Zed
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 1
- Notes: Fixed duplicate loop-wrap event scheduling.

### Kimi k3-256k — Kimi Code CLI
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 1
- Notes: Implemented the poly synth source device end to end (DSP voice pool,
  engine integration, Slint face, and persistence).

### ox-alpha — opencode
- First seen: 2026-08-23
- Last seen: 2026-08-23
- Sessions: 2
- Notes: Rescued the ZoomScrollBar widget from an abandoned WIP branch and
  wired it into the piano roll's time and pitch axes.

### Kimi k3-256k — opencode
- First seen: 2026-08-21
- Last seen: 2026-08-21
- Sessions: 1
- Notes: Implemented the effects-chain vertical slice (filter effect, end to end).
