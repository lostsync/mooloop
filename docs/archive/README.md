# Archived documents

Documents that have been fully consumed by the work they describe. They are
kept because they explain why something is shaped the way it is, and because
source comments and older plans still point at them. Nothing here is current;
do not use one as a contract.

For the equivalent at plan granularity, see `docs/plans/archive/`.

| Document | Why it is here |
| --- | --- |
| `EFFECTS_PLAN.md` | The 2026 build order for the effect suite, written before the filter shipped. Its structural-ring transport details were superseded by `AUDIO_ARCHITECTURE.md`'s prepared-state ownership, and its "Explicitly out of scope" section by `MODULATION_PLAN.md`. Retained for the narrative of how the effect slice was built. |
| `EFFECTS_FEEDBACK.md` | Adam's raw notes on the effect devices, August 2026. Every item became a numbered plan under `docs/plans/archive/effects-feedback/`, and all fourteen landed. |
| `SHORTCUTS.md` | The eight-line list of shortcuts Adam wanted. All eight exist as registered actions; `docs/ACTIONS.md` is the live contract and `crates/mooloop-ui/src/actions.rs` is the list. Its one standing product decision — no F-keys for default bindings — is restated in `ACTIONS.md`. |
| `MIXER_PLAN.md` | The signal-slot mixer design, August 2026. Built by `plans/archive/console/` on 2026-09-11, which went further than it asked. Archived while still claiming "nothing in it has been built" — it had been wrong for three days and was the third-most-cited design document in the tree. |
| `ARCHITECTURE_REVIEW.md` | The engine graded against `reference/ANATOMY_OF_A_DAW.md`, 2026-09-02. Every action in its summary is closed. Its verdict stands: barely divergent, do not rebuild. |
| `ROADMAP.md` | The whole product ordered by dependency, in seven phases. Superseded by `SCOPE.md` (the freeze line) and `FOCUS.md` (the sequence). Kept for the dependency argument. |
