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

*Done 2026-09-04:* he ran it, found the two faults the second pass below
records, and confirmed the rebuilt binary works. Merged.

## What was crossed, and what to click — 2026-09-04

The single pass added, in `main.slint`: `preset-options: [string]` on
`EffectSlotRow`, and two callbacks, `effect-preset-selected(slot, index)` and
`save-effect-preset-requested(slot)`. `DeviceFrame` in `device-rack.slint`
grew `preset-enabled`, `preset-options`, `preset-selected` and
`save-preset-requested`; the source face leaves them off and keeps its own
preset field in the editor header. No new panel: the two rail buttons that
had been sitting disabled since the shell was drawn (`⌑` save, `▱` load) are
what got wired.

To try it:

1. Insert any effect. On its **left rail**, `▱` opens the presets for that
   kind — every kind ships a Factory bank, so the menu is never empty on a
   fresh install.
2. Pick one. The row's knobs move; **Undo** puts the previous settings back,
   bypass and trims included.
3. Turn a knob, press `⌑`, name it. It appears in that kind's menu and in
   `presets/effects/<kind>/` under the config directory, ahead of the
   Factory entries because an empty category sorts first.
4. Open the rack on a **bus** and do the same: a bus row saves and loads the
   same way.
5. Open the save dialog from row 1, cancel it, and instead drag row 1 to
   row 3 with the dialog open — the save lands on the device you opened it
   from, not on whatever is now in row 1.

The `→` and `←` placeholders were removed rather than left inert;
`00-status.md` says why.

## Second pass — 2026-09-04

Adam found that presets did not load when chosen. The cause and the fix are
in `00-status.md`; the short form is that the load was routed through the
document pipeline, which exists to decode samples off the UI thread, and an
effect preset has none. It is now a synchronous rack edit like add, remove
and reorder.

Two more properties crossed in the same pass, which is why they are here
rather than in a step of their own: `preset-name` on `EffectSlotRow`, and
`preset` on `DeviceHeader` by way of `preset-name` on `EffectDeviceShell`.
A row that was loaded from — or saved as — a preset now reads
`Filter | Telephone` in its header. The label follows the device through a
reorder and dies with it, and it deliberately survives a knob move: it says
where the settings came from, which stays true after they are adjusted.

It does **not** survive undo. Undo restores the project, and the label lives
beside the session rather than inside `EffectSlotState`, which is `Copy` and
is exactly what the preset bundle stores. Putting it in the snapshot means
threading it through `ProjectEdit`, which is more machinery than a label is
worth; if it starts to grate, that is the fix.


