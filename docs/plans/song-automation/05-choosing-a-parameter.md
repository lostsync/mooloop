# 05 — choosing a parameter: a cascade

Adam: *"selecting the param from the dropdown is super cumbersome. there
should be cascading menus"*. Today's picker is one flat, scrolling list,
grouped by device (`AutomationLaneMenu`, `main.slint:572`). A song can name
every parameter of every device on every channel and track, so that list
gets long.

## The cascade

The cascade goes **track → device → parameter**. A channel's devices sit under
the track it feeds, as in the panel's groups (step 03). So the second column
holds the track's own devices, then each feeding channel's devices under the
channel's name.

**Build it as columns inside one popup, not as nested popups.** A hand-built
`PopupWindow` swallows hover outside itself (`menubar.slint:163-175`). A
second popup opened from the first could never take the pointer. With
columns:
- hovering or clicking a track fills the device column;
- a device fills the parameter column;
- clicking a parameter chooses it.

It looks like a cascading menu and needs no submenu machinery. The menu bar
has none (`main.slint:3385`), and this step doesn't add any.

**Keyboard:**
- Up and Down move within a column.
- Right and Left move between columns.
- Return chooses.
- Escape closes.
- Typing filters the current column, as the browser's search does.

**Rows already open** carry the "●" mark today's picker uses. Choosing one
scrolls the panel to its lane instead of opening a duplicate.

**Callback first, close after** on every choosing row
(`scripts/dupe-audit popup-close-order`). This is the fifth menu that rule
would otherwise catch.

The data is Rust's. `song_automation_destinations()` (step 03) is reshaped
into three models, or one model with a depth field that the columns filter
by index. The markup filters and draws; it doesn't group
(`docs/workflows/rust-slint-boundary/`).

## Also from here

- **A knob's context menu** gains **Automate in song** beside **Automate**
  (`controls.slint:1486`, handled at `ui/src/lib.rs:11789`). It opens, or
  scrolls to, that parameter's song lane, and shows the playlist if it is
  hidden. **Automate** keeps opening a pattern lane.
- **The piano roll's lane picker** uses the same cascade when step 09 lands.
  Until then it keeps its list.

## Done when

- **Picking a lane:** Add lane opens the cascade, and three clicks or six
  keys open any parameter's song lane.
  - A test drives the columns through `on_*` callbacks and asserts the lane
    opened.
  - A second test uses only the keyboard.
- **Automate in song** on a knob opens its song lane.
- **The cascade fits** at the playlist's default dock height and at 200% text.
  Snapshot tests cover both; MOO-278 and MOO-328 were rows that didn't fit.
- **`docs/ACTIONS.md`** lists the new menu row if it gets an action id.
