# Action Registry

Status: active design contract, September 2026.

## Why this exists

`ENHANCEMENTS.md` names the goal directly: keyboard shortcuts, a future
console (Quake-style command entry), a future MCP server, and eventually
node-based devices that pass control data around, should all be surfaces
over the *same* underlying set of operations — not each grow their own
bespoke wiring to the same internal state. This document is that contract.
The eight shortcuts Adam originally asked for are in
`docs/archive/SHORTCUTS.md`; all eight exist, so that list is history and
this is the live rule.

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

### An id outlives its label

`view.pane-source` is labelled **Show Devices** and reveals the `DEVICES`
view. The id was not renamed with it, and that is the rule rather than an
oversight: a user's rebindings are stored against the id, so renaming one
silently drops whatever they had bound to it. Change the label, leave the id.

`Ctrl+1`..`Ctrl+5` also stopped meaning "switch a particular pane to this
page" on 2026-09-08 and started meaning "reveal this view, wherever it lives",
without the table changing at all — which is what the ids had said all along.

**A shortcut that reaches into a widget's internal state is a bug, not a
shortcut** — this line is inherited from `FOCUS.md`'s original framing of
the command layer, and applies equally to any future console/MCP command.

## What's registered today

`actions.rs`'s `ACTIONS` table is the source of truth; read it rather than
this document for the current list. **It holds 68 actions in 12 categories**
as of 2026-09-19, and a test in `actions.rs` reads that sentence and fails if
either number stops being true.

This sentence has been wrong twice. On 2026-09-08 it said 46 where the table
held 45; it was corrected to 47 on 2026-09-12, and the table held 49 by then.
Both corrections were made by counting, and counting is what went wrong both
times — which is why the third fix is a test rather than a fourth count.

The categories are:
Transport (play/pause on Space, stop on Shift+Space, return-to-start on Home,
the song loop on L, and arming MIDI recording), File, Edit (undo/redo, the
three contextual clipboard verbs, select-all and delete), Navigation (the
four arrow keys — transpose lives there now, because the same key picks a
channel or walks the browser tree when the roll is not where you are), Notes
(the five pointer tools on keys 1-5 and the snap toggle on 6), View
(revealing a view, splitting the top pane on Ctrl+\\, zooming a pane to the
window on Ctrl+Shift+\\, and piano-roll zoom), Channel (add, remove, clone,
mute, solo), Track (solo on Ctrl+Shift+M and mute on Ctrl+Alt+M, plus moving
the track one seat left or right, all aimed at the track the rack is editing),
Device (the clipboard's four on Ctrl+Shift+C/X/V/D, plus bypass, remove,
wrap in a container, save a preset, and stepping the selection along the
chain), Browser (focus it on Ctrl+B, then Enter and Ctrl+Enter), Pattern
(including lengthening and shortening the pattern by a beat, on
Ctrl+Shift+= and Ctrl+Shift+-), and MIDI (arming controller mapping). Six
entries are registered with no default chord and are listed so they can be
bound. `pattern.clear` has none because every nearby Pattern action already
claims a Ctrl+modifier combination. `channel.solo` has none for the same
reason one step over: Ctrl+M is the channel's mute beside it, and
Ctrl+Shift+M and Ctrl+Alt+M are the track's solo and mute. `track.move-left`
and `track.move-right` have none because the Ctrl+Shift and Ctrl+Alt arrows
sit beside the roll's nudges, and a rarely-used move is not worth a chord
that close to transposing. They also have rows in the Track menu, which greys
them from the same predicate (`Session::can_move_track`) that decides whether
the chord fires. `transport.record-arm-toggle` and `midi.learn-toggle` have none
because both were toolbar-only until 2026-09-19 — added to the registry so
they can be bound and appear on the Shortcuts page, not because either ships
with a default binding.

## Scope: where a chord applies

A chord resolves to exactly one action id. `ShortcutTable` is a
`HashMap<KeyChord, &'static str>` and cannot be anything else, so "Ctrl+C
means three different things" is **one action that asks what has focus**,
not three actions sharing a chord.

Every `ActionSpec` therefore carries a `Scope`, and Preferences > Shortcuts
draws it in a Context column beside the chord — blank for the global
majority, so the column marks the exceptions rather than restating the rule
sixty-five times. The scopes:

| Scope | Column reads | Means |
| --- | --- | --- |
| `Anywhere` | *(blank)* | Fires wherever focus is. The default. |
| `Focused` | Focused panel | Resolves against `Surface`, below. |
| `Notes` | Piano roll | Only while the roll is the visible editor. |
| `Rack` | Selected device | Only with a device selected. |
| `Track` | Edited track | Only while the rack is editing a track. |
| `Browser` | Browser | Only while the browser holds the keyboard. |

A scope is documentation *and* a promise: an arm that ignores its own scope
is a bug, and the keyboard is held to the same condition the matching menu
row is enabled by — Select All Notes is live only on the roll in both.

### The focused surface

`Surface` (`actions.rs`) is which panel a `Scope::Focused` action points at:
`Channels`, `Notes`, `Rack`, `Browser`. It crosses into `main.slint` as the
string property `focused-surface` rather than an index, so neither side
spells a number; `focused_surface_names_match_the_markup` holds the two
spellings together.

Two rules decide it, and the order matters:

1. **The roll wins whenever it is on screen with something selected.** That
   is the rule the clipboard chords have shipped with since 2026-09-07, and
   a user who has just dragged a marquee is not thinking about the browser
   row they opened before it.
2. **Otherwise it is where the last click was**, written from Rust in the
   handlers a click already round-trips through — `channel-selected`,
   `device-selected`, `source-select-toggled` — and from the markup for the
   browser's own rows and tabs.

`Channels` is the fallback, not a fifth state meaning "nothing". Before this
existed the clipboard chords meant the channel unconditionally, so a surface
nobody has clicked behaves the way it did then, and **nothing a user relied
on changed shape**. There is deliberately no Escape-to-nowhere: the way back
to the fallback is selecting a channel, which is the ordinary thing.

Seven actions are `Focused`: `edit.cut-channel`, `edit.copy-channel`,
`edit.paste-channel` (ids unchanged, per the rule above — what they *mean* is
the focused panel's clipboard), and the four `notes.nudge-*` arrows, which
nudge on the roll, walk the tree in the browser, and pick a channel
otherwise.

### Why the Device actions keep their own chords

They were on Ctrl+Shift+C/X/V/D because the dispatcher did not know what had
focus. It does now, and the bare chords reach the same three verbs in the
rack — but the explicit four are kept rather than retired, because reaching
the rack's clipboard *from the roll* is worth four bindings, and anyone who
disagrees can clear them from Preferences without a code change. The
one-clipboard question `docs/plans/archive/interface-iteration/02-device-clipboard.md`
raised is still open and is still not this: three tagged clipboards behind
one focus model is the part that had to exist first.

## How a new action is added

1. Add one `ActionSpec` entry to `actions.rs`: id, label, category, and a
   default `KeyChord` (or `None` if it shouldn't ship with a default
   binding). Use `action!` if it fires anywhere and `scoped_action!` if it
   does not; `Anywhere` is the macro default, so **forgetting the scope is
   the silent failure**, and `every_contextual_action_declares_its_scope`
   pins the set that must not be global.
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

### The Super key is read before the chord is matched

`UiSettings.shortcuts.super_key` (Preferences > Shortcuts, *Modifier keys*)
says what an event's Super/Meta flag means: **Separate keys**, the default
and what shipped before 2026-09-20; **Super acts as Alt**, where either key
presses an Alt chord; or **Swap Alt and Super**, where the two exchange
places. It answers two opposite complaints with one control — a desktop
whose window manager eats Alt leaves Super as the only modifier an
application can reach, and a keyboard with the two transposed wants them
back the other way round.

`SuperKeyMode::read` is applied by `KeyChord::from_event`, which is the only
way a key event should become a chord, at the two places one is made: the
dispatcher in `lib.rs`'s `on_shortcut_key` and the prefpane recorder in
`on_preferences_shortcut_rebind_key`. It is deliberately **not** inside
`ShortcutTable`, so the table holds nothing but canonical chords: changing
the mode leaves every stored binding and every row of the prefpane exactly
as it was, and changes only which physical key arrives at them. A chord
recorded while Super means Alt is written down as the Alt chord it will be
pressed as, which is why the two sites share one reading rather than each
having its own.

The roll's drag modifiers (`gestures.rs`) are deliberately outside it. They
are matched in the markup, against a table of booleans Rust publishes, so a
reading applied on the Rust side would not reach them -- and they do not need
one: Meta is already one of the eight combinations a gesture role can be
assigned, on the same preferences page. A desktop that eats Alt moves the
role onto Meta; the keyboard, whose chords are fixed by the registry rather
than picked per action, is the half that needed a setting.

**Two ladders written by hand and checked against nothing is how a shipped
action stayed dead for a week.** `transport.loop-toggle` landed on a bare L
on 2026-09-07. The registry held it, the prefpane drew it, `ShortcutTable`
resolved it — and no L ever arrived, because the root ladder forwarded six
digits and nothing else unmodified, and the recorder refused an unmodified
key outright, so it could not even be rebound to something that worked. Every
test was green throughout, because every test asked the registry what it held
rather than asking the markup what it could deliver.

Both ladders end in a catch-all now, and `actions.rs`'s `decoding` module
asks the markup: `every_default_chord_reaches_the_dispatcher` fails if a
registry default is a chord `main.slint` cannot produce, and
`the_recorder_decodes_what_the_dispatcher_does` fails if the two ladders name
different keys. They scrape the real `.slint` files rather than mirroring a
table here, because a mirrored table would be the third copy of the thing
that already drifted twice. Escape is the one asymmetry, and it is a
decision: it cancels a capture, so the recorder can never hand it back as a
chord, and the test asserts nothing in the registry binds it.

Ctrl+H, Ctrl+I and Ctrl+J are unreachable, whatever the registry says: their
control codes are Backspace, Tab and Return, and those branches take them
first. `CTRL_UNREACHABLE` is that sentence in a form that can fail.

No F-keys are used for default bindings, by product decision
(`docs/archive/SHORTCUTS.md`).
