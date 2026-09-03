# 02–04 — Sketch only

Read `00-status.md` first. **Nothing here is a plan yet.** These three steps are
written as a sketch so the shape of the whole is visible from step 01, and they
should be replaced with proper step documents once the spike has numbers.

Writing them out in detail now would be inventing answers to questions the spike
exists to ask.

---

## 02 — The widget vocabulary

egui gives containers, text and a painter. It does not give a synth's controls.
Before any pane is migrated, build the small set everything else is made of:

- **Knob**, with the project's existing drag semantics: draggable from the label
  as well as the body (`docs/ROADMAP.md`), fine-drag modifier, double-click to
  default, and value-only tooltips per the standing convention — explanatory
  text goes to the status bar, not the tooltip.
- **Meter**, fed from `Session::tick`'s report, with the existing segment
  behaviour.
- **Polyline plot.** The one Slint could not express.
  `docs/WIDGET_INVENTORY.md` entry 1 is the specification, and this is where
  most of the visual payoff is.
- **Step grid** and **piano grid**, if the spike showed they need to be shared
  rather than drawn per pane.
- **Theme**, reading the existing `AppearanceSettings` (`settings.rs`) so
  schemes, contrast and corner radius carry across rather than being redesigned.

`docs/WIDGET_INVENTORY.md` is the input to this step. It was written as a menu
of components Slint should have had; it becomes a build list.

Read `docs/UI_DESIGN.md` before starting — the interaction contract is the part
that must not be renegotiated by accident during a toolkit change.

---

## 03 — Pane by pane, behind a second binary

Keep both UIs alive. `mooloop-app` continues to launch Slint; a second binary
launches egui, and panes move across one at a time in roughly this order:

transport → channel rack → mixer → device rack → sampler → piano roll →
playlist → browser → preferences.

Piano roll late, because it is the most interaction-dense. Preferences last,
because it is the least interesting and the most tedious.

Both binaries depend on the same `mooloop-session`, so there is no divergence to
manage and either can be abandoned at any point. The rule is that the egui
binary is never the only way to do anything until step 04.

---

## 04 — Cutover

When every pane is across and Adam has used the egui build for real work rather
than for testing: `mooloop-app` launches egui, and `mooloop-ui` plus its 43
`.slint` files are deleted in one commit.

Deleting in one commit matters. A half-deleted Slint tree kept "just in case" is
how a project ends up maintaining two view layers indefinitely.

`.github/workflows/release.yml` and the `cargo deb` metadata in
`crates/mooloop-app/Cargo.toml` name shared-library dependencies with `$auto`,
so the packaging should follow the toolkit change without hand-editing — worth
confirming rather than assuming.

The mockup tool (`mockup.rs`, `mockup.slint`, `mockup-catalog.slint`) is a
casualty to decide about explicitly. `docs/ROADMAP.md` names its catalog as the
interaction contract, so either it is rebuilt on the egui vocabulary in step 02
or the contract moves somewhere else. Do not let it fall off the edge silently.
