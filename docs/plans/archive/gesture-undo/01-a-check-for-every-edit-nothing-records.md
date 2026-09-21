# 01 · A check for every edit nothing records

**Write this first, against the unfixed tree.** `AGENTS.md` gives the reason
twice over: `bar-arithmetic` was written before its fix and found ten sites
where the survey that commissioned it had found eight, and `navigation-sends`'
first draft reported *nothing* against a tree with two violations — silence
that would have looked exactly like correctness had it been written
afterwards. A check written after this plan lands is shaped by what the author
already fixed.

## What it reports

For every value-reporting callback declared on `MainWindow`
(`callback <name>-changed(...)`, `<name>-edited(...)` — about 110 of them
today), whether the Rust handler that serves it reaches the history. A handler
reaches the history if it calls `record_project_history`, `with_project_history`,
`with_continuous_history`, `queue_structural_edit`, or the general recorder
step 02 adds.

Its expected answer at the end of this plan is **zero**, which makes it a
regression guard rather than a lead generator — the `one-sided-test` and
`navigation-sends` shape — and its count today is this plan's progress bar.

## Where it goes

`scripts/dupe-audit` as a tenth check, named `unrecorded-edit`. It belongs
there rather than in a test for the same reason the other nine do: it reports
leads rather than failures, it needs no build, and CI does not run it. Follow
`unchecked-face`'s structure most closely — it is the existing check that
answers "what is *not* covered" rather than "what is duplicated".

## What makes it honest rather than decorative

Three things, and the first two are the ones a naive version gets wrong.

**It must resolve the callback to its handler.** `window.on_channel_muted(...)`
is the wiring; the recording happens inside the closure. A check that greps for
the callback name and the word `record_project_history` on the same line finds
nothing. Match the `on_<callback>(` wiring site, take the closure body to its
closing brace, and look inside it.

**Not every callback is an edit, and the exemptions must be named in the
check, with reasons.** `navigation-sends` is the model: it sanctions exactly
one send and says why in the allowlist rather than leaving a reader to
rediscover it. Selection callbacks change what is drawn, not what is saved
(`MOO-57`'s whole rule); view and zoom callbacks likewise; transport,
record-arm, input monitoring and seek are the four that
`EngineCommand::edits_document` already declares are not edits, and the check
should read that list rather than keep a second copy of it. **A check that is
never clean stops being read.**

**It must be validated against a tree where it finds something it did not
already know.** Run it at `4bef6dc~1` — before the eleven mixer verbs were
recorded — and confirm it reports those eleven. If it does not, it is not
reading handlers properly, and the zero it eventually prints would mean
nothing.

## Done when

- [ ] `scripts/dupe-audit unrecorded-edit` lists every value callback whose
      handler does not reach the history, with the file and line of the
      wiring site.
- [ ] Its exemption list is written down with a reason per entry, and takes
      the "not an edit" four from `EngineCommand::edits_document` rather than
      restating them.
- [ ] Run at `4bef6dc~1` it reports the eleven mixer verbs; run at `HEAD` it
      does not.
- [ ] `scripts/dupe-audit --list` describes it, and `AGENTS.md`'s duplication
      section gains the paragraph the other nine checks each have — what it
      looks for, what it deliberately does not, and what it was validated
      against.
