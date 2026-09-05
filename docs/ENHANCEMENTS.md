# Enhancements

Adam's standing wish list, in his own words. It is not a plan and not
ordered; items leave it by becoming a plan under `docs/plans/` or by being
built. Indented notes are agent annotations recording what has landed against
an item — they are the only additions to Adam's text, and they should be kept
short. Last audited September 2026; eight items added 2026-09-05, in the
block at the bottom.

---

Anything that can be muted should also be able to be soloed.
  STILL OPEN: solo is a button style with nothing behind it. `MIXER_PLAN.md`
  specifies the behaviour (an AFL-style monitor tap, not a routing change) as
  part of its v0.1 pass.


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

PARTLY DONE: the status bar exists, fed by a `hover-hint` property that about
forty sites now set, and it takes priority over the standing status message.
What has not happened is the audit itself — deciding per control which half
of the rule below it falls under, and plumbing the surfaces that were missed
(the sampler face has no `hover-hint` at all, which is why its stretch toggle
cannot explain why it is lit while nothing stretches).

This has to happen. Almost every tooltip in the whole application should just show up in the app's statusbar. Tooltips are not code comments for users. This should be the general rule:

- If you want to explain something, use the message area in the statusbar
- If you want to report the name of a control and its current state, tooltip.

Examples:

A mixer fader: tooltip would probably just read the fader's setting, e.g. "-5.9 dB". the statusbar could say something like "Drag to change. Ctl+drag for fine control." (assuming that's true). 

Mute button: tooltip: (Un)Muted, statusbar: Item mute. Toggle with click. Mutli-select with Ctl+click (which we can't do at this time, but for example)

Keyboard shortcuts:

I feel like we probably should have paid more attention to this from the beginning. Ideally, I'd like to have super robust action and shortcut support a la REAPER. We'll have to make our way there, I suppose. Partly, I feel like it would have been a good piece of foundation for some of the more advanced stuff I want to do because we would have been setting up an action engine, could've made a console for that engine, then if you build nodes that use the engine and let them pass control and audio data...pretty much kinda have max/msp, reaktor, bidule, etc. we were supposed to have built this with some sort of passing awareness that maybe an MCP server would be cool, or at least useful during dev. that'd run off of the same underlying system, i'd think. 

At this point, minimally, we just need to add the ability to configure some common keyboard commands in the prefs page.
  DONE: `docs/ACTIONS.md` is the contract, `mooloop-ui/src/actions.rs` is the
  registry, and Preferences > Shortcuts rebinds all 39 of them. The console
  and MCP surfaces this paragraph wants are still hypothetical, but they now
  have one seam to hang off rather than needing their own wiring.

GUI focus issues:

There seems to be something in slint where there's sort of an input caret. in the first few version of this app, every control had to be clicked once to select and again to use it. that's not really an issue now, thank god, but that select caret can still intercept e.g. spacebar for play/pause in some situations. i should probably note them when they occur. I do think keyboard navigation is important so i see the need for such a caret, but we'll have to be intentional and thoughtful about how we use that so that we aren't blocking key commands that should work wherever you are.
  CONFIRMED 2026-09-05, and no longer hypothetical — see the first item in the
  2026-09-05 block below. It is a live defect, not a suspicion.

General application design:

GUI:

we're ending up with a fair number of panes. currently the layout is totally static. we might want to allow the user to customize their layout.
  PARTLY: the lower dock is resizable by a splitter and can collapse, and the
  browser sidebar has its own grip. Neither is a dockable-pane system; panes
  still cannot be moved or torn off.

on a 1080p monitor, in a 16 step pattern there is plenty of room to the right of the seq steps for us to split the pane and have the playlist seq beside it. this is what made me think we might want to kinda ape REAPER's dockable dialogs thing.

In appearance prefs, we should be able to set up different shading options.
  STILL OPEN. Appearance prefs now derive the whole palette from three seeds
  plus roundness and contrast scalars, so there is a place for this to live,
  but no shading pattern is configurable. Like, right now i think every 4th step in the step seq is brighter. Let the user configure that by setting a pattern. Maybe I want to brighten every 3rd step, or 6th. Maybe I want 8 bright, 8 dark, 8 bright, 8 dark. Maybe this shading could extend to the piano roll's grid? It would help a lot with editing sequences.

---

## Added 2026-09-05

Eight items, in Adam's words, none of which were written down anywhere before
this. `FOCUS.md` decides which of them are in the active sequence; three are.

keyboard is still wonky. you often have to click into a background area to make shortcuts work, even spacebar
  IN THE SEQUENCE (`FOCUS.md` step 3, and named as a fix that may interrupt).
  The cause is structural rather than per-control: the window has one
  `FocusScope` (`main.slint:1253`) reached by `forward-focus`, so any focusable
  thing inside it — a text field, a touch area that takes focus — consumes the
  key before the dispatcher ever runs. Clicking the background works because it
  hands focus back. This is the same thing the "GUI focus issues" section above
  guessed at; it is now confirmed and reproducible.

og drumsynth was simple but honestly sounded pretty good. why has simply updating it for automation support never been on the table? let's put it up there
  IN THE SEQUENCE (`FOCUS.md` step 2). It was off the table because of a note
  on `DeviceKind::descriptors` calling `DrumSynthParams` a mode-union. It is not one —
  it is a flat struct whose fields keep one meaning forever, and the module
  comment at `drumsynth.rs:7` says so. Sixteen continuous fields, one
  descriptor table, the shape every other generator already has.

i want to move and redesign the modulation rack. i have an image somewhere, a mockup from chatgpt. ah its here: reference/img/mooloop-1.0-mockup.png
  IN THE SEQUENCE (`FOCUS.md` step 3). The mockup puts modulation in a
  right-hand panel with PATTERN/CONTROL/PLAYBACK/MAPPING tabs, and draws the
  modulator itself as a *tracker* — which is the same shape as the automation
  idea already sitting in `IDEAS.md`. Whether those are one design or two is
  the first thing to settle. `MODULATOR_SYSTEM_SPEC.md` holds the contracts a
  move must not break.

i want to make a sidebar on the left that lets you change channel settings like name, track color, input channel, etc. its also illustrated in the mockup
  IN THE SEQUENCE (`FOCUS.md` step 3). Track colour does not exist anywhere
  today — not as a field, not in the project format — so this one adds
  persisted state and `PROJECT_FORMAT.md`'s defaulted-field rule applies. The
  mockup's MIDI input/output/channel rows are settings for a MIDI path that is
  wired but reaches nothing; build the rows, leave them inert, and do not let
  the sidebar pull MIDI configuration forward.

we need keyboard nav in the sample browser panel
  IN THE SEQUENCE (`FOCUS.md` step 3). Downstream of the focus fix above: a
  tree that cannot hold focus predictably cannot be navigated either.

i think i want that panel to also be able to browse and load presets
  IN THE SEQUENCE (`FOCUS.md` step 3). This is the browser that
  `docs/plans/preset-system/` has been holding for two instrument banks to
  design against. Both banks now ship, so nothing is blocking it.

i want to do a text label -> icon pass at some point
  DELIBERATELY NOT YET. Wanted, but it is polish over panes that step 3 is
  about to move, so doing it first means doing it twice.

i think i want to expand our use of color. some of the app is ide-inspired so maybe we should build toward colorscheme support. imo it would be dope as hell to have a music app that had dracula, monokai, everforest, nord, etc built in. base16? mmm. this idea holds hands with pywal/wallust support
  DELIBERATELY NOT YET, same reason, and there is somewhere real for it to
  land: Appearance already derives the entire palette from three seeds (base,
  accent, alert) plus roundness and contrast scalars, with six built-in schemes
  and user schemes that save and remove. The open design question is whether a
  named scheme like Nord is *three seeds* — in which case this is content, and
  cheap — or a full sixteen-colour ramp, in which case the seed model has to
  grow a second form and the shading-pattern item further up this file wants
  the same thing. base16 and pywal/wallust both answer "full ramp", so they
  decide it. That question is worth answering before any of it is built.
