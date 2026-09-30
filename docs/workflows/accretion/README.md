# Accretion

A function edited once per problem gains a block, a field, a condition or a
paragraph per edit, and nothing is ever taken back out. Each addition was the
smallest change that fixed the thing in front of it. The sum is a function
that no longer says what it does in one place, and the next edit has to
guess where to add itself.

This is the shape that finding-per-failure reviews miss. Every line reads as
reasonable, no single input fails, and the tests that each patch brought
with it are green. The 2026-09-30 team review found about ninety issues and
none of them was this, because its brief required a concrete failing input
for every finding.

## What it looks like here

Not the textbook shape. The example that commissioned this workflow
(`scripts/fixtures/accreted_fn.rs`) overwrites one local 29 times in 19
top-level `if`s; on 2026-09-30 no function in the workspace overwrote a local
more than 3 times. mooloop's code accretes in these four ways instead, each
first seen in `RenderState::carry_strips_from` (`engine/src/render.rs`):

1. **The field list.** A function that carries, copies, resets or compares a
   struct field by field, where each fix added one more field. The channel
   loop swaps compensation, destination, solo verdict, modulators and a voice
   release, one line and one paragraph each; the track loop below writes the
   same kind of list again with three console fields. The rule deciding
   *which* fields must stay fresh ("derived from the whole project") is
   stated in prose twice and in code twice, and lives in no method on the
   strip. The next derived field somebody adds to `ChannelStrip` will not be
   carried, and nothing will say so.

2. **A predicate restated weaker.** The same question answered twice, the
   second time with less of the answer. `carry_strips_from` decides "same
   instrument" as *same kind, and for a plugin the same slot and identity*;
   twenty lines further on, `kept_sounding` re-derives it as *same kind*. The
   second copy was written for a different fix and never learned about the
   plugin clause.

3. **Comment strata.** A paragraph per fix, each with its own issue id,
   appended rather than folded in: "One whose note the incoming song no
   longer has ... (MOO-99). Also one whose note-off the install moved out of
   reach ... (MOO-383)." The reasons are good and the ids are right; what is
   missing is the one sentence that says what the block is *for* now.

4. **The orphaned doc comment.** A function inserted between another and its
   doc block, so the block describes the wrong thing. MOO-410 found one in
   `modulation.rs`; `session.rs` has another, where `retune_effect`'s summary
   ("One effect's answer to a tempo change") heads `source_params`' doc.

A search cannot find any of these reliably. Tried on 2026-09-30: runs of
field-name-only-differs statements match every UI property-setter list and
miss `carry_strips_from`, whose swaps are broken up by comments; an orphaned
doc block has no textual signature a regex can tell from an ordinary second
paragraph. So the script ranks **where to read**, and the reading is this
workflow.

## Stages

### 1. Find

```sh
git fetch --deepen=3000 origin main        # a shallow clone has no history to rank by
scripts/dupe-audit accreted-fn             # the whole tree, about 13 s
scripts/dupe-audit accreted-fn --under crates/mooloop-engine --under crates/mooloop-dsp/src/sampler.rs
```

`--under` takes path prefixes, so a team's run is its rows in
`docs/TEAMS.md`. The ranking is from history first (fix commits returning,
lines added against lines removed, a body stitched from many commits), then
issue ids stacked in comments, then the textbook shape. A function over 600
lines is marked "size first": `AppUi::new` is a size problem before it is an
accretion one, and splitting it is MOO-102's, not this workflow's.

**The list is where to start, not where to stop.** Read the functions beside
each lead in the same file too; the orphaned doc block in `session.rs` was two
functions below a lead.

### 2. Read the history, then the function

For each lead, before reading the current body:

```sh
git -c core.attributesFile=<(printf '*.rs diff=rust\n') log --format='%h %ad %s' --date=short \
    -L :carry_strips_from:crates/mooloop-engine/src/render.rs | grep -E '^[0-9a-f]{7,} [0-9]{4}-'
```

The attribute matters: the repository sets no `diff=rust`, and without it
`-L :name:` cannot find a Rust function ("no match"). The output is the list
of patches, newest first, and the subjects say what each was for. For
`carry_strips_from` it is eleven commits from 2026-09-17 to 2026-09-30, and
reading them in order is most of the diagnosis: keep the strips, carry the
tracks, keep a ring of the same length, then solo, the instrument box, voice
releases, the Buffer's history, plugins, plugin identity, moved notes. Each
added a case; none restated the whole.

Then read the body looking for the four shapes above, and ask:

- **Field lists:** what decides which fields are in the list? Is that rule
  written down once, as code, where the struct is? What happens to the next
  field added to the struct?
- **Restated predicates:** is any condition computed twice? Does the second
  copy say everything the first does?
- **Strata:** can the comments be replaced by one statement of what the
  function guarantees now, with the history left to `git log`?
- **Dead branches:** is there a guard or a case that an earlier block has
  already made impossible? The fixture's `/ 100` branch runs after `gain` was
  clamped to 1.0 and can never be taken.

### 3. Judge: state the contract

Write, in one paragraph, what the function is for and what it guarantees.
If you cannot, because two blocks disagree about what should happen, you
have found a policy nobody decided, and that is a question for Adam, not a
refactor.

For a function whose result is decided by several flags (the fixture's
`muted`, `soloed`, `preview_mode`...), the paragraph is a **truth table**:
every combination, the output today, and whether it is intended. Rows that
never change or change for no reason are the dead and overridden blocks.

### 4. File

One issue per finding, labelled for the team that owns the function, with the
`accreted-fn` lead line quoted so the next run can see it was read:

| Found | Issue |
| --- | --- |
| A restated predicate that disagrees, or a dead branch hiding a wrong result | `Bug` (with `Triage` unless the source makes it certain). These are real defects and are fixed like any other. |
| A field list whose rule should be one method | `Improvement`, LARGE unless the struct is small. The first step is a **characterization test** pinning today's behaviour, so the consolidation cannot change it silently. |
| Rows of a truth table nobody decided | `Question` on the issue, numbered, with a recommendation (`AGENTS.md`, *Questions for Adam*). |
| Comment strata | Fold them into the contract paragraph in the same change as any fix to the function. Not an issue of its own. |
| An orphaned doc comment | `Improvement`, QUICK: move the block, comments only. |

Do not refactor during the review. The review's output is the issue list, and
a consolidation lands only after its characterization test.

## Brief for a team agent

A team's run is read-only, like any review: no edits, no Cargo. Give the team
agent this directory, its `--under` paths from `docs/TEAMS.md`, and ask for
the report block from the `team-brief` skill plus, per lead read: the lead
line, the contract paragraph (or why one cannot be written), and the issues
filed. A lead read and found clean is worth reporting; say what was checked.

## Runs

**2026-09-30.** Calibration, before any team run. `accreted-fn` was written
against the tree and a synthetic example together, and came back with the
finding recorded at the top of this file: mooloop's code does not accrete in
the textbook shape, so the first draft (overwrites, patch comments) returned
one lead, `AppUi::new`, for the wrong reason. Ranking by history and comment
strata put `carry_strips_from` second of eight, and reading it by hand gave
the four shapes above. Two defects seen in passing and left for the first
team run to file: the weaker `kept_sounding` predicate in `carry_strips_from`,
and `retune_effect`'s orphaned doc block in `session/src/session.rs`.
