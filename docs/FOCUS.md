# Focus

Status: active working sequence, September 2026.

`ROADMAP.md` orders the whole product by dependency. This document is narrower:
it names the active sequence and the work that should not interrupt it. Rewrite
it when that sequence is exhausted; do not let it become a second roadmap.

Read `PRODUCT.md` for the product argument and `CURRENT.md` for the implemented
surface. Source and tests settle any disagreement with either document.

## The line has moved

The previous focus did its job. The command layer and its editing surfaces,
audio preferences, the effect suite, the gain contract, retained-audio Buffer,
clip automation, and audible channel modulation all exist. The sample browser
also loads and previews the formats users are likely to bring to it.

Those are foundations to use, not projects to restart. Work should now turn
them into a more distinctive instrument.

## The rule

**Prefer changes that produce a musical decision over changes that merely add
capacity.**

The engine already has more breadth than the instrument has identity. A new
source type, graph abstraction, effect, or routing primitive is not progress by
itself. Each step below must end in something that can be played, heard, saved,
reopened, and rendered through the ordinary UI.

## The sequence

### 1. Make Mono a monosynth

**Substantially done.** Steps 02-07 are in, the six-patch factory bank is in,
and Adam played it on 2026-08-31 and found it very good. What is left is
taste, not structure: the accent, pre-drive and filter-compensation constants
still stand where measurement put them, and Acid's cutoff corner is
deliberately left three quarters of an octave below the other two models
pending Adam's call on whether it should track them at all. See
`docs/plans/mono-synth-v2/00-status.md`.

Execute `docs/plans/mono-synth-v2/` in order. Start with the shared filter
envelope and keytracking foundation, then give Mono the note behavior and
filter path that distinguish it from a one-voice Poly: a held-note stack,
priority and legato behavior, pre-filter drive, ladder and acid filter models,
and velocity accent.

Do not split shared descriptor or serialization work into a second instrument
implementation. Keep ML-M1's authored instrument modulation separate from the
channel rack's general-purpose and cross-device modulation; neither should
duplicate the other's routes.

Done when: Mono plays bass and lead lines with intentional note priority and
legato, its filter models are audibly different rather than renamed variants,
old projects still load conservatively, automation addresses remain stable,
and a small factory patch set proves the range from the normal UI.

### 2. Build ML-P8 as the deep polysynth

**In progress.** Steps 02 and 03 are in: the device plays, with the
three-oscillator network, sync, sub, noise, eight voices, two envelopes, and
the multimode filter. Steps 04-07 — its native LFO and internal routes,
allocation and the finishers, typed outlets, and the listening pass — are
next, in order.

Execute `docs/plans/poly-synth-v2/` as a new instrument beside the retained
original Poly. ML-P8 is exactly eight physical voices built around a
three-oscillator audio-rate network, derived sub, colored noise, sync,
cross-modulation, oscillator and filter feedback, separate envelopes, and
native per-voice modulation. Its useful internal LFO, envelopes, note values,
gate/trigger, and oscillator taps publish through typed device outlets.

Do not import ML-M1's acid semantics, held-note rules, or character filters.
Do not make Drift, Unison, Spread, or Chorus carry ML-P8's identity; they are
finishers around an oscillator and modulation architecture that must already
stand on its own. The channel modulation rack extends the instrument and
routes its published signals elsewhere; it is not a prerequisite for a
complete ML-P8 patch.

Done when: ML-P8 plays eight ordinary notes, the oscillator/feedback network
remains deterministic and bounded under automation, native per-voice routes
work independently across a chord, voice groups never strand or click, typed
outlets obey their rate and latency contracts, old Poly projects remain
unchanged, and the non-unison/non-chorus factory patches prove ML-P8's
character.

### 3. Turn Buffer into a composition workflow

**Not started.**

The retained-audio engine, insert position, collision policy, gestures,
automation addresses, and UI face already exist. The remaining product test is
not more Buffer DSP; it is whether a musician can deliberately route a source
into Buffer, sequence a transformation, understand the active head, and keep
the result as part of a project without relying on debug controls or a hidden
MIDI mapping.

Use the shared automation and modulation language. Add only the control
vocabulary and feedback that a concrete source-to-buffer workflow proves it
needs. Preserve Buffer as an ordinary insert whose capture point follows its
place in the rack.

Done when: a project can generate or load sound, capture it continuously at a
chosen insert point, sequence an audible jump/reverse/repeat transformation,
show what the read head is doing, survive save and reload, and render the same
result offline. If that workflow is not materially better than bouncing a
sample and loading it again, record why before expanding the device.

### Queued, not yet placed in this sequence

`docs/plans/drum-synth-v2/` is an approved design for **DS-01**, a second drum
instrument, written in full before any code. Nothing is built. It is not
numbered above because Adam has not said where it goes against the three
steps; treat it as ready to start rather than as next.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

## Deliberately not now

**More effect kinds or a broad effect-polish pass.** The rack already has
twelve effects and a common host, and the whole `EFFECTS_FEEDBACK` pass has
landed and been archived. A synth or Buffer step may fix a concrete defect it
exposes, but the suite does not need more breadth.

**More modulation taxonomy for its own sake.** Superseded in part, and only
in part: Adam pulled in the module grid on 2026-08-31, and
`docs/plans/modulator-modules/` and `docs/plans/modulator-capacity/` have both
completed, so the rack now has LFO, envelope, step, random, and math modules,
eight slots, and durable route identities. The deferral still stands for
everything they did not cover — device outlets, cross-channel sources,
multiple visible automation lanes, and an expert matrix — which should be
pulled in only by a demonstrated workflow.

**Parallel sends, sidechains, and plugin delay compensation.** Compensation is
required before parallel paths are trustworthy, but none of the active steps
needs a new graph shape. Build them together when sends or sidechain become the
actual product task.

**Broad arrangement and recovery work.** Playlist clip manipulation, explicit
loop ranges, autosave, crash recovery, and richer missing-sample relinking
remain important. They do not interrupt the source and Buffer sequence unless
one becomes necessary to preserve its work.

**Metronome, plugin hosting, MIDI configuration, and the graph editor.** None
is required to prove the active instrument workflows.

## Working discipline

Keep one audible acceptance case per branch and run it through realtime,
persistence, and offline rendering where the change crosses those boundaries.
The plan files define the order inside Mono and Poly; update their status as
steps land rather than duplicating implementation notes here.

Keep branches small enough to listen to and revert independently. Preserve
stable parameter IDs, conservative project defaults, deterministic rendering,
and the realtime rules in `AUDIO_ARCHITECTURE.md`. `AGENTS.md` governs
worktrees, commits, and verification.
