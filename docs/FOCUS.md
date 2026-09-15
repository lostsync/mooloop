# Focus

Status: active working sequence, rewritten 2026-09-12, amended 2026-09-14 when
`interface-iteration/` closed and its step left the sequence, and again on
2026-09-15 when `eq-v2/` came down to one optional step and its section was
cut back to what is live. That cut is this document's own rule about itself
being applied: three closed steps had left section 1 mostly archaeology. The previous
version was written 2026-09-05 and amended 2026-09-07 and 2026-09-08; it was
replaced rather than amended again, because the mixer it described was not the
mixer that exists and the work it parked included a feature that shipped.

`archive/ROADMAP.md` orders the whole product by dependency. This document is narrower:
it names the active sequence and the work that should not interrupt it. Rewrite
it when that sequence is exhausted. Two failure modes to keep it out of: do not
let it become a second roadmap, and **do not let it accumulate the archaeology
of closed steps** — that is what `docs/plans/<name>/00-status.md` and
`JOURNAL.md` are for, and it is most of why the last version reached
twenty-four kilobytes.

Read `PRODUCT.md` for the product argument, `CURRENT.md` for the implemented
surface, and `docs/plans/README.md` for what state each plan is in. Source and
tests settle any disagreement with any of them.

## The line has moved, and it moved somewhere this document did not name

The 2026-09-05 sequence was outlets, then the v1 drum synth's parameters, then
the interface, then Buffer. Its first two steps closed the same day and are
recorded in `docs/plans/archive/`. Then eleven days happened, and the largest
single arc in them was never a step here at all.

- **The console pass, 2026-09-09 to 2026-09-11, closed and archived.** Four
  items from one morning's list — reorder channels, group them, make the mixer
  work like a console, proper sends — which turned out to be one design with a
  character layer on top. The mixer is a list of tracks somebody made rather
  than a fixed seventeen. A track routes a copy of itself to another track, so
  **sends exist**. Any summing point can glue what feeds it, and the bus that
  decodes is invisible. Every track carries a channel strip — an input stage,
  four EQ bands, a compressor and polarity, all out by default, under one
  strip-wide voicing. Solo is **in place**, not the monitor tap
  `archive/MIXER_PLAN.md` had specified since the mixer's first pass. Adam played it
  on 2026-09-11: *"three distinct and musical characters"* on headphones, with
  the studio still to come.

  Read `docs/plans/archive/console/00-status.md` before touching the mixer,
  the strip or the summing, and `THE-STRIP.md` beside it for the strip's
  shape. Two rules in it outlive the feature: **a voicing selects laws, never
  values**, and **no control on a strip face declares a range** — the
  descriptor table crosses into the markup once at startup and
  `tests/strip_face.rs` fails if a bound is ever spelled in `strip.slint`.
- **`pane-layout/` closed 2026-09-08.** Five views, three slots, a view in
  exactly one slot at a time; the top pane splits, any pane zooms, a tab drags
  between panes, and `editor-page` is gone. Read its `README.md` before
  touching the work area.
- **`ui-consistency-pass/` closed 2026-09-08.** The audit Adam's standing list
  asked for, and every finding was a control saying something the engine was
  not doing. Its `README.md` records the method, so the sweep can be re-run
  rather than re-invented.
- **`interface-iteration/` closed 2026-09-14 and archived.** Four steps: a
  browser that browses presets, a device clipboard keyed by identity rather
  than slot, a channel that has a name and a colour, and the keyboard pass.
  Two things in it outlive the plan. **`Ctrl+C` asks what has focus** — a
  chord resolves to one action id and cannot do otherwise, so three meanings
  is one action with a `Scope`, and the fallback is the channel list because
  that is what these chords meant before there was a second clipboard.
  And **a registry is not a keyboard**: `transport.loop-toggle` had been
  dead since it shipped, on a bare L no branch of `main.slint`'s key ladder
  forwarded, with the registry, the prefpane and the whole suite green over
  it. `actions.rs`'s `decoding` module reads the markup now. Read
  `docs/plans/archive/interface-iteration/00-status.md` before touching the
  action registry or either key-decoding ladder.
- **Three days of correctness work, 2026-09-10 to 2026-09-12, that are not a
  plan and are worth knowing as a class.** Faces held to their descriptors by
  index rather than by order, the 441 persisted parameter ids frozen where
  they sit, the outlet tables pinned by number rather than by name, two of
  every synth's three oscillators found resetting to a tuning no patch has, a
  duplication audit with a tool (`scripts/dupe-audit`) and three recorded
  limits, one event splitter where twelve effects each had their own, and one
  device-name table where two had already drifted. None of it was asked for
  and all of it is the same shape: **a claim the source no longer supported.**

## The rule

**Prefer changes that produce a musical decision over changes that merely add
capacity.**

The engine has more breadth than the instrument has identity. A new source
type, graph abstraction, effect, or routing primitive is not progress by
itself. Each step below must end in something that can be played, heard, saved,
reopened, and rendered through the ordinary UI.

Step 1 is parameter-model work before it is a device, and any interface work
that turns up beside it gets a sibling rule, kept here because it outlived the
plan it came from: **an interface change is judged by whether something that
already exists becomes easier to reach, not by how much new surface it adds.**
A pane that shows what a menu already showed is not progress either.

## The sequence

Set by Adam on 2026-09-12: finish the interface iteration, then the EQ, then
Buffer. The interface iteration closed on 2026-09-14 and the EQ on 2026-09-15
but for one optional step.

**As of 2026-09-15 the sequence is waiting on Adam at both remaining points**,
and that is a state worth naming rather than working around. The EQ's step 04
is a taste question and wants his ear; Buffer's first move is a question about
the device's *shape* that the step below says explicitly not to begin
unprompted. Neither is blocked on a branch. Until one of them is answered, the
work that is genuinely available is the listening passes below and the fixes
further down.

### 1. `docs/plans/eq-v2/` — one optional step left, and two listens owed

**Steps 01, 02 and 03 landed on 2026-09-14 and 2026-09-15.** Every EQ band and
both pass filters carry their own stable ids; a shelf's Q knob designs its
slope instead of being ignored; and the response plot is the bank's own
coefficients evaluated rather than a shape drawn to resemble them. Read
`eq-v2/00-status.md` for what each one found — this document is not where that
belongs.

The one thing worth carrying out of it, because it is about the next device
and not this one: **the face did not have to change.** The EQ's parameter
space was shaped like its face, and resolving the selection one layer earlier
left the markup untouched. That is the answer to the question the plan was
written around — mooloop intends to host CLAP, a plugin's parameters are N
independent ids with no context, and the EQ was the one native device whose
model could not be expressed that way. What it did *not* settle is that
`EffectKind::descriptors()` is a table per **kind** while a plugin's
parameters belong to an **instance**; that crossing is `SCOPE.md` §"the four
things standing between here and CLAP", item 1.

**What is left is step 04, it is optional, and it is Adam's call.** It is the
measured character from five reference EQs — the only step that changes how
the device sounds when nobody asked it to.

**Two listening passes are owed before it, and they are the live work here --
but they are smaller than they were written up as.** Step 03's change was
measured on 2026-09-15 rather than left as "it sounds different": at the Q both
shelves rest at, it peaks at **0.45 dB**, it pivots about the corner rather
than moving the shelf, and it is gone three octaves out. The change that is
worth hearing is on shelves whose **Q knob is away from 0.707** -- up to 1.9 dB
-- which is exactly the knob that did nothing before step 03, so the patches
affected are the ones where somebody tried to use it and gave up.
`eq-v2/00-status.md` has the table and says where to listen. Step 02 changed
nothing audible and changed what the picture *claims*, which wants a look with
a patch moving.

Done when: ~~every EQ band and pass filter has its own stable ids, a lane on a
band means one band forever~~ (2026-09-14), and `eq-v2/00-status.md` says which
of 02 to 04 were taken. It does: 02 and 03 are in, and **04 is the only open
question, for Adam's ear.**

### 2. Turn Buffer into a composition workflow

Still the honest product test, and still last, because its value is a workflow
judgement better made against finished instruments and an interface that can
be driven than against neither.

The retained-audio engine, insert position, collision policy, gestures,
automation addresses and UI face all exist. The remaining question is not more
Buffer DSP; it is whether a musician can deliberately route a source into
Buffer, sequence a transformation, understand the active head, and keep the
result as part of a project without relying on debug controls or a hidden MIDI
mapping.

**Raise the shape of the device before building the workflow.** Adam's
position, 2026-08-30: making Buffer an ordinary insert device was partly the
wrong call. He designed it as though it had to work unchanged in another DAW,
and Buffer is not meant to be portable — it is meant to be part of how audio
playback works inside mooloop. His stated intent is to build it into the **end
of a device rack, with its own sequencing lane**. The lane design is not worked
out and he deprioritized it at the time. `BUFFER_ENGINE.md` still specifies the
insert model, so **do not treat that document as settled when this step
starts**: ask first, and do not begin the redesign unprompted.

One piece of Stage 1 is still unverified and is not a design question: its
acceptance test 8 — no allocations or locks in the callback — needs an
allocation-tracking harness rather than a reading of the code.

Done when: a project can generate or load sound, capture it continuously at a
chosen insert point, sequence an audible jump/reverse/repeat transformation,
show what the read head is doing, survive save and reload, and render the same
result offline. If that workflow is not materially better than bouncing a
sample and loading it again, record why before expanding the device.

## Waiting on Adam, not on work

Six things are finished or priced and are held up by a judgement rather than
by a branch. None should be worked around, and as of 2026-09-15 the last two
of them *are* the sequence rather than sitting beside it.

- **`ui-consistency-pass/`** — all six steps landed; it archives once Adam has
  played it.
- **`pane-layout/`** — done including the two pieces first left out; ready for
  `archive/` now.
- **`mono-synth-v2/`** — complete and played, kept out of the archive by one
  finding: Acid's Cutoff knob means 0.41x nominal where the other two models
  mean 0.65–0.68x. The compensation constant is load-bearing rather than a
  typo, so lining the corners up means re-deriving it, and whether it *should*
  track the others is a taste question.
- **`containers/06-layers-and-selectors.md`** — layers are priced and
  deferred to Adam after living with chain containers. Step 02's clipboard is
  more time living with them, not a reason to revisit.

The console's studio listening pass is the fifth, and it is the one that could
still change something that shipped.

The sixth is the EQ, and it is two things at once: **the listening passes
owed for steps 02 and 03**, and whether step 04 is taken at all. Step 03 is
the one to listen to first, and it has been measured so the listen is short:
half a decibel at the resting Q, up to two on a shelf whose Q knob somebody
moved, all of it within an octave or two of the corner.

Adam, if you are reading this list and want the sequence moving again, the
two answers that unblock the most are **whether the EQ's step 04 is worth
having**, and **what shape Buffer should be** -- `BUFFER_ENGINE.md` still
specifies the insert model you said on 2026-08-30 was partly the wrong call.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

- **The sampler's stretching-polyphony cap is not enforced anywhere.**
  `StretchPool::new` builds a reader for all sixteen voices at 100 KB each —
  1.6 MB a stretching channel against 401 KB for four — although the contract
  in #13 names four. Sizing the pool to `polyphony` is not free: it arrives on
  the audio thread, so voices above the old size would silently stop
  stretching when it was raised.
- ~~**`poly-v1-mono-mode/` is one step and unblocks a deletion.**~~ **Taken
  2026-09-15.** The v1 poly has a mono mode -- a held-note stack, a note
  priority, and a Retrig/Legato switch, on the three ids reserved for it -- so
  `DeviceKind::MonoSynth` is deletable and `MlM1` can take the plain name. The
  migration itself is the next branch and is deliberately not that one, and it
  wants a listen before it opens: the acceptance clause the step file ends on
  is that a v1 mono patch reproduces closely enough for the migration to be
  mechanical, and that has been argued rather than played.
- **`from_index` answers out-of-range input two different ways**, depending on
  which enum is asked: the `ALL`-table convention clamps to the nearest end,
  the hand-written `match` convention falls through to variant 0. The input is
  a Slint selector index, so the disagreement arises from this codebase's
  recurring fault — an option list and a Rust table differing about how many
  options exist — and the two conventions make one bug present as two. It is a
  decision about the edges before it is a refactor, which is why it is
  recorded rather than done.

The Sampler v2 GitHub issues (#20 and its children) are real work and are not
in this sequence. Weigh them against the loop story — chopped and stuttered
breaks, loops mangled per repeat — rather than against generic sampler
completeness, and do not treat them as the default pick when nothing else is
named.

## Deliberately not now

**All four queued plans, by Adam's call on 2026-09-12.** Each has a written
argument and none of them produces a musical decision, which is the rule at the
top of this document doing its job:

- **`device-registry/`** — a survey, not a work order. Adding a device kind
  touches fourteen files, nine of which hold a one-line arm stating one fact.
  Three findings are worth having before anyone tries: the typed
  `EffectParams` enum is the reason the DSP reads well and should not be
  flattened into function pointers; a table spanning crates cannot exist; and
  Slint has no dynamic component instantiation. What *is* cheap is that those
  arms are 444 lines of which 245 are the same eleven bindings fourteen times
  — a face host component would take an arm from twenty-seven lines to eight
  with no Rust change at all. Take that piece if a device step is already open
  in `main.slint`; do not open one for it.
- **`theming/`** — `Theme` already is the stylesheet, and the survey says
  which axes it is missing: colour, radius and motion are tokenized, type and
  stroke are not at all (313 literal `font-size`, 81 literal `border-width`).
  The reason to build it is **accessibility rather than the homage** — the
  working type size is 7–11px and is not adjustable — and that reason does not
  expire. It also costs more with every device face added, so it gets cheaper
  to defer and more expensive to do, which is a fact to re-read rather than
  forget.
- **`pattern-bank-floor/`** — every project reserves 1.00 GiB of pattern
  storage before it holds anything, and an ordinary edit rebuilds it at 20 ms
  a time. The measurements are committed either way, which is what makes this
  safe to park: nothing has to be re-derived to take it later.
- **`egui-view-layer/`** — `mooloop-ui` rebuilds in four minutes because
  `build.rs` expands `ui/main.slint` into a single 39 MB Rust module, and
  `edit-loop/04-decide.md` found no Slint arrangement that reaches it. That is
  a real cost and it is still not a musical decision. `scripts/antibox` took
  64% off `cargo test --workspace` and is the cheap half already taken; a view
  layer is a rewrite of how the application is drawn, and it needs step 01's
  spike and one post-change `scripts/loop-profile` run before it is even a
  decision.

**The modulation rack's move, and whether its modulator is a tracker.** The
shelf is a relocation and a layout; the tracker is a genuine design question
that `IDEAS.md` has held longer than this document has existed. Settle whether
they are one design or two before planning either. `MODULATION.md`
holds the contracts a move must not break.

**More effect kinds or a broad effect-polish pass.** Thirteen effects now, a
common host, and a fourteenth kind that is not an effect — the container. A
step may fix a concrete defect it exposes; the suite does not need more
breadth.

**More modulator kinds, or raising the slot count.** Five kinds, eight slots,
durable identity, and a measured price per slot are enough. Raising
`MAX_MODULATORS_PER_CHANNEL` is a one-line decision, which is the point of the
capacity work and not an invitation to make it.

**The text-label-to-icon pass.** Wanted, and polish over a shell that is still
moving. `ENHANCEMENTS.md` holds it in Adam's words.

**Sidechain key inputs, and plugin delay compensation for hosted plugins.**
Sends are no longer on this list — they landed 2026-09-09 and did it without
extending the compiled edge model, because a send carries its own compensation
and is aligned by construction. Sidechain still needs what sends did not: a
dependency edge that schedules a producer without summing it in. Read
`docs/plans/archive/typed-audio-edges/` before building it.

**Plugin hosting itself** (#10, #26–#30). Deferred rather than refused, and
`PRODUCT.md` says why: not before the instrument model is coherent. Step 1
above is the part of it worth doing early, and the only part.

**A curated factory bank.** Every device that ships presets ships them to
prove its architecture reaches its range from the controls, and that is the
only bar they are held to. Authoring content by taste, across every device, is
a deliberate later push. Do not fold it into a device step, and do not hold a
device step open waiting for it.

**Broad arrangement and recovery work.** Playlist clip manipulation, autosave,
crash recovery, and richer missing-sample relinking remain important. They do
not interrupt this sequence unless one becomes necessary to preserve its work.

**Metronome, MIDI configuration, and the graph editor.** None is required to
prove the active workflows. MIDI in particular is decoded and routed and
configurable nowhere, and the channel sidebar draws rows for it as of
2026-09-13: build the setting, let them stay inert.

## Working discipline

Keep one audible acceptance case per branch and run it through realtime,
persistence, and offline rendering where the change crosses those boundaries.
The plan files define the order inside a plan; update their status as steps
land rather than duplicating implementation notes here. When every step of a
plan is done, **move the directory to `archive/`** — an active directory should
always hold live work.

Keep branches small enough to listen to and revert independently. Preserve
stable parameter IDs, conservative project defaults, deterministic rendering,
and the realtime rules in `AUDIO_ARCHITECTURE.md`. `AGENTS.md` governs
worktrees, commits, and verification.

**Listening is a step, not a formality.** The last recorded passes are DS-01
and its kit on 2026-09-04, ML-P8 and its bank on 2026-09-05, the ML-M1 with
its patches, and the channel strip's three voicings on headphones on
2026-09-11. Both steps above change what can be *done* to a sound rather than
how it sounds, and a moving patch is the only proof they worked; `eq-v2`'s
step 04 is the one that changes a sound outright and needs a pass of its own.
