# 02 — Judge

`dupe-audit` ends every run with the sentence that governs this stage:

> None of them is a finding until you have read the code.

Three questions, in order. Most leads die on the first.

## Is it still true?

**Check the tree before you check the claim.** Of the ten items worked on
2026-09-13/14, three were already fixed and the row had been left behind:

- `render_blocks` "is written about seven times" — `render_test_support.rs`
  existed and six of the seven named modules imported from it.
- "Eight device faces spell a number the descriptor table already states" — it
  was two, because the agreement test's parser had become block-based and its
  list had grown to take six of them.
- "The faces that are covered are covered for their ranges, not their resting
  positions" — `every_effect_face_knob_agrees_with_its_table` had been written.

A stale row costs more than a missing one. It sends somebody to fix a thing
that is fixed, and — worse — it is *believed*, so the numbers in it get used to
size a guard. See `MIN_DB` in [01-find.md](01-find.md).

**A row that is stale is itself a finding.** Rewrite it or delete it in the
same pass. Do not annotate: `LOOSE_ENDS.md`'s own scope rule is that an item
belongs there only if it is small, specific and true of the code right now.

## Is the copy real?

Three things that look like copies and are not:

- **A binding is not a copy.** `default-value: root.defaults[root.param]` reads
  the table at run time. DS-01's whole paged face is built this way and it is
  the shape the rest could move to.
- **A derivation is not a copy.** `maximum: GainMath.db-to-linear(GainMath.fader-db[0])`
  reads a checked list. The literal `12.0` *inside* a conversion is a copy; the
  conversion is not.
- **Copied arithmetic does not drift.** Four synths computing `sin(2*pi*phi)`
  will agree forever, because a sine has no parameters to disagree about. The
  2026-09-12 audit removed a fair amount of copied code and almost none of it
  could have hurt anybody.

And one thing that looks like a false positive and is not: a value that is
*correct today* is still a finding if nothing would report it going wrong. That
is the normal case, not the marginal one.

## Is it worth a change?

Not everything found should be fixed, and saying which is part of the work.

**Some literals have nowhere else to live.** `bus-device.slint`'s two are a
`MiniKnob`'s pan centre and a `MixerFader`'s unity. Neither is
descriptor-backed, so there is no table for a widened parser to compare them
against. Unity and centre are not copies of anything.

**Some copies are the only thing anything checks.** `device-displays.slint`'s
`threshold-min-db` and `floor-db` are held to `gain::MIN_DB` by
`strip_face.rs`, which finds them by *parsing the number out of the
declaration*. Replacing the number with a property reference would take the
guard off rather than improve it. That one is written down rather than done,
with what fixing it would cost.

**Some are a question about the interface, not about duplication.** The roll's
eleven musical divisions and the modulator's twenty-one are different
vocabularies that overlap. Collapsing them means deciding whether the roll's
picker grows to twenty-one entries.

## When the note's proposed fix is wrong

Read the fix a row proposes against the code before taking it.

`LOOSE_ENDS.md` said the LFO and Random rate knobs were drawn
`ValueScale.logarithmic` while their descriptors declared `ParamCurve::Linear`,
and that "the correct resolution is to fix the Rust curve". Right about the
fault. But there is no `ParamCurve::Logarithmic` — the variant that means what
`ValueScale.logarithmic` means is `Exponential`, "even in ratio, so a knob
feels right on frequencies and gains". The two names for one idea were
themselves the reason nobody had noticed.

## Recording the judgement

If the answer is "not now", say so in the row **with what would change it**.
A row that says only "not small" gets re-litigated by the next person from
scratch. A row that names the three options and says which one the code's own
claims already commit to can be acted on in an hour — which is exactly what
happened with `default_band_position`.
