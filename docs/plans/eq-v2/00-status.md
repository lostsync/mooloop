# EQ v2 status

## Step 01 — per-band parameters

Landed on `feat/eq-per-band-parameters` (2026-09-14). Every band and both pass
filters carry their own stable ids: **50 descriptors where there were seven**,
and not one of them means "the selected target's". `selected_target` is what it
always was -- which control set the face is showing -- and it is no longer
reachable by an automation lane.

Steps 02 to 04 are unstarted. They are an EQ that is merely better and are
separable, which is what the plan said when it was written.

### The face did not change, and that is the finding

The step file's cost estimate reads like a device rewrite: forty-three
parameters, the modulation arrays resized, `modulation-allowed[N]` reindexed,
the freeze table rewritten. All true. What it did not anticipate is that
**`eq-device.slint` needed no edit at all, and neither did `main.slint`.**

Because a face showing one band at a time is right, and was always right. Seven
duplicated control sets is not an interface. What was wrong was that the
*parameter space* was shaped like the face -- that the DSP had to consult a
selection before it knew what a Freq event meant. So the fix is to resolve the
selection one layer earlier: `EqFaceControl` is the face's vocabulary,
`EqParams::id_for_selected` is the single place a selection becomes an id, and
`Session::set_effect_param` grew one branch.

Two things made that free rather than clever:

- **The face's control indices were kept at the retired ids.** Freq was id 2,
  so the face still calls it 2 and `modulation-allowed[2]` still draws the
  Freq knob's arc. `a_face_control_index_round_trips_and_matches_the_retired_ids`
  is what stops that being an accident nobody wrote down.
- **The overlays are gathered rather than re-indexed in the markup.** Every
  other face reads `modulation-allowed[<id>]` and the ids are dense from zero,
  so the id *is* the index; the EQ's four overlay arrays are gathered down to
  the seven the face reads. Teaching the markup a strided id layout would have
  been this codebase's characteristic fault -- a table spelled in Rust and
  again in `.slint`.

### No `FORMAT_VERSION` bump, and the step file's reasoning was too coarse

The step file says to bump: "the deciding question is whether an existing
lane's `ParamAddr` still means what it did, and it does not." The question is
right and the remedy was priced without looking at what it costs.

`FORMAT_VERSION` is refused outright on load -- `Error::UnsupportedVersion`,
no migration path -- so a bump does not retire seven ids, it **refuses every
document**: every song with no EQ lane in it, and every factory preset already
on disk behind a seeding marker that `LOOSE_ENDS.md` records can never
re-seed.

What makes the bump unnecessary is the thing the step file already asked for
in a different sentence. `EqParams`'s persisted *shape* is unchanged -- same
bands, same fields, `selected_target` still saved -- so a v1 song's EQ loads
and sounds exactly as it did. And because the new ids **start at 16**, a stale
lane on the retired 0..6 lands in a hole: `get` and `set` answer `None` and it
is inert, rather than moving some unrelated band. Leaving a gap under the new
base is what buys that, and `the_retired_eq_ids_address_nothing` holds the gap
open.

### What the plan worried about and should not have

**The modulation arrays are not a realtime cost.** The step file says to
measure `descriptor_slots` growth with `block_cost.rs` before assuming it is
free. It is free, and no measurement was needed to say so: `descriptor_slots`
is called from `mooloop-ui` and `mooloop-session` only. Neither
`mooloop-engine` nor `mooloop-dsp` mentions it. The arrays are built when the
UI publishes a row, at UI rates, over fifty descriptors.

Two costs are real and neither is audio:

- **A hundred-slot array per effect row**, because ids are positions and the
  EQ's highest is 99. The strip has done the same since 2026-09-11.
- **Fifty rows in the automation destination menu for one EQ.** That is what
  per-band addressing means. Recorded in `LOOSE_ENDS.md` as an argument for
  filtering the menu, not for fewer ids.

### The table is generated, unlike the strip's

The channel strip writes its sixteen band rows out longhand and says why: a
`const fn` cannot `concat!` a name, and a table whose rows a reader cannot see
is not a table anybody will check a face against. Fifty rows is where that
reasoning inverts -- four hundred lines of literal is not a table anybody reads
either. So the names and the per-band defaults are two small arrays, the shape
of a field is written once, and a `const fn` builds the static. What a reader
needs to verify is *which name sits at which id*, and `param_id_freeze_tests`
pins all fifty of those explicitly.

### A correction, and the joke it is at this file's expense

The first version of this status, the commit message beside it and four other
documents said **63** descriptors. There are fifty: seven bands of six fields
and two pass filters of four. 63 was the *action registry's* count, from the
step that landed earlier the same day, carried over by hand.

Which is precisely the failure `ACTIONS.md` records about itself twice, and
which the keyboard pass answered with a test that reads the sentence -- written
hours before this was typed. The number is right in the code
(`EQ_DESCRIPTOR_COUNT` is the arithmetic, not a literal) and was right in three
of the places prose stated it; what went wrong is the one place a human
recalled it instead of deriving it. Left written down rather than quietly
corrected, because "counting is what went wrong both times" has now happened a
third time, in a different file, on the same day it was written down.

### One thing a test found that the plan did not name

`slint_face_agreement.rs` failed, correctly, the moment the ids moved: it pairs
a knob to a parameter by the overlay index the markup reads, and the EQ's index
had stopped being an id. Teaching it `EqFaceControl` fixed it -- and doing so
exposed a live defect the old model had hidden. **A knob's double-click returns
to band 2's default whatever band is selected**, because the face's resting
values are hardcoded and one `Freq` descriptor used to stand for all seven
bands at 1 kHz. Band 1 rests at 120 Hz. Equally true before this change and
uncheckable then. In `LOOSE_ENDS.md`; the fix is a per-row defaults array,
which is a face-contract change and belongs to a later step.

## Why this plan exists

Written 2026-09-12, out of a fix to one bug and a question Adam asked about
what that bug implied.

## What prompted it

`a452bf4` split `EQ_PARAM_CHARACTER`, which had meant a band's Q profile when a
band was selected and a pass filter's slope when a pass filter was -- two
settings of different arity behind one automatable id, so four of the Shape
control's five positions collapsed onto one and an automation lane reported a
value it had not set.

Adam's question was the useful part: **mooloop intends to host CLAP, CLAP
plugins expose arbitrary automatable parameters, and our own devices should
reach the automation system the same way plugins will.** Under that test the EQ
is not a device with a missing feature. It is the one native device whose
parameter model cannot be expressed in the model a plugin host has to have.

## Where this sits against FOCUS.md

**Step 2 of the sequence as of the 2026-09-12 rewrite**, behind finishing
`interface-iteration/` and ahead of Buffer. That plan closed on 2026-09-14, so
this became step 1 the same day and step 01 landed on it. Adam asked for a
plan, not for it to be built; then he put it in the order.

Two things about the sequencing are worth saying here rather than in a step.
Step 01 is the only one the CLAP argument forces; 02 to 04 are an EQ that is
merely better, and they are separable. And step 01 is worth doing *before* any
plugin work rather than after: it is a small instance of the same
instance-scoped-parameters problem, on a device whose behaviour is already
understood, which makes it a cheap way to find out whether the model holds.

`README.md` carries the argument and the measurements. The steps are the work.
