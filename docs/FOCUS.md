# Focus

Status: active working sequence, rewritten 2026-09-02.

`ROADMAP.md` orders the whole product by dependency. This document is narrower:
it names the active sequence and the work that should not interrupt it. Rewrite
it when that sequence is exhausted; do not let it become a second roadmap.

Read `PRODUCT.md` for the product argument and `CURRENT.md` for the implemented
surface. Source and tests settle any disagreement with either document.

## The line has moved

The previous sequence was Mono, then ML-P8, then Buffer. **Its first step is
done and its second is half done**, and three things landed beside it that the
sequence never named:

- **ML-M1 exists and has been played.** Adam played the factory bank on
  2026-08-31 and the verdict was that it sounds very good. One DSP finding came
  out of it and was fixed; one was diagnosed and deliberately left (see
  `docs/plans/mono-synth-v2/00-status.md`).
- **Channel modulation grew up.** Eight slots a channel, five module kinds,
  durable identity across a reorder, and a capacity number that is now a
  one-line edit with a measured linear price rather than a hunt through layout
  code. Both plans are complete and archived.
- **Positional addressing was a live bug and is now fixed.** Routes and
  automation lanes named their destination by slot and their channel by index,
  so any structural edit silently re-aimed them. `mooloop_core::structure`
  states each edit once as a permutation and runs it over everything that
  stores a position.

Those are foundations to use, not projects to restart.

## The rule

**Prefer changes that produce a musical decision over changes that merely add
capacity.**

The engine already has more breadth than the instrument has identity. A new
source type, graph abstraction, effect, or routing primitive is not progress by
itself. Each step below must end in something that can be played, heard, saved,
reopened, and rendered through the ordinary UI.

## The sequence

### 1. Finish ML-P8

Execute `docs/plans/poly-synth-v2/` steps 06 and 07. Steps 02 through 05 are
in: the device plays, with the three-oscillator network, all six directed
XMOD routes, sync, the derived sub, coloured noise, both envelopes, four filter
modes, the feedback loop with drive inside it, and its own modulation — an
audio-rate LFO, six per-voice sources, authored routes onto thirty-one
continuous destinations, and an ML-P8 MOD page to build them on. A complete
moving patch needs nothing from the channel shelf. Step 05 finished the pool
around it: group allocation, Unison that spends the eight slots rather than
growing them, symmetric Detune and Spread, entropy-free per-slot Drift, and a
finishing chorus that is off by default.

What remains is the one that matters beyond this device — **step 06's
published outlets** — and then the listening pass. 06's second half is
genuinely blocked: the audio outlets need the typed auxiliary audio edges
`AUDIO_ARCHITECTURE.md` describes, which do not exist, so 06 lands in the two
slices the step already names.

Its first slice is most of the way in. `mooloop_core::outlet` is the
vocabulary, ML-P8 declares fourteen outlets and publishes its seven control
values, and a route can name one: the mechanism DS-01's step 07 also waits on
is built and tested. What is left of the slice is the source picker offering
them, so such a route can be made by hand rather than only by a project file.

This step is first because it is most of the way built, its cost is known, and
DS-01's own step 07 publishes outlets the same way. Doing it here means
designing that contract once.

Do not import ML-M1's acid semantics, held-note rules, or character filters. Do
not let Drift, Unison, Spread, or Chorus carry ML-P8's identity; they are
finishers around a network that already stands on its own, which is why every
one of them is off in the default patch.

Done when: eight ordinary notes play, the network stays deterministic and
bounded under automation, voice groups never strand or click, typed outlets
obey their rate and latency contracts, old Poly projects are unchanged, and the
non-unison/non-chorus factory patches prove the character.

### 2. Build DS-01

Execute `docs/plans/drum-synth-v2/` in order. Nothing is built; the plan is
complete, including three rendered face concepts at the real face size against
the real widgets, of which two were built and rejected and are checked in as
the argument for the third.

**The drum synth is the only generator that cannot be modulated at all**, in a
program whose default new song is a four-channel drum kit. The reason is
structural — `DrumSynthParams` is a mode-union, so a flat descriptor table over
it yields ids whose meaning changes with the Mode switch — which is why this is
a new generator beside the v1 device rather than a table added to it. The v1
device stays and old projects load unchanged.

Descriptor addressing is step 02, not step 09. It is the reason the instrument
exists and does not get to be the last thing anyone gets to.

One constraint that binds steps 02 through 07, not just step 08: **DS-01's
controls do not fit on one face unless the scopes are the envelope editor.**
Envelope times are handles dragged on the curve, not knobs. A scope without
handles is not a smaller version of that face; it is a different one that needs
a page. Build the parameter model knowing the face has already been decided.

Done when: one universal percussion voice covers kick, snare, hat, tom, rim,
clap and roll from range and factory patches rather than from mode branches;
every parameter is descriptor-addressed and modulatable; the note-on latch rule
from step 02 is published and tested; and the kit in step 09 proves the range
from the normal UI.

### 3. Turn Buffer into a composition workflow

Unchanged from the previous focus, and still the honest product test.

The retained-audio engine, insert position, collision policy, gestures,
automation addresses, and UI face already exist. The remaining question is not
more Buffer DSP; it is whether a musician can deliberately route a source into
Buffer, sequence a transformation, understand the active head, and keep the
result as part of a project without relying on debug controls or a hidden MIDI
mapping.

It sits third because its value is a workflow judgement, and that judgement is
better made against three finished instruments than against one and a half.

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

Two carried items are close enough to the sequence to be worth naming, because
both are small and both will be in the way:

- **The stretching-polyphony cap is not enforced anywhere.** `StretchPool::new`
  builds a reader for every one of the sampler's sixteen voices and nothing
  limits how many stretch at once, although the contract in #13 names four.
- **`poly-v1-mono-mode` is one step and unblocks a deletion.** It is the only
  thing keeping `DeviceKind::MonoSynth` alive, and that deletion is what lets
  `MlM1` take the plain name. The held-note stack it needs already exists.

## Deliberately not now

**More effect kinds or a broad effect-polish pass.** The 2026 effects-feedback
pass is complete and archived. The rack has twelve effects and a common host. A
synth step may fix a concrete defect it exposes; the suite does not need more
breadth.

**More modulator kinds, or raising the slot count.** Five kinds, eight slots,
durable identity, and a measured price per slot are enough for this sequence.
Raising `MAX_MODULATORS_PER_CHANNEL` is now a one-line decision — which is the
point of the capacity work, not an invitation to make it. Device outlets arrive
with ML-P8 step 07 because that device needs them, not as a taxonomy exercise.

**Parallel sends, sidechains, and plugin delay compensation.** Compensation is
required before parallel paths are trustworthy, but no active step needs a new
graph shape. Build them together when sends or sidechain become the actual
product task.

**Broad arrangement and recovery work.** Playlist clip manipulation, explicit
loop ranges, autosave, crash recovery, and richer missing-sample relinking
remain important. They do not interrupt this sequence unless one becomes
necessary to preserve its work.

**The preset system revisit.** `docs/plans/preset-system/` decided on
2026-09-04 that a preset's unit is a **device, with relative addressing**,
and its steps 01 to 04 ran the same day: the effect-level preset exists end
to end — format, `presets/effects/<kind>/`, the session path, and the rack
row's save and load controls — and every effect kind ships a factory bank.
`PresetSummary` now names three preset classes, which is the structural half
of the taxonomy done. `00-status.md` records what building it taught; the
short form is that the specific form was a stepping stone, not a detour.

What still waits for DS-01 is the rest: the browser, the taxonomy *surface*,
and a factory-content mechanism that can update what it shipped. Those want
two instrument banks to design against, and DS-01's step 09 ships the second.
Do not let the rack row's preset menu grow into a browser to avoid the wait.


**Metronome, plugin hosting, MIDI configuration, and the graph editor.** None is
required to prove the active instrument workflows.

## Working discipline

Keep one audible acceptance case per branch and run it through realtime,
persistence, and offline rendering where the change crosses those boundaries.
The plan files define the order inside ML-P8 and DS-01; update their status as
steps land rather than duplicating implementation notes here.

Keep branches small enough to listen to and revert independently. Preserve
stable parameter IDs, conservative project defaults, deterministic rendering,
and the realtime rules in `AUDIO_ARCHITECTURE.md`. `AGENTS.md` governs
worktrees, commits, and verification.

**Listening is a step, not a formality.** The last recorded listening pass was
the ML-M1 bank on 2026-08-31. A stretch engine, a slice mode, a commit path,
and most of ML-P8 have landed since, and the gain contract deliberately moved
every default 12 dB quieter. A step that changes what something sounds like is
not done when its tests pass.
