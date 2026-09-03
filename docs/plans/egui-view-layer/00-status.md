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

Three things, all measured against this repository rather than general
preference:

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

## The case against, and it is not weak

**Iteration speed collapses.** This is the strongest argument and it should not
be waved through.

`AGENTS.md` records the current numbers: `scripts/slint-sketch` type-checks a
`.slint` edit against the real widgets in about **0.05 seconds** and screenshots
it in about **0.2 seconds**, where `cargo build -p mooloop-ui` is about **four
minutes for any edit at all**.

In egui every UI change is a Rust rebuild. Layout tweaks, spacing, colour, a
knob's travel — all of it moves from a fifth of a second to minutes. On the most
iterative work in the project, that is a difference of three orders of
magnitude, and it is permanent rather than a migration cost.

There are mitigations — a small standalone binary that draws one pane, a
gallery binary that builds fast because it does not pull in the engine — and
step 01 exists partly to find out how well they work. But the mitigation is
"make the rebuild smaller," not "avoid the rebuild," and the honest expectation
is seconds rather than a fifth of a second.

**The second cost:** 20,451 lines of `.slint` across 43 files is real work that
gets thrown away, including the appearance system, the mockup catalog that
`docs/ROADMAP.md` names as the interaction contract, and the theming in
`settings.rs`.

**The third:** `docs/FOCUS.md` says to prefer changes that produce a musical
decision over changes that merely add capacity. A toolkit migration is the
largest available non-musical change. That rule is Adam's own and it argues
against this plan.

## How to decide cheaply

Do not decide from this document. Do step 01 — one real pane, in egui, against
the real session, in a throwaway binary — and decide from that. It is a few
days, it produces a genuine answer to both the "does it feel better" question
and the "how bad is the rebuild loop" question, and it is discardable.

The decision is not "egui or Slint" in the abstract. It is whether losing
`slint-sketch` is worth deleting the projection layer.

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
