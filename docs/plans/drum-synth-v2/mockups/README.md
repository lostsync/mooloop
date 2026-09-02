# DS-01 face concepts

Two renderings of the same instrument, at the real face size (1040 x 268, where
`DeviceRackMetrics.face-height` is 268px). They import the real widgets from
`crates/mooloop-ui/ui`, so spacing, type, and colour are honest; the values are
literals and nothing is wired to anything.

Re-render either one without building the UI crate:

```sh
scripts/slint-sketch --shot docs/plans/drum-synth-v2/mockups/concept-lanes.slint
scripts/slint-sketch --shot docs/plans/drum-synth-v2/mockups/concept-overlay.slint
```

`docs/AGENT_OPERATIONS.md` says sketches belong in `$TMPDIR` rather than in the
repository, and it is right about working sketches. These two are checked in on
purpose because they are the argument for a layout decision rather than notes
from making one, and the decision is easier to revisit with the source than
with a screenshot.

## A: lanes — adopted

![lanes](concept-lanes.png)

Each layer owns a row: its controls on the left, its contour on the right, both
on one time axis shared by every other row. The header carries the global
controls and the ruler, and the burst impulses are marked on the ruler because
they are positions on that axis.

It is a multitrack arrangement view of a single two-hundred-millisecond event,
which is the honest description of what a drum patch is. The noise envelope
being shorter than the tone envelope is a thing you see rather than two numbers
to compare, and a value never has to be carried across the panel to the display
it belongs to.

## B: overlay — rejected

![overlay](concept-overlay.png)

The layout `08-the-face.md` originally described: three parallel source columns
above one shared display with every envelope drawn on top of the others.

Building it is what settled the question. Two faults, and neither is a tuning
problem:

- **Four contours and a waveform in one ninety-pixel box is soup.** The
  envelopes cross each other and the trace, the fill of the focused one hides
  the others, and a legend is needed to say which curve is which — which is the
  admission that the picture failed. Concept A shows the same five signals with
  no legend at all.
- **The source columns are twenty-six near-identical small knobs in a grid.**
  That is the "pages of knob rows" that got rejected on ML-P8's first face,
  arrived at from a different direction.

Kept rather than deleted, because the reason a layout was not chosen is worth
as much as the one that was.
