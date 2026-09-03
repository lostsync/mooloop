# 02 — Move the plain data types

Read `00-status.md` first.

Step 01 moved free functions. This step moves the types the session is made of.
Like 01, these are already toolkit-free; unlike 01, they have callers throughout
`lib.rs`, so the diff is wide even though the change is shallow.

## What to move

Into `mooloop-session`, unchanged:

| Type | `lib.rs` | Note |
| --- | --- | --- |
| `ChannelState` | 586 | The big one. Already has no Slint in it or in its `impl`. |
| `LoadedSample` | 708 | Result of a background load. |
| `LoadResult` | 716 | |
| `ResolvedDocument` | 726 | |
| `ChannelClipboard` | 735 | |
| `ProjectSnapshot` | 743 | `Project` plus sample handles — the undo unit. |
| `CommandState` | 752 | Clipboard, history, gesture tokens, pending edit. |
| `HistoryMove` | 828 | |
| `ProjectEdit` | 837 | |
| `DocumentProblem` | 848 | Plus its `From` impls. |
| `ScaleBase` | 1103 | Piano-roll scale-drag geometry. |

Move with them the functions that operate purely on these types and the project
model: `quarantine_song`, `fresh_starter_seed`, `copied_channel_name`,
`snapshot_channel_clipboard`, `normalize_project_pattern_banks`,
`resolve_document`, `apply_sample_references`, `warning_suffix`,
`repair_suffix`, `log_repairs`, `format_bars`, `parse_typed_value`, and the
`stretch_*_to_norm` / `stretch_*_from_norm` pairs with `measured_loop_bars`.

## What stays behind, and why

- **`Pane` (777) and `apply_pane` (794).** `Pane` itself is plain, but it exists
  to describe which Slint properties are set, and `PANE_CYCLE` is a view
  concern. Leave it; revisit in step 03 if the session turns out to need it for
  shortcut dispatch.
- **`DocumentResult` (921).** It carries `SharedString` in its failure arm.
  Either move it with a plain `String` and convert at the boundary, or leave it.
  Prefer moving it — the conversion is one line and the type is genuinely
  session-level.

That second case is the pattern to watch for throughout the rest of the plan: a
type that is ninety percent portable with one `SharedString` in it. The rule is
that the session speaks `String` and the view converts. Never the reverse.

## The trap

`ChannelState` is referenced from a large fraction of `UiState`'s methods and
from most of the 187 callbacks. Moving it will produce a very large mechanical
diff and a long tail of import fixes, and it will be tempting to "tidy while in
there." Do not. A refactor whose diff cannot be read as "same code, new module"
loses the only verification this plan has.

## Definition of done

- The listed types live in `mooloop-session` and are `pub`.
- `mooloop-session` still does not depend on `slint`.
- The application builds and behaves identically.

## Verification

Full build plus a manual pass over document open, save, and undo — the three
paths these types are load-bearing for. `docs/AGENT_OPERATIONS.md` for how to
run the app.
