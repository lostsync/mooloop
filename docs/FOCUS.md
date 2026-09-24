# Focus

Status: **sequence complete 2026-09-24; the next one is Adam's call.** The
2026-09-23 sequence below (the layer device, a plugin you can hear, the
master bus compressor) landed in full overnight, worked by teams (see
`JOURNAL.md`), and every step's plan is archived or has moved past it. What
it leaves is listening, not construction. **Do not start new sequence work
from this document until it is rewritten** with the choice under "The next
sequence" below.

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
  durability, export parity, and almost none of it is a new sound. The
  sequence below opens with two things you can hear.

## The sequence, 2026-09-23: done

All three steps are on `main`, each rendered and measured, **none heard**.
Their listening passes are items 1, 6 and 7 of "Listening is a step" below.

1. **The layer device**: `containers/` 09 and 10 (MOO-71 `4ca4c4bf`, MOO-72
   `590e0f67`), built as Bitwig's FX Layer on Adam's answer: a branch list
   with S, M, a meter and `+`, the picked branch's chain to the right, per
   branch Level/Mute/Solo, wrap-as-layer, remove-a-branch, and a bank of
   three layer presets. Plan archived. Left in its Linear project: MOO-210
   (a container's output trim is inert) and MOO-160 (deferred).
2. **A plugin you can hear**: `plugin-hosting/` 05 and 06 (MOO-80
   `cae78bd5`, MOO-81 `47669d11`). The scanner runs each library in a child
   process and caches the result; a CLAP effect plays in a chain, saves,
   reopens with and without the plugin, and exports. Nothing in the window
   inserts a plugin until step 08. Filed on the way: MOO-212 (a plugin in a
   container), MOO-213 (the placeholder/plugin swap has no fade, Triage).
3. **The master bus compressor** (MOO-13, MOO-169; `d49401e0`, `5ef812e0`,
   `25a58a12`, `1bbf3c12`): Grip, Punch and Tube fitted to the measured
   units, per-voicing faces, a needle meter, and the limiter's lookahead
   knob (default 0, bit-identical there). Plan archived. Filed: MOO-209
   (recorded MIDI is compensated for no output latency at all).

## The next sequence

Not chosen. The candidates this document already named, in the order it
named them:

- **Plugin hosting 07 onward** (parameters and automation, then 08, the
  face and the browser that finally puts a plugin in a chain from the
  window). The previous version said 07+ were next after 06 "unless this
  document is rewritten first".
- **Sampler key zones** (MOO-14), "the next largest 0.2.0 item, and
  unblocked": zones for 0.2.0, with room for velocity layers.
- **The listening passes themselves**, before any of it. Seven are queued
  and three of them are this sequence's acceptance cases. Nothing since
  2026-09-18 has been heard.

## Answered 2026-09-23, now construction

Adam answered the `Question` batch in one sitting on 2026-09-23. Each answer
is on its issue. These are the ones that change what gets built:

- **Key zones** (MOO-14): zones for 0.2.0, with a data model that leaves room
  for velocity layers, which are not built.
- **Sampler V2**: the stretch pool follows Voices by a structural resize
  (MOO-7), loop seams get a crossfade *and* zero-crossing snap (MOO-43),
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
- **The next sequence** (above): plugin hosting 07+, key zones, or
  listening first.

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

**Sampler V2 is in for 0.2.0 and is not in this sequence** (Linear project
Sampler V2, umbrella MOO-42; `SCOPE.md` §4). Weigh it against the loop story,
chopped and stuttered breaks and loops mangled per repeat, rather than
against generic sampler completeness. **Do not treat it as the default pick
when nothing else is named.** Three of its issues are questions for Adam
(step 3).

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
   section out, then Grip, Punch and Tube in, and Grip with the limiter
   looking ahead 3 ms, to float WAVs with their reduction printed beside them:
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
