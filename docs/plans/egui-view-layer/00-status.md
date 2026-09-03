# egui view layer — plan status

Not started, and **not yet decided**. Written 2026-09-02, out of
`docs/ARCHITECTURE_REVIEW.md`.

This plan exists so the decision can be made against real numbers rather than a
mood. It is deliberately shorter than
`docs/plans/session-layer-extraction/`, because the honest state is that a
detailed plan for the second half would be pretending to know things that the
spike in step 01 is supposed to find out.

## Prerequisite, hard

**`docs/plans/session-layer-extraction/` must be finished first.**

Not as a matter of tidiness. Today the application's live model is stored inside
Slint containers — about a dozen `Rc<VecModel<...>>` fields on `UiState` — and an
immediate-mode toolkit cannot carry those across. Starting egui first means
writing a second session layer inside the egui code and then owning two, which
is the outcome this ordering exists to prevent.

Once the extraction is in, this plan is a view rewrite against a stable session
crate, and the 51,700 lines of `mooloop-core`, `mooloop-dsp`, `mooloop-engine`
and `mooloop-project` are not involved at all.

## The case for egui

Four things, all measured against this repository rather than general
preference. The fourth gets its own section because it was originally written
as the argument *against*, and measuring it turned it around.

**1. The projection layer disappears.** `mooloop-ui/src/lib.rs` carries 589
property set/get calls and 187 callback registrations, and roughly half of
`UiState`'s methods exist only to push plain data into models. In immediate
mode there is no model to push into — the draw code reads the session directly.
That whole class of "the view is stale because a `sync_` was missed" bug stops
existing.

**2. Custom drawing is the project's main UI activity, and Slint fights it.**
`docs/WIDGET_INVENTORY.md` opens with the finding that Slint has no plotting
primitive: `Path` elements are static children rather than a model, so a
polyline over a series cannot be expressed, and the one-element-per-sample-pair
workaround is hand-rolled **17 times across 8 files**. Mooloop is a synth with
eight device visualizers, waveforms, envelopes, meters and a piano roll. An
immediate-mode painter is the natural shape for all of it.

**3. One language.** No `.slint` markup, no `slint-build`, no i32-indexed enum
converters at the boundary (`lib.rs:1810-1970` exists purely because Slint has
no Rust enums).

## 4. Compile time and memory — the assumption that inverted

The original reasoning was that `scripts/slint-sketch` iterates a `.slint` edit
in 0.05s where `cargo build -p mooloop-ui` is about four minutes, so a move to
egui would turn every visual tweak into a multi-minute Rust rebuild.

That was wrong, and wrong in the direction that matters. **The four minutes is
Slint's cost, not the cost of compiling a UI.**

`crates/mooloop-ui/build.rs` calls `slint_build::compile("ui/main.slint")`,
which expands 20,451 lines of `.slint` into a **single 39 MB, 395,450-line Rust
module**. `mooloop-ui` therefore compiles roughly 412,000 lines, 96 percent of
them generated, in **one rustc process** — which is why capping Cargo jobs never
helped and why `scripts/cargo-capped` had to exist at all. That script's own
header states the diagnosis and carries the numbers.

Measured on this laptop. Slint figures from `scripts/cargo-capped`; egui figures
from a scratch `eframe` 0.32 probe (glow backend) carrying 21,355 lines of
generated immediate-mode drawing code across 12 modules — deliberately *larger*
than `mooloop-ui`'s 16,490 hand-written lines, so the comparison is pessimistic
for egui.

| | `mooloop-ui` today | egui probe |
| --- | --- | --- |
| incremental check, one edit | 26s (`.rs`) / 41s (`.slint`) | **1.4s** |
| incremental build, one edit | ~4 min | **6.7s** |
| cold check, deps included | 2m13s | 77s |
| cold build | — | 112s |
| peak RSS, incremental | 3.24–3.42 GB | **0.21–0.54 GB** |
| peak RSS, cold | 5.24 GB | 0.64 GB |

So the loop that matters — change something, see it in the real application —
goes from 26–41 seconds to about a second and a half to check, and from minutes
to seconds to build. Peak memory drops by roughly an order of magnitude, which
is the number that removes the reason `scripts/cargo-capped` and its cgroup
exist.

**Two honest caveats.** The probe links a small binary, where a real
`mooloop-app` build also links the engine, DSP and project crates, so the
6.7s figure is a floor rather than a prediction. And generated repetitive code
may suit rustc better than diverse hand-written code. Neither touches the
finding, because the finding is the 39 MB module, and that goes away regardless.

**What survives of the original objection**, and it is now the whole of it:
`slint-sketch` type-checks in 0.05s and screenshots in 0.2s, and nothing in a
Rust rebuild loop will match that for *isolated widget work*. But it only ever
covered markup-only changes viewed on their own — seeing a `.slint` change in
the real application already costs the full build today, and any change touching
`lib.rs` always did.

## The case against

**Sunk work.** 20,451 lines of `.slint` across 43 files gets thrown away,
including the appearance system, the mockup catalog that `docs/ROADMAP.md` names
as the interaction contract, and the theming in `settings.rs`.

**Losing the sketch loop.** As above: `slint-sketch` stays faster than any Rust
rebuild for isolated widget iteration, and there is no egui equivalent unless
one is built.

**`docs/FOCUS.md`'s own rule.** It says to prefer changes that produce a musical
decision over changes that merely add capacity. A toolkit migration is the
largest available non-musical change. That rule is Adam's own and it argues
against this plan regardless of how well it would compile.

## How to decide cheaply

The compile-time question is answered and does not need the spike. What is left
is whether the *interaction* survives, which no amount of measuring from outside
can settle.

So do step 01 — one real pane, in egui, against the real session, in a throwaway
binary — and decide from that. It is a few days and it is discardable.

The decision is now: is losing `slint-sketch`'s isolated widget loop, plus
throwing away 20,451 lines of working `.slint`, worth deleting the projection
layer, getting a real plotting primitive, and a UI crate that checks in 1.4
seconds inside 0.2 GB instead of 41 seconds inside 3.4 GB.

## Steps

| Step | What it does |
| --- | --- |
| `01` | Spike: one pane, one binary, decide from it |
| `02` | The widget vocabulary — knob, meter, plot, grid |
| `03` | Pane-by-pane migration behind a second binary |
| `04` | Cutover and deletion |

Steps 02–04 are sketched only. If step 01 says yes, they get written properly
then, with what the spike learned in them. If it says no, this directory is
archived with the spike's findings and the answer is on record.
