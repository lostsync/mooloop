# Focus

Status: active working sequence, **rewritten 2026-09-23**. The previous
version was written 2026-09-21 and ran out within two days. By 2026-09-23
containers 07 and 08 had landed, `audio-recording/` was archived, and almost
all of the teams report's polish backlog (`reports/teams-2026-09-22.md`) had
closed in one day, about eighty issues, alongside plugin-hosting 01-04.
**The sequence below starts from that tree.**

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

## The sequence

### 1. The layer device is a gesture — `containers/` step 10 (MOO-72)

07 and 08 landed 2026-09-23: a layer is a device, it is in the insert menu,
it splits its input across its branches, aligns them and sums them. What it
does not have is the gestures: wrapping a selection *as a layer* (the wrap
button only knows chains), adding and removing a branch as a named act, and
a layer that saves as a preset. Step 10 is those, and it closes the plan
except for the drawing.

- **10 does not wait on 09.** 09 is the drawing and stays blocked on Adam's
  mock-up (MOO-71), deliberately: the chain's enclosure was drawn three times
  without one. 10's own text never depends on the drawing. It keeps whatever
  the rack draws for a layer today and must not invent a branch treatment on
  the way. That is 09's question, and 09 lays out three options.
- **The case to play** is the one the device exists for: a drum loop into a
  clean branch and a Drive → Bitcrush branch, mix swept. That is parallel
  compression. It has never been heard (08's pass was closed unheard), so
  play it at the start of 10 as well as the end.
- **`scripts/dupe-audit popup-close-order`** before any menu is written. A
  wrap-kind menu is the fifth place that trap would be found.

### 2. A plugin you can hear — `plugin-hosting/` steps 05 and 06 (MOO-80, MOO-81)

01-04 and MOO-56 landed 2026-09-23: clack-host runs a CLAP plugin, a plugin
parameter has its own owner, a missing plugin is kept, and a plugin rack
holds each instance until its processor is gone. None of it can be heard yet,
because nothing finds a plugin on disk and nothing puts one in a chain.

- **05, the scanner**, runs out of process from the first version. That is
  Adam's answer 4, and it is not negotiable: a plugin that crashes while
  being scanned must not crash mooloop.
- **06, a headless CLAP effect in a chain**, is the step with a sound. Its
  case: a real third-party CLAP effect on a drum loop, saved, reopened with
  the plugin present and with it missing, and exported.
- 07 onward (parameters and automation, the face, the boxed source,
  instruments, GUI windows) are the rest of the plan. They are next after 06
  unless this document is rewritten first.

### 3. Adam's answers, as a batch

About twenty open Linear issues carry the `Question` label, and they now gate
more of 0.2.0 than the engineering does. Take them in one sitting rather than
one at a time as each blocks a step: the master bus compressor (MOO-13),
sampler key zones (MOO-14), Sampler V2's loop seams, tempo fit and mono/legato
(MOO-43, MOO-39, MOO-45), the stretching-polyphony cap (MOO-7), the left
sidebar (MOO-8), the browser (MOO-9), the layer's drawing (MOO-71), and the
smaller calls listed under "Waiting on Adam" below.

**Record every answer on its issue and in the plan it settles**, then rewrite
this document. What gets answered decides what step 4 is.

## Waiting on Adam, not on work

The `Question` label in Linear is the complete list. These bear on 0.2.0:

- **The layer device's drawing** (MOO-71): a mock-up, not a paragraph. 09
  has the three options (the rack grows, branches scale, or branches are
  tabs).
- **The master bus compressor** (MOO-13): three voicings, all measured
  (`SCOPE.md` §2.1). Open: the face, since vari-mu is six coupled positions
  rather than an attack and a release, and whether the safety limiter should
  look ahead (MOO-169).
- **Sampler key zones and Sampler V2** (MOO-14, MOO-43, MOO-39, MOO-45,
  MOO-7).
- **The left sidebar and the browser** (MOO-8, MOO-9).
- **Smaller calls:** whether tails ring after Stop (MOO-171), whether a
  saved Buffer freeze keeps its audio (MOO-196), the drag-lock key (MOO-164),
  whether a solo click is an undo step (MOO-162), whether the clipboards
  outlive the song (MOO-161), Grip's low shelf (MOO-163), a modulator inside
  a container (MOO-160), and the tracker question (MOO-159).
- **Theming:** relief (MOO-153) and the face paddings (MOO-157).

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
  grows glyphs but not boxes), and the two questions above.
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

1. **The layer device's parallel compression** (step 1's case).
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
