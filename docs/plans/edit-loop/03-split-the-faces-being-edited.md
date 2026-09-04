# 03 — Split the device faces actually being edited

Read `00-status.md` first, and
`docs/plans/egui-view-layer/slint-split-experiment.md`, which is where every
number below comes from.

This is the only step in the plan that touches UI iteration. Steps 01 and 02
fix Rust sessions and do nothing at all for a session spent on a device face.

## The finding it acts on

On the box, a release rebuild costs **12 s after a Rust edit** and **522 s
after a `main.slint` edit**. A device face compiled as its own crate rebuilds
in **13 s**, and checks in **2 s against 31 s**. The prototype is real and
running: `crates/mooloop-ui-ds01` on `spike/slint-split-build`, reached from
`main.slint` through a `ComponentContainer`, with `cargo test --workspace`
green at 1083 passing.

So the mechanism works and the win is an order of magnitude. This step is
about whether to buy it, because it is not free and the price is not compile
time.

## What it costs

**An experimental feature the vendor disowns.** `ComponentContainer` and the
`component-factory` type are deliberately removed from Slint's standard type
register (`i-slint-compiler-1.17.1/typeregister.rs:675`) and come back only
under `SLINT_ENABLE_EXPERIMENTAL_FEATURES=1`, whose own doc comment says *"Do
not use in production code!"*. It has been in that state since 1.5.0, it is
still in that state on the unreleased 1.18, and it appears nowhere in any
changelog between 1.1.0 and now. Two years available and disowned.

This is the sentence step 04 has to weigh, and it is a different kind of cost
from a slow build: a slow build is paid in minutes and an experimental
dependency is paid in some future release that removes it.

**Two to four days of rewiring.** Each face is instantiated in `main.slint`
with its properties bound from `root.*` — `Ds01DeviceFace` at `main.slint:2994`
binds seventeen properties and seven callbacks. A split crate cannot be bound
to, because the shell cannot see a type in another compilation unit, so every
binding becomes Rust setting a property on a held handle.

**A factory that rebuilds on kind change.** Switching a channel's source kind
has to rebuild the embedded component, which loses its state, so each kind
needs a push-everything refresh. Most of those exist already (`refresh_ds01`
and friends).

## What it is worth beyond the build

**230 of `main.slint`'s 447 property declarations exist only to forward a
value into a device face.** They would go away. That is a simplification the
project would want on its own terms, and it is the part of this step that is
not a trade.

## What it does not reach

`main.slint` and `controls.slint` stay exactly where they are: 30 s to check,
8.7 minutes to a release binary. The measured breakdown says why — removing
all twenty-one faces cuts the generated module by 41% and peak RSS by only
15%, because what is left is `MainWindow` itself with 217 properties and 296
callbacks. The shell is the floor, and no arrangement of Slint crates gets
under it.

So this step makes *device face* work fast and leaves *shell* work exactly as
slow as it is today. Which of those two a session is depends on the session.

## The measurement that settles it — run 2026-09-04

The split is days of work on a disowned feature, so before starting it, the
question of which files the UI work is actually in. Three months of `.slint`
history, 502 file-touches: `main.slint` 145 (29%), `controls.slint` 32 (6%),
the twenty-one faces together 178 (35%).

The faces look like a good target on that alone. The coupling is what kills
it:

- **61 of the 77 commits that touch a device face also touch `main.slint`** —
  79%.
- **Only 10 of 78 touch no other `.slint` file at all** — one in eight.
- Of the `main.slint` lines those commits change, **25% are forwarding
  property declarations and `root.*` bindings**, which a split deletes. **The
  remaining 75% is shell work, which a split leaves exactly where it is.**

A face in its own crate rebuilds in 13 s instead of 522 s, but only for a
commit that touches nothing else — and that is one commit in eight. For the
other seven the `main.slint` rebuild is still paid, and three quarters of what
forced it is work the split cannot remove.

(The 25% is a regex over diff lines, so multi-line bindings and callback
forwards are undercounted. The direction is not in doubt; the number is a
proxy.)

## Verdict: do not start this

Two to four days, on a feature Slint has left disowned for two years, to make
one commit in eight fast. Closed unstarted.

The evidence stays: `crates/mooloop-ui-ds01` on `spike/slint-split-build`
works, and if the shell were ever broken up for other reasons the mechanism is
proven and the measurements are here.

**This is also the finding step 04 decides on.** The thing that makes UI work
slow is `main.slint` itself, and no arrangement of Slint crates reaches it.

## Done when

Done. This step is closed with the reason above; step 04 carries it forward.
