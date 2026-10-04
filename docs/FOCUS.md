# Focus

What to work on now, and what must not interrupt it. This file is one page on
purpose. History goes in `JOURNAL.md`, the state of each issue is in Linear,
`SCOPE.md` says what 0.2.0 is, and `plans/README.md` lists the live plans.
**Rewrite this file when its sequence runs out. Don't append to it.**

## Now

- **No sequence is queued, and the next one is Adam's call.** The most recent
  pushes (overnight 2026-09-25, the 0.1.6 label, the backlog runs of
  2026-09-29 and 2026-10-01) are all done. `JOURNAL.md` has the record.
- **0.1.6 was cut on 2026-10-02** (`v0.1.6` at `d7d86da0`). MOO-456 (Wrap
  in Layer) still carries the label, In Review: its fix shipped, and it waits
  on a look in the running app. Moving it on is Adam's call.
- **0.1.7** is whatever carries the `0.1.7` label. So far that's steps 03 to
  05 of `plans/icon-pass/`. Before tagging, dispatch `release.yml` on `main`
  (`OPERATIONS.md`, *Releases And Tags*).
- **Waiting on Adam:** the listening queue in [LISTENING.md](LISTENING.md),
  and MOO-456's look. The `Question` label in Linear is the complete list of
  open questions.

## The rule

**Prefer changes that produce a musical decision over changes that merely add
capacity.**

The engine has more breadth than the instrument has identity. A new source
type, graph abstraction, effect, or routing primitive is not progress by
itself. Each step must end in something that can be played, heard, saved,
reopened, and rendered through the ordinary UI.

- **An interface change is judged by whether something that already exists
  becomes easier to reach**, not by how much new surface it adds.
- **Order the work so something new is audible early.** A sequence made of
  nothing but correctness work leaves nothing to hear: 2026-09-23 fixed
  clicks, NaNs, undo, durability and export parity, and added almost no new
  sound.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step, when it threatens realtime safety or project
compatibility, or when it's a small regression in the surface being touched.
File larger adjacent work in Linear rather than folding it into the current
branch.

## Deliberately not now

- **`device-registry/`** is a survey, not a work order. The one exception: a
  **face host component** would take each of `main.slint`'s face arms from
  twenty-seven lines to eight, with no Rust change. Take it if a step already
  has `main.slint` open. Don't open a step for it.
- **`pattern-bank-floor/`** (MOO-152): every project reserves 1.00 GiB before
  it holds anything. The measurements are committed, which is what makes
  parking it safe.
- **A toolkit swap.** If the view layer ever leaves Slint, Qt is the
  candidate Adam named (MOO-146).
- **The modulation rack's move, and the tracker idea** (MOO-159: an interest,
  not a decision, and recorded in `IDEAS.md`). Settle whether they're one
  design or two before planning either. Modulators inside containers
  (MOO-160) come after 0.2.0.
- **More effect kinds, or more modulator kinds.** The short-notes device
  ideas (mid/side, a gain/pan/width utility) get cheaper once the layer
  device has its gestures. A mid/side device is close to a two-branch layer
  with an encode in front and a decode behind. Raising
  `MAX_MODULATORS_PER_CHANNEL` is a one-line decision, and it's not an
  invitation.
- **Sidechain key inputs, and MIDI out.** Both are in 0.2.0, and neither is
  next. Sidechain needs a dependency edge that schedules a producer without
  summing it in, so read `archive/typed-audio-edges/` first. A layer's
  branches are not this: they have no strip and no place in the bus graph.
- **A curated factory bank.** Every device ships presets that prove its
  architecture reaches its range. That's the only bar until there's a
  deliberate content push.
- **Playlist clip manipulation, richer missing-sample relinking, a metronome
  (a take's count-in is still a silent bar), and the graph editor.**

Everything else in 0.2.0 is in `SCOPE.md` and in Linear (Rendering, Theming,
Polish backlog, Song automation, Sampler V2). None of it is deferred. It gets
worked when Adam asks for it.

## Standing judgements

**Buffer stays a device, and the rack-end placement is not ruled out.** Adam's
position (2026-08-30) is that Buffer belongs at the **end of a device rack
with its own sequencing lane**, as part of how playback works inside mooloop.
A realtime-sampler framing put *"some of my doubts about a device-based
implementation to rest"* (2026-09-15). A lane would drive the same published
parameters, so the lane is not foreclosed.

**Sampler V2** (umbrella MOO-42): weigh what's left, the mapping workspace
(MOO-40) and SFZ import (MOO-41), against the loop story rather than against
generic sampler completeness. The loop story means chopped and stuttered
breaks, and loops mangled on each repeat.

## Working discipline

Keep one audible acceptance case per branch, and run it through realtime,
persistence, and offline rendering wherever the change crosses those
boundaries. Keep branches small enough to listen to and revert on their own.
Preserve stable parameter IDs, conservative project defaults, deterministic
rendering, and the realtime rules in `AUDIO_ARCHITECTURE.md`. The plan files
set the order inside a plan. Record what a step found in its `00-status.md`,
not here. `AGENTS.md` governs worktrees, commits, and verification.

### Listening is a step

An agent can render and measure. Only Adam can say whether something sounds
right. A change that alters a sound or a look isn't finished until it has
been heard, so it adds an item to [LISTENING.md](LISTENING.md): what to play,
the command that renders it, and what to listen for. Nothing since 2026-09-18
has been heard. Adam's 2026-09-22 instruction closed the outstanding passes
as done with nothing heard, and the wording everywhere says *treated as
done*, never *heard*.
