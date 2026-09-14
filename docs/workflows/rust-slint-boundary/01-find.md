# 01 — Find

Four tools, and each one's blind spot. Run them in this order: the cheap ones
narrow where the expensive one has to look.

## `scripts/dupe-audit`

About a second, no build, reports leads rather than failures. Six checks;
`--list` says what they are. Two matter most here:

- **`unchecked-face`** — a face that spells a literal where no test reads it
  back. Written for this exact fault. It keys on whether
  `slint_face_agreement.rs` `include_str!`s the file, so adding a face to that
  test is what makes the lead go away.
- **`one-sided-test`** — a test named for a markup file that never opens one.
  Zero hits is its expected answer; it is a regression guard, not a lead
  generator.

**Its blind spots, both load-bearing.**

`repeated-line` matches bytes, so a rename hides a copy completely. On
2026-09-12 it reported an effect's event-splitting loop in ten files; there
were twelve, and the two it missed had spelled their loop variables
differently and put an `if let` on one line. *A clean `repeated-line` is
evidence that nothing was copied verbatim, which is weaker than it looks: the
copy most likely to diverge is the one somebody edited on the way past.*

And **the lead messages themselves go stale**. `unchecked-face` reported
unreachable faces as the ones declaring their binding and `default-value:` on
separate lines, which was true of the line-based parser it was written against
and stopped being true when that parser became block-based. The real limit had
become "`face_knobs` only walks `ParameterKnob`". A lead is read as a hint
rather than a claim, so a wrong reason can ride in one for a long time.
**Check a lead's stated reason against the test it names before believing it.**

## `grep` for the constant

For shape 4 — a shared constant spelled inline. Pick the Rust constant, grep
its *value* across `crates/mooloop-ui/ui/`, and read every hit.

Do not trust a count you were given. `LOOSE_ENDS.md` said `gain::MIN_DB` was
spelled 26 times across three files. It was 50 across ten, and the first guard
written for it swept only the three files the note named — it would have gone
green with most of the copies still in place. The note's numbers are a place to
start looking, never the extent of the problem.

## `codegraph_explore`

For shape 6 — a table with no reader — and for shape 1, where you need to know
whether two functions have diverged. One call returns the verbatim source of
every named symbol plus its callers, and the **blast radius** line is the one
that matters: `⚠️ no covering tests found` against a symbol that crosses the
boundary is the lead.

This is how `LFO_DESCRIPTORS` and its four siblings turned out to have no
reader outside their own module. A grep would have found the same thing; the
blast radius put it in front of the question rather than behind it.

## Reading every implementor

The one that does not generalise into a search and finds what the searches
cannot. **When a component has one required binding and a dozen instantiations,
read all twelve before believing they differ.**

That is what found the clip lamps: `ChannelMeter` always drew a
`ClipIndicator`, four instantiations existed, and three of them bound nothing
to it. No tool reports that, because nothing is duplicated — the fault is a
*default* that three callers never thought about.

The same habit found `BusDeviceFace` declaring `left-db` and `right-db` with
`main.slint` passing both in and nothing in the component reading either.

## Where to look when you have no lead

In rough order of what has paid off:

1. Any `.slint` file that states a number a `ParamDescriptor` also states.
2. Any pair of functions in different crates that derive the same plan
   (`Session::console_plan` against `RenderState::install_console` is an
   outstanding one).
3. Any `global` in markup that mirrors a Rust `const` block.
4. Any widget property with a default that a caller is expected to override.
