# Focus

Status: **the overnight sequence and a first performance pass are done
(2026-09-25); the next sequence is Adam's call.** The Linear project
**Performance** holds what was fixed and what is left: MOO-254 (Reverb), and
the repaint cost while playing (MOO-258,
upstream Slint), plus MOO-263. MOO-264 closed on 2026-09-26: Cold Metal at
Unison X8 is 79 µs a block (`device_cost`), down from 112, with the output
bit-identical. Adam ruled on MOO-263 on 2026-09-25: *"we can
do v2, totally"*, so Linux release builds move to x86-64-v2. The Slint FemtoVG clip and ellipsis
costs (MOO-256, MOO-258) are fixed upstream in Slint 1.18.1, so nothing was
reported and nothing is patched. The upgrade is MOO-268 (Adam, 2026-09-26: no fork). MOO-227's effect half was ruled out on 2026-09-26:
an effect preset's click only selects.
Nothing from either push has been heard. `JOURNAL.md` has the record.

**The next push is 0.1.6, scoped 2026-09-26.** It is the open issues with
the `0.1.6` label in Linear, and nothing else. The label is the list, and this
document does not copy it. `AGENTS.md` explains the labels under *Releases: the
`Release` labels*. **Order: bugs first** (Adam, 2026-09-26). Correctness comes first
(MOO-242, 243, 135, 200, 239, 237), then the UX items, then the plugin face,
then performance.

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
  sequence below, lane 2 opens on a zipper and moves straight to glide and
  key zones, which are new ways to play a sample, and lane 3 opens on the
  oldest defect Adam reported himself.

## The sequence, 2026-09-25 (overnight): done

All of it is on `main` and closed, and MOO-227's effect-preset half was ruled out
(2026-09-26: an effect preset's click only selects). The night also found and fixed MOO-241 (two
takes in one second overwrote each other) and filed MOO-237, MOO-238,
MOO-239, MOO-240 and MOO-242. The lanes as they were queued:

**Lane 1, Rendering (0.2.0, all of it).** Export becomes a job that can write
several files.
1. MOO-180: the dialog picks a folder and a name, and one render writes
   several files. Start from the parked branch `feat/moo-180-render-job`
   (see the issue's comment). Everything else in the lane stands on it.
2. MOO-181: render a range (whole song, loop selection, custom, current
   pattern).
3. MOO-188: auto-bump the file name instead of overwriting.
4. MOO-186: 16-bit with dither, dither on 24-bit, mono.
5. MOO-223: an export flushes denormals the way playback does, so the two
   are bit-identical.
6. MOO-182: stems from mixer tracks, in one pass.
7. MOO-183: channels straight out, bypassing the mixer.
8. MOO-190: the dialog remembers its last settings across launches.

**Lane 2, the sampler's instrument side (Sampler V2, 0.2.0).**
1. MOO-221: ML-P8's Pan and Spread may zipper the way Volume did. Confirm
   it first (it carries `Triage`).
2. MOO-201: a NaN in the ML-P8's voice feedback, or any self-fed delay in a
   source, stays there.
3. MOO-7: the sampler's stretch pool follows Voices, by a structural resize.
4. MOO-45: mono, legato and glide for the sampler, using ML-M1's `GlideMode`
   plus `EnvTrigger` pair and the shared glide block.
5. MOO-14: key zones. The zone type leaves room for a velocity range, and
   velocity layers are not built. A v1 song loads as one full-range zone.
6. MOO-227: a click on an instrument preset in the browser auditions it.
   What an effect preset's audition sounds like is a question for Adam; ask
   it on the issue and build the instrument half.

**Lane 3, timing, spikes and small correctness.**
1. MOO-234: MIDI recording over a looping pattern (`SHORT_NOTES.md`), then
   an overdub/replace toggle.
2. MOO-209: recorded MIDI is compensated for the driver's playback latency.
3. MOO-233: the Preamp's band display runs two analyzers in the callback
   for nobody. It is the largest source of callback spikes in Adam's own
   songs.
4. MOO-210: a container's output trim is inert.
5. MOO-226: an insert at full wet leaks its dry signal at -147 dB.
6. MOO-230: a hosted instrument's restart fades out instead of cutting.
7. MOO-231: the null-driver test that flakes on a loaded macOS runner.

**Rulings for this sequence:**
- Adam's 2026-09-23 answers apply as written on each issue. Rendering is
  all in 0.2.0, and an agent designs the export dialog's layout. Key zones
  are built and velocity layers are not. The sampler's stretch pool is
  resized structurally. Sampler glide uses ML-M1's pair of controls.
- MOO-234's Replace mode removes the recording channel's notes in the span
  the playhead crosses, and each pass stays one undo step. If that doesn't
  fit the pattern model, the team asks on the issue and lands Overdub.
- Out of this sequence: key-zone mapping workspace and SFZ (MOO-40, MOO-41),
  the rest of Rendering (MOO-184, 185, 187, 189, 191, 193, 194), and
  everything under "Left for later" below.

## The sequence, 2026-09-24: done

Twenty issues in three lanes: interface fixes, CLAP hosting through a
plugin as a channel's instrument, and the sampler's loop story. All of it is
on `main`, and all of it is closed except MOO-219, which is In Review for
Adam. `JOURNAL.md` has the record. Its rulings still stand: MOO-43 is
crossfade only, the master's safety limiter has no lookahead, the master
keeps its built-in Bus Comp, and the insert runs the same DSP. A plugin's
role (effect or instrument) comes from its own CLAP features, and its ports
decide only what can be wired.

## Left for later

- **Plugin hosting's rest:** a plugin face's modulation ring and parameter
  naming (MOO-228), pinned parameters and a Plugins preferences page
  (MOO-229), and plugin presets (MOO-222). Also an instrument's parameters,
  which need a source-slot id (MOO-74's open point), GUI windows (MOO-86),
  VST3 (MOO-87) and AU (MOO-88). Most of this needs someone to look at it.
- **The browser:** rename by double-click (MOO-224), spring-loaded preset
  drag (MOO-225), and the full-size pane (MOO-10).
- **The listening passes themselves:** eighteen are queued, and nothing
  since 2026-09-18 has been heard.

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

The `Question` label in Linear is the complete list (an `Answer` label marks
one he has answered that nobody has acknowledged yet), and after 2026-09-23 it
is short:

- **Relief beyond the first controls** (MOO-153). `d3c90211` shipped the
  bevel, Platinum and Impulse, and stopped where the step says to look before
  converting the rest of the controls. It needs a look, not a paragraph.
- **The listening passes** below, which nobody has taken since 2026-09-18.
- **MOO-219:** whether a collapsed device stays collapsed when the song
  reopens, and one look at the folded strip's rotated label in the running
  app (the test renderer ignores rotation).

MOO-159 (the tracker) keeps its label, because "not now" defers the question
rather than answering it.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

- **`from_index` answers out-of-range input two different ways.** The
  `ALL`-table convention clamps and the hand-written `match` falls through to
  variant 0. It is the recurring fault, an option list and a Rust table
  disagreeing about how many options exist, presenting as two bugs. Decide
  the edges before refactoring.
- **There is no driver-free `EngineHandle`**, so the control-plane boundary
  cannot be exercised whole in a test. `archive/control-plane-seams/01`'s
  `CommandSink` is the partial answer.
## Outside the sequence, and in 0.2.0

These are in `SCOPE.md` and not deferred. They are outside this sequence the
way `coreaudio-driver/` was: asked for directly, and worked when asked.

- **Rendering** (Linear project Rendering; joined 0.2.0 2026-09-23, all of
  it). Its first eight issues are lane 1 of the current sequence. The rest
  (pattern renders, wrap tail, sample rate, name templates, presets,
  normalize, and settings saved in the song) follow MOO-180.
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

**Sampler V2 is in for 0.2.0** (Linear project Sampler V2, umbrella MOO-42;
`SCOPE.md` §4). Its loop half landed on 2026-09-24, and its instrument half
(legato and key zones) is lane 2 of the current sequence. Weigh it against
the loop story, chopped and stuttered breaks and loops mangled per repeat,
rather than against generic sampler completeness. The mapping workspace and
SFZ import (MOO-40, MOO-41) come after key zones.

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
   So should a hosted plugin's restart (MOO-213). With the LSP filter from
   item 7 on a sustained pad at a low cutoff, change the latency or
   oversampling setting that makes it ask for a restart, or change JACK's
   sample rate while it plays: the filter should fade to dry and back, with
   no click. There's no offline render for this, because a restart only
   happens live. The measured version is
   `scripts/antibox --no-incremental cargo test -p mooloop-engine --lib -- hosted_plugin_for_its_placeholder --nocapture`.
   A hosted *instrument's* restart fades too (MOO-230): hold a pad on a
   CLAP instrument and force a restart or a rate change. It should fade out
   with no click, and the held note stays silent until the next note-on,
   which is deliberate. The measured version is
   `scripts/antibox --no-incremental cargo test -p mooloop-engine --lib -- hosted_instruments --nocapture`.
6. **The master bus compressor, each voicing** (MOO-13, step 3). *"Turning it
   on should feel special"* is the acceptance case and only a listen can pass
   it. A kick, a bass line and chords a few decibels hot, rendered with the
   section out, then Grip, Punch and Tube in (the safety limiter has no
   lookahead since MOO-217, so there is no lookahead render), to float WAVs
   with their reduction printed beside them:
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
9. **ML-P8's Volume under modulation** (MOO-214). A held chord of sines,
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
10. **The sampler's loop seam** (MOO-43). A one-bar synthetic break whose
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
11. **Fit to tempo, and SYNC off keeping the sound** (MOO-39). A 1.5 s loop
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
12. **A lane sweeping Loop start on a grid** (MOO-47). A one-bar break
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
13. **Slices detected on a break** (MOO-44). A two-bar synthetic break with
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
14. **A sliced break played back from its pattern** (MOO-46). The same kind
    of two-bar break is sliced by detection, and PATTERN's function writes a
    note per slice. It renders once as written and once with neighbouring
    slices swapped:

    ```sh
    scripts/antibox --no-incremental --pull target/slice-pattern \
      cargo run -p mooloop-session --example slice_pattern_case -- target/slice-pattern
    ```

    The binary compares the pattern's render with the break. Their 10 ms
    energy envelopes correlate at 0.997, and every note lands within 2 ms
    of its slice; a note starts on a whole tick, which at 120 BPM is 5.2 ms.
    Play `pattern.wav` against `break.wav`, which should be the same loop,
    and `reordered.wav`, which is the chop. Then, in the app, slice a real
    break, press PATTERN, then REPLACE, and move a few notes in the roll.
15. **The Bus Comp on a drum bus** (MOO-216). A two-bar kick, snare and
    hat loop on a drum bus, rendered dry, then with a Bus Comp insert on
    the drum bus, then with no insert and the master's own section in at
    the same settings (threshold -24 dB, no makeup), for each voicing:

    ```sh
    scripts/antibox --no-incremental --pull target/bus-comp-insert \
      cargo run -p mooloop-engine --example bus_comp_insert -- target/bus-comp-insert
    ```

    The binary prints each render's reduction against `dry.wav`, per 10 ms
    window. MEASURED. Play `insert-grip.wav`, `insert-punch.wav` and
    `insert-tube.wav` against `dry.wav`. Each `master-*.wav` should be the
    same sound. Then, in the app, put a Bus Comp on a real drum bus from the
    insert menu and try each voicing.
16. **A folded device's label, turned on its side** (MOO-219). A look, not a
    listen: fold a device with the `<` at the top of its rail. Its name and
    kind should read top to bottom down the strip, rotated a quarter turn
    clockwise, as the header reads left to right. It is the interface's
    first rotated element. The software renderer (`slint-sketch`, the tests)
    ignores rotation, so it has only been seen unrotated. The desktop app
    renders with femtovg, which rotates. Also fold a Chain holding a folded
    device, open it again, and check the inner fold is still folded.
17. **A CLAP filter put in a chain from the window** (MOO-83,
    plugin-hosting 08: lane 2's acceptance case). In the app, open a drum
    loop, hover the arrow after its last device, pick **Plugin…**, type
    `filter` in the PLUGINS tab and double-click LSP's *Filter x2 Stereo*
    (`/usr/lib64/clap/lsp-plugins.clap`). Its face shows its parameters by
    their own names, a page at a time; turn the frequency (a page or two in)
    while the loop plays, undo it, redo it, then save, reopen and export.
    The export should sound like the last thing heard, and the reopened face
    should say what it said. Also try the rack with the plugin uninstalled
    or renamed: its face should stay, greyed, saying it is missing, and the
    loop play dry. The measured version runs the in-repo test gain through
    the same window handlers, saves, reopens and exports, and holds the
    export's level to the knob's gain within 0.1%:
    `scripts/antibox --no-incremental cargo test -p mooloop-ui --lib plugin_ui`.
    The face has only been seen with the software renderer (`slint-sketch`).
18. **A CLAP instrument playing its channel's pattern** (MOO-85,
    plugin-hosting 10). A one-bar melody of six notes, the note on the last
    step held across the loop point, played by the in-repo test sine as the
    channel's source and exported the way the app exports:

    ```sh
    scripts/antibox --no-incremental --pull target/clap-instrument \
      cargo run -p mooloop-session --example clap_instrument_case -- target/clap-instrument
    ```

    Play `instrument.wav`. Each note should start clean on its step, with no
    click, and release over 50 ms; `missing.wav` is the same song with the
    plugin missing, and is silence. The binary prints each note's first
    sounding frame against the pattern's: all six landed on their frame on
    2026-09-25. `instrument.mooloop` is the song. A real instrument through
    the same path: `clap_instrument_case <dir> --real <clap id>` on the
    laptop opens one through `~/.config/mooloop/plugins.toml` and writes
    `real.wav`. No installed plugin makes sound this way yet: LSP's *Sampler
    Stereo* is accepted, opens and plays, and is silent without a sample
    loaded into it; its *Trigger MIDI* declares itself an effect and is
    refused as a source; Airwindows has no instruments. Surge XT or Dexed
    would be the real case.
19. **The sampler's mono glide and legato** (MOO-45). A long sine sampled
    at A3 plays a one-voice line whose first four notes overlap and whose
    last starts after a gap, rendered three ways: Glide 80 ms with Env trig
    Legato, the same with Retrig, and no glide (how every older song plays):

    ```sh
    scripts/antibox --no-incremental --pull target/sampler-legato-glide \
      cargo run -p mooloop-engine --example sampler_legato_glide -- target/sampler-legato-glide
    ```

    Play `legato.wav`: the overlapping notes should slide into each other
    with no new attack, and the note after the gap should start fresh at its
    own pitch. `retrig.wav` slides the same way but restarts each note.
    `none.wav` steps. The binary prints how long each render's first slide
    takes and how far its level dips across it. Measured 2026-09-25: the 80 ms glide's first slide takes 75 ms
    in both glided renders (the measure's floor is about 15 ms, which is what
    `none.wav`'s plain step reads). The level dip across the overlap is
    0.3 dB in both: a Retrig note fades the stolen voice out while the new one
    attacks, so level can't tell the two apart, and only listening will say
    whether Retrig's restart from the top of the sample is heard. Then, in the app,
    set a sampler to one voice, turn Glide up, and play legato on a keyboard.
20. **A click on an instrument preset** (MOO-227). There's no render for
    this: it's the browser. With autoplay on, open the PRESETS tab and click
    an ML-P8, an ML-M1 and a DS-01 preset in turn. Each should play a short
    phrase (an arpeggio into a chord, or a bar of hits) without changing the
    song or its undo history, and a click on a second preset while the first
    plays should cut to it. An effect preset's click plays nothing, by Adam's
    ruling on MOO-227. The render path is pinned by
    `an_instrument_preset_auditions_as_a_rendered_phrase` (session).
21. **Sampler key zones** (MOO-14). Two sine files, the sampler's own at C4
    and a zone's at C5 an octave up and half the level. One sampler plays its
    own up to B3 and the zone from C4, rooted at C4 and C5. It is saved
    embedded, reopened through the app's loader and exported through the
    app's export path:

    ```sh
    scripts/antibox --no-incremental --pull target/sampler-key-zones \
      cargo run -p mooloop-session --example sampler_key_zones -- target/sampler-key-zones
    ```

    Play `key-zones.wav`: C3 and B3 are the louder base tone, then C4 and C6
    the quieter zone, each at the pitch its key names. B3 to C4 should be a
    semitone step across the split, with no jump in pitch. The binary prints
    each note's measured pitch and level against what its zone should give.
    Measured 2026-09-25: every note within 0.1 cent of its key (130.8,
    246.9, 261.6 and 1046.5 Hz), and the base zone's notes 6.02 dB over the
    zone's, the files' own difference. A song with no zones renders
    bit-identical to `main`: the 16 WAVs of `sampler_legato_glide`,
    `sampler_loop_seam` and `sampler_slice_detect` hash the same. Then, in the
    app, add a zone on the ZONES page, play
    across the split on a keyboard, and undo the add.
22. **The Modulation phaser at its fastest** (MOO-235). The phaser now
    works out its all-pass coefficients every 8 samples and draws a straight
    line between them, where it used to do an `exp2` and a `tan` per stage
    per sample. The claim is that it sounds the same. A sustained ML-P8 saw
    chord through a 12-stage phaser at half wet, Depth 100%, Feedback 70%,
    two bars at 12 Hz (the fastest Rate) and then two at 0.5 Hz:

    ```sh
    scripts/antibox --no-incremental --pull target/phaser-sweep \
      cargo run -p mooloop-engine --example phaser_sweep -- target/phaser-sweep
    ```

    Play `phaser.wav` against `dry.wav`. The 12 Hz half should be a fast,
    even warble with no grain or buzz riding on it, and the 0.5 Hz half a
    smooth sweep. There's no old build in the render to compare against; the
    comparison with the old per-sample formula is measured instead, in
    `control_rate_phaser_matches_the_per_sample_formula` (mooloop-dsp), on
    noise and a saw at 12 Hz, full depth and 85% feedback with knobs moving.
    Measured 2026-09-25: the difference is 81, 74 and 71 dB under the wet
    signal at 4, 8 and 12 stages.
23. **ML-P8 unison at a constant level** (MOO-244). Turning Unison up should
    thicken a note, not make it louder. One held A3 on Init Saw at 1x, 2x,
    4x and 8x, at a sweep of Detune and Drift:

    ```sh
    scripts/antibox --no-incremental cargo test -p mooloop-dsp --release --lib \
      unison_level_table -- --ignored --nocapture
    ```

    The table prints RMS against 1x (before the change it read up to +18 dB
    at 8x). In the app, step a held pad through the Unison counts: the level
    should hold while the width grows. Then open Adam's songs whose ML-P8s
    use unison, which now play quieter than they were mixed. The renders and
    each channel's level, from:

    ```sh
    scripts/antibox --no-incremental --pull target/mlp8-unison \
      cargo run --release -p mooloop-session --example mlp8_unison_levels -- \
      target/mlp8-unison ~/perf-songs/housey-dropout-factory.mooloop ~/perf-songs/ok-then.mooloop
    ```

    Measured 2026-09-25, before and after, each channel alone:
    `housey-dropout-factory` "ML-P8 9" (2x) -2.1 dB; `ok-then` "ML-P8 1"
    (8x) -9.6 dB, "ML-P8 7" (4x) -2.3 dB and "ML-P8 15" (4x) -1.3 dB. The
    whole songs moved 0.06 and 0.01 dB. Whether those channels want their
    faders back up is a listening call.
24. **The Drive after its oversampler changed** (MOO-250, MOO-251). The
    2x oversampler is now a 31-tap half-band with a rational `tanh` inside.
    A saw chord, one bar each through the default Drive, Tape Warmth and Hard
    Clip, then dry:

    ```sh
    scripts/antibox --no-incremental --pull target/drive-curves \
      cargo run -p mooloop-engine --example drive_curves -- target/drive-curves
    ```

    Play `drive.wav`. Each Drive should sound as it did: the smooth curves
    measure 64 to 70 dB from the old path, and Hard Clip 45 dB, where the
    difference is the two kernels' aliasing
    (`the_drive_matches_the_loop_before_the_oversampler_changed`, mooloop-dsp).
    There's no old build in the render to A/B against. A song with a Drive
    saved before today is the real comparison.
25. **ML-P8's filter at control rate** (MOO-246). A voice's filter
    coefficients are now worked out every 16 samples and ramped toward
    where the cutoff is heading, instead of every sample. The claim is that
    it sounds the same. Play the fastest filter envelope the device has (a
    1 ms filter attack on a resonant 24 dB low-pass, a pluck) and Furnace
    Stab, whose voice feedback runs the filter's output back through a
    saturator, before and after. There's no old build in the render. The
    comparison with the per-sample filter is measured instead, in
    `a_control_rate_voice_filter_tracks_the_per_sample_one_closely`
    (mooloop-dsp), on every factory patch as a chord and on that pluck.
    Measured 2026-09-25: the difference is 54 dB under the pluck and 62 to
    78 dB under the patches, with every octave band within 0.01 dB. The
    exception is Furnace Stab. Its feedback loop turns the smallest change
    in rounding into a different waveform (only 29 dB under), but its
    octave bands stay within 0.35 dB, so it is the one to listen to.
26. **The starter kit** (MOO-267). A new song now opens with four DS-01
    channels from the Machine Kick, Machine Snare, Machine Hat and Machine
    Open Hat factory patches, voiced as plain 80s drum machine sounds from
    what the 808, 909 and LinnDrum are known to do. Nobody has heard them. A
    two-bar beat at 120 BPM (kick on 1 and 3, snare on 2 and 4, closed hats
    on the eighths, one open hat that the next closed hat chokes), then each
    hit on its own:

    ```sh
    scripts/antibox --no-incremental --pull target/starter-kit-audition \
      cargo run -p mooloop-engine --example starter_kit_audition -- target/starter-kit-audition
    ```

    Play `starter-kit-80s.wav`, then `kick.wav`, `snare.wav`,
    `closed-hat.wav` and `open-hat.wav`. Measured 2026-09-25, each hit alone
    at velocity 110 through the starter's mixer: kick peak -12.4 dBFS, 40 dB
    down after 299 ms; snare -11.2 dBFS, 143 ms; closed hat -19.6 dBFS,
    38 ms; open hat -19.6 dBFS, 394 ms. The choke cuts the open hat to
    silence where it would still be at -68 dBFS. Whether they sound like a
    drum machine, and whether the balance is right, is the listen. Every
    value is in `crates/mooloop-core/src/ds01_factory.rs`.
27. **The Modulation device at high feedback** (MOO-200). Past Feedback 75%
    the wet output is now trimmed so a feedback resonance peaks at +12 dB,
    where it reached +22 dB at the knob's end (92%). Below 75% nothing
    changed, and no factory preset goes that high (Jet Flange is 70%). At
    92% the wet is 9.9 dB quieter than it was, at 85% 4.4 dB. The loop is
    untouched, so the claim is that a flanger at full feedback still
    sounds like a jet, only not 22 dB louder. A sustained ML-P8 saw chord at
    half wet, two bars each of a Flanger at 50%, 75%, 92% and -92%, a
    Phaser at 92%, then dry:

    ```sh
    scripts/antibox --no-incremental --pull target/modulation-feedback \
      cargo run -p mooloop-engine --example modulation_feedback -- target/modulation-feedback
    ```

    Play `modulation-feedback.wav`. There's no old build in the render; the
    old level is the new one plus the trim. The measured version is
    `every_insert_kind_stays_under_its_gain_bound` (every mode at 75% and
    ±92%, bound +12.5 dB; +12.02 dB was the most) and
    `a_flanger_at_full_feedback_still_rings` (80 ms on, the ring is
    -21 dB from its first echoes at 92%, against -47 dB at 75%), both in
    mooloop-dsp. Songs with a Modulation device above 75% feedback play
    quieter than they were mixed; whether the jet still sounds right is the
    listen.
28. **A range export that starts inside a reverb** (MOO-239). A range is
    now the song's own frames, rendered from the top with nothing written
    before the range. In the app, put a long reverb on a channel whose note
    ends just before bar 2, and export Range: custom, from bar 2. The file
    should open inside the reverb's tail, and a note held across the range's
    start should sound from its first sample. The measured version holds
    the range to the whole song's frames:

    ```sh
    scripts/antibox --no-incremental cargo test -p mooloop-engine --lib -- \
      a_range_holds_the_reverb_from_before_it a_note_held_into_a_range
    ```
29. **A mono CLAP effect in a chain** (MOO-266). A mono effect now runs:
    the chain reaches it as `(L + R) / 2` and its output goes to both sides,
    so a centred sound should come out at the level it went in, not twice
    as loud, and a hard-panned one 6 dB down on both sides. In the app, open
    the browser's PLUGINS tab: LSP's `_mono` plugins are no longer greyed.
    Put `Filter Mono` on a centred drum loop with the filter open, and A/B it
    against bypass: the level should not move. Then the same with the loop
    panned hard left. The offline case is step 06's binary with a mono id:

    ```sh
    bin/clap_effect_case --plugin /usr/lib64/clap/lsp-plugins.clap \
      --id in.lsp-plug.filter_mono --out clap-case-mono
    ```

    The measured version is
    `scripts/antibox --no-incremental cargo test -p mooloop-engine --lib -- plugin_mono`.
28. **The Modulation device's Width and Mix** (MOO-245). Width is new: a
    mid/side scale on the wet signal, from both voices folded to the
    centre (0%) to the mode's own image (100%, the default and today's
    sound, bit for bit). The face's Mix is the slot's own wet/dry, which
    the face already carried as "Wet". The face's knobs are now five a row
    and its trace is narrower. A sustained ML-P8 saw chord through a chorus
    at full Spread and half Mix, two bars each at Width 100%, 50% and 0%,
    then at full Mix, then dry:

    ```sh
    scripts/antibox --no-incremental --pull target/chorus-width \
      cargo run -p mooloop-engine --example chorus_width -- target/chorus-width
    ```

    Play `chorus-width.wav`. Measured 2026-09-26, side against mid: -5.2 dB
    at 100%, -11.2 dB at 50%, none at 0%, and the level barely moves
    (-24.3, -25.2 and -25.5 dBFS RMS). Whether 0% sounds like a good mono
    chorus rather than a comb, and whether the face reads well with five
    knobs a row, are the listen and the look.
