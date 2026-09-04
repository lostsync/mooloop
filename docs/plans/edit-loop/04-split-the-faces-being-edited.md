# 04 — Split the faces that are actually being edited

Read `01-measure-the-loop.md` first, and only start this if its question 3
came back as "device faces". If most editing happens in `main.slint` or
`controls.slint`, **this step buys nothing** -- go to `05`.

## What is already proven

`spike/slint-split-build` has a working prototype: `crates/mooloop-ui-ds01`
compiles the DS-01 face as its own crate, `main.slint` leaves a
`ComponentContainer` where it used to instantiate the face, and
`mooloop-app` -- the only crate that depends on both -- fills it through
`mooloop_ui_ds01::face_factory()`. Measured against the whole binary:

| Edit | check | build |
| --- | --- | --- |
| the DS-01 face, own crate | **2 s** | **3 s** |
| `main.slint` | 31 s | 56 s |
| `controls.slint` | 30 s | — |

`docs/plans/egui-view-layer/slint-split-experiment.md` has the method, the
numbers, and the reasoning. Read it before touching this; it will save
rediscovering the three traps below.

## The three things that must all be true

The prototype measured **29 s** on its first attempt -- no better than
before. Each of these was why, and missing any one puts it back:

1. **No directory watches in `build.rs`.**
   `crates/mooloop-ui/build.rs`'s `emit_component_audit` emits
   `cargo:rerun-if-changed=ui`. Touching any `.slint` anywhere under that
   directory reruns the build script and regenerates the whole module.
2. **The shell must not import the face's file for anything.**
   `main.slint` needed one struct, `Ds01Contour`, out of
   `ds01-device.slint`, and that import alone kept the face in the shell's
   compilation unit. Shared structs go in `ui/device-types.slint`.
3. **The face's markup leaves `mooloop-ui/ui` entirely**, into its own
   crate's `ui/`, with `slint_build`'s `with_include_paths` pointing back at
   the shared modules so `import { Theme } from "theme.slint"` still
   resolves.

## The costs, all of them

- **An experimental Slint feature.** `ComponentContainer` and the
  `component-factory` type are removed from Slint's standard type register
  and restored only by `builtin_experimental`, whose own doc comment says
  "Do not use in production code!". `SLINT_ENABLE_EXPERIMENTAL_FEATURES=1`
  is the only switch. It has been that way since v1.5.0 and still is on
  master. **This is the single biggest reason not to do this step**, and it
  should be Adam's call rather than assumed by whoever picks the plan up.
- **The bindings become Rust.** A face is instantiated with its properties
  and callbacks bound from `root.*` -- DS-01 binds seventeen and seven. The
  shell cannot see a type in another compilation unit, so those move to
  whatever crate does the wiring. Mostly a rename, since the values already
  come from Rust, and it deletes a layer: 230 of `main.slint`'s 447 property
  declarations exist only to forward into a face.
- **It breaks the shell's tests for whatever moved.**
  `render_the_ds01_face` fails under the prototype and is `#[ignore]`d there,
  because the snapshot harness builds a `MainWindow` directly and nothing
  fills the container. Each face's tests move to that face's crate -- where
  they get faster too. Thirteen of `mooloop-ui`'s snapshot tests render
  device faces and every one pays the full rebuild today.
- **About 16% more total generated code**, from shared components being
  generated into each unit that reaches them. Measured at 1.40x for the four
  biggest faces. The largest unit drops 41% in exchange.
- **The case it makes worse.** Editing `controls.slint` regenerates 43.9 MB
  in one process today and would regenerate about 50.9 MB across twenty-two
  under a full split -- more total work, parallel where it currently cannot
  be. Not measured. **Measure it with two faces split before doing twenty**,
  because if it lands badly it changes the whole shape of this step.

## Do it incrementally and stop early

Split the faces that step 01 showed being edited, in that order, measuring
after each. Not all twenty-one. If three faces cover the actual work, three
is the answer and the other eighteen keep costing 31 s each, which nobody
will notice.

The order that reveals problems earliest: the face being edited most, then
the largest (`mlp8-device.slint`, 1,688 lines), then measure a
`controls.slint` edit with two split and compare it against one.

## Done when

Editing the faces Adam actually works on costs seconds, the tests that moved
still run, `cargo test --workspace` is green, and the `controls.slint`
number has been measured with more than one face split rather than
extrapolated.
