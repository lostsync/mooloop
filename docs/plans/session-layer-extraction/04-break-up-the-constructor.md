# 04 — Break up `UiState::new`

Read `00-status.md` first. Step 03 must be in.

This is the plan. Everything before it clears the way and everything after it
collects the winnings.

## What is wrong today

`UiState::new` runs from `lib.rs:4370` to `lib.rs:12078`. Seven thousand seven
hundred lines, one function, 187 callback registrations. Every one of them is a
closure that captures `Rc<RefCell<UiState>>` and a `MainWindow`, and most of
them contain real edit logic inline.

A representative one, from around `lib.rs:5000`, resolves the export scope from
session state, maps two integer menu indices onto an `ExportFormat`, sets four
window properties, and spawns a thread — all in one closure body. The scope
resolution and format mapping are session logic. They are unreachable from
anywhere else and untestable by anything.

The cost is not aesthetic. It is that no edit the application performs can be
called twice — once from a menu and once from a shortcut — without duplicating
it, and none of it can be tested at all.

## The transformation

For each callback, mechanically:

1. Name what the closure *does* as a `Session` method taking plain arguments and
   returning plain data or a list of engine commands.
2. Move the body there, minus the `window.set_*` calls.
3. Leave the closure as an adapter: read what it needs from the window, call the
   session method, apply the result to the window through the step 03
   projections.

The end state is that a closure is a handful of lines and contains no decisions.

## Work it by area, not by line range

A single commit moving 7,700 lines is not reviewable. Split by UI area, each its
own commit, each leaving the application working:

| Area | Roughly | Notes |
| --- | --- | --- |
| Document | open, save, save-as, export, quit, quarantine | Highest value: this is where `dirty`, `bundle_path` and undo interact, and where the zenity helpers from step 01 already landed. |
| Transport & pattern | play/stop, tempo, swing, pattern select, pattern length, song mode | Small and self-contained. Good first commit. |
| Channel rack | step edit, tools, channel add/remove/reorder/clone, mute, volume, pan | `retarget_effect_slots` is the risk here; `mooloop_core::structure` already owns the permutation. |
| Piano roll | note create/move/resize/scale, selection, marquee, clipboard | The most logic per callback. `ScaleBase` and the selection sets already moved in 02 and 03. |
| Sampler & browser | load, slice, stretch, commit, markers, browser navigation | Bounded by the step 01 moves. |
| Device rack & effects | add, remove, reorder, bypass, parameter edit | |
| Modulation | source add/remove, route arm/assign, depth drag | Gesture-scoped undo lives here (`modulation_edit_before`). |
| Mixer & buses | strip select, bus assign, bus editor | |
| Preferences & appearance | theme, audio config, shortcuts, gestures | Mostly already separate in `settings.rs`. |

Start with transport, because it is the smallest complete instance of the
pattern and it establishes the shape the other eight follow.

## Two things to hold onto

**Gesture tokens.** `CommandState::gesture` is what collapses a pointer drag
into one undo entry. The token is currently stamped inside closures. When the
logic moves, the token must move with it — a session method that performs a drag
frame takes the gesture as an argument rather than inventing one. Losing this
turns every drag back into twenty undos, and it will not show up in a build.

**Ordering.** The six sender types (`lib.rs:147-206`) all wrap one
`mpsc::Sender<PendingEngineMessage>` precisely so that commands from different
closures stay in one ordered stream. A session method that emits commands must
emit them into that same stream in the same order. Do not let extraction turn
one ordered channel into several.

## Definition of done

- `UiState::new` is under a few hundred lines and registers callbacks that
  contain no decisions.
- Every extracted edit is a named `Session` method that could be called from a
  menu, a shortcut, or a test.
- Behaviour is identical, including undo granularity.

## Verification

Per area, not once at the end: build, run, and exercise that area by hand.
Undo granularity on drags is the specific thing to check and the specific thing
a build will not tell you about.
