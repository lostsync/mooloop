# 04 — Guard

The fix is the smaller half. A boundary fault recurs because the *conditions*
recur, and the guard is what changes them.

## Walk the directory, never a list

**This is the single rule that matters.** A hand-written list of files inherits
whoever wrote it, and `AGENTS.md` records the recurrence: the generator faces
were covered for months while the effect faces were not, because the list had
simply never been extended.

The first version of the meter-floor guard swept `meters.slint` and
`controls.slint` — the two files `LOOSE_ENDS.md` named — and would have passed
with 26 of the 50 copies still in place. Rewritten to read `ui/`, it found
them.

So: `std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/ui"))`, and
exclusions by name with a stated reason.

Where a list genuinely cannot be derived, check it **in both directions**.
`shelf_agreement.rs` pairs 42 markup ids with Rust constants by hand — the two
sides spell nothing alike, `rate` is `RATE_HZ` and `fade-in` is `FADE_IN_S` —
so it asserts every markup property appears in the pairing *and* every pairing
entry still exists in the markup. A derivation would have been deriving one
side from the other, which is the thing being guarded against.

## Count what you found

Every walk needs a tripwire, because **a parser that stops matching reports
zero and passes**. Assert the count:

```rust
assert!(checked >= 4, "only {checked} ... the walk has stopped matching");
```

This is not ceremony. The tripwire is what proved the `SyncMiniKnob` anchoring
worked in `shelf_agreement.rs`: `MiniKnob {` is a suffix of `SyncMiniKnob {`,
so a naive search reads every sync knob twice — once with the wrong key set,
silently, because the keys it looks for are then simply absent and every range
reads as the widget default. The count came out 21 instead of 27, which is the
only reason anybody knew.

## Read markup, not pixels, for an invisible defect

A lamp that cannot light looks exactly like a lamp that is not lit. A snapshot
test cannot tell them apart and neither can a human looking at one frame.

`no_channel_meter_draws_a_clip_lamp_it_cannot_light` parses `.slint` and
requires every `ChannelMeter` to either bind `clipping` or set
`show-clip: false` — and requires a bound lamp to also be clearable, because a
lamp that lights and cannot be cleared is worse than one that is not drawn.

The same argument applies to `every_editable_text_field_has_a_way_out`. A field
with no exit behaves correctly right up until somebody clicks away and presses
Space.

## Require a deliberate act, not a correct value

The strongest guards do not check that a number is right. They check that
somebody *decided*. `show-clip: false` and `clipping: ...` are both acceptable;
saying neither is not. The fault being prevented is a default drawing something
nobody thought about, so what must be made impossible is not thinking about it.

## Exclude by name, with the reason

`mockup-catalog.slint` is a gallery of specimens where every control is inert
on purpose, behind the `mockup` feature, not part of the shipped interface. It
is excluded **by name and not by a pattern**, so "inert on purpose" stays a
claim about one file rather than a category a later file can join by accident.

A `read-only` `TextInput` is excluded by *property*, because read-only is
exactly the thing that makes it a selectable label rather than a field. Exclude
on the property when the property is the reason; on the name when it is not.

## Split out the part that decides

When the defect is a decision rather than a value, extract the decision so it
can be read back. `spectrum_subscription_plan` returns `(slot, enabled)` pairs
for a whole chain because **the defect was a walk that only ever said `true`,
and the only way to see that a walk says `false` where it should is to look at
what it says.**

## Where a new check goes

- A **lead generator** over the whole tree → `scripts/dupe-audit`, as a new
  check with a docstring saying what it is for and what it deliberately misses.
- A **contract between two named things** → a test in
  `crates/mooloop-ui/tests/`, in the binary that already owns that subject.
  Reuse one: each test binary is a separate link of a large crate.
- A **fact about one value** → a unit test beside it.

Validate a `dupe-audit`-shaped check against the commit it was written for.
`AGENTS.md` records `one-sided-test`'s first version *missing* the defect it
was written for, because the broken test mentioned `main.slint` in its own
comment and a mention is not a read. A check of that kind that has not been run
against a tree where it should fail is decoration.
