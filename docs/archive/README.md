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
