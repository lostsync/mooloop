# 03 — the automation panel

The playlist gets a panel under its pattern rows. You can add a song lane
there, draw points on it, and hear it. This is the plan's first audible
milestone. Editing is the piano roll lane's gestures for now; step 06
replaces them.

## Adam sees a mock-up first

This is the shape every later step builds on, so draw it before wiring it.
Use the mock-up catalogue or `scripts/slint-sketch` at the playlist's **real**
size: the default dock height is 376 px (`main.slint:1582`).

Post it on this step's Linear issue with the `Question` label. Show:
- the divider;
- two track groups, one folded;
- a channel's lanes under its track;
- three lanes of different heights.

Carry on with the Rust half (below) while he looks. Build the markup only
when he has answered.

## Layout

The playlist's `ViewSlot` (`main.slint:7239`) today is one `ScrollView` over
both axes, with nothing pinned (`:7352`). It becomes, top to bottom:

1. The pattern rows, as now. Their header strips and rows keep their
   gestures (`UI_DESIGN.md`, *Playlist gestures*).
2. **A divider.** Drag it to move the boundary between rows and panel.
   - Copy the dock grip's idiom (`:7915-7967`): re-anchor on the grab, clamp,
     `ns-resize`, and `layout-changed()` only at the end of the drag.
   - The height is saved with the layout, like `playlist_dock_height`
     (`ui/src/settings.rs:1098`).
   - A panel with no lanes collapses to a header strip with an **Add lane**
     button.
3. **The panel**, with a vertical scroll of its own.
   - Its x follows the rows: `x: -<rows scroll>.content-x`, the way the
     roll's lanes follow the roll (`:6940`).
   - It has a 104 px header column, so lane names line up with the pattern
     row headers.

## Groups

Lanes are grouped **by track, then by device**. Adam: *"for the song
automation it needs to group by track, then by plugin"*.
- **A track group**, in mixer order with Master first, holds:
  - the track's own **strip** (fader, pan; step 04 makes them play);
  - its **inserts**;
  - then each **channel that feeds the track**, as a sub-group holding that
    channel's source, inserts and strip.
- A channel routed to Master sits under Master.
- A group folds from its header. Fold state lives in the UI for the session
  and is not saved, until someone asks for it to be. It is not a document
  edit and records no undo.
- Only groups with at least one lane are drawn. **Add lane** is how a group
  appears.

The grouping is computed in Rust from the project, one model row per group
header or lane. The markup draws rows and doesn't compute structure. That is
the boundary rule in `docs/workflows/rust-slint-boundary/`.

## A lane

- **The header** (104 px column) shows:
  - the parameter's name and its device;
  - the value under the playhead, formatted by the parameter's own
    `format_param_value`;
  - a remove ✕ from the icon registry (`icons.slint`).
- **The body** draws the line and the points on the playlist's tick axis.
  - **The line's ends land on the points' centres at every zoom.** That is
    MOO-467's defect in the roll's lane. Write the snapshot test that would
    catch it before drawing anything.
  - The line holds flat from the song's start to the first point, and from
    the last point to the song's end, because that is what `value_at` plays.
- **Editing**, for now: the roll lane's gestures (`main.slint:7170`). Click to
  add, drag to move (snapped, Shift for free), right-click to remove. Each
  edit is a step 01 verb. Step 06 replaces this with selection and a context
  menu.
- **Height** is a fixed 72 px until step 07.

## Adding a lane

**Add lane** in the panel header opens the picker. For this step, it's the
existing flat grouped list (`AutomationLaneMenu`, `main.slint:572`) fed by a
new `Session::song_automation_destinations()`. Step 05 turns the list into a
cascade.

That function lists **every** channel and track, not just the selected
channel as `automation_destinations` does (`session.rs:800`). Its order is
the group order above, and plugin parameters are merged as
`plugin_ui::lane_destinations` does (`ui/src/plugin_ui.rs:1042`).

Rows in the menu are wired **callback first, close after**
(`scripts/dupe-audit popup-close-order`).

## Done when

- The playlist has a panel under its rows, with a divider. The divider's
  position survives a restart, and `tests/dock_resize.rs` gains a case for it.
- A song lane added from the panel's picker, with points drawn on it, is
  heard in Song mode. This is checked in the running app, driven over
  `scripts/mooloop-mcp`, not just in tests.
- **Groups:**
  - Lanes group by track, then by channel and device, and fold.
  - A track move or channel reroute regroups them.
  - Undo of a lane edit leaves the panel showing the same lanes.
- **The line test:** a snapshot test at the panel's real size puts points on
  the first and last grid line at two zooms and checks the line's ends
  against the dots.
- **Navigation sends nothing:** scrolling the panel or folding a group sends
  nothing to the engine (`scripts/dupe-audit navigation-sends` stays at zero).
- **`docs/CURRENT.md` and `docs/UI_DESIGN.md`** describe the panel.
