# 09 — The rack draws branches

> **Superseded in part, 2026-09-23.** Adam answered the height question with
> none of the three options below: copy Bitwig's FX Layer
> (`reference/img/bitwig-fxlayer.png`). A one-unit face lists the branches
> (name, S, M, meter, `+`, Gain, Mix), and the selected branch's chain
> continues to the right in the rack. See `00-status.md` and MOO-71. What
> follows is kept for the surrounding facts (`ContainerEnclosure`,
> `containers_closing_at`, the `lvl`/`level` trap), and this step needs
> rewriting before it is built.

The drawing step, and the one with a genuine design question in it.

**Read `04-the-rack-draws-the-box.md`'s status entry before starting.** The
container's enclosure was drawn three times and each wrong version was
*restyled* rather than questioned, because a wrong thing that looks like a
plausible thing gets restyled instead of fixed. It was settled by Adam with a
mock-up. Three iterations is the measured cost of not having one.

**So: get the mock-up first.** This step is not blocked on engineering.

## The premise 06 inherited is wrong, and it helps

`04` recorded the constraint as *"vertical adjacency means the chain
continues, so a layer's branches need a treatment that is not adjacency"*, and
`06` repeated it as a price.

The rack is **horizontal**. `rack-row` is one `HorizontalLayout` with
`alignment: start` in a horizontally scrolling viewport, which is the same
fact `04` had to correct itself about when it discovered the rack does not
wrap. It is *horizontal* adjacency that means the chain continues.

Which means the axis a layer wants is **free**. Branches stack vertically
inside the enclosure; the chain continues to the right, past it. Nothing has
to be invented to distinguish them, because the two directions already mean
two different things and only one of them was spoken for.

## The question that is actually open

Every face is `DeviceRackMetrics.face-height`, a flat 268px
(`device-rack.slint:10`), and `rack-height` is that plus padding. A layer of
three branches is three faces tall. Three ways out, and the choice is Adam's:

1. **The rack grows.** A layer's enclosure is `n × rack-height`, the rack pane
   scrolls vertically as well as horizontally. Honest, and it makes a
   three-branch layer dominate the pane.
2. **Branches scale.** Faces inside a layer draw at `face-height / n`. Keeps
   the rack one unit tall and makes a knob inside a three-branch layer 89px
   of face — which, per `reference/` on face width, is where a face stops
   being usable and starts being a picture of one.
3. **One branch at a time.** The layer draws its branches as tabs and shows
   the selected one at full height. Cheapest to draw, and it makes the thing
   a layer is *for* — seeing two signal paths beside each other — invisible.

My read is (1), with a cap: a layer wider than two branches is rarer than a
layer of two, and vertical scroll in the rack is a thing the pane can already
be given. But this is a taste call about the instrument's shape and it should
be a mock-up, not a paragraph.

## What is already there to build on

- `ContainerEnclosure` (`device-rack.slint:101`) — the box, drawn one slice
  per row: head caps the left, the row before a shallower one caps the right.
- `EffectSlotRow.depth`, `.children`, `.closing` — what a row is inside, and
  which boxes end at it. `closing` is computed by `containers_closing_at`
  (`mooloop-ui/src/lib.rs:2218`) because a repeater item cannot walk its own
  model, and a layer needs the same answer for the same reason.
- `DeviceFrame::contained` — turns off a row's own card so the container's
  surface shows through — and `tail-rail`, which lets a container not draw a
  rail against its own face.
- The surface ramp is the depth vocabulary and does not run out.

What a layer adds to the model is **which branch a row is in**, which is
`parent_of` walked up to the layer, and **whether a row starts a branch**,
which is "its parent is a layer". Both are derivations in `mooloop-ui`
alongside `containers_closing_at`, not new fields in the session.

## The bug this step is most likely to repeat

`for level[lvl] in [1, 2, 3, 4]` binds `level` to the **value** and `lvl` to
the **index**. The enclosure used `lvl`, so every device in the rack wore a
level-0 container band for three iterations, and it hid because it was drawn
behind opaque cards. A branch treatment indexed by branch number is the same
shape of loop over the same kind of small integer.

**Instrument rather than reason** when a layer's enclosure does not look
right: `04`'s three rounds went on appearance while the defect was arithmetic.

## Acceptance

- Adam's mock-up exists and the drawing matches it.
- A layer of two branches reads as two paths, and the device after it reads as
  after *both*.
- A chain inside a layer branch reads as inside the branch, not as a third
  branch.
- An empty layer caps itself, the way an empty container already does.
- `slint_face_agreement.rs` covers the layer face, and
  `scripts/dupe-audit unchecked-face` reports it as seen. `AGENTS.md`: the
  list is hand-written for a stated reason, and the failure it keeps having is
  a face nobody extended it to.
