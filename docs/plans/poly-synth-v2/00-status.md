Not started.

**Prerequisite: `docs/plans/mono-synth-v2/02-split-the-filter-envelope.md`.**
That step is the shared v2 foundation — separate filter ADSR and filter
keytracking on *both* devices, plus the serialization and descriptor-table
work both plans depend on. It lands once, in the Mono plan, and is not
repeated here. Nothing below can be done properly without it: Drift has no
envelope times to vary, and the filter modes have no independent sweep to
demonstrate.

Then work 01 → 07 in order. 01 is the contract every other step refers to and
should be read first even when picking up a later step.

02 (drift) is the step that makes Poly immediately sound like a different
instrument and is worth doing first for that reason alone. 03 (filter modes)
is independent of 04 (unison) and 05 (chorus) and can be reordered if
something blocks. 06 (oscillator sync) is genuinely optional for v2 — it is
character on top of an identity that is already complete after 05.

Source spec: `~/Downloads/mooloop_synth_v2_spec.md`, sections 4, 6, 8-13.
