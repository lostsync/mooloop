# 04 — accessibility

The part of this plan that is worth doing on its own terms rather than for the
homage. Adam had not considered it; this step is what considering it produces.

Depends on 01 (the tokens) and 03 (somewhere to put the settings).

## The one real problem

**The interface's working type size is 7 to 11 pixels**, and 291 of 313
literal sizes are in that band. It is not adjustable. On a high-DPI laptop
panel a 7px label is at the edge of legible for a person with good vision and
past it for a person without.

`Theme.type-scale` from step 01 is the fix, and the step is:

- a control on the Appearance page, in the plain part rather than behind the
  disclosure, offering something like 1.0 / 1.25 / 1.5;
- a pass over the layouts at 1.5 to find what clips. **This is the actual
  work.** A `ValueReadout` sized to fit `-99.9 dB` at 9px does not fit it at
  13.5px, and `controls.slint:375` already documents that these boxes are
  width-pinned on purpose so a knob drag does not jitter them. Every such pin
  has to be expressed against the token rather than against a number.
- `Theme.density` moves with it or beside it -- bigger text in the same 24px
  control is not a legibility win.

Do not ship the control before the pass. A type scale that clips readouts is
worse than none, because it looks like the program is broken rather than like
a setting is unavailable.

## Contrast: derivable, and currently unchecked

`derive_palette` already computes `relative_luminance` and uses it to decide
whether the ramp is dark or light. Nothing checks the *result*. A user can set
a base and a contrast that put `text-faint` at 1.4:1 against `surface`, and
the Appearance page will show them the swatches without comment.

So:

- a `contrast_report(&ThemePalette)` returning the WCAG ratio for the pairs
  that matter -- `text`/`background`, `text`/`surface`, `muted`/`surface`,
  `faint`/`surface`, `accent`/`surface`, and each meter color against
  `panel`;
- the Appearance page shows the worst pair and its ratio, and says whether it
  clears 4.5:1 and 3:1;
- **it warns, it does not refuse.** `reference/ADAM.md`'s position is that the
  interface communicates rather than forbids, and a deliberately murky theme
  is a legitimate thing to want.
- a unit test asserting every one of the six built-in themes clears 4.5:1 on
  body text and 3:1 on muted. That is the check that stops the built-ins
  drifting, and it costs nothing to keep.

A **High Contrast** built-in theme is then a preset that passes at a wide
margin: `contrast` near its maximum, `hairline` at 2px, `relief: flat`, and a
type scale of 1.25.

## Reduced motion already exists and is not labelled

`Motion.speed` has an Instant option and `Motion.curve` a linear one. A user
looking for "reduce motion" will not find it under a speed picker called
Instant. Add the words, and consider honouring the platform's own reduced-motion
preference on first run.

This is the cheapest item here by a wide margin and should go in first
regardless of what else is built.

## Hit targets

`ToolButton` is `min-width: 44px; min-height: 24px`, which is honest. The
knobs, `MiniKnob`, `TrimKnob`, and the modulation shelf's controls are not,
and several are under 20px square. `Theme.density` is what would let them grow
together; without it each is a face edit.

Worth doing after the type-scale layout pass, because it is the same pass.

## What this plan does not fix, stated plainly

**Screen readers.** `accessible-role` and `accessible-label` are set on the
controls in `controls.slint` and absent from the ~190 anonymous `Rectangle`s
that make up the panels, rails and headers. A blind user cannot navigate
mooloop today and no theme changes that. It is real work, it is not this
work, and it should not be counted as a benefit of this plan.

**Keyboard reachability** is partial -- `interface-iteration/` has been
building it out and the root focus scope was only fixed 2026-09-07. Same
answer: adjacent, not included.

Both belong in a separate `accessibility/` plan if Adam wants them. This step
covers what falls out of themes and says so.
