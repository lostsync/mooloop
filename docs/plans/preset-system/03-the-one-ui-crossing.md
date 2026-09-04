# 03 — The one UI crossing

Read `02-where-they-live.md` first. Steps 01 and 02 are free to iterate on;
this one is not, so it is arranged to be crossed once.

## Why this step is shaped differently

`AGENTS.md`'s "Order device work so the face contract comes last" applies
directly. A new property or callback crossing `main.slint` and
`mooloop-ui/src/lib.rs` costs 30 s to check and 8.7 minutes to a release
binary. Steps 01 and 02 cost about a second each.

So: **do not cross into `main.slint` until every property and callback this
step needs is known, and then add them all in one pass.** Write the list down
before touching the file.

Expect roughly:

- a callback to open the save dialog for a rack row, carrying the slot;
- a callback to load a named preset into a rack row;
- a model of the presets available for the row's effect kind;
- whatever the row header needs to show the entry.

## The surface

A rack row header gets a preset entry: save the current settings, or load one
of the presets for that row's kind. That is all. The row already has a header;
this is an addition to it, not a new panel.

`scripts/slint-sketch` type-checks against the real widgets in about 0.05 s.
Iterate the row header's appearance there, not through a build.

## Deliberately not in this step

`00-status.md` puts these out of scope and they stay out:

- **A preset browser.** The flat-list taxonomy was fixed structurally in step
  01 by widening `PresetSummary`; the browser that would show it is a
  different piece of work and it wants DS-01's bank to design against.
- **Tags, ratings, search.** `PresetInfo` already carries `category` and
  `tags`; nothing has to read them yet.
- **Sharing or importing preset packs.**

If the row header turns out to need a scrolling list to be usable, that is
the browser arriving through the back door. Stop and write it down rather
than building it at 2 a.m.

## Verification

This is the step that needs the box.

- `cargo check -p mooloop-ui` after the single crossing — **backgrounded**.
- The relevant UI snapshot for the rack row, software-rendered, per
  `docs/AGENT_OPERATIONS.md`.
- Rung 4 once, before committing.

Background every one of them and keep working; the harness notifies on exit.
Do not poll the output file — a run piped through `tail` writes nothing until
it finishes, so polling reads an empty file and learns only that time has
passed. This is measured: see `docs/plans/edit-loop/00-status.md`.

## Done when

A rack row can save its settings as a preset and load one back, through the
real window, and the workspace is green.

**The listening pass is Adam's and is not part of this step.** Leave the
branch unmerged with a note saying what to click.
