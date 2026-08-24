Anything that can be muted should also be able to be soloed.


Piano roll:

Piano roll in general feels like its from 1994 kinda. It does work. It can do what you need. But it's limited

Grid lines in the piano roll - need to be at least bold on every 1beat. ideally this is configurable easily from within the piano roll view

Smart grid on piano roll tied to zoom level? 

Big one: Select multiple notes and drag, Edit (c/c/p) via keyboard shortcut or menu. If we could do keyboard based selection somehow that would be cool, like using the arrow keys or vim keys or something -- honestly being able to navigate this whole app by keyboard should be getting a lot more attention.

Standard piano roll pointer tools? Select/normal, draw mode, slice/heal (heal with modifier i think?)

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
