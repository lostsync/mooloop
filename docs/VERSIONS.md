# Version Targets

Status: working release targets, August 2026.

These are outcome-based milestones, not dates or promises. `CURRENT.md` says
what has actually shipped in the code; `ROADMAP.md` explains the dependency
order; `FOCUS.md` is the short-lived next-work sequence. A version target is
ready only when its stated outcome can be used or heard, not when its
supporting types merely exist.

## 0.1.1 — A usable post-0.1.0 instrument

Status: in progress.

This is the first point release after the `v0.1.0` baseline. It collects the
work that turns the prototype into a more dependable instrument before the
next architectural step.

Milestones:

- Command surfaces share the undo-recorded action path: menus, shortcuts, and
  context menus do not bypass it.
- Pattern editing is complete enough for ordinary use: multi-selection,
  clear, clone, delete, and their keyboard/context-menu paths work, while
  unfinished actions remain visibly disabled rather than pretending to exist.
- Preferences expose the current audio, appearance, and shortcut choices, and
  persist them correctly.
- The sampler, piano roll, device controls, and tooltips receive the
  interaction and responsive-layout polish needed for common work without a
  separate DAW.
- The current effect and performance work is validated as a release: focused
  checks throughout development, then the integration suite in `CURRENT.md`
  before tagging.

This release does not claim general automation, retained audio, sends,
sidechains, recording, or a metronome.

## 0.2.0 — Automation that is audible

Status: next feature milestone.

Milestones:

- One modulator controls one real destination end-to-end—initially LFO to
  filter cutoff—with a result that is visible, audible, saveable, and stable
  on reload.
- The parameter address and modulation model expands from that vertical slice
  without creating a separate automation language.
- The parameter lane becomes a useful editor for selected targets rather than
  a velocity-only placeholder.

This milestone follows the approved design in `MODULATION_PLAN.md`; it is not
a commitment to every possible modulator or tracker command at once.

## 0.3.0 — Decide the retained-audio thesis

Status: planned experiment.

Milestones:

- A bounded, realtime-safe buffer device can be inserted at a meaningful
  point in an ordered device chain.
- Its ordinary state is a trustworthy live bridge; automation can jump the
  read head into retained history, loop a window, change rate, reverse, and
  return live.
- Its memory, read/write collision behavior, reset/persistence semantics, and
  deferred reclamation are specified and tested.
- Hands-on use establishes that the workflow is materially more immediate or
  distinct than bounce-to-sample; otherwise the product hypothesis is revised
  or rejected.

This is intentionally a decision milestone, not a promise of a full looper,
destructive editor, or synth suite. See `BUFFER_ENGINE.md`.

## Beyond 0.3

Routing, source-to-buffer workflows, groups, sends, selected resampling,
project recovery, and broader song editing remain dependency-ordered work in
`ROADMAP.md`. They should receive their own version targets only after the
automation and buffer outcomes above are proved. `1.0` has no target yet:
calling the application stable before those outcomes would make the version
number less honest, not more useful.
