# Focus

Status: active working sequence, rewritten 2026-09-12, amended 2026-09-15 when
`musical-time/` was worked start to finish and archived -- its section is cut
back to what outlives it, which is this document's own rule about itself.
Amended 2026-09-14 when
`interface-iteration/` closed and its step left the sequence, and amended twice
on 2026-09-15: first when `eq-v2/` came down to one optional step and its
section was cut back to what is live, then again when Adam **closed the EQ and
settled Buffer's shape**, which emptied this document's "waiting on Adam" of
everything that was the sequence and left it with a work order for the first
time in days. The cuts are this document's own rule about itself being applied:
closed steps had left section 1 mostly archaeology. The previous
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

Set by Adam on 2026-09-12 as: finish the interface iteration, then the EQ, then
Buffer. **All three of those are now settled.** The interface iteration closed
2026-09-14. MIDI control, which this document had parked, was built and closed
2026-09-15. And on the same day Adam answered both of the questions the
sequence had been stalled on: **the EQ is closed** — step 04 declined, the plan
archived at 03 — and **Buffer stays a device**, which unblocks the step below
that was forbidden to start.

**So the sequence is no longer waiting on anybody, for the first time since it
was written.** It is Buffer, it has a work order rather than a design question,
and `musical-time/`, the one piece of it that was separable, landed the same
day — so nothing in the sequence is waiting on anything.

### 1. Turn Buffer into a composition workflow

Still the honest product test, and still last, because its value is a workflow
judgement better made against finished instruments and an interface that can
be driven than against neither.

The retained-audio engine, insert position, collision policy, gestures,
automation addresses and UI face all exist. The remaining question is not more
Buffer DSP; it is whether a musician can deliberately route a source into
Buffer, sequence a transformation, understand the active head, and keep the
result as part of a project without relying on debug controls or a hidden MIDI
mapping.

**The shape was raised on 2026-09-15, and Buffer stays a device.** Adam's
2026-08-30 position was that making Buffer an ordinary insert device was partly
the wrong call — he designed it as though it had to work unchanged in another
DAW, and Buffer is meant to be part of how audio playback works inside mooloop,
built into the **end of a device rack, with its own sequencing lane**. Put to
him again with a realtime-sampler framing, his answer was that it *"puts some
of my doubts about a device-based implementation to rest."* The rack-end
placement and the lane are still not ruled out; they are simply not the next
move, and nothing in the work order forecloses them, because a lane would drive
the same published parameters.

That work order is
`docs/plans/buffer-implementation/03-freeze-and-the-grid.md`: an always-rolling
history with FREEZE, a Rate parameter that Freeze forces into existence, Offset
retired in favour of a normalized Position, Length and freeze quantization on
the shared `ModTimeDivision` grid, and a 2U face whose buttons are macros over
published parameters.

One piece of Stage 1 is still unverified and is not a design question: its
acceptance test 8 — no allocations or locks in the callback. **The harness it
was waiting for exists as of 2026-09-15** (`control-plane-seams/04`): it is
`CountingAllocator::allocations()` in `mooloop-engine`, and it was three lines
on an allocator that crate had installed all along. What is left is coverage —
one block on the preview path is measured, the Buffer operations are not, and
the locks half is not measured at all. That is writing tests, not building an
instrument.

Done when: a project can generate or load sound, capture it continuously at a
chosen insert point, sequence an audible jump/reverse/repeat transformation,
show what the read head is doing, survive save and reload, and render the same
result offline. If that workflow is not materially better than bouncing a
sample and loading it again, record why before expanding the device.

### 2. `musical-time/` — **closed 2026-09-15, and step 5 is unblocked**

Three steps, one day, archived. `time::BEATS_PER_BAR` is the only place in the
workspace that says four beats to a bar, where the survey found nine spellings
across six crates and the note that prompted it guessed three. `BbtPosition`
and `BbtDuration` exist — two types, because a position counts from one and a
duration from zero, so a one-bar Length reads `1:0:0` — and `BbtText` in
`controls.slint` is what Buffer's face draws instead of writing the second copy
of the transport's formatter. Nothing on screen changed, and a test says so.

Two things in it outlive the plan and are in `AGENTS.md`: **a guard written
before its fix is shaped by the tree** — `dupe-audit bar-arithmetic` reported
ten sites where the survey had found eight — and **a check that is never clean
stops being read**, which is why it ships narrower than its plan specified.
Read `docs/plans/archive/musical-time/00-status.md` before touching bar
arithmetic or either transport readout.

## Waiting on Adam, not on work

Four things are finished or priced and are held up by a judgement rather than
by a branch. **None of them is the sequence any more**, which is new as of
2026-09-15: two entries left this list that day by being answered — the EQ,
closed at step 03 — and one, `pane-layout/`, turned out to be held up by
nothing but the move and archived. None should be worked around.

- **`midi-control/`** — all four steps landed 2026-09-15 and every layer is
  tested, but **none of it has been run against a keyboard**. That is the only
  thing between it and the archive, and it needs hardware rather than a
  judgement. `scripts/mooloop-mcp` and a controller are the check.
- **`ui-consistency-pass/`** — all six steps landed; it archives once Adam has
  played it.
- **`mono-synth-v2/`** — complete and played, kept out of the archive by one
  finding: Acid's Cutoff knob means 0.41x nominal where the other two models
  mean 0.65–0.68x. The compensation constant is load-bearing rather than a
  typo, so lining the corners up means re-deriving it, and whether it *should*
  track the others is a taste question.
- **`containers/06-layers-and-selectors.md`** — layers are priced and
  deferred to Adam after living with chain containers. Step 02's clipboard is
  more time living with them, not a reason to revisit.

The console's studio listening pass is the fourth, and it is the one that could
still change something that shipped.

**The EQ's two listening passes are still owed** and are no longer on this
list, because closing the plan turned them from a decision into a check against
shipped code. They moved to `LOOSE_ENDS.md`. Neither is a defect and step 03's
is measured, so the listen is short: shelves with a Q away from 0.707, an
octave either side of the corner.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

- **`docs/plans/archive/control-plane-seams/` — all five steps landed 2026-09-15**,
  and the directory archived the same day. Five confirmed control-plane defects
  from an outside architectural review that read the system end to end and
  passed the layering. `00-status.md` has what each step found; three things
  belong here rather than there.

  **The allocation harness this document has been asking for since
  `buffer-implementation/` Stage 1 came out of step 04**, and it was three
  lines on an allocator `mooloop-engine` already had. See the amended entry
  above and `LOOSE_ENDS.md`.

  **Two of the five could not be given the test their step specified.** Both
  wanted to drive an `EngineHandle`, and one cannot be built without opening
  an audio driver — nothing in the workspace had ever constructed one in a
  test, which is some of why defects this mechanical survived. Step 01 grew a
  `CommandSink` trait so a refusal is reachable; step 03 could not, and put
  the guarantee in the signature instead. **That gap is worth a decision
  sometime**: a driver-free `EngineHandle` would make the control-plane
  boundary testable as a whole, and it is the thing standing between these
  fixes and a test that exercises the actual seam.

  **Step 03 turned up a live defect nobody was looking for.** A project old
  enough to carry a legacy `Builtin` sample reference opened with that channel
  silent — the install published the default kick and a republication four
  lines later cleared it — while its name, waveform and duration all described
  a kick.

  The review's sixth finding, that the UI/session seam is becoming a god
  layer, is deliberately **not** a step; it is a direction in that plan's
  `README.md`, and step 01 is a down payment on it either way.
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

**Three of the four queued plans, by Adam's call on 2026-09-12.** Each has a
written argument and none of them produces a musical decision, which is the
rule at the top of this document doing its job. The fourth, `theming/`, was
unparked by Adam on 2026-09-15 and is struck through below:

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
- ~~**`theming/`**~~ **Unparked and largely taken, 2026-09-15**, on Adam's
  own instruction rather than by anyone deciding the parking had expired. Its
  steps 01, 03 and most of 04 landed: type, stroke and metric tokens; a theme
  as a sixteen-colour ramp with light and dark variants; thirteen built-in
  schemes; pywal and wallust; a Dark/Light/Auto control that follows the
  desktop; the swatch palette following the scheme; and the type scale, which
  is the accessibility reason this was written down. `02-relief.md` is still
  open and is still the only design problem in the directory —
  `00-status.md` has what landed and what did not.
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

**Metronome and the graph editor.** Neither is required to prove the active
workflows. MIDI configuration was on this list until 2026-09-15, when
`docs/plans/midi-control/` built it: a channel picks its input, a controller
maps to any parameter, and the transport takes gestures. What is left of it is
not construction — **none of it has been run against a keyboard**, and that is
the next MIDI thing to do.

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
how it sounds, and a moving patch is the only proof they worked. Step 04 of
`eq-v2/` would have been the one that changed a sound outright, and Adam
declined it on 2026-09-15 — so nothing in the current sequence needs a pass of
that kind, and the two EQ passes still owed are checks on shipped code rather
than on anything being decided (`LOOSE_ENDS.md`).
