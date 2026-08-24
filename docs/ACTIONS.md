# Action Registry

Status: active design contract, August 2026.

## Why this exists

`ENHANCEMENTS.md` names the goal directly: keyboard shortcuts, a future
console (Quake-style command entry), a future MCP server, and eventually
node-based devices that pass control data around, should all be surfaces
over the *same* underlying set of operations — not each grow their own
bespoke wiring to the same internal state. This document is that contract.

## The rule

Every operation a shortcut, a menu row, or (eventually) a console/MCP
command can perform is a named **action**: a stable string id
(`"transport.play-pause"`, `"pattern.clone"`, `"view.pane-next"`) registered
once in `crates/mooloop-ui/src/actions.rs`, with a human label, a category,
and a default key chord. A surface never reaches into a widget's internal
state to perform an edit; it resolves to an action id and dispatches, the
same way every existing surface already does for undo/redo/cut/copy/paste
(`edit-command-requested` in `main.slint`, handled in `lib.rs`) and now for
the full action set.

**A shortcut that reaches into a widget's internal state is a bug, not a
shortcut** — this line is inherited from `FOCUS.md`'s original framing of
the command layer, and applies equally to any future console/MCP command.

## What's registered today

`actions.rs`'s `ACTIONS` table is the source of truth; read it rather than
this document for the current list. As of this writing it covers transport,
file, edit, view (pane switching and piano-roll zoom), channel, and pattern
operations — the set `SHORTCUTS.md` asked for, plus the shortcuts that
already existed before this registry (Ctrl+O/S/Z/etc.), migrated in so the
Preferences > Shortcuts page is a complete, reassignable list rather than a
partial one.

## How a new action is added

1. Add one `ActionSpec` entry to `actions.rs`: id, label, category, and a
   default `KeyChord` (or `None` if it shouldn't ship with a default binding).
2. Add one match arm in `lib.rs`'s `on_shortcut_key` dispatcher, calling
   whatever already performs that operation — usually an existing
   `window.invoke_*()` for a callback a menu row already calls. If the
   operation doesn't exist as a callback yet, add it the normal way (a
   `callback` in `main.slint`, handled in `lib.rs`), then reference it here.
3. That's it. The Preferences > Shortcuts page, conflict detection, and
   persistence all come from the registry automatically — none of them
   enumerate actions by hand.

## What this is not, yet

The registry only has one real dispatcher today: keyboard shortcuts
(`root.shortcut-key` in `main.slint`). The menu bar still calls its own
callbacks directly rather than routing through action ids — safe, because
those are exactly the same callbacks the keyboard dispatcher invokes, so
keyboard and menu already agree on effect. A console or MCP surface would
need `on_shortcut_key`'s match arms (or the ids they dispatch to) exposed as
a callable-by-id lookup rather than an inline match; that refactor is
deliberately deferred until there's a second real consumer of action ids,
per the working discipline in `FOCUS.md` ("vertical slices stopped one step
short of the payoff" is the named failure mode to avoid — but so is building
the second consumer before anything asks for it).

## Key chords

`KeyChord` (`actions.rs`) is a modifier set plus one canonical key name.
Chords are matched and displayed as text (`"Ctrl+Shift+Z"`), parsed back by
`KeyChord::parse`, and persisted as per-action overrides in
`UiSettings.shortcuts.overrides` (only entries that differ from the
registry default are stored). Ctrl+letter combinations decode through a
mechanical, written-once branch in both `main.slint`'s root `FocusScope` and
the Shortcuts page's per-row capture `FocusScope`, because at least one
windowing backend delivers `Ctrl+<letter>` as the raw ASCII control code for
that letter rather than as plain text with a modifier flag — see the
comments at both call sites before touching either. Adding a new
Ctrl+letter *action* never requires touching that decode branch; only a
genuinely new *key* (one not already decoded) would.

No F-keys are used for default bindings, by product decision (`SHORTCUTS.md`).
