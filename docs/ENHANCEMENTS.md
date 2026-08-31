Anything that can be muted should also be able to be soloed.


Piano roll:

Piano roll in general feels like its from 1994 kinda. It does work. It can do what you need. But it's limited

Grid lines in the piano roll - need to be at least bold on every 1beat. ideally this is configurable easily from within the piano roll view

Smart grid on piano roll tied to zoom level? 

Big one: Select multiple notes and drag, Edit (c/c/p) via keyboard shortcut or menu. If we could do keyboard based selection somehow that would be cool, like using the arrow keys or vim keys or something -- honestly being able to navigate this whole app by keyboard should be getting a lot more attention.
  DONE, mouse half: marquee select, and a selection that moves, resizes, and
  scales as one object. Still open: cut/copy/paste of notes, and keyboard
  selection and navigation.

Standard piano roll pointer tools? Select/normal, draw mode, slice/heal (heal with modifier i think?)
  DONE: Select, Draw, Paint, Slice, Erase on keys 1-5, with heal/join as
  Slice plus the add-to-selection modifier.

Axis-constrained note drag (lock to time-only or pitch-only) is the one
standard gesture deliberately left out. Every conventional binding for it is
Alt, which is the chord you flagged as WM-hostile, and shipping it bound to
nothing would be a dark feature. The gesture registry in `gestures.rs` is
where it goes once there is a key worth giving it.

Tooltip audit:

This has to happen. Almost every tooltip in the whole application should just show up in the app's statusbar. Tooltips are not code comments for users. This should be the general rule:

- If you want to explain something, use the message area in the statusbar
- If you want to report the name of a control and its current state, tooltip.

Examples:

A mixer fader: tooltip would probably just read the fader's setting, e.g. "-5.9 dB". the statusbar could say something like "Drag to change. Ctl+drag for fine control." (assuming that's true). 

Mute button: tooltip: (Un)Muted, statusbar: Item mute. Toggle with click. Mutli-select with Ctl+click (which we can't do at this time, but for example)

Keyboard shortcuts:

I feel like we probably should have paid more attention to this from the beginning. Ideally, I'd like to have super robust action and shortcut support a la REAPER. We'll have to make our way there, I suppose. Partly, I feel like it would have been a good piece of foundation for some of the more advanced stuff I want to do because we would have been setting up an action engine, could've made a console for that engine, then if you build nodes that use the engine and let them pass control and audio data...pretty much kinda have max/msp, reaktor, bidule, etc. we were supposed to have built this with some sort of passing awareness that maybe an MCP server would be cool, or at least useful during dev. that'd run off of the same underlying system, i'd think. 

At this point, minimally, we just need to add the ability to configure some common keyboard commands in the prefs page.

GUI focus issues:

There seems to be something in slint where there's sort of an input caret. in the first few version of this app, every control had to be clicked once to select and again to use it. that's not really an issue now, thank god, but that select caret can still intercept e.g. spacebar for play/pause in some situations. i should probably note them when they occur. I do think keyboard navigation is important so i see the need for such a caret, but we'll have to be intentional and thoughtful about how we use that so that we aren't blocking key commands that should work wherever you are.

General application design:

GUI:

we're ending up with a fair number of panes. currently the layout is totally static. we might want to allow the user to customize their layout.

on a 1080p monitor, in a 16 step pattern there is plenty of room to the right of the seq steps for us to split the pane and have the playlist seq beside it. this is what made me think we might want to kinda ape REAPER's dockable dialogs thing.

In appearance prefs, we should be able to set up different shading options. Like, right now i think every 4th step in the step seq is brighter. Let the user configure that by setting a pattern. Maybe I want to brighten every 3rd step, or 6th. Maybe I want 8 bright, 8 dark, 8 bright, 8 dark. Maybe this shading could extend to the piano roll's grid? It would help a lot with editing sequences.
