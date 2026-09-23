# Focus

Status: active working sequence, **rewritten 2026-09-21**. The previous
version was written 2026-09-12 and amended five times; its sequence —
interface iteration, then the EQ, then Buffer — was exhausted on 2026-09-18
when Buffer closed with no further notes, and it then ran three days past its
own rule while the work that actually happened (recording, transport
discontinuity, undo) all landed outside it. That is the failure this document
is most prone to, and the cure is in the rule below about itself.

`archive/ROADMAP.md` orders the whole product by dependency. This document is
narrower: it names the active sequence and the work that should not interrupt
it. **Rewrite it when that sequence is exhausted.** Two failure modes to keep
it out of: do not let it become a second roadmap, and **do not let it
accumulate the archaeology of closed steps** — that is what
`docs/plans/<name>/00-status.md` and `JOURNAL.md` are for, and it is most of
why the version before last reached twenty-four kilobytes.

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
  becomes easier to reach**, not by how much new surface it adds. A pane that
  shows what a menu already showed is not progress.
- **Order the work so something new is audible early.** A week of correctness
  work is worth having and does not feel like anything. 2026-09-21 was a whole
  day of undo and review findings — right to do, invisible to the ear — and it
  is why the sequence below opens with a device rather than with its tail.

## The sequence

### 1. The layer device — `containers/` steps 07-10

Set by Adam on 2026-09-21, from his own note (*"need a device like chain but
for layers"*) and his ruling on step 06 three days earlier (*"i do want a
layer device"*). The work order was written the same day, against the tree
rather than against step 06's estimate.

Four steps and they are in `docs/plans/containers/`: **07** is one container
predicate, latency as a tree rather than a flat sum, and `EffectKind::Layer`
landing silent; **08** is the engine splitting and summing, which is the
"parallel routing inside a chain" this document has parked since it was
written; **09** is the drawing; **10** is the gestures and a preset.

Three things to carry into it:

- **Step 06 was wrong about the representation, and that is why this is
  affordable.** It priced a layer at a second representation in
  `EffectSlotState` on the argument that parallel branches are not contiguous.
  They are: a branch is a *direct child's run*, and the flat list already
  describes a tree. What does change is that `chain_latency` stops being a
  sum — the time a signal spends inside a layer is its longest branch.
- **Step 09 is blocked on a mock-up from Adam and should stay blocked.** The
  container's enclosure was drawn three times without one, and each wrong
  version was restyled rather than questioned. Steps 07 and 08 do not wait for
  it.
- **08 is the step with a sound.** Do not close it on green tests: a drum loop
  into a clean branch and a Drive → Bitcrush branch, mix swept, is parallel
  drum compression and it is the oldest reason this device exists.

### 2. Close `audio-recording/`

One step short of the archive: 06's clean-up dialog for takes nothing refers
to (MOO-38). The quit prompt landed 2026-09-20; the dialog did not. Half a
day, and it takes a plan out of the active directory.

Not ahead of the layer device, because it adds nothing anybody can hear. Take
it when the layer device is between steps, or when 09 is waiting on the
mock-up.

## Waiting on Adam, not on work

None of these should be worked around. Each is a Linear issue carrying the
`Question` label, which is the complete list; these are the ones that bear on
this sequence.

- **The layer device's drawing** (`containers/09`, MOO-71) — a mock-up. See
  above.

Two listening passes were listed here until 2026-09-22: the console's studio
pass (MOO-166), and recording and undo, neither of which had been played
(MOO-165). Both were closed that day on Adam's instruction to treat
outstanding listening passes as done with nothing heard — see "Listening is a
step" below.

`midi-control/` and `mono-synth-v2/` were listed here until 2026-09-22; Adam
had answered both on 2026-09-18 -- the keyboard worked, and the Acid cutoff is
*"fine"* -- and both are archived.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

- **The sampler's stretching-polyphony cap is not enforced anywhere**
  (MOO-7, where which way to fix it is a question for Adam).
  `StretchPool::new` builds a reader for all sixteen voices at 100 KB each —
  1.6 MB a stretching channel against 401 KB for four — although the contract
  names four. Sizing the pool to `polyphony` is not free: it arrives on the
  audio thread, so voices above the old size would silently stop stretching
  when it was raised.
- **`from_index` answers out-of-range input two different ways.** The
  `ALL`-table convention clamps to the nearest end; the hand-written `match`
  convention falls through to variant 0. The input is a Slint selector index,
  so this is the recurring fault — an option list and a Rust table differing
  about how many options exist — presenting as two bugs. A decision about the
  edges before it is a refactor.
- **There is no driver-free `EngineHandle`**, so the control-plane boundary
  cannot be exercised as a whole in a test: one cannot be built without
  opening an audio driver, which is some of why the defects
  `archive/control-plane-seams/` found were as mechanical as they were. Worth
  a decision sometime; `01`'s `CommandSink` trait is the partial answer.
- **`midi recording doesnt loop properly`** (`SHORT_NOTES.md`) — after one
  playthrough notes stack at the last tick. The transport-position wrap this
  describes was fixed for *capture* on 2026-09-17; what Adam is reporting is
  the pattern not starting over, and it wants an overdub/replace toggle
  beside it. Not yet diagnosed against the tree.

## Deliberately not now

- **`device-registry/`** — a survey, not a work order. Parked 2026-09-12 with
  one exception that still stands: those fourteen arms are 444 lines of which
  245 are the same eleven bindings fourteen times, and a **face host
  component** would take an arm from twenty-seven lines to eight with no Rust
  change. Take that piece if a device step already has `main.slint` open —
  which `containers/09` will. Do not open a step for it.
- **`theming/02-relief.md`** (MOO-153) — the only design problem left in that
  directory; 01, 03 and most of 04 landed 2026-09-15.
- **`pattern-bank-floor/`** (MOO-152) — every project reserves 1.00 GiB
  before it holds anything. The measurements are committed, which is what
  makes parking safe.
- **A toolkit swap.** `egui-view-layer/` was archived unstarted on
  2026-09-22, on Adam's ruling (MOO-146): *"we can archive. i think QT would
  be better than egui if we do switch toolkits."* `main.slint` still costs
  8.7 minutes to a release binary, and `archive/edit-loop/04-decide.md` found
  no Slint arrangement that reaches it. That cost is real, and it is not a
  musical decision.
- **The modulation rack's move, and whether its modulator is a tracker.** The
  shelf is a relocation; the tracker is a genuine design question `IDEAS.md`
  has held longer than this document has existed. Settle whether they are one
  design or two before planning either (MOO-159). `MODULATION.md` holds the
  contracts a move must not break.
- **More effect kinds, or a broad effect-polish pass.** Thirteen effects, a
  common host, and a fourteenth kind that is not an effect. The layer device
  makes a fifteenth that is also not an effect, and that is the argument
  *against* treating the short-notes device ideas — mid/side, a gain/pan/width
  utility — as next: build the thing that makes combinations possible first.
- **More modulator kinds, or raising the slot count.** Five kinds, eight
  slots, durable identity, a measured price per slot. Raising
  `MAX_MODULATORS_PER_CHANNEL` is a one-line decision, which is the point of
  the capacity work and not an invitation to make it.
- **The text-label-to-icon pass.** Wanted, and polish over a shell that is
  still moving. `ENHANCEMENTS.md` holds it in Adam's words.
- **Sidechain key inputs, and plugin delay compensation.** Sends landed
  2026-09-09 without extending the compiled edge model, because a send carries
  its own compensation. Sidechain needs what sends did not: a dependency edge
  that schedules a producer without summing it in. Read
  `archive/typed-audio-edges/` before building it. **Note the adjacency:** a
  layer's branches are *not* this — they have no strip and no place in the bus
  graph, which is why `containers/08` is affordable and sidechain is not yet.
- **Plugin hosting** (`plans/plugin-hosting/`, thirteen steps). Not deferred —
  `SCOPE.md` put CLAP in for 0.2.0 — but outside this sequence, the way
  `coreaudio-driver/` is.
- **A curated factory bank.** Every device ships presets to prove its
  architecture reaches its range from the controls, and that is the only bar.
  Authoring content by taste across every device is a deliberate later push.
  Do not fold it into a device step or hold one open waiting for it.
- **Broad arrangement and recovery work.** Playlist clip manipulation,
  autosave, crash recovery, richer missing-sample relinking. Important; they
  do not interrupt unless one becomes necessary to preserve this sequence's
  work.
- **Metronome and the graph editor.** Neither is required to prove the active
  workflows. The metronome is the one `audio-recording`'s pre-roll notices is
  missing — a take's count-in is currently a silent bar.

## Two standing judgements that are not steps

**Buffer stays a device, and the rack-end placement is not ruled out.** Adam's
2026-08-30 position was that making Buffer an ordinary insert was partly the
wrong call — he designed it as though it had to work unchanged in another DAW,
where it is meant to be part of how playback works inside mooloop, at the
**end of a device rack with its own sequencing lane**. Put to him again with a
realtime-sampler framing on 2026-09-15 his answer was that it *"puts some of
my doubts about a device-based implementation to rest"*, and the plan closed
on 2026-09-18 with no further notes. The lane is not foreclosed: it would
drive the same published parameters.

**Sampler V2 is in for 0.2.0 and is not in this sequence** (Linear project
Sampler V2, umbrella MOO-42; `SCOPE.md` §4, 2026-09-18). Weigh it against the
loop story — chopped and stuttered breaks, loops mangled per repeat — rather
than against generic sampler completeness, and **do not treat it as the
default pick when nothing else is named.**

## Working discipline

Keep one audible acceptance case per branch and run it through realtime,
persistence, and offline rendering where the change crosses those boundaries.
The plan files define the order inside a plan; update their status as steps
land rather than duplicating implementation notes here. When every step of a
plan is done, **move the directory to `archive/`** — an active directory
should always hold live work.

Keep branches small enough to listen to and revert independently. Preserve
stable parameter IDs, conservative project defaults, deterministic rendering,
and the realtime rules in `AUDIO_ARCHITECTURE.md`. `AGENTS.md` governs
worktrees, commits, and verification.

**Listening is a step, not a formality.** The last recorded passes are DS-01
and its kit (2026-09-04), ML-P8 and its bank (2026-09-05), the ML-M1 with its
patches, the channel strip's three voicings on headphones (2026-09-11), and
`incremental-structure/` (2026-09-18, *"sounds good. i think you can close
it"*). The passes still outstanding on 2026-09-22 — recording and undo
(MOO-165), the console's studio pass (MOO-166), the EQ's shelf-Q listen and
response-plot look (MOO-167), and a dev build under JACK with a real song
(MOO-168) — were closed that day on Adam's instruction: *"If an issue is
blocked because of a listening pass, assume the pass has happened and there
were no issues heard."* They count as done with nothing heard. The list above
is still the last passes on record as actually heard, and the rule stands for
whatever lands next.
