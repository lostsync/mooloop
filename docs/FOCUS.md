# Focus

Status: **the 2026-09-24 sequence is chosen and queued; not started.** Adam
picked it on 2026-09-24: the sidebars, CLAP hosting, and the sampler's loop
story, with a handful of fixes and interface tweaks alongside. Every item is
a Linear issue in **Todo**, 22 of them, in the three lanes below. A new
session starts the work. The 2026-09-23 sequence (the layer device, a plugin
you can hear, the master bus compressor) landed in full and its record is in
`JOURNAL.md` and each step's archived plan.

`archive/ROADMAP.md` orders the whole product by dependency, and `SCOPE.md`
says what 0.2.0 is. This document is narrower: it names the active sequence
and the work that should not interrupt it. **Rewrite it when that sequence is
exhausted.** Two failure modes to keep it out of: do not let it become a
second roadmap, and **do not let it accumulate the archaeology of closed
steps**. That is what `docs/plans/<name>/00-status.md`, Linear and
`JOURNAL.md` are for.

Read `PRODUCT.md` for the product argument, `CURRENT.md` for the implemented
surface, and the projects in Linear for what state each plan is in. Source and
tests settle any disagreement with any of them.

## The rule

**Prefer changes that produce a musical decision over changes that merely add
capacity.**

The engine has more breadth than the instrument has identity. A new source
type, graph abstraction, effect, or routing primitive is not progress by
itself. Each step must end in something that can be played, heard, saved,
reopened, and rendered through the ordinary UI.

Two siblings, kept because they outlived the plans they came from:

- **An interface change is judged by whether something that already exists
  becomes easier to reach**, not by how much new surface it adds.
- **Order the work so something new is audible early.** 2026-09-23 was the
  biggest day of correctness work this project has had, clicks, NaNs, undo,
  durability, export parity, and almost none of it is a new sound. In the
  sequence below, lane 3 opens on a zipper Adam heard in a real tune and
  moves straight to new loop gestures, and lane 2 ends with a plugin put in
  a chain from the window.

## The sequence, 2026-09-24

Three lanes, one team each: at most three at once, each in its own
worktree, every build on antibox (`docs/OPERATIONS.md`). Work each lane top
to bottom; an item lower in a lane may start early only if the one above it
is blocked. **Linear is the state**: the issue's status, and its comments for
any ruling made since it was written.

**Lane 1: interface, and the fixes Adam hits every day** (Interface, with
Mixer and Engine for the last two)

1. **MOO-220**, a knob stops following its parameter once dragged. The EQ's
   knobs keep the last band's values when a new point is picked, and undo,
   presets and MIDI are hidden the same way on any touched knob. First,
   because it is a bug met every session and the fix (controlled by default,
   plus an all-faces test) touches the shared controls every later step uses.
2. **MOO-8**, the left sidebar: an action and a shortcut, and mute/solo,
   volume and pan in both the sidebar and the rack row.
3. **MOO-9**, the browser sidebars: search and filter, click auditions a
   preset, double-click or Enter loads it, drag places it.
4. **MOO-218**, add a device from the `→` between two devices; the rail `+`
   and the end slot go.
5. **MOO-219**, a device collapses to its header turned on its side, from a
   `<` where the rail `+` was. After MOO-218 or with it, never before.
6. **MOO-217**, remove the master safety limiter's lookahead knob (reverses
   MOO-169; no lookahead at all).
7. **MOO-215**, the load readout says "not realtime" while the callback
   thread is SCHED_FIFO.

**Lane 2: CLAP hosting** (`docs/plans/plugin-hosting/`; Parameters & Control,
Engine, Effects, Interface)

1. **MOO-82**, step 07: a plugin's parameters, automation, modulation and
   state round-trip.
2. **MOO-213**, the placeholder/plugin swap has no fade. Triage first: write
   the continuity test and see whether it fails.
3. **MOO-212**, a plugin inside a container gets its alignment sized for zero
   latency.
4. **MOO-83**, step 08: the plugin browser, the menu row, and the face for
   plugins with no GUI. This is the step that puts a plugin in a chain from
   the window, so it is the lane's audible acceptance case.
5. **Stretch: MOO-84 and MOO-85**, steps 09 and 10, CLAP instruments.

**Lane 3: the sampler's loop story, then the Bus Comp** (Instruments,
Effects)

1. **MOO-214**, ML-P8's Volume is not smoothed and zippers under modulation.
   Small, and Adam hit it in a real tune.
2. **MOO-43**, click-free loop seams by **crossfade** (0 ms default,
   bit-identical there).
3. **MOO-39**, tempo fit's leftovers; turning SYNC off **freezes** the
   derived ratio.
4. **MOO-47**, loop bounds quantized to divisions and slice markers.
5. **MOO-44**, transient detection and automatic slice markers.
6. **MOO-46**, a sequencer pattern from slice markers.
7. **MOO-216**, the bus comp as an insert device too, and "Bus Comp"
   everywhere the user sees it.

**Left in Backlog on purpose:** key zones and multisample (MOO-14, MOO-40,
MOO-41) and legato (MOO-45). They are the sampler's instrument side, not its
loop side, and a day of their own.

**Rulings made 2026-09-24**, on the issues:

- MOO-43 is **crossfade only**. This supersedes the 2026-09-23 answer below
  ("a crossfade *and* zero-crossing snap"); a snap is not part of it.
- MOO-39: SYNC off freezes the ratio, as answered on 2026-09-23.
- MOO-217: the master's safety limiter has no lookahead and no knob.
- MOO-216: the master keeps its built-in section; the insert runs the same
  DSP; the name is "Bus Comp".
- MOO-219: nested devices collapse, and a collapsed container hides its
  contents. Whether the fold is saved with the song is still Adam's call; the
  issue recommends saving it without an undo step.

## The last sequence, 2026-09-23: done

The layer device (MOO-71, MOO-72), a CLAP effect in a chain with its scanner
(MOO-80, MOO-81), and the master bus compressor (MOO-13, MOO-169, MOO-208).
All landed, none heard; their listening passes are items 1, 6 and 7 of
"Listening is a step" below. Left over: MOO-210 (a container's output trim is
inert), MOO-160 (deferred), MOO-209 (recorded MIDI compensated for no output
latency).

## Answered 2026-09-23, now construction

Adam answered the `Question` batch in one sitting on 2026-09-23. Each answer
is on its issue. These are the ones that change what gets built:

- **Key zones** (MOO-14): zones for 0.2.0, with a data model that leaves room
  for velocity layers, which are not built.
- **Sampler V2**: the stretch pool follows Voices by a structural resize
  (MOO-7), loop seams get a crossfade *and* zero-crossing snap (MOO-43; **superseded
  2026-09-24: crossfade only**),
  turning SYNC off freezes the derived ratio (MOO-39), and mono glide uses
  ML-M1's `GlideMode` + `EnvTrigger` pair (MOO-45).
- **Left sidebar** (MOO-8): mute/solo, volume and pan appear in both the
  sidebar and the rack row, and rename stays in both places.
- **Browser** (MOO-9): a click on a preset selects and auditions it.
  Double-click or Enter loads it, and dragging places it.
- **Tails ring out after Stop** (MOO-171). The rack keeps rendering while
  stopped until it is at rest.
- **Buffer's Freeze is not saved at all** (MOO-196): *"freeze is
  temporary."*
- **Clipboards survive New and Open** and re-resolve ids against the new song
  (MOO-161).
- **Note-drag axis lock is Alt**, rebindable (MOO-164).
- **Grip gets the SSL's drive-dependent low shelf**, and only Grip (MOO-163).
- **A solo click keeps its own undo step** (MOO-162, closed, no change).

Deferred past 0.2.0: modulators inside containers (MOO-160) and the tracker
question (MOO-159).

## Waiting on Adam, not on work

The `Question` label in Linear is the complete list, and after 2026-09-23 it
is short:

- **Relief beyond the first controls** (MOO-153). `d3c90211` shipped the
  bevel, Platinum and Impulse, and stopped where the step says to look before
  converting the rest of the controls. It needs a look, not a paragraph.
- **The listening passes** below, which nobody has taken since 2026-09-18.
- **Whether a collapsed device stays collapsed when the song reopens**
  (MOO-219).

MOO-159 (the tracker) keeps its label, because "not now" defers the question
rather than answering it.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

- **`midi recording doesnt loop properly`** (`SHORT_NOTES.md`): after one
  playthrough, notes stack at the last tick. Capture's position wrap was
  fixed 2026-09-17. What Adam reports is the pattern not starting over, and
  he wants an overdub/replace toggle beside it. Not yet diagnosed against
  the tree, and the oldest user-reported defect still open.
- **`from_index` answers out-of-range input two different ways.** The
  `ALL`-table convention clamps and the hand-written `match` falls through to
  variant 0. It is the recurring fault, an option list and a Rust table
  disagreeing about how many options exist, presenting as two bugs. Decide
  the edges before refactoring.
- **There is no driver-free `EngineHandle`**, so the control-plane boundary
  cannot be exercised whole in a test. `archive/control-plane-seams/01`'s
  `CommandSink` is the partial answer.
- **A NaN in the ML-P8's voice feedback loop, or any self-fed `DelayLine` in
  a source, stays there** (MOO-201). It is the one latch the 2026-09-23 NaN
  work left.

## Outside the sequence, and in 0.2.0

These are in `SCOPE.md` and not deferred. They are outside this sequence the
way `coreaudio-driver/` was: asked for directly, and worked when asked.

- **Rendering** (Linear project Rendering; joined 0.2.0 2026-09-23, all of
  it). MOO-180, the dialog that picks a folder and a name and lets one render
  write several files, is in progress and the rest stand on it.
- **Theming's remainder**: MOO-154 (accessibility), MOO-205 (the type scale
  grows glyphs but not boxes), MOO-157 (face paddings), and relief (MOO-153,
  above).
- **The polish backlog's remainder** (Linear project Polish backlog, about
  seventeen issues). The ones that matter to a player: the control menu and
  typed entry stopping at `ParameterKnob` (MOO-202), and keyboard reach in
  the menubar and pickers (MOO-203). The file splits (MOO-102, MOO-105,
  MOO-109, MOO-100) are real and are not musical decisions.

## Deliberately not now

- **`device-registry/`**: a survey, not a work order. The exception still
  stands: a **face host component** would take each of `main.slint`'s
  fourteen face arms from twenty-seven lines to eight with no Rust change.
  Take it if a step already has `main.slint` open. Do not open a step for it.
- **`pattern-bank-floor/`** (MOO-152): every project reserves 1.00 GiB
  before it holds anything. The measurements are committed, which is what
  makes parking it safe.
- **A toolkit swap.** `egui-view-layer/` was archived unstarted on Adam's
  ruling (MOO-146); if the view layer ever leaves Slint, Qt is the candidate
  he named.
- **The modulation rack's move, and whether its modulator is a tracker**
  (MOO-159). Settle whether they are one design or two before planning either.
- **More effect kinds, or more modulator kinds.** The short-notes device
  ideas (mid/side, a gain/pan/width utility) are cheaper *after* the layer
  device has its gestures: a mid/side device is at least close to a layer of
  two branches with an encode in front and a decode behind. Raising `MAX_MODULATORS_PER_CHANNEL` is a one-line decision
  and not an invitation.
- **Sidechain key inputs, and MIDI out.** Both in 0.2.0, neither next.
  Sidechain needs a dependency edge that schedules a producer without summing
  it in. Read `archive/typed-audio-edges/` first. A layer's branches are not
  this: they have no strip and no place in the bus graph.
- **The text-label-to-icon pass**, and **a curated factory bank.** Every
  device ships presets to prove its architecture reaches its range, and that
  is the only bar until a deliberate content push.
- **Playlist clip manipulation and richer missing-sample relinking.**
  Autosave and crash recovery landed 2026-09-23 (MOO-103).
- **Metronome and the graph editor.** A take's count-in is still a silent
  bar.

## Two standing judgements that are not steps

**Buffer stays a device, and the rack-end placement is not ruled out.** Adam's
2026-08-30 position was that making Buffer an ordinary insert was partly the
wrong call. It is meant to be part of how playback works inside mooloop, at
the **end of a device rack with its own sequencing lane**. Put to him again on
2026-09-15 with a realtime-sampler framing, he said it *"puts some of my
doubts about a device-based implementation to rest"*, and the plan closed on
2026-09-18. The lane is not foreclosed: it would drive the same published
parameters. Buffer now keeps its history across a tempo change, an undo and a
reload (MOO-137); whether a saved freeze keeps its audio is MOO-196.

**Sampler V2 is in for 0.2.0, and its loop half is lane 3 of this
sequence** (Linear project Sampler V2, umbrella MOO-42; `SCOPE.md` §4). Weigh
it against the loop story, chopped and stuttered breaks and loops mangled per
repeat, rather than against generic sampler completeness. Its instrument half
(key zones, multisample, legato) is not in this sequence.

## Working discipline

Keep one audible acceptance case per branch and run it through realtime,
persistence, and offline rendering where the change crosses those boundaries.
The plan files define the order inside a plan; update their status as steps
land rather than duplicating implementation notes here. When every step of a
plan is done, **move the directory to `archive/`**.

Keep branches small enough to listen to and revert independently. Preserve
stable parameter IDs, conservative project defaults, deterministic rendering,
and the realtime rules in `AUDIO_ARCHITECTURE.md`. `AGENTS.md` governs
worktrees, commits, and verification.

**Listening is a step, not a formality.** The last passes on record as
actually heard are DS-01 and its kit (2026-09-04), ML-P8 and its bank
(2026-09-05), the ML-M1 with its patches, the channel strip's three voicings
(2026-09-11), and `incremental-structure/` (2026-09-18). Everything since was
closed on Adam's 2026-09-22 instruction to treat outstanding passes as done
with nothing heard. **Nothing from 2026-09-22 or 2026-09-23 has been
heard**, and some of it changes existing sounds rather than adding new ones.
In the order worth playing:

1. **The layer device's parallel compression** (step 1's case). A drum loop
   on a track whose layer holds a clean branch and a Drive → Bitcrush branch,
   the layer's Mix at 0, 25, 50, 75 and 100% in turn, two bars each:
   `scripts/antibox --no-incremental --pull target/listening/layer-parallel-drums.wav cargo test -p mooloop-engine --lib -- layer_parallel_drums --nocapture`,
   then play `target/listening/layer-parallel-drums.wav`. The test prints
   each branch's and each section's level. Then build the same thing in the
   rack, from the list's `+`, and play with S and M.
2. **Bend, sustain, mod wheel and aftertouch** on every source, from a real
   keyboard (MOO-126, MOO-128).
3. **Saved patches whose sound moved.** The cutoff law is now one law at
   every sample rate (MOO-112, MOO-119), the resonance taper and 24 dB slopes
   changed (MOO-123), and glide now arrives in its stated time (MOO-145).
   ML-P8, ML-M1 and Acid patches are where a difference would show.
4. **The insert dynamics**: the Limiter looking ahead at true peaks, the Gate
   with hysteresis, and the Compressor with its own Mix (MOO-142).
5. **Transitions that used to click**: fader, mute, solo, bypass, preset
   loads and sampler steals (MOO-104, MOO-172). These should now be silent.
6. **The master bus compressor, each voicing** (MOO-13, step 3). *"Turning it
   on should feel special"* is the acceptance case and only a listen can pass
   it. A kick, a bass line and chords a few decibels hot, rendered with the
   section out, then Grip, Punch and Tube in (the limiter-lookahead render
   goes when MOO-217 removes the lookahead), to float WAVs with their reduction printed beside them:
   `scripts/antibox --pull target/master-bus-comp cargo run -p mooloop-engine
   --example master_bus_comp -- target/master-bus-comp`. From step 04 the
   same is reachable on the master's rack.
7. **A CLAP effect on a drum loop** (step 2's case, MOO-81): LSP's flanger
   on a two-bar drum-synth loop, exported, and the same song reopened with the
   plugin present and with it missing. Build the case and run it on the
   laptop, which has the LSP plugins:

   ```sh
   scripts/antibox --no-incremental --pull bin/clap_effect_case sh -c \
     'cargo build -p mooloop-session --example clap_effect_case && mkdir -p bin && cp "$CARGO_TARGET_DIR/debug/examples/clap_effect_case" bin/'
   bin/clap_effect_case --plugin /usr/lib64/clap/lsp-plugins.clap \
     --id in.lsp-plug.flanger_stereo --out clap-case
   ```

   Run it again with `--id in.lsp-plug.filter_stereo --out clap-case-filter`
   for the host's own check. On 2026-09-23 the filter passed all six
   measurements, and its 64-, 333- and 512-frame renders were bit-identical.
   The flanger passed five: it moves its LFO once a block, so its output
   depends on the block size by its own design, not the host's.

   Listen to `clap-case/dry.wav` against `wet.wav`. `reopened.wav` should be
   `wet.wav`, and `missing.wav` should be `dry.wav`. The binary prints what it
   measured. In the app, open `clap-case/clap-case.mooloop`: the flanger is
   on channel 1, and it plays live on JACK at any buffer size.
8. **A CLAP filter's cutoff automated on a drum loop** (MOO-82,
   plugin-hosting 07): LSP's filter on the hats of a two-bar loop, its
   cutoff lane falling across the loop, saved, reopened, exported, and played
   through the executor the way a callback plays it. Build on the box, run
   on the laptop:

   ```sh
   scripts/antibox --no-incremental --pull bin/clap_automation_case sh -c \
     'cargo build -p mooloop-session --example clap_automation_case && mkdir -p bin && cp "$CARGO_TARGET_DIR/debug/examples/clap_automation_case" bin/'
   bin/clap_automation_case --plugin /usr/lib64/clap/lsp-plugins.clap \
     --id in.lsp-plug.filter_stereo --out clap-automation
   ```

   Listen to `clap-automation/wet.wav` against `flat.wav` (the same song
   with the lane held at its start): the hats should darken across the two
   bars and snap back bright at the loop point, under an untouched kick and
   snare. `reopened.wav` should be
   `wet.wav` and `missing.wav` `dry.wav`. On 2026-09-24 it passed all six
   measurements: the twelve hats on the filter's channel came out darker hat
   by hat against the flat lane (high-pass energy 1.000, 0.397, 0.172,
   0.069 ... 0.000); the executor at 64 and at 512 frames matched the export
   to below the smallest normal float (MOO-223 is the subnormal rest); and it
   reopened present identically and missing as the dry loop, its slot and
   lanes kept. `live-64.wav` is what the executor played. In the app, open
   `clap-automation/clap-automation.mooloop` and play it on JACK.
8. **ML-P8's Volume under modulation** (MOO-214). A held chord of sines,
   its Volume pumped by a quarter-note LFO. It should duck smoothly, with
   no buzz on the duck:

   ```sh
   scripts/antibox --no-incremental --pull target/mlp8-volume-pump \
     cargo run -p mooloop-engine --example mlp8_volume_pump -- target/mlp8-volume-pump
   ```

   Play `pump.wav`, and `steady.wav` for the same chord unpumped. The
   binary prints the zipper lines beside each tone: 67 dB under it before
   the fix and 100 dB under after. Then do the real case, an Envelope gated
   by the kick and routed to ML-P8's Volume with negative depth, at an
   Attack well under 500 ms.
9. **The sampler's loop seam** (MOO-43). A one-bar synthetic break whose
   bass tone doesn't fit the bar, so the hard seam clicks. It's looped two
   ways, from the top (starting on the kick, with nothing before it) and
   after a lead-in, at Loop fade 0, 2, 10 and 30 ms:

   ```sh
   scripts/antibox --no-incremental --pull target/sampler-loop-seam \
     cargo run -p mooloop-engine --example sampler_loop_seam -- target/sampler-loop-seam
   ```

   Play `top-0ms.wav` against `top-10ms.wav`, and the same for `lead-in-*`.
   The binary prints what it measured. The step across the seam goes from
   0.18 to 0.0004 from the top, and from 0.16 to 0.012 after the lead-in,
   where 0.012 is the kick's own attack. From the top the fade only takes
   level away. Its first millisecond fades back in, which moves the kick's
   onset by up to 15 dB under its peak. Listen for whether that softens the
   downbeat. The 0 ms files hash the same as before the change.
10. **Fit to tempo, and SYNC off keeping the sound** (MOO-39). A 1.5 s loop
    fitted to one bar, rendered three ways: synced at 120 BPM, synced at
    90, and frozen at 120 then played at 90:

    ```sh
    scripts/antibox --no-incremental --pull target/sampler-fit-freeze \
      cargo run -p mooloop-engine --example sampler_fit_freeze -- target/sampler-fit-freeze
    ```

    The binary measures each loop's period from the render. Synced, the
    period follows the bar: 2.00 s at 120 and 2.66 s at 90. Frozen, it stays
    at 2.00 s at 90 BPM. Play the three, and the frozen one should be the
    120 BPM loop, unchanged. Then, in the app, turn SYNC off on a fitted loop
    and change the tempo. The loop's sound should stay put.
11. **A lane sweeping Loop start on a grid** (MOO-47). A one-bar break
    loops for four bars while a lane sweeps Loop start from the top to the
    last quarter, with the grid off and then on sixteenths:

    ```sh
    scripts/antibox --no-incremental --pull target/sampler-loop-grid \
      cargo run -p mooloop-engine --example sampler_loop_grid -- target/sampler-loop-grid
    ```

    With the grid off, the lane's 12,000 control ticks resolve to 12,000
    loop starts, a slide through every frame. On sixteenths they resolve to
    13, which is the grid. Play `off.wav` against `sixteenth.wav`. The
    second should step in rhythm, a sixteenth at a time.
12. **Slices detected on a break** (MOO-44). A two-bar synthetic break with
    a ghost snare and a bass tone under it, where every hit's frame is
    known. It's detected at three sensitivities, then chopped with the
    default's slices played backwards on the sixteenths:

    ```sh
    scripts/antibox --no-incremental --pull target/sampler-slice-detect \
      cargo run -p mooloop-engine --example sampler_slice_detect -- target/sampler-slice-detect
    ```

    At the default the binary reports 18 of 18 hits within 2 ms and no
    marker on anything else. At 0.25 it finds 16 and misses the ghost
    snares. Play `chop.wav`: every slice should start on its hit, with no
    flam and no pre-echo. Then, in the app, DETECT a real break, move a
    marker by hand, and REPLACE. The moved marker should stay.
