# Focus

Status: active working sequence.

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

Execute `docs/plans/mono-synth-v2/` in order. Start with the shared filter
envelope and keytracking foundation, then give Mono the note behavior and
filter path that distinguish it from a one-voice Poly: a held-note stack,
priority and legato behavior, pre-filter drive, ladder and acid filter models,
and velocity accent.

Do not split shared descriptor or serialization work into a second Poly
implementation. Do not add a device-local modulation system; the channel rack
already owns modulation.

Done when: Mono plays bass and lead lines with intentional note priority and
legato, its filter models are audibly different rather than renamed variants,
old projects still load conservatively, automation addresses remain stable,
and a small factory patch set proves the range from the normal UI.

### 2. Make Poly a different instrument

Execute `docs/plans/poly-synth-v2/` after the shared Mono foundation lands.
Poly's identity is a deterministic voice pool: per-voice drift, a clean
multimode filter, group-aware unison, stereo spread, and an internal
chorus/ensemble stage after the voice sum.

Do not import Mono's acid semantics, held-note rules, or character filters.
Do not reduce the three-oscillator architecture to imitate a named hardware
synth. Mono and Poly should invite different musical gestures even when they
start from the same waveform.

Done when: repeated offline renders are deterministic, `drift = 0` preserves
the old sound, voice and unison changes do not strand or click active notes,
chords have movement and width without requiring insert effects, old projects
load safely, and factory patches demonstrate Poly's own character.

### 3. Turn Buffer into a composition workflow

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

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

## Deliberately not now

**More effect kinds or a broad effect-polish pass.** The rack already has ten
effects and a common host. A synth or Buffer step may fix a concrete defect it
exposes, but the suite does not need more breadth.

**More modulation taxonomy for its own sake.** LFO and envelope sources,
direct assignment, visible movement, persistence, and parameter-wide
destinations are enough for the active sequence. Device outlets, additional
generators, multiple visible automation lanes, and an expert matrix should be
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
