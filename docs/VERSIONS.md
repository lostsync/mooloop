# Version Targets

Status: working release targets, September 2026.

These are outcome-based milestones, not dates or promises. `CURRENT.md` says
what has actually shipped in the code; `ROADMAP.md` explains the dependency
order; `FOCUS.md` is the short-lived next-work sequence. A version target is
ready only when its stated outcome can be used or heard, not when its
supporting types merely exist.

## 0.1.1 — A usable post-0.1.0 instrument

Status: shipped 2026-08-27.

This is the first point release after the `v0.1.0` baseline. It collects the
work that turns the prototype into a more dependable instrument before the
next architectural step.

Milestones:

- **Done.** Command surfaces share the undo-recorded action path: menus,
  shortcuts, and context menus do not bypass it. 39 actions in one registry,
  one keyboard dispatcher, one Preferences page.
- **Done.** Pattern editing is complete enough for ordinary use:
  multi-selection, clear, clone, delete, and their keyboard/context-menu paths
  work, while unfinished actions remain visibly disabled rather than
  pretending to exist.
- **Done.** Preferences expose the current audio, appearance, and shortcut
  choices, and persist them correctly.
- **Partly.** The sampler, piano roll, and device controls have had the
  interaction pass; slicing, stretch, and the commit path landed with it. The
  status bar the tooltip audit in `ENHANCEMENTS.md` asked for exists and is
  fed by a `hover-hint` property, but only some surfaces are plumbed into it —
  the sampler face is not — so tooltips still explain in places where they
  should only name. Responsive layout still has edge cases.
- The current effect and performance work is validated as a release: focused
  checks throughout development, then the integration suite in `CURRENT.md`
  before tagging.

The effect suite is finished for this release: twelve kinds, the whole
`EFFECTS_FEEDBACK` pass landed, and the gain contract measured and pinned.

This release does not claim general automation, retained audio, sends,
sidechains, recording, or a metronome.

## 0.1.2 — Three instruments, and a grid to modulate them with

Status: shipped 2026-09-05.

This release does not change the architecture 0.1.1 settled; it fills it.
Three addressable generators, a modulator grid on every channel, device
presets, and the sampler's stretch and slice work.

Milestones:

- **Done.** Three new generators ship, addressable through `GeneratorParams`
  from their first commit and each with a paged face sized in rack units:
  the ML-M1 filter/performance mono, the eight-voice ML-P8 and its
  oscillator network, and the DS-01 drum synth with its seventeen-patch
  factory bank.
- **Done.** The channel modulation shelf is a module grid: five module
  kinds, eight modules and sixteen routes per channel, and durable route
  identities, so reordering the grid or the device chain moves a module
  without changing what a route means.
- **Done.** A device preset is saved and loaded from that device's own rail,
  every effect kind ships a factory bank, and the header names the preset
  the device came from.
- **Done.** The sampler stretches and slices, and a stretch can be committed
  to the buffer and reverted. The four gaps that push left open are recorded
  in `CURRENT.md` rather than hidden.
- **Partly.** The tooltip debt 0.1.1 recorded is still open on the sampler
  face: `hover-hint` reaches the effect faces, not that one, so the ON toggle
  that Slice mode bypasses cannot yet say so.

This release does not claim MIDI input that reaches the engine, recording, a
metronome, sends, or sidechains.

## 0.2.0 — Automation that is audible

Status: next feature milestone.

Milestones:

- **Done.** One modulator controls one real destination end-to-end, visible,
  audible, saveable, and stable on reload.
- **Done, further than the milestone asked.** The model expanded from that
  slice without a separate automation language: five module kinds, eight
  slots, durable route identities, and an assign gesture on ordinary
  controls. `MODULATOR_SYSTEM_SPEC.md` records what is left — device outlets,
  cross-channel sources, macros.
- **Partly.** The parameter lane addresses every effect parameter on the
  channel and on every bus rather than velocity alone, but only one lane is
  visible at a time.

This milestone follows the approved design in `MODULATION_PLAN.md`; it is not
a commitment to every possible modulator or tracker command at once. What now
gates the release is less the modulation model than what still has to be
audible through it.

## 0.3.0 — Decide the retained-audio thesis

Status: device built, decision outstanding. The first two milestones below are
met; the third is what remains.

Milestones:

- A bounded, realtime-safe buffer device can be inserted at a meaningful
  point in an ordered device chain.
- Its ordinary state is a trustworthy live bridge; automation can jump the
  read head into retained history, loop a window, change rate, reverse, and
  return live.
- Its memory, read/write collision behavior, reset/persistence semantics, and
  deferred reclamation are specified and tested. Collisions are counted and
  published; the allocation-detector harness that would settle realtime
  hygiene by measurement rather than by reading is still missing.
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
