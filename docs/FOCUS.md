# Focus

Status: working sequence, August 2026.

`ROADMAP.md` orders the whole product by dependency. This document is
narrower and shorter-lived: it names what we work on *next*, in order, and
what we are deliberately not working on yet. When the sequence below is
finished, this document is rewritten or deleted — it is not a second
roadmap.

Read `PRODUCT.md` for why any of this exists and `CURRENT.md` for what
actually ships today.

## The rule

**Nothing is done until you can hear it, or use it.**

The recurring failure in this repo is not over-architecture. The
architecture bets have paid: `delayline` served the delay and waits for the
buffer device, `dynamics` gave three effects for the price of one,
`EffectChain` covered channels and buses, `KnobFace` kept two knob sizes
from drifting. The failure is vertical slices stopped one step short of the
payoff — modulation groundwork with nothing driving it, gain-reduction and
correlation meters with no audio behind them, solo as a button style,
per-channel meters drawn but unfed, a ring primitive with no device on top.

The filter is the counter-example and the model: core types, DSP node,
engine path, UI face, in one pass, audible at the end.

## The sequence

### 1. Command layer and undo

Everything in `ROADMAP.md` Phase 3's remaining work that is really one
thing: undo/redo, cut/copy/paste, keyboard shortcuts, right-click context
menus, and the seven `enabled: false` rows sitting in `main.slint` waiting
for it.

Do this first because every editing feature built before it has to be
retrofitted afterward. The existing `ChannelClipboard` in
`mooloop-ui/src/lib.rs` is the shape of the problem: it works, and it
generalizes to nothing.

Shortcuts and context menus are command *surfaces*. They dispatch the same
commands the menu bar does. A shortcut that reaches into a widget's internal
state is a bug, not a shortcut. **This now has a real implementation**: see
`ACTIONS.md` for the action registry every keyboard shortcut resolves
through, and its Preferences > Shortcuts page for (re)assignment. Pattern
add/clone/remove route through the same undo-recorded whole-project-edit
pipeline channel cut/copy/paste/clone/delete already used, so two of the
seven `enabled: false` rows (Clone Pattern, Delete Pattern) are live; Clear
Pattern and Select All remain disabled, and right-click context menus for
patterns are still open.

Done when: undo works across rack steps, notes, playlist placements,
channels, and patterns; every menu row is either enabled or deleted; the
channel clipboard is one case of a general mechanism.

### 2. Preferences, audio device selection, metronome

Days, not a phase. Take it as the palate cleanser between the two large
pieces.

The metronome matters more than it sounds like it does: `CURRENT.md`
records that the toolbar deliberately has no click-track toggle because
nothing in the DSP graph produces one. For an instrument aimed at rhythm,
that is a bigger gap than the prefs dialog.

/* NOTE (from the human, with ears): this app has no ability to record. it will, probably, but it doesn't. you dont need a click yet. if you really really did you could just do one on a pattern. yall have been hilarious about this metronome. */

### 3. Modulation and automation, to audible

`MODULATION_PLAN.md` is approved and says to implement it rather than
re-litigate it. Take it at its word.

Order within the step: one modulator driving one destination end to end —
LFO to filter cutoff, knob to ear — before the general parameter-lane
editor. `ParamAddr` lands here, because this is where it finally gets its
second and third target.

Done when: a modulator visibly and audibly moves a parameter, the knob and
the modulator do not fight over the value, and the result survives save and
reload.

### 4. The buffer device

The retained-audio device in `BUFFER_ENGINE.md`, built on the existing
`delayline` primitive.

**It is fourth on purpose.** Built now, with only a trigger input to drive
it, it is Supatrigga or dblue Glitch — plugins that are over twenty years
old and did that job well. The differentiator is not the buffer. It is the
buffer *addressed by the automation language*: read heads moved with the
same precision as notes, from a device placed anywhere in the insert chain.
Those plugins are a fixed slot at the end of the chain with random-or-MIDI
triggering and no parameter language underneath.

So the buffer device is not delayed by step 3. It is *made worth building*
by step 3, and it becomes the flagship demonstration of it rather than a
separate feature.

Before it ships, `CURRENT.md`'s architecture risks 3 and 4 come due: budget
channel buffer memory and specify read/write collision behavior, and add
deferred reclamation so a large buffer is never freed on the audio thread.

## Deliberately not now

**Plugin delay compensation.** It gates limiter lookahead, parallel sends,
and sidechain — none of which are in the sequence above. `compile_bus_graph`
already exists, which is what makes PDC cheap whenever we want it. Do it as
the opening move of the sends/sidechain pass. Doing it now is architecture
ahead of a consumer, which is the habit this document exists to break.

**Generative: 1/f, pattern mutation.** Yes, and the cost is almost entirely
a question of ordering. A 1/f source is a sibling of `SampleAndHold` in the
modulator rack — nearly free after step 3. Pattern mutation needs a
selection model and undo to be usable at all — nearly free after step 1, and
miserable before it. That both fall out cheaply from the sequence is
evidence the sequence is right; neither justifies reordering it.

**Everything else in `ROADMAP.md`.** Still true, still ordered, still not
now.

## Working discipline

One step at a time, to its "done when" line, before taking the next
interesting detour. The steps are large; the branches inside them should not
be. `AGENTS.md` governs how work is split across branches and worktrees.

Record deferrals with their reasons, the way `MODULATION_PLAN.md` and
`EFFECTS_PLAN.md` already do, so a later pass doesn't mistake an absence for
an oversight.

Keep writing the journal. It is the instrument that shows when we have
drifted off this page.
