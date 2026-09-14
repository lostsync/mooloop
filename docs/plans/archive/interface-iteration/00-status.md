# Interface iteration status

## Step 04 — the keyboard reaches the rest of the application

Landed on `feat/keyboard-pass` (2026-09-14). The registry is 63 actions in
eleven categories, `Ctrl+C` resolves against the focused panel, the browser
tree is navigable without the mouse, and Preferences > Shortcuts says which
chords are contextual. **All four steps have landed; the directory archives
with this one.**

### The thing to carry forward: a registry is not a keyboard

The step's own list of gaps was about coverage -- no stop, nothing
device-level, no browser focus. What the doing found is that **the registry
and the keyboard had drifted apart and nothing could notice**, twice, in the
same shape `FOCUS.md` names as this month's class: a claim the source no
longer supported.

- **`transport.loop-toggle` had been dead since it shipped on 2026-09-07.**
  It defaults to a bare `L`. `main.slint`'s root ladder forwarded exactly six
  keys unmodified -- the digits 1-6, for the roll's tools -- so an `L`
  reached the final `reject` and no action ever fired. The registry held it,
  the prefpane drew it, `ShortcutTable` resolved it, and the suite was green,
  because every test asked the registry what it held rather than asking the
  markup what it could deliver.
- **The Shortcuts recorder refused unmodified keys outright**, so the bare-L
  and bare-digit defaults could not be rebound to anything either: Reset
  followed by Record could not put back what the registry shipped with.
- **Space was forwarded with all four modifier flags hardcoded `false`**, in
  both ladders. Shift+Space and Space were one chord, which is why
  `transport.stop` could not have been bound before this step whatever the
  registry said.

Both ladders end in a catch-all now, and `actions.rs`'s `decoding` module
reads the real `.slint` files: `every_default_chord_reaches_the_dispatcher`
fails if a registry default is a chord the markup cannot produce, and
`the_recorder_decodes_what_the_dispatcher_does` fails if the two ladders name
different keys. They scrape rather than mirror, because a mirrored table
would be the third copy of the thing that had already drifted twice. Escape
is the one asymmetry and it is a decision -- it cancels a capture, so the
recorder can never hand it back -- and the test asserts nothing binds it.

Ctrl+H/I/J stay unreachable whatever the registry says: their control codes
*are* Backspace, Tab and Return. `CTRL_UNREACHABLE` is that sentence in a
form that can fail.

**Both were validated against the defect, per `AGENTS.md`'s rule that a check
of this kind is otherwise decoration.** Reverting `main.slint`'s catch-all to
the digits-only ladder makes `every_default_chord_reaches_the_dispatcher` fail
with *"transport.loop-toggle defaults to L, which main.slint's key ladder
never produces"*. That run also found a gap in the sibling check:
`the_recorder_decodes_what_the_dispatcher_does` **passed** under the same
mutation, because the two ladders still named identical keys and only their
catch-alls differed. It compares the catch-alls now.

**A third instance turned up while writing this up.** `ACTIONS.md` states how
many actions there are, confesses in the same sentence to having said 46 where
the table held 45, and was corrected to 47 on 2026-09-12 — by which time the
table held 49. Two corrections, both made by counting, and counting is what
went wrong both times. The third fix is
`the_registry_count_in_actions_md_is_the_registry_count`, which reads the
sentence out of the Markdown and compares it with `ACTIONS.len()`. A sibling,
`a_category_is_one_run_of_the_table`, catches the thing that would make a
recount *look* right: `shortcut_rows` marks a section boundary by comparing
each entry with the one before it, so a category split across two runs would
draw two headings with the same name and report nothing.

### What `Ctrl+C` means, which is the decision the step existed to make

A chord resolves to exactly one action id -- `ShortcutTable` is a
`HashMap<KeyChord, &str>` and cannot be anything else -- so three meanings is
**one action that asks what has focus**, not three actions sharing a chord.
`ActionSpec` grew a `Scope`, `Surface` is what a `Scope::Focused` action
points at, and the prefpane draws the scope in a Context column.

Three things about it that are decisions rather than mechanism:

- **The fallback is the channel list, not "nothing".** Before this the
  clipboard chords meant the channel unconditionally, so a surface nobody has
  clicked behaves the way it did then and nothing a user relied on changed
  shape. There is deliberately no Escape-to-nowhere either: the way back is
  selecting a channel, which is the ordinary thing.
- **The roll wins whenever it is on screen with a selection**, ahead of the
  last click. That is the rule the chords have shipped with since 2026-09-07
  and it is still right: a user who has just dragged a marquee is not
  thinking about the browser row they opened before it.
- **The ids did not change.** `edit.copy-channel` is labelled "Copy" and
  means the focused panel's clipboard; `notes.nudge-up` is labelled "Move Up"
  and picks a channel when the roll is not where you are. A user's
  rebindings are stored against the id, so renaming one silently drops them
  -- which is `ACTIONS.md`'s own rule, applied rather than restated.

The four arrow keys are the same mechanism, and taking them meant **deleting
a branch from `main.slint`'s root FocusScope**: Up and Down picked a channel
there, guarded by a hand-written "unless the roll has a selection". That
guard could only ever know about two answers. The browser could not have been
a third without leaving the markup.

### The browser does not get a FocusScope

The step file's diagnosis was that the tree "has no `FocusScope` of its own
and so cannot be reached by a key at all". The first half is true and the
second does not follow, and giving it one would have been a regression:
**a FocusScope without focus swallows the pointer press that would focus
it**, which is the two-clicks-per-control bug the root scope's own comment
and `tests/first_click.rs` are both about. Nesting one inside the browser
would have made every row a two-click row.

The root scope already hears every key. What the browser was missing was
somewhere for them to be *aimed*, which is `focused-surface` plus one
`browser-focus-index`. Ctrl+B reveals the sidebar and takes the keys; Up and
Down move the highlighted row, Right opens a closed folder or steps into it,
Left closes an open one or climbs to its parent, Enter does what clicking the
row does, and Ctrl+Enter is the row's context-menu load.

Three smaller findings in it:

- **Left climbs by walking back to the first shallower row.** The model is
  flattened, so a row carries no pointer to its parent and there is no other
  way to ask. `browser_parent_of` is pure for that reason and is tested on a
  depth list rather than a rendered tree.
- **Scrolling the keyboard's row into view has to happen in the markup.**
  Rust knows the index; only the `ScrollView` knows how many rows fit. A
  local property aliasing `browser-focus-index` plus a `changed` callback is
  how movement in a root property is observed from inside the element that
  can answer.
- **A collapse takes rows away underneath the keyboard**, so the focus is
  clamped after every toggle rather than bounds-checked at every read.

### `focused-surface` is a string

Four names crossing into the markup, not four indices -- so neither side
spells a number, which is the boundary rule `docs/workflows/rust-slint-boundary/`
exists for. `focused_surface_names_match_the_markup` fails if the markup ever
assigns a name no `Surface` answers to, and if its default stops being
`Surface::default()`.

### What is not here

- **No channel solo.** Solo is a *track's*, in place, since 2026-09-11;
  `track.solo` and `track.mute` bind that, aimed at the track the rack is
  editing. Inventing a channel solo would have been the step adding
  capability, which is the plan's own line.
- **No record action.** There is no recording to bind, and the step's last
  clause is that no action in the registry is bound to a feature that does
  not exist.
- **No axis-constrained note drag.** The step said to revisit it only if a
  rebindable *modifier* vocabulary came out of this. One did not -- the
  gesture registry already existed and this changed nothing in it -- so it
  stays out, per `ENHANCEMENTS.md`.
- **Keyboard note selection still does not exist.** The arrows move a
  selection and cannot build one. It is `ENHANCEMENTS.md`'s, not this step's.
- **The menu bar still calls its own callbacks rather than action ids.** Safe,
  because they are the same callbacks, and `ACTIONS.md` still defers the
  callable-by-id lookup until a console asks for it. What this step did add
  is the guard that keyboard and menu agree on *when* an action applies:
  Select All Notes is live only on the roll in both, and Delete only with a
  selection, because the shortcut arms now carry the condition the menu row
  is enabled by.


## Between steps — a track's head is bundled with its strip

Not a numbered step: `04-the-keyboard-pass.md` keeps that number and is still
ahead. Landed on `feat/track-head-strip` (2026-09-14) the way step 03's
channel sidebar landed inside step 03 -- direct feedback on what had just
shipped, worked the same day rather than queued behind the next planned step.

A track's rack drew four
things -- a sparse head face, the pinned channel strip beside it, the track's
own devices, and the fader -- and the head face had gone nearly empty on
2026-09-13 once its sends moved to the channel sidebar and its output stage
moved to the tail. Adam: *"maybe the strip could be bundled with some basic
track controls like name, maybe ins and outs."* The rack is three rows now:
head (identity, routing, polarity, then the strip's drive/EQ/comp), inserts,
fader.

### Where the merge actually happened

The head face (`BusDeviceFace`) and the strip's row (`StripRackRow`) are two
different shapes for a reason that survives the merge: the head stands inside
the shared `DeviceFrame` host, which is what gives a bus its own IN meter and
the leading `+` that inserts the chain's first effect; the strip's row has
neither, because it can be neither inserted nor removed. Folding the strip's
content into the *head's* box, rather than the reverse, keeps that meter and
that `+` rather than losing them -- so `BusDeviceFace` grew a `show-strip`
flag and, when it is on, draws `StripSections` beside its identity content
instead of the identity content alone. The identity markup itself moved into
a small unexported `BusIdentityPanel` so the merged and standalone layouts
share one copy of it rather than two that can drift.

**Confirmed with Adam before the build**, per two open questions a render
answered one way and a question the render could not answer:

- Keep the shared host chrome (the IN meter, the leading `+`) rather than
  drop rails the way the strip's own row does -- losing the meter would have
  been a real regression, not just tidying.
- Nothing beyond name, in/out and polarity belongs in the merged row; mute,
  solo and pan already live on the fader row, and sends live in the channel
  sidebar.

### The width, and what it reclaims

**3U**, not 2U and not 4U. `MergedHead { units: 3 }`, sketched with the real
identity column and the real `StripSections { wide: true }` side by side, fit
the compressor's full column with room to spare; `units: 4` only added slack
nothing used. That is one unit narrower than the 2U head plus 2U strip it
replaces -- the same unit the EQ's response plot gave back on 2026-09-11,
spent this time on content that used to stand in an empty box beside it
rather than on margin. Adam's standing complaint, quoted in `StripRackRow`'s
own comment before this step, was *"there's a ton of space between the start
of the rack area and the first device in a chain."* One box, at 3U, both
answers it and states in one place why it is not 2U.

### STRIP_PIN stays a live switch

`mooloop_core::mixer::STRIP_PIN` still decides where the strip's processing
draws, and the rack still holds two mutually exclusive instantiations of it
guarded by `root.strip-pin-head` -- `HorizontalLayout` places children in
declaration order, so "at the head" and "after the chain" are still two
positions in `main.slint`, not one that moves. What changed is only which
position also carries identity: when the pin is at the head (the shipped
default, `StripPin::Head`), `BusDeviceFace` draws both and the standalone
`StripRackRow` before the chain is gone; when the pin is at the tail,
`BusDeviceFace` goes back to identity alone and `StripRackRow` -- unchanged,
since it never carried identity to begin with -- draws the processing past
the chain. Polarity did not move either way: it stays at the head regardless
of where the pin puts the rest, because it is applied at the top of the
track's block and everything after it, including a pre-fader send, sees the
flipped signal.

## Step 03 — a channel is a thing you named

Landed on `feat/channel-identity` (2026-09-13). A channel has a name and a
colour, both saved; a pattern has both in the project format; a left channel
sidebar holds the controls; and the gesture that used to throw a name away
does not.

### What the doing changed about the step

**Two of the three pieces were already in, and a fourth was missing.** The
step file had been corrected on 2026-09-12 to record that `rename_track` and
`rename_channel` landed with the console pass, so what was left was colour,
the reset bug and the inert MIDI rows. What it did not know is that
**`rename_pattern` had nowhere to write.** `Session::pattern_names` existed,
the transport toolbar wrote to it, and `Project` had no field for it at all --
`load_project` blanked the list outright -- so a pattern named "Chorus" came
back numbered after a save and reload, and nothing reported a thing. It had
been that way since patterns became renamable on 2026-09-07.

That is why this step touches the project format twice. Persisting a pattern
*colour* beside a name that evaporates would have been incoherent, so the
name is persisted with it, and both live in one `pattern_meta` entry per
pattern rather than two parallel lists that can disagree about their length.

**The canonical form of "no colour" had to be decided before anything could
round-trip.** The first version gave `Project::default()` one blank entry per
pattern and a legacy-loading test failed on it, correctly: an entry that says
nothing is not the same as no entry, and a song where nobody has named or
coloured anything must write the bytes it wrote before these fields existed.
So the session holds one entry per pattern -- nothing bounds-checks an index
-- and `trim_pattern_meta` drops the trailing empties on the way out. Opening
a song and saving it does not rewrite it.

### The bug, and the question that separates its two cases

`reset_channel_source` re-derived `channel.name` from the index on every
source change. Live from 2026-09-09, when renaming shipped, and correct
before that: while every name was derived, re-deriving one was a refresh.

The fix is not a flag saying "the user named this". It is a question asked at
the moment of the change: **is the current name still the *outgoing* device's
default?** If it is, nobody chose it and it follows the device; if it is not,
it stays. No new state, and a channel that was renamed back to "Sampler 1" by
hand behaves like one that was never renamed -- which is right, because those
two channels are not distinguishable and should not be.

### The panel

Built because Adam asked for it on 2026-09-13 -- *"i think we were going to
put some of this in a left sidebar like that mockup had"* -- which overrides
the step file's "do not build the sidebar in this step". It is the mockup's
CHANNEL tab and only that: the PLUGINS and MIXER tabs it also draws are
second views of the rack and the mixer, and the plan's own rule is that an
interface change is judged by whether something already built becomes easier
to reach.

Three things in it outlive the feature:

- **The two side panels are one mechanism.** The channel sidebar is the
  browser's mirror -- in flow so it can animate to zero width, content
  clipped, grip outside the clip because `clip` cuts pointer events with
  pixels. What differs is the sign of the drag. `UI_DESIGN.md`'s new "Side
  Panels" section is that mechanism written down.
- **Two panels cannot each clamp against the whole window.** At 1000px a pair
  that each allowed itself 400px leaves 200 for the editor. Each measures its
  ceiling against the window minus its sibling, which is why
  `sidebar-ceiling` is a function.
- **The palette is a Rust table and cannot be anything else.** Slint cannot
  parse a hex string into a colour, so a swatch cannot derive its tint from
  the value it writes -- the same constraint `appearance-dialog.slint`
  records about its seed colours. In the markup the palette would have to be
  spelled twice, as colours to draw and as strings to store.

**The status bar's chip row had a hardcoded count**, `(3 - k) * 26px`, and a
fourth chip is exactly the event that makes a hardcoded three wrong. It reads
the list's length now. The chips still read in the screen order of the
regions they toggle, which is why the new one goes first.

One change is visible outside this feature: **a disabled `MenuField` now
mutes its value text.** The background already said "disabled" and the value
did not, which is not enough for a control drawn because a setting *will*
exist. The piano roll's snap division field gets it too, when snap is off.

### What is not here

- **Adoption finished on 2026-09-13, and it found that the mixer cannot have
  it.** The step names the rack, the mixer and the playlist as the surfaces
  that take a colour one at a time. The rack plate wears a channel's as a 3px
  bar; the playlist wears a pattern's as the same bar on its gutter plate and
  as the fill of every clip. **The mixer draws tracks, and a track is not a
  channel** -- there is no channel strip in it for a channel's colour to
  appear on, which is a fact about the console design rather than work left
  undone.
- **A filled shape needed one thing the bar did not**: an ink that can be read
  on it. `ProjectColor::ink` decides black or white by luminance, and its
  threshold was set by rendering all eleven swatches under both inks rather
  than by picking a round number -- 0.55 reads well and puts orange and sky
  on the wrong side.
- **A pattern's colour is set from the transport toolbar, not the sidebar.**
  The sidebar is a channel panel, so the chip went beside the pattern's name
  field instead -- the two facts about a pattern in one place. It shares the
  channel's palette: one set of suggested colours across the application,
  because a song colouring its Kick and its Chorus from two different elevens
  is harder to read rather than richer.
- **Colour is not undoable**, exactly like the two renames beside it. It
  marks the document dirty and nothing more, which is the shape
  `rename_channel` and `rename_track` already have.
- **The MIDI rows are inert on purpose** and say so by being disabled.

## Step 02 — a device can be copied

Landed on `feat/device-clipboard` (2026-09-07). A rack device -- or a
container and everything in it -- can be copied, cut, pasted and duplicated,
within a chain or across channels, with fresh identities and undoably.

### What it had to build first, and why that is the news

**The selected device.** Step 01 found that no such concept existed: only the
modulation rack had a selection, because every effect rail button belongs to a
row and already knows its own index. A keyboard shortcut does not, so this
step had to make one.

`Session.selected_device` is an `Option<(EffectTarget, DeviceId)>` -- an
identity, not a position -- and `selected_device_slot()` derives the slot when
it is wanted. **This is the first thing in the tree to actually spend
`containers/` step 01.** The test that says so is
`the_selected_device_survives_a_reorder_and_dies_with_its_device`, and under
the slot scheme it could not have been written: there, a selection would have
had to be rewritten by every reorder, which is precisely the class of bug
`SlotRemap` existed for and was deleted for.

Scoped to `effect_target`, because a `DeviceId` is only unique within one
chain, and cleared by `forget_device`, because a departed device is not
selected -- it is gone.

### The three verbs and the one new core primitive

`copy_device` lifts the run at a slot with every identity stripped, for the
reason `take_preset_save` gives: a clipboard holds a design, not a device.
`paste_device` inserts it after the run it was dropped on. `duplicate_device`
is copy-then-paste that deliberately does not disturb the clipboard, the same
relationship `channel.clone` has to channel copy and paste.

`mooloop_core::insert_run` is the one thing that had to be written.
`replace_run` existed -- built so a container preset could replace a run --
and insertion did not. It refuses a malformed run rather than trusting one,
which is what stops a straddle entering a chain that was fine before.

### The boundary rule, which is the only non-obvious thing about paste

**A paste lands beside a container's last child, not inside it.** `run_of`'s
end is a run's end *boundary*, and `insert_run` treats that boundary the way
`insert_effect` already documents for the rack's own `+`: landing on it is
landing after the container, not in it. One rule at every depth, and
`pasting_onto_a_containers_last_child_lands_outside_the_box` is it as a test.

### What does not travel

Modulation routes and automation lanes. A route's source is a module in the
*channel's* rack, so it cannot follow a device to another channel. This step
does not re-solve that: it is the question `containers/` reserved, and
`CommandState.device_clipboard`'s doc comment says so where someone will read
it. Recorded in `CURRENT.md` as a limit rather than left to be discovered.

### The chords, and the clipboard question Adam raised

Ctrl+Shift+C/X/V/D rather than the bare chords, which are the channel
clipboard's and unconditionally so. Adam asked, while this was in flight,
whether the clipboard should be unified with a history. The answer this step
records rather than acts on:

- **It is not the system clipboard, and nothing in the tree ever was.** No
  `arboard`, no `copypasta`, no clipboard crate in any manifest, and Slint
  1.17.1 exposes none to `.slint`. All three clipboards are in-memory Rust on
  `CommandState`.
- **Unifying is a tagged enum, not arbitrary bytes**, because paste has to
  know what it is pasting -- a note phrase pasted into a rack is nonsense.
- **The payoff is bare `Ctrl+V`**, which needs a focus model, which is step
  04's. So the two are one piece of work and should be sequenced together.
- **A history has a substrate already**: `EffectRun` is both the device
  clipboard payload and the container preset payload, and a preset bundle is
  a versioned, typed, serialized clipboard item. Two of the three clipboards
  are `serde` types; the third holds `Arc<SampleData>` on purpose, so a
  history of channel copies pins decoded audio and needs a bound from the
  first commit -- the lesson `5eb6e07` already paid for once.

### Acceptance

Six tests in `mooloop-session`, run in 0.12s on the laptop, which is the
whole argument for having built this half first:
`a_pasted_device_is_a_new_device_with_the_same_sound` is the step's headline,
`copying_a_container_takes_everything_in_it` is the container unit,
`pasting_onto_a_containers_last_child_lands_outside_the_box` is the boundary
rule, `duplicate_leaves_the_clipboard_alone` is the one behaviour that would
otherwise be a surprise, and `a_paste_refuses_what_it_cannot_take` covers the
empty and straddling runs.

## Step 01 — the browser browses presets

Landed on `feat/preset-browsing` (2026-09-07). The browser sidebar has two
tabs over one row model. PRESETS scans every well-known preset directory,
groups what it finds, and loads one onto the selected channel.

In plain terms: the ninety-nine presets a seeded machine ships were reachable
only from a rail button on a row that already existed. They are now something
you can look through, and an effect preset can be added to a chain that does
not have that effect in it yet.

Where it is, all in `crates/mooloop-ui/src/lib.rs` except the markup:

- `scan_preset_catalog`, `build_preset_rows`, `preset_detail` — the catalogue
  and the rows
- `PresetSlot`, `PresetGroup`, `BrowserTab` — the vocabulary
- `append_effect_preset`, `load_preset_document` — the two load paths
- `BrowserRow`, the tab header and the row rendering — `ui/main.slint`

### Three things the doing turned up

- **The plan said the target is "the selected rack row for an effect". There
  is no such thing.** Only the *modulation* rack has a selected slot
  (`modulation_selected_slot`); the effect rack has none, because every rail
  button belongs to a row and so already knows its own index. Rather than
  invent a selected-device concept for this step, an effect preset from the
  browser **appends** a device to the end of the chain. That turned out to be
  the better gesture anyway, and it is the one this plan should have asked
  for: the rail replaces what is in a row, and the browser adds a row, which
  is how you audition a reverb you do not already own. It is also why an
  effect preset is always loadable while a generator preset is not.

  A selected-device concept is still wanted — step 02's clipboard and step
  04's device actions both need one — and it is now unblocked rather than
  needed here.

- **Category is not worth a tree level, which is what the plan asked for.**
  Of the ninety-nine presets on a seeded machine, sixty-six are categorised
  `"Factory"` and seventeen `"DS-01"` — restatements of the directory they
  are already filed under. A category level would have been two rows of
  chrome around one useful row. The taxonomy is one level deep (the kind),
  and category and tags are a trailing detail shown only when they say
  something the group does not. `a_category_that_restates_its_group_is_not_shown`
  is that rule as a test.

- **A preset group's identity is its directory, so expansion came free.**
  `Session::browser_expanded` is a `HashSet<PathBuf>` built for sample
  folders; a group row hands back its own directory path, so
  `toggle_browser_folder` works on it unchanged and the session never learns
  what a preset is. The one place this leaked was the right-click handler,
  which removed a browser *location* on any depth-0 row and would have tried
  to remove a preset group as one — it checks the kind now, not just the
  depth.

### What it does not do

- **No keyboard navigation.** It is the other half of Adam's ask and it is
  step 04's, which owns the focus vocabulary the tree would need. The tab
  makes the case sharper rather than solving it.
- **No preview.** A sample previews on click; an effect preset cannot without
  being loaded, and loading is one undo away, so load *is* the audition.
- **No filtering to the selection, and no search.** The catalogue is ninety-
  nine rows across thirteen or so groups, which fits. Both become worth
  building when a user's own bank does not.
- **Nothing about the factory-bank update problem.** `LOOSE_ENDS.md` records
  that banks self-seed once behind a marker and can never update. A browser
  makes that more visible and does not change it.

### One thing the tests found, and one they could not have

- **Two of the three new interaction tests were passing vacuously**, and the
  third is why that was found. `tests/browser.rs` locates a row by scanning a
  column for the first y that repaints under hover, and that column runs
  through the panel header -- which now holds two tabs instead of a static
  label. An *active* tab draws `surface-active` whether or not it is hovered,
  so the sample tests, which run on the SAMPLES tab, never noticed. On the
  PRESETS tab the SAMPLES tab is inactive, hovering it does repaint, and the
  scan stopped in the header. The two tests asserting that something did *not*
  happen passed anyway; `clicking_a_preset_loads_it`, which asserts something
  did, failed. Preset tests now scan `PRESET_ROW_X`, clear of the tab strip.

- **A generator preset's loadability went stale on a channel switch**, and no
  test would have caught it because none of them switches channels. Whether
  the row is greyed depends on the selected channel's device kind, and nothing
  rebuilt the rows when that changed -- so selecting a DS-01 channel left the
  DS-01 presets drawn as unloadable. `refresh_preset_menus` already exists to
  catch exactly those switches and is called from all four sites, so the fix
  is one call at the end of it.

### What clippy found once it could reach this crate

Not part of the step, and the most valuable thing in the commit. `cargo
clippy` had been failing at `mooloop-core` since `df52933`, and a run that
dies there never lints anything downstream — so `mooloop-ui` had never been
linted at all. With `mooloop-core` cleared earlier the same day, two errors
surfaced here, neither of them from this work:

- A redundant `let sample_rate = sample_rate;` in `on_wrap_effect_clicked`.
  Harmless; `sample_rate` is a `u32` and the `move` closure copies it anyway.
- **A `#[test]` attribute that had come adrift from its function.** The
  container face test was inserted between
  `effect_rack_scrolls_horizontally_to_reach_a_long_chain`'s doc comment and
  its `fn`, so both `#[test]`s landed on the container test and the scroll
  test had none. It had not run since. Its doc comment records the regression
  it exists for — a rack viewport sized for an empty chain, leaving every
  device past it laid out but unreachable — and that guard was off.

Both are fixed here rather than recorded, because they are two deleted or
moved lines in a crate this branch is already rebuilding, and because leaving
clippy red on `mooloop-ui` would have gone on hiding the next one. The test
passes now that it runs again, so nothing had regressed behind it.

### Acceptance

`build_preset_rows` and `preset_detail` carry six unit tests in
`mooloop-ui`'s `preset_browser_tests`, the ones worth naming being
`an_effect_preset_is_loadable_whatever_the_channel_holds` and
`a_generator_preset_is_loadable_only_on_its_own_kind` — together they are the
append-versus-replace rule above, which is the only real decision in the step.
