Not started.

**The prerequisite changed.** This plan used to name
`docs/plans/mono-synth-v2/02-split-the-filter-envelope.md` as shared
foundation landing on both devices. It no longer is: per
`docs/plans/mono-synth-v2/00-status.md`, Mono v2 is a new instrument with its
own parameter struct, so 02 landed only there. Poly v2 needs its own
equivalent — a separate filter ADSR and filter keytracking — and it is still a
prerequisite for everything below, because Drift has no envelope times to vary
and the filter modes have no independent sweep to demonstrate without it.

Note also that the original poly synth is being *kept* as a third device and
gaining a mono/poly toggle and a legato toggle; Poly v2 is a new instrument
beside it, not a rewrite of it.

Then work 01 → 07 in order. 01 is the contract every other step refers to and
should be read first even when picking up a later step.

02 (drift) is the step that makes Poly immediately sound like a different
instrument and is worth doing first for that reason alone. 03 (filter modes)
is independent of 04 (unison) and 05 (chorus) and can be reordered if
something blocks. 06 (oscillator sync) is genuinely optional for v2 — it is
character on top of an identity that is already complete after 05.

Source spec: `~/Downloads/mooloop_synth_v2_spec.md`, sections 4, 6, 8-13.
