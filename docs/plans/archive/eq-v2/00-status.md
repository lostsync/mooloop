# EQ v2 status

## Closed 2026-09-15: step 04 is declined, and the plan is done

**Adam's call, 2026-09-15: close the EQ.** Step 04 — band-dependent saturation,
the measured character from five reference EQs — is **not taken**, and the plan
archives at step 03.

That is the step file's own recommendation followed rather than overruled. It
opens with *"Do not start here"* and gives three reasons in order, the first of
which is the one that decided it: **steps 01 to 03 are corrections — things the
device claims and does not do — and 04 adds a claim.** It is a sound nobody
asked for. The plan is therefore complete in the sense that matters: every
defect it was written around is fixed, and the only unbuilt step is the only
one that was ever optional.

Nothing is lost by closing. `04-band-dependent-saturation.md` stays in this
directory with its measurements intact — the VEQ4 THD table, the observation
that an inductor EQ's nonlinearity sits *inside* the filter network and so
appears only where a band is boosted, and the note that `preamp.rs`'s
tilt/shape/untilt sandwich is already the right structure with the band's own
response as the tilt. If band-dependent saturation is ever wanted, that file is
the work order and it does not need rewriting.

**Two listening passes were owed and are now owed against shipped code rather
than against a decision.** They have moved to `LOOSE_ENDS.md` so that closing
this plan does not bury them. Step 03's is short and is measured: shelves with
a Q away from 0.707, an octave either side of the corner. Step 02's is a look
rather than a listen — the plot claims something new about the pass filters and
wants a patch moving under it.


## The listening brief for step 03, measured 2026-09-15

Step 03 says "**this changes how an existing shelf boost sounds**" and leaves
it there, which is true and not much use to somebody about to listen. Step 02
handed us the instrument to say it properly: `Biquad::magnitude_db` is the
running filter's response, so the two shelf laws can simply be subtracted.
`shelf_law_change_in_decibels` prints the table; run it with `--ignored`.

**The change is smaller than the sentence implies, and it is somewhere
specific.**

- **At the Q both shelves rest at (0.707), the difference peaks at 0.45 dB**,
  an octave from the corner, for a shelf at ±12 dB. At ±3 dB it is 0.21 dB.
- **It is a pivot, not a move.** The two laws agree *exactly* at the corner --
  a cookbook shelf passes through half its gain there whatever its slope -- and
  they agree again beyond about three octaves either side, where the shelf is
  its gain and unity. Everything happens in between, and it is antisymmetric:
  what one side loses the other gains.
- **The real change is across the Q knob, which did nothing at all before.**
  At Q 0.15 the same +6 dB shelf differs by 1.86 dB; at Q 4 by 0.96 dB the
  other way. So a patch where somebody left the shelf Q alone barely moved,
  and a patch where somebody *tried* to use that knob — and heard nothing, and
  probably gave up — is the one that changed.

**So the listen is: shelves with a Q away from 0.707, an octave either side of
the corner.** A default EQ is unaffected, and a song that never touched a
shelf's Q is within half a decibel.

`the_shelf_law_change_pivots_about_the_corner` holds all three of those claims,
and the first version of it asserted the *opposite* of the corner one. The
table is what corrected it — which is the argument for measuring before
writing prose about a change, and this file had already written the prose.

### And a number `LOOSE_ENDS.md` had estimated at nearly twice the truth

That entry says the shelf Q knob "stops steepening above 2" and calls it "the
top four-fifths of the knob's travel". Measured, it is **the top 46%**:
`shelf_slope` clamps the slope at 2.0, the Q descriptor runs 0.15..18
exponentially, and `ln(2/0.15) / ln(18/0.15)` is 0.54 of the way along.
`the_shelf_q_knob_saturates_a_little_past_half_its_travel` pins it, because the
fraction is a product of a clamp in `mooloop-dsp` and a range in
`mooloop-core` and neither of them looks like it has anything to do with a
knob.

## The face and the bank's resting arrangement, 2026-09-15

Not a step in this plan and recorded here because anyone reading it about the
EQ should know the defaults moved. Adam's mockup, and two things he named:
**the low shelf is band 1 and the high shelf is band 7**, and the seven bands
init to the seven-band graphic EQ's own centres -- 63, 160, 400, 1k, 2.5k,
6.3k, 16k -- all of them on. The high shelf was band 3 and four bands sat
stacked at 1 kHz, which is what made a seven-band EQ read as a three-band one
with some spare handles hidden under band 2's.

The face followed: the target row runs high-pass, seven bands, low-pass, in
the order the plot reads; a target with a shape of its own draws that shape
(the two shelves and the two pass filters) and a plain bell draws its number;
the analyzer switch moved out of that row into the plot's corner; and ON
moved down beside the knobs it switches. The glyph follows the band's **live**
kind, through a `band-kinds` array published per frame, so it cannot go on
saying "shelf" about a band a preset has made a bell.

Band kind is still a parameter with no control on the face -- it can be
automated and preset, not clicked. That was deliberate for this pass; a Type
control is an eighth face control and widens the modulation arrays with it.

## Step 02 — the curve tells the truth

Landed on `feat/eq-curve-truth` (2026-09-15). **02 and 03 are done; 04 is
optional and wants Adam's ear.**

All the work and every date written into the code is 2026-09-14; the merge
landed at 00:07 the next morning. Where a document's job is to say *when*, it
says the 15th.

### The fidelity question had a third answer, and it is the strong one

The step file said the acceptance line could not be met in the form the plot
used, and left a decision: either evaluate the real magnitude response in
Slint, or state the weaker standard out loud. Both were wrong about where the
work goes. **Rust samples the curve and the markup draws what it is handed**,
which is what `DynamicsCurveDisplay.curve-db` has done for the channel strip's
compressor since the console pass, in the same words: *"two formulas, one
name, and the copy that drifts is the one a user is reading."*

So `mooloop_dsp::effects::eq_response_db` designs the bank with the function
`EqEffect::update_coefficients` designs it with -- one `design_eq_bank`, not
two loops that agree -- and evaluates `Biquad::magnitude_db` at each point.
The standard the step asked to have stated is therefore the strongest one
available: **the drawn curve is the running filter's magnitude response**, and
`the_plotted_curve_is_what_a_sine_measures_through_the_bank` holds it to a
tone rendered through a real `EqEffect`, to a tenth of a decibel, at eight
frequencies, over a bank with all three band kinds and both pass filters in.
`Biquad::magnitude_db` is held to a measured sine the same way.

The markup's rational approximation is **gone**, not improved. With it went
the fixed `1.6` shelf exponent, the question of what `k = 2S` should be, and
the two fields of `band-data` that only it read.

### What the display still reads, and what a test now pins

Three floats a band and two a pass filter, and they place *handles*: a
position, a height, and whether the handle exists. A pass filter has no gain,
so its handle rides the curve -- you grab the corner where it is.

The pass filters have their own property, which is the shape the step file
said would survive: one array with two strides, written in `lib.rs` and read
in `device-displays.slint`, with nothing asserting either. `eq_face.rs`
reads both strides back out of the markup and compares them to the constants
the publisher uses, and one `mooloop_ui::eq_plot_band` builds the row for both
banks.

### The fourth acceptance line cost the most and paid for itself twice

*"No range or curve constant is spelled in `eq-device.slint`."* That is the
strip's arrangement, so the EQ got the strip's answer: an `EqSpec` global that
Rust fills once at startup from the descriptor table. It carries the ranges,
the units, the counts, the selector labels and -- the part the strip does not
need -- **a resting value per target**, because this face is one control set
over nine targets.

Two things fell out of it that were not the point:

- **The double-click defect closed.** `LOOSE_ENDS.md` had carried it since
  step 01: a knob's double-click returned to band 2's default whatever band
  was selected, because the resting values were three numbers in the markup.
  It was priced as "a face-contract change, a later step" and this was that
  step.
- **The band buttons say what the parameters say.** They read LOW, 1..6 while
  the descriptors read B1..B7, so the face and the automation menu counted
  differently. They read 1..7 now, held there by a test that compares the
  button's label with the descriptor's name.

### And a defect the plot could not have drawn around

**The pass-slope selector was wrong by a factor of two from the day it
shipped.** `EqSlope::Db6` runs one `Biquad::pass`, which is the cookbook's
*second-order* section -- 12 dB per octave -- so the five positions are
12/24/36/48/72 and the face called them 6/12/18/24/36. Nothing could have
noticed: the plot did not draw pass filters at all, which is the fault this
step opened on, and the label was checked against nothing.

`EqSlope::db_per_octave` is the arithmetic and the face reads it. The variant
*names* were left wrong on purpose -- `serde` writes them, so renaming them
either refuses every saved project or silently re-maps one slope to another,
to correct a spelling nothing reads. That is in `LOOSE_ENDS.md` for the next
`FORMAT_VERSION` bump that happens for a reason worth having one.

### What it cost elsewhere, and what it gave back

`strip_eq_response_db` is the same treatment for the channel strip, because
the two banks share one display and a display that holds a law for one of them
holds it for both. `StripEq::update` and `EqEffect::update_coefficients` are
both two calls to a free `design_*_bank` now, and the EQ's two pass loops --
ten lines each, differing in a field name and a boolean -- became one.

`strip_row` and `effect_slot_row` both take a sample rate, because a biquad's
coefficients are designed against one and the plot is those coefficients.
`UiState` holds the number the driver came up at.


## Step 03 — a shelf has a slope, and one bank designs both EQs

Landed on `feat/eq-shelf-slope` (2026-09-14), out of order: step 02 is a
display rewrite and this is two function calls, so it went first.

**A band's Q knob did nothing at all while that band was a shelf.**
`effects/eq.rs` called `Biquad::shelf`, which takes no Q. It calls
`Biquad::eq_band` now, and so does the channel strip.

### The step asked for a test and the answer was to delete the copy

Its third acceptance line: *"a test compares the effect EQ's designed
coefficients against the strip's for one identical band, so the two banks
cannot diverge while claiming the same laws."* They were the same three-arm
`match` in two files. One function is the stronger form of that test, because
there is nothing left to hold in agreement -- `Biquad::eq_band` is the law, and
what stays with each caller is what genuinely differs: the strip resolves a
band's frequency from a *position* through its voicing and takes the Q profile
from the voicing, while the effect EQ has a frequency and a per-band profile.

### The plan said the effect EQ "does not apply" proportional Q. It did — badly

`effects::eq` carried a **private copy of `eq_effective_q`**, byte-identical
to the core function and therefore green. `mooloop-core`'s own doc comment on
that function says it lives there because three callers need one answer, and
ends "the law written twice is the law that drifts, and the copy that drifts
is the one deciding what is heard." This was the caller that had stopped
asking. Exactly `AGENTS.md`'s shape: still correct when found, nothing able to
report the day it stopped being.

### **This changes how an existing shelf boost sounds**

Say it plainly, because `FOCUS.md` believed otherwise -- it records steps 02
and 03 as "an EQ that is merely better" and step 04 as "the only step that
changes how the device sounds when nobody asked it to". That is not true of
this step and could not have been: `shelf` and `shelf_slope` reach `alpha`
differently, so the only shelf the two forms agree on is a flat one.

What that means concretely:

- **A default EQ is unaffected.** Both its shelves rest at 0 dB, and a shelf
  at 0 dB is flat whatever its slope -- `A == 1` makes the numerator and
  denominator identical. `a_shelf_at_unity_is_flat_whatever_its_slope` pins
  that, and it is what makes the change safe to ship without a migration.
- **A song that boosted or cut a shelf now sounds different.** There is no
  honest alternative: the knob was doing nothing, so any value it takes is a
  change from "ignored".
- **It wants a listening pass**, on `FOCUS.md`'s own rule that listening is a
  step. Step 04's pass was always going to be needed; this adds a smaller one
  in front of it.

### The limit this leaves, and why it is not fixable here

`Biquad::shelf_slope` clamps the slope to 0.1..2.0 -- past 2 the cookbook's
radicand goes negative -- and a band's Q descriptor runs to 18, because the
*same id* has to serve that band as a bell. So a shelf's knob stops steepening
above 2 and the face does not say so, which is the thing `strip.rs` calls
dishonest and avoids by giving its two shelf-capable bands a narrower range.

That answer is not available here: every band of a seven-band EQ can be any
kind, so the range would have to depend on a *value*, and a descriptor is
static per id. It is the same shape as everything else in this plan -- a
parameter model that cannot express a condition -- and it is recorded in
`LOOSE_ENDS.md` rather than bodged.

## A regression step 01 shipped, and what found it

Fixed on `fix/eq-selection-refresh` (2026-09-14), hours after step 01 merged.

**Clicking a band did nothing visible.** The selector highlight stayed put and
all six knobs went on showing the band you had just left, until some unrelated
edit republished the row.

`set_effect_param` returned `Option<EngineCommand>`, and step 01 made the band
selector return `None` -- correctly, because the engine has no opinion about
which control set a face is showing. The caller read `None` as "nothing
happened" and skipped `refresh_effect_row`. Two different facts collapsed into
one absence: *this write was refused* and *this write moved something the
engine does not need to hear about*.

It is now `EffectParamWrite`, three outcomes rather than two, and the enum
exists to make the third one impossible to drop by reflex.

**What found it was step 02's own notes.** That step lists, as a fault to
investigate, "clicking a point selects a band and the controls lag it --
confirm the sequence under `scripts/mooloop-mcp` before designing anything, if
it is a publish-ordering bug it is a much smaller fix than a redesign". Reading
that against the code step 01 had just landed turned a *lag* into an absolute:
not late, never. Which is the useful shape of that note — a plan file
describing a symptom precisely enough that the next change to the same path
can be checked against it.

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
