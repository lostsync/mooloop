# 01 — the field takes the keys

When the browser becomes the focused pane, its filter field gets keyboard
focus, so typing filters straight away.

- Wherever `focus-pane(PaneViews.browser)` runs (a click on a tab or a row,
  `browser.focus` / Ctrl+B, `browser.plugins`, the insert menu's
  "Plugin…"), focus the field. A press on a row must not take a second
  click to act (`tests/first_click.rs`).
- From inside the field, Up and Down move the focused row, Left and Right
  open and close a group when the field is empty, Enter acts on the focused
  row, Ctrl+Enter loads it, Esc clears the filter and then hands the keys
  back to the root scope (`keys`). These go through the same functions the
  root scope reaches (`browser_move_focus`, `browser_activate_focused`, ...),
  not copies.
- Leaving the browser for another pane takes focus off the field, so
  shortcuts work again there.

**Done when** a UI test focuses the browser, types `comp` without clicking
the field, and sees the filter set; Down then moves the row; Esc empties the
filter.
