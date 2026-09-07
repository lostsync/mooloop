# 04 — The keyboard reaches the rest of the application

Adam has asked for this more than once and in stronger terms than anything
else on his list:

> honestly being able to navigate this whole app by keyboard should be getting
> a lot more attention

> I feel like we probably should have paid more attention to this from the
> beginning. Ideally, I'd like to have super robust action and shortcut
> support a la REAPER.

The foundation exists. `docs/ACTIONS.md` is the contract,
`crates/mooloop-ui/src/actions.rs` is the registry, Preferences > Shortcuts
rebinds every entry, and as of 2026-09-07 a shortcut fires from wherever focus
happens to be, which it did not before. This step is about what the registry
does not yet cover.

## What thirty-nine actions do not include

Reading `ACTIONS` (`actions.rs:100`) against what the application can do:

- **Transport is one action.** `transport.play-pause` and nothing else. There
  is no stop, no return-to-start, no toggle-loop, no record — and
  `stop-clicked` already exists as a callback on the window
  (`main.slint:1249`), so at least one of these is a registry line.
- **Nothing is device-level.** No add effect, no bypass, no wrap in
  container, no save preset, no next/previous device. Step 02 adds the
  clipboard half; this is the rest.
- **Nothing is channel-level except add/remove/clone.** No mute, no solo, no
  arm. Mute is a real command; solo is a button with nothing behind it
  (`controls.slint:1856` has the property, `mooloop-core` has no solo state),
  so **do not add a solo action in this step** — bind what exists.
- **The browser has no actions and cannot hold focus.** Adam: *"we need
  keyboard nav in the sample browser panel."* The tree is a flattened
  `[BrowserRow]` list (`main.slint:553`) with no `FocusScope` of its own, so
  there is nothing for a key to reach. This is the largest single piece here
  and it is what makes step 01 usable without the mouse.
- **Note editing has no clipboard.** `ENHANCEMENTS.md` names cut/copy/paste
  of notes as still open, alongside keyboard selection — the marquee and the
  moving selection landed, the keyboard half did not. `note_clipboard` already
  exists on `Clipboards` (`command.rs:23`).

## The decision this step exists to make

**What `Ctrl+C` means.** Today it is `edit.copy-channel`, unconditionally,
whatever is focused. Once a device clipboard and a note clipboard both want
it, the registry needs a notion of context — which is the difference between
"a list of shortcuts" and the action engine Adam described:

> we would have been setting up an action engine, could've made a console for
> that engine, then if you build nodes that use the engine and let them pass
> control and audio data...pretty much kinda have max/msp, reaktor, bidule,
> etc.

That is the horizon, not this step. What this step should produce is the
smallest thing that makes `Ctrl+C` correct: an action that resolves against
the focused surface, with a defined fallback, and a Preferences page that can
still show a user what a chord will do. Design it so a console could later
invoke an action by id without a keypress — `ACTIONS.md` already treats the
id as the canonical name, which is most of that.

## Axis-constrained drag, and the Alt problem

`ENHANCEMENTS.md` records that axis-constrained note drag is the one standard
gesture deliberately left out, because every conventional binding for it is
Alt and Adam flagged Alt as WM-hostile, and shipping it bound to nothing would
be a dark feature. `gestures.rs` is where it goes once there is a key worth
giving it. **If this step produces a rebindable modifier vocabulary, that is
the moment to revisit it** — and if it does not, leave it out again rather
than binding it to something worse.

## The tooltip rule is adjacent and is not this step

Adam's rule — tooltips report a control's name and state, the status bar
explains — is partly implemented: the status bar exists and about forty sites
feed `hover-hint`. The audit itself has not happened, and the sampler face has
no `hover-hint` at all. It is not part of this step, but a shortcut that
exists and is undiscoverable is only half a feature, so a control that gains
an action here should gain its hint at the same time.

## Do not

- **Do not add a solo action.** There is no solo. `MIXER_PLAN.md` specifies an
  AFL-style monitor tap; building it is its own change.
- **Do not raise `MAX_MOD_ROUTES_PER_CHANNEL` or any other ceiling** because a
  keyboard made one easier to hit.
- **Do not rebuild the shortcut preferences page.** It rebinds all thirty-nine
  actions and works; a context dimension is a column in it, not a rewrite.

## Done when

Every shortcut in the registry fires from anywhere it sensibly should; the
browser tree can be focused, navigated and made to load without the mouse;
transport has more than one verb; `Ctrl+C` does the right thing in the piano
roll, the rack and the channel list, and Preferences can explain which; and no
action in the registry is bound to a feature that does not exist.
