# Plugin hosting — plan status

Linear: [MOO-11](https://linear.app/mooloop/issue/MOO-11/clap-plugin-hosting-effects-instruments),
filed before the docs/plans-into-Linear migration and not yet rewritten to
mirror this file step-for-step the way MOO-5/16/30 do. The GitHub issue
numbers below (`#10`, `#26`-`#30`) predate Adam's move away from GitHub
issues; MOO-11 is the live tracking issue.

**Written 2026-09-16.** Steps 01 (the spike), 02 (the neutral contract), 03 (parameters belong to an instance), 04 (the plugin rack), 05 (the scanner) and 06 (a headless CLAP effect in a chain) landed on 2026-09-23, as did MOO-56's one boxed source slot, which closes blocker 4 below. Step 07 (parameters, automation, modulation and state) landed on 2026-09-24, and so did steps 08 (the plugin browser, the menu row and the generic face) and 09 (a channel source that is a hosted plugin). Adam asked for it directly:
*"let's go ahead and plan out how we'll add CLAP support. Later we'll add
VST/3 and AU. instrument support as well."* `SCOPE.md` already put CLAP in
for 0.2.0 (item 9). This plan is outside the `FOCUS.md` sequence for the same
reason `coreaudio-driver/` is: Adam asked for it.

GitHub #10 and #26–#30 were written before this plan and describe the same
order. This plan replaces their ordering and keeps their "done when" lists.
Each step says which issue it closes.

**Every `file:line` below was re-verified on 2026-09-19, after the
audio-recording series merged with `origin/main`** — and the merge is the
point. The 2026-09-19 pass that folded MOO-26 in was accurate the hour it
ran: `crates/mooloop-ui/src/lib.rs:12623` really was `pump.start(` at
`968a2f9`. Three commits later it was not. **A line number in this
repository has a shelf life of about a day**, so the ones here are paired
with the symbol they point at wherever a symbol exists; when they drift
again, the name is what you search for.

Crate-relative prefixes are shorthand: a bare `effect.rs`, `generator.rs`,
`channel.rs`, `project.rs` or `modulation.rs` is `crates/mooloop-core/src/`,
`session/` is `crates/mooloop-session/src/`, `engine/` is
`crates/mooloop-engine/src/`, `ui/src/` is `crates/mooloop-ui/src/`, and a
bare `.slint` file is `crates/mooloop-ui/ui/`.

## What this is

A third-party plugin, effect or instrument, sits in a channel exactly where a
native device sits, and nothing above the adapter can tell the difference.
You can automate it, modulate it, save it, reorder it and bypass it. If it is
missing when a song opens, the song still opens and keeps everything that
belonged to it.

CLAP comes first. VST3 comes after it, through the same seam. AU comes last
and is optional, because **mooloop is built for Linux users**. macOS is
supported for Adam's own convenience, and Windows is "eventually" at very low
priority.

## Adam's answers, 2026-09-16

These were asked while the plan was being written. They are decisions, not
defaults.

1. **A plugin's state is saved inside the project TOML** (base64), not in a
   separate asset file. `PROJECT_FORMAT.md`'s rule that samples never go into
   TOML stays as it is. Plugin state is a different kind of thing, and step 02
   writes that difference into the format document.
2. **A plugin's own GUI opens in a separate window first.** Embedding it in
   the rack comes later, if at all. **Plugins that have no GUI at all**
   (Airwindows is the named case) need a face you can actually work with, not
   a debug list. See step 08.
3. **Linux is the audience.** It sets the bar: X11 and Wayland, JACK, and
   the free plugins a Linux musician actually has.
4. **The scanner runs out of process from the first version.**

## Adam's answers, 2026-09-22: one boxed source slot (MOO-56)

MOO-56 is the other half of blocker 4, and step 09 leaves it out on purpose.
It replaces `ChannelStrip`'s eight concrete generator fields with one boxed
`SourceNode` slot. Two questions were put to Adam on the issue, and he
answered both the same day.

1. **Do it, even with `device-registry/` parked.**

   > hi its adam. this needs to be done, i feel like it is pretty important
   > and the way we're currently switching instruments is pretty much
   > scaffolding quality - yeah it works and there are hints that someone
   > understands that a system needs to be built…but that system hasnt been
   > built. we're not fn ruby on rails over here.

2. **Changing a channel's source may become a queued structural edit.** With
   one boxed slot, a source change moves ownership. The new node crosses as a
   `StructuralCommand`, and the displaced one leaves through the reclaim
   ring. When the ring is full, the executor re-queues the edit, so the
   change can land a block or more after the click, not in the block the
   click arrives in. Asked whether that is acceptable, he answered:

   > totally fine. there's no reason to expect that this action should be
   > instantaneous.

   He also answered *"1. yes"* to the numbered option that accepts it.

**What this changes here.** Step 09 was written before these rulings. It
adds a ninth `hosted` field beside the eight and says to leave them alone,
because boxing them was "a separate refactor with its own reasons". Adam
has now given the reasons, and MOO-56 carries the work (Realtime Engine,
`docs/TEAMS.md`). Re-read step 09 against whichever lands first. With
MOO-56 in, a plugin source is one more occupant of the one slot rather than
a ninth field, and both specify an `InstallSource` structural command.

## The rule that makes VST3 and AU cheap later

**No format type leaves `crates/mooloop-plugin-host`.** Core, project,
session, engine and UI see only the neutral types below. Only that crate's
`Cargo.toml` names `clack-*` (and later `vst3` or an AudioToolbox binding).
Step 02 adds a test that fails if any other crate does. #10 says the same
thing: *"CLAP types stop at the adapter."*

## Architecture

### The neutral types (`mooloop-plugin-host`, re-exported where core needs them)

Types that must be saved live in `mooloop-core`, because the project types
live there. Anything that loads, runs or scans a plugin lives in the host
crate.

- `PluginFormat { Clap, Vst3, Au }`. It is saved as a string tag.
- `PluginRef { format, id, name, vendor, version }`. This is what a song
  remembers. For CLAP, `id` is the plugin's reverse-DNS id. The file path is
  **not** saved: a song moved to another machine should find the plugin by id
  through the scan cache.
- `PluginParamInfo { id: u32, name: String, module: String, min, max,
  default, stepped: Option<u16>, automatable, modulatable, hidden }`. Names
  are owned `String`s, which is why these are not `ParamDescriptor`s
  (`ParamDescriptor`'s names are `&'static str`, `effect.rs:197`).
- `PluginState { chunks: Vec<PluginStateChunk { tag, data }> }`. It holds
  chunks, not a single blob, because VST3 saves component state and
  controller state separately. CLAP writes one chunk. (Planned with a
  `format` too, and built without one, because the `PluginRef` beside it
  already says the format. See "Step 02, recorded".)
- `PluginSlotId(u32)`, `Copy`, minted **per project**, not per chain.
  `DeviceId` is minted per chain ("every chain mints its ids from zero"), and
  a per-chain key would have to be renumbered whenever a channel moves.

### Two halves per plugin

`clack-host` already splits a plugin into two halves, and this plan follows
that split.

- **`PluginInstance`** stays on the control thread, which is the Slint
  event-loop thread. **No single line says so** — it is a property of how
  the control side is assembled, so the 2026-09-16 citation
  (`ui/src/lib.rs:12364`) was never the right kind of reference. The
  evidence is the pump: a repeating `slint::Timer` started at
  `ui/src/lib.rs:12912` (`pump.start`), whose closure runs to `:14367` and
  owns the session behind a non-`Send` `Rc<RefCell<_>>`. A Slint timer
  callback runs on the thread with the event loop, and the non-`Send` state
  means no other thread could take that work over. It owns the loaded
  library, the parameter info, state save and load, the GUI, and
  `on_main_thread`.
- **`PluginProcessor: AudioNode + Send`** is what goes through
  `StructuralCommand::InstallEffect` (`engine/src/lib.rs:209`; the citation
  read `:175`, then `:201` when it was corrected earlier on 2026-09-19 —
  the merge moved it again the same day), the way every
  native node does.

The control thread keeps the instances in a **`PluginRack`**, keyed by
`PluginSlotId`. Teardown always goes in this order: send the remove, wait
until the processor comes back on the `reclaim` ring, and only then drop the
instance. That fixes blocker 5 below: without the rack, nothing on the
control thread can reach an installed node.

### Requests that must reach the main thread

A plugin may ask for main-thread work from any thread. Examples are CLAP's
`request_callback`, `request_restart`, `params.rescan`, `latency.changed`
and `state.mark_dirty`. Each instance has an `AtomicU32` flag word. The
plugin's host callbacks set bits in it, and the pump drains the bits on its
next tick. Nothing blocks and nothing allocates. The CLAP thread-check
extension compares the calling thread against the UI thread id and the audio
thread id, both recorded when the thread starts.

### Parameters

**Adam's decision, 2026-09-23 (MOO-74).** This section used to say a plugin
parameter's address was the existing `Effect { device }` or `Source` owner
with the plugin's own `u32` id reinterpreted in `param` and no new variant.
That was the plan author's default, not a ruling, and Adam overturned it.
Asked to choose, he answered in his own words:

> i think option 2 is the 'most right'? we don't want to not know what
> something belongs to

and, told that a variant carrying only a `DeviceId` costs no bytes:

> that sounds like it would also id the device. and be free. so i would
> probably choose that, all else being equal

So a plugin parameter's saved address is
`ParamAddr { scope, owner: ParamOwner::PluginParam { device }, param }`: a
**new `ParamOwner` variant whose only payload is a `DeviceId`**, with the
plugin's own `u32` id in `param` (`ParamOwner`, `core/src/modulation.rs`).
The address says what it belongs to. `size_of::<ParamAddr>()` stays 16,
because the discriminant fits in the padding the `DeviceId` payload already
has. The ten exhaustive `owner` matches become compile errors that force an
arm, the integrity pass included, instead of a native table silently judging
a plugin id. `ParamAddr::device()` has a `_ => None` arm and compiles either
way, so step 03 gives it the new arm by hand. Step 03 lands the variant
(MOO-78).

**Missing parameters are never dropped** (Adam, 2026-09-23: *"i'd say ignore
it, maybe it gets represented in the UI as missing somehow (greyed
out/crosshatched, or like itallicized/thin weight title in the lane selection
menu)"*). A lane, route or binding that names a plugin parameter that does not
exist, because the plugin is missing or because its list no longer has that
id, is **kept, saved back unchanged, and shown as missing**. When the
parameter comes back, the address finds it again. Step 03 has the rule.

**`params.rescan` is supported** (Adam: *"if its possible in the plugin
format then we should support it. a lane in this situation (automated param
was removed) would be handled by the missing lane handler and reunited with
its param should it return"*). A rescan replaces `PluginSlotState.params`,
and the lanes and routes whose ids went away go through the same
missing-parameter handling.

**The callback should iterate what is actually driven.** Adam didn't follow
the question about scanning (*"shouldnt the plugin just have a list? idk what
is being scanned or why"*). The reading recorded on MOO-74 is that the
per-block control pass should walk a per-channel list of the parameters that
are actually automated or modulated, not every descriptor with a route scan
for each one. That is its own issue (MOO-195), not part of this plan's steps.

Arrays sized by id (`descriptor_slots`, `session/values.rs`) become
sparse-safe (step 03). A dense per-instance index exists only in session and
UI.

Values travel in the plugin's own plain units. Those are mooloop's
"natural" units, so `Event::ParamValue { id, value }` needs no change, and
automation and modulation need no plugin-specific code. A parameter the
plugin marks as stepped uses `ParamCurve::Stepped`. Every other parameter
uses `Linear`. The adapter turns `ParamValue` events into CLAP parameter
events at the same sample offsets.

### Saving

- `EffectKind::Plugin`, `EffectParams::Plugin(PluginSlotId)`,
  `DeviceKind::Plugin`, `ChannelSource::Plugin(PluginSlotId)`,
  `GeneratorParams::Plugin(PluginSlotId)`. All of these are new tagged
  variants with permanent `serde(rename)`s. **Four of the five are `Copy`,
  not all five**: `EffectKind` (`effect.rs:12`), `EffectParams`
  (`effect.rs:2889`), `DeviceKind` (`channel.rs:26`) and `GeneratorParams`
  (`generator.rs:914`). `ChannelSource` is **not** — `project.rs:109`
  derives `Debug, Clone, PartialEq, Serialize, Deserialize`, because it
  holds `SamplerState` and the other per-source states by value. Checked
  against the 2026-09-16 tree: it was not `Copy` then either, so this was
  wrong the day it was written rather than drifted into. Nothing here
  depends on it — a `Copy` variant inside a non-`Copy` enum is fine, and the
  side table of blocker 2 turns on `EffectParams` and `GeneratorParams`,
  which are `Copy` — but the sentence claimed a property of five types
  having checked two. The old citations `effect.rs:2663,3128` and
  `generator.rs:877` land on a Buffer id constant, a Gate match arm and an
  oscillator-index helper; none is a derive.
- `Project.plugins: BTreeMap<PluginSlotId, PluginSlotState>` with
  `serde(default)`, where
  `PluginSlotState { plugin: PluginRef, params: Vec<PluginParamInfo>,
  pinned: Vec<u32>, state: PluginStateText }`.
  `params` is the list as the plugin last reported it. It is what makes a
  missing plugin's lanes and routes still readable.
- **The state is base64 in the TOML** (Adam, answer 1). It is written as a
  multi-line string wrapped at 76 columns, so a song stays readable and
  diffable. The decoder ignores whitespace. Large states (sampler plugins can
  reach megabytes) are a known cost of this choice, and the file stays valid
  with them.
- The format stays at version 1. Older readers ignore `plugins`, but they
  would fail on an unknown `EffectParams` tag. That is the behaviour we want
  (they refuse the song instead of loading half of it), and step 02 tests it.
- Presets of a plugin device use the existing preset envelope, with a new
  `contains` entry so older readers refuse them.

### Latency

For a plugin, `EffectKind::latency_frames()` (`effect.rs:62`) cannot answer.
Compensation takes a plugin effect's latency from its `PluginInstance` after
activation. When a plugin reports a new latency, the host deactivates it,
activates it again, builds a new processor, swaps it in with
`ReplaceEffect`, and recomputes compensation. The id-freeze and
latency-agreement tests keep covering the native kinds.

### Scanning

The app runs its own binary as `mooloop --scan-plugin <path>`, with a
timeout, and reads JSON from the child's stdout. A plugin that crashes or
hangs takes only the child down. The results go to `<config>/plugins.toml`,
keyed by path and mtime, and failures are recorded so the same file is not
retried on every launch. Search paths are CLAP's defaults (`~/.clap`,
`/usr/lib/clap`, `/usr/local/lib/clap`, and the macOS equivalents), then
`CLAP_PATH`, then paths added in Preferences. Flatpak and distro packages
put plugins in more places. Step 05 lists which ones are checked.

### Failure

Processing stays in-process. There is no sandbox (#10 puts one out of
scope). Every call into a plugin goes through `catch_unwind` where Rust can
catch anything, and a plugin that reports an error or produces non-finite
output is **bypassed and flagged**. The song stays playable and saveable.

**A missing plugin loads as a placeholder**: pass-through for an effect,
silence for an instrument. Its `PluginSlotState`, lanes and routes are kept,
and saved back out without changes.

### Faces

Slint cannot create component types at runtime, but a `for` over a model is
fine. That gives one `plugin-device.slint` face, driven by a model. For a
plugin with no GUI it is the only face, so it has to be a good one (step 08).
For a plugin with a GUI, it sits in the rack while the GUI opens in its own
window (step 11).

## Blockers (verified 2026-09-16; re-verified 2026-09-19, post-merge)

Items 1–4 are `SCOPE.md` §5's list. Items 5 and 6 were found while writing
this plan.

1. Parameter descriptors are static per kind (`effect.rs:128`,
   `EffectKind::descriptors`). The id-sized arrays assume small, dense ids,
   and `EffectSlotRow` has fixed, hand-written parameter fields
   (`main.slint:243-266`; the citation read `:211`, the comment above the
   struct). **They are `p0..p17`, not `p0..p9`** — `p0..p15` are the sixteen
   slots a descriptor table fills, and `p16`/`p17` carry the tempo-sync flag
   and beat division, which are not descriptors. There were eighteen on
   2026-09-16 too, so the count was wrong when written rather than outgrown.
   It does not soften the blocker: eighteen is still a ceiling spelled in
   markup, and a plugin with forty parameters clears it as easily as one
   with eleven. **Step 03.**
2. `EffectParams` is a closed enum and it is `Copy`. **Step 02**, through
   the side table.
3. Latency comes from the effect kind. **Step 04.**
   Added 2026-09-18 from `reports/fable-2026-09-18.md`: compensation is not
   the only per-install table this touches. `AudioTapBank` is **rebuilt whole
   on every install** (`engine/src/render.rs:3691`, in
   `RenderState::load_project`; the citation read `:3379`). It is
   deliberate, and the line says why: an offline render builds its own
   `RenderState` and never runs a pump, so without it an export would be the
   one place the tap plan was missing. It is swapped whole a second way the
   2026-09-18 note did not mention — `Session::sync_audio_graph`
   (`session/engine.rs:661`) derives the plan once a pump tick and sends a
   whole new bank through `StructuralCommand::SetAudioGraph` when it
   differs. A plugin's
   latency is known only after activation and can change while it runs.

   **Corrected 2026-09-20** from `reports/fable-2026-09-20.md`: a runtime
   latency change does *not* imply an install, and the sentence that said so
   survived the 2026-09-20 citation re-verification because the citations it
   carried were sound and the claim between them was not.
   `StructuralCommand::SetCompensation` (`engine/src/lib.rs:283`, applied at
   `engine/src/render.rs:4390`) swaps one target's delay ring in place, and
   `Session::sync_compensation` (`session/engine.rs:493`) sends it per target
   from a diff against `compensation_sent` — which the install path resets
   (`session/session.rs:1527`) so every length is re-derived against the
   incoming project rather than trusted from the outgoing one. The tap-bank
   rebuild therefore does not follow from the latency change either: it
   follows from an install, or from `sync_audio_graph`'s own diff, and both
   stand on their own.

   So what step 04 still needs is narrower than "stop a latency change being
   an install". The delivery already works one target at a time with no
   install; what is missing is a *source*. `compile_latency` reads the effect
   kind, so a number that only exists after activation and moves while the
   plugin runs has nowhere to enter and nothing to trigger the re-derive.
   Step 04 is that entry point and its trigger.

   Blocker 3 also ordered the identity work of
   `plans/archive/channel-identity/` step 05 before a plugin's latency could
   move without tearing the graph down. That work landed on 2026-09-18 and
   the plan is archived; the prerequisite is met.
4. `ChannelStrip` holds its eight generators as concrete fields
   (`engine/src/render.rs:2273`, the eight named at `:2274-2281`; the
   citation read `:2154`, then `:2189`, and `:2189` was `BusStrip::new`, a
   different type) and has no slot for a boxed source. Three numbers in four
   days: search for `pub struct ChannelStrip`. **Step 09.**
   Note delivery to those eight is a closed match calling three different
   method shapes rather than one trait method, and unifying it is worth
   doing while every source is still native --
   `reports/fable-2026-09-18.md`, Plan D.

   **Landed 2026-09-19: the closed match is now one trait method.**
   `SourceNode: AudioNode` (`dsp/src/node.rs`) gives the strip a single
   `process_source` call plus `publish_outlets_into`, and
   `ChannelStrip::source_and_bus` (`engine/src/render.rs`) returns the
   generator and the bus together so the match and the field split happen in
   one place. The strip still holds its eight generators as concrete fields
   and still has no slot for a boxed source -- that half of this blocker was
   untouched then (see the next paragraph) -- but the call shape a boxed source
   would have to satisfy now exists, and it has one signature rather than
   three.

   **Landed 2026-09-23: the slot (MOO-56).** `ChannelStrip` holds one
   `source: Box<dyn SourceNode + Send>` and nothing else of its instrument.
   `SourceNode` grew what the strip used to reach past the trait for:
   `kind`, `set_generator_params`/`generator_params`, `choke_group`,
   `set_route_amount` and `as_sampler(_mut)`. A source change is
   `StructuralCommand::InstallSource`, built on the control thread by
   `EngineHandle::send`, and the displaced node comes back as
   `StructuralReclaim::Source`. `render::build_source` is the one match over
   the native kinds. **Blocker 4 is closed**; step 09 now only has to write a
   `SourceNode` for a plugin and put it in that slot.
5. Once a node is installed, the control thread has no handle to it, and
   nothing lets the audio thread ask for main-thread work. **Step 04.**
6. The effect menu is not driven by data: 14 hard-coded `EffectTypeRow`s in
   `EffectTypeMenu` (`device-rack.slint:172-194`; the citation read
   `:172-196`). The count of 14 was right on 2026-09-16 and is right now —
   the one number in this list that has never moved.

   **The source popup claim is wrong, and has been since 2026-09-18.** It
   *is* driven by a model. **Re-addressed 2026-09-21** (`18f560b`, MOO-53,
   moved it the same day the previous correction was written): the popup is
   `AddSourceButton` in `crates/mooloop-ui/ui/channel-rack.slint:81`, its
   `PopupWindow` at `:94`, and its rows are `for label[i] in
   SourceKinds.labels` at `:113`. `main.slint:3716` is a `spacing` line and
   `main.slint` has no loop over `SourceKinds.labels` at all -- it imports
   `AddSourceButton` (`:33`) and uses the list as a combo-box `options`
   (`:3837`) and a name lookup (`:4515`). (`main.slint:3619` was wrong even
   on 2026-09-16: it was piano-grid pointer handling that day too.) What
   survives is narrower and is what step 08 must build: the model is a
   literal spelled in markup — eight strings in `global SourceKinds` at
   `channel-rack.slint:65-70`, held against `DeviceKind::label()` by
   `tests/source_kind_menu.rs` — so no row can be added from Rust at
   runtime, which is the whole job of a scanned plugin list. The popup's
   `for` is the shape step 08 wants; its source of rows is not. The effect
   menu needs both. **Step 08.**

   Two things this correction is the second instance of, and they are the
   lesson rather than the addresses. **Search by name, not by line** -- this
   file already says so, and every *count* in it has stayed right while its
   line citations drifted 13 to 61 lines. And `tests/source_kind_menu.rs`
   was entirely correct for the whole time the menu added no channel: it
   held the right labels in the right order against the right table, and
   nothing in it pressed the button (`AGENTS.md`, the `popup-close-order`
   check). Step 08 replaces this markup literal with a Rust-supplied model;
   whatever guards it must click it.
7. **Found 2026-09-17, and not solved by this plan.** A channel paste, delete
   or move rebuilds the whole `RenderState` through `install_project`
   (`LOOSE_ENDS.md`, "Every structural edit stops the song"). `PluginSlotId`
   stops a move from *renumbering* plugins, but not from tearing down every
   plugin processor in the song and loading it again. Solved by
   `plans/archive/channel-identity/` step 05, which **must land before step 06
   here**.

   **Closed 2026-09-23 by step 06 (MOO-81).** The carry plan keeps an
   unchanged plugin device's processor across the install, and
   `PluginRack::retire_except` now keeps its instance with it, where
   `replace_project` used to retire every instance. See "Step 06, recorded".

## Three things the tree already gives, and two it does not

From `reports/fable-2026-09-17.md`, "Architecture: what will fight the next
four features", item 5 — tracked as MOO-26 and folded in here 2026-09-19,
when the Linear project was made, so the issue can close. **Every citation
was re-read against the tree on that date.** Four of the five had drifted and
two of the claims were wrong as written; the corrections are the point of
keeping them.

Easier than this plan assumes:

- **Host time is free.** `ProcessContext` already carries `playing`, `bpm`,
  `position_ticks` and `position_frames`
  (`crates/mooloop-dsp/src/node.rs:49-55`; the report read `:49-52`, before
  the comment on `position_frames` grew). The adapter fills CLAP's transport
  from fields every native node already reads, so nothing new has to cross
  into the audio thread to carry it.
- **The boxed source slot is a copy of an existing path, not a new
  mechanism.** `StructuralCommand::InstallEffect`
  (`crates/mooloop-engine/src/lib.rs:209`) and `ReplaceEffect` (`:227`)
  already move a `Box<dyn AudioNode + Send>` over the ring, and the displaced
  occupant leaves through the reclaim ring rather than being dropped on the
  audio thread — the rule is stated on `InstallEffect` itself and enforced in
  `crates/mooloop-engine/src/executor.rs:199-202`, which checks the ring has
  room *before* applying the edit. Blocker 4 and step 09 are a new field on
  `ChannelStrip`, not a new path. (The report cited `:189,208` and the
  Architecture section above said `:175`; both predate the one-executor
  refactor, and `:175` is corrected there.)
- **A CLAP main-thread callback can run inside a pump tick.** The 8 ms pump
  (`PUMP_INTERVAL_MS`, `crates/mooloop-ui/src/lib.rs:139`; the timer at
  `:12912`, its closure running to `:14367`) waits on nothing: all ten of its
  channel drains are `try_recv`, and the body holds no lock, no blocking
  `recv`, no `join` and no sleep. **One correction to the report:** the body
  is not free of *I/O*. Four `settings.save()` calls write a small TOML
  synchronously (`:12934`, `:13693`, `:13714`, `:13734`), on user-driven
  events rather than every tick. That is bounded and waits on no other
  thread, so the conclusion stands — but a callback drain added here shares a
  tick with a file write and should not be written as if the tick were
  I/O-free.

Harder than this plan assumes — though the second claim is wrong as the
report stated it:

- **`format_version` is an exact-match gate, and the passes beside it are the
  migration story.** Three sites refuse anything that is not `FORMAT_VERSION`
  outright: the bundle loader (`crates/mooloop-project/src/lib.rs:1008`), the
  preset lister (`:1155`) and `validate_envelope` (`:1212`). The report read
  the first two at `:982,1118`, and the 2026-09-19 pass corrected them to
  `:995,1142,1199` — all three moved again in the `origin/main` merge that
  same day. Search `header.format_version != FORMAT_VERSION`. Its "one bespoke fixup" is now **four**, all
  run on the way in before the repair pass: `assign_device_ids` (`:1062`),
  `assign_channel_ids` (`:1068`), `assign_track_ids` (`:1071`) and
  `migrate_retired_buffer_offset` (`:1076`). That makes the plan's position
  *stronger*, not weaker — see below.
- **"Roughly 25-30 exhaustive matches across six crates" is two-thirds
  right.** The arms are counted in `docs/plans/device-registry/README.md`, not
  in its `00-status.md` as the report said — that file states of itself that
  "`README.md` is the whole of it". The README's table sums to about 25 arms
  and its own headline is fourteen files, so the figure holds. The crate count
  does not: the table spans **four** crates — `mooloop-core` (`effect.rs` ~14
  arms, `effect_factory.rs` 2), `mooloop-dsp` (2), `mooloop-engine` (1) and
  `mooloop-ui` (2 in `lib.rs`, 1 in `settings.rs`, 3 in its `.slint` markup).
  Six is the number of crates that *name* `EffectKind` at all, which is not
  the same cost. The real warning is one the report missed: that table counts
  **one `EffectKind`**, and the Saving section above adds five variants across
  four enums. `DeviceKind` carries its own exhaustive matches that the effect
  table never counted — ten of them in `mooloop-engine`, `mooloop-ui` and
  `mooloop-session`, including `session.rs:465`, in a crate the table does not
  list at all (`:457` until the merge). Treat 25 as the floor.

### The format-migration question is closed

**Resolved 2026-09-17, no action needed**, in the same report's "Outcome".
A plugin's state is opaque bytes that the plugin versions itself, so mooloop
never reads inside one; `plugin-hosting/` keeps the format at version 1 and
adds `Project.plugins` with `serde(default)`, as the Saving section says.
That is what the tree already does four times over: every one of the four
passes above backfills defaulted fields on the way in without moving
`FORMAT_VERSION`, and
`a_song_with_no_channel_identities_loads_with_its_positions`
(`crates/mooloop-project/src/lib.rs:1477`; `:1464` until the merge) is the
test that says so in as many
words. **Do not reopen this as a version-bump question.**

## Steps

| Step | What | Closes | Rung | State |
| --- | --- | --- | --- | --- |
| 01 | Spike: `clack-host` loads and runs a CLAP; an in-repo test plugin | — (MOO-76) | new crate only | **done 2026-09-23** |
| 02 | The neutral contract: types, `Project.plugins`, TOML state, a fake plugin end to end | #26 | core, project | **done 2026-09-23** (MOO-77) |
| 03 | Parameters belong to an instance | #26 | core, session | **done 2026-09-23** (MOO-78) |
| 04 | `PluginRack`, main-thread requests, latency known at runtime | #26 | engine, session | **done 2026-09-23** (MOO-79) |
| 05 | The scanner, out of process, with its cache | #27 | plugin-host, app, settings | **done 2026-09-23** (MOO-80) |
| 06 | A headless CLAP effect in a chain | #27 | plugin-host, session | **done 2026-09-23** (MOO-81) |
| 07 | Parameters, automation, modulation and state round-trip | #28 | session, project | **done 2026-09-24** (MOO-82) |
| 08 | Plugin browser, the menu row, and the face for plugins without a GUI | #28 | **UI build**, drafted with `slint-sketch` | **done 2026-09-24** (MOO-83; remainder MOO-228, MOO-229) |
| 09 | A channel source that is a boxed node | #29 | core, engine, session | **done 2026-09-24** (MOO-84) |
| 10 | CLAP instruments | #29 | plugin-host, engine | not started |
| 11 | Plugin GUIs in their own windows | #30 | plugin-host, **UI build** | not started |
| 12 | VST3 | — | plugin-host | outline only |
| 13 | AU (macOS, optional) | — | plugin-host | outline only |

Steps 01–07 and 09–10 need no `mooloop-ui` build. Steps 08 and 11 are the
only ones that change the face contract. `AGENTS.md` wants a face contract
change made in one pass, so if both are in flight, **batch their
`main.slint` changes**. Draft in `scripts/slint-sketch` and build once
(`AGENTS.md`: a `mooloop-ui` build is about four minutes for any edit at all).

Step 09 depends only on step 03 and can run beside steps 05–08.

Step 10 builds against the pattern-note ceiling, and it is a decision rather
than an omission. A hosted CLAP instrument has no per-track clip to record a
long part into: `MAX_NOTES_PER_CHANNEL_PATTERN` is 1024 and is derived from
`MAX_PATTERN_STEPS` (`crates/mooloop-core/src/pattern.rs:13` and `:27`), so
the two ceilings cannot be raised separately. Adam settled the model question
in favour of the groovebox on 2026-09-17 -- patterns stay, there are no
per-track clips -- and `docs/CAPACITY_POLICY.md` records why, and what has to
happen first if a part ever genuinely needs more than sixteen bars. Do not
answer it here by inventing a clip.

Steps 12 and 13 are outlines on purpose. They are written in detail once
step 07 has shown which parts of the neutral types held.

## Step 01, recorded 2026-09-23 (MOO-76)

**The decision: `clack-host` 0.2 is enough. Nothing comes from `clap-sys`.**
The six checks in `crates/mooloop-plugin-host/tests/spike.rs` pass against
`crates/mooloop-test-plugin`, loaded by path. The only `unsafe` in the host
is `load_entry`, which is `PluginEntry::load`. Loading a library runs its
initialisers, and nothing else in the six needed `unsafe`. The test plugin
is `#![deny(unsafe_code)]`, and its only `unsafe` is inside
`clack_export_entry!`. The checks:

1. Load, describe, create, and activate at 48 kHz for 1..4096 frames. Blocks
   of 1, 64 and 4096 frames run through it.
2. A block of known audio comes out with the gain applied, and a parameter
   event at offset 100 lands exactly at frame 100.
3. Save state from one instance, load it into a new one, and the outputs are
   equal.
4. Parameter info (sparse ids 10/20/30, names, ranges, the stepped flag),
   `value_to_text`, `text_to_value` and `get_value`.
5. A `latency` change while processing makes the plugin call
   `request_restart`. After deactivate and activate, the plugin calls
   `latency.changed()` from inside `activate`, `get` reads 512, and an
   impulse comes out 512 frames late.
6. The sine: note on, note off, one `note_end` exactly where the release
   finishes, then `ProcessStatus::Sleep` and silence.

Processing runs on a thread of its own. The test plugin asks the host
(`thread-check`) about every call and logs `HostMisbehaving` for a call on
the wrong thread, and every check asserts that the log has none.

**How a test finds the plugin: neither of the two ways this step
suggested.** `CARGO_CDYLIB_FILE_*` belongs to artifact dependencies, which
are nightly-only (`-Z bindeps`). `CARGO_TARGET_DIR` is unset when the default
target is used, and the build box redirects it. What works on the pinned
stable toolchain: the host crate names the test plugin as a
**dev-dependency** (the plugin is `crate-type = ["cdylib", "rlib"]`), so cargo
builds its cdylib into the same `<target>/<profile>/deps/` as the test
binary, and the test looks next to `current_exe()`. This is confirmed on the
box, and CI's macOS job runs the same tests on every push.

**What deviates from `01-spike.md`.** The `.gui` variants implement the
`gui` extension's lifecycle (supported APIs, create, parent, show, hide,
resize, destroy, refusing calls made out of order), but they open no window.
A real window needs an X11 or AppKit dependency and a display, and CI has
neither. Step 11 owns the host side and its own headless tests.

**Build cost, measured on the box** (8 cores): with no sccache and the clack
crates cleaned, `cargo build -p mooloop-plugin-host -p mooloop-test-plugin
--tests` takes **27 s** wall. That covers `clap-sys`, `clack-common`,
`clack-host`, `clack-plugin`, `clack-extensions` and the two new crates. The
same run also rebuilt `mooloop-core` and `mooloop-dsp` (the rustc wrapper
changed), so the clack share is less than that. Nothing outside the two new
crates depends on clack yet.

**The manual test set: free plugins with a Linux CLAP build.** Researched on
2026-09-23 from the vendors' pages and Fedora's package index. None has been
installed or run yet.

| Plugin | Kind | GUI | How to get it on Fedora |
| --- | --- | --- | --- |
| Surge XT 1.3.4 | instrument and effects | yes | Not in Fedora's repositories. The upstream RPM (Standalone, CLAP, LV2, VST3) is on the GitHub release page linked from surge-synthesizer.github.io/downloads |
| LSP (Linux Studio Plugins) | effects | yes | `dnf install lsp-plugins-clap` (Fedora 43 and later) |
| Dexed | instrument (FM) | yes | Not in Fedora's repositories. Linux builds are on github.com/asb2m10/dexed/releases (Arch packages `dexed-clap`) |
| ChowDSP (Chow Tape Model, BYOD) | effects | yes | Linux CLAP builds from chowdsp.com (products page and nightlies). The Flathub BYOD is the standalone app, not a plugin |
| free-audio `clap-plugins` | reference effects and instruments | yes | Build from source, github.com/free-audio/clap-plugins |
| Airwindows Consolidated | effects | **no** | `LinuxVSTs.zip` (x86) from airwindows.com/consolidated, which carries the CLAP. The community `stevefolta/airwindows-clap-build` is an alternative |
| ZAM plugins | effects | yes | `dnf install clap-zam-plugins`. Not on the original list, but packaged in Fedora |

## Step 02, recorded 2026-09-23 (MOO-77)

**What landed.** `crates/mooloop-core/src/plugin.rs` holds the neutral types:
`PluginFormat`, `PluginRef`, `PluginParamInfo`, `PluginSlotId`,
`PluginSlotState`, `PluginState`/`PluginStateChunk` and `PluginStateText`.
`Project` has `plugins` (a `BTreeMap` keyed by slot) and `next_plugin_slot`.
Both are defaulted and skipped when empty or zero, so a song with no
plugins is byte-identical to one written before them. `Project::add_plugin_slot`
is the mint. `Session` carries both through `replace_project` and
`project_snapshot`, and edits neither. `EffectKind::Plugin` and
`EffectParams::Plugin(PluginSlotId)` (tag `plugin`) exist. `build_effect`
builds a `PluginPlaceholder` for a plugin device: a pass-through, which is
also what a missing plugin plays as. `PROJECT_FORMAT.md` has "Hosted plugins".

**What differs from `02-the-neutral-contract.md`.**

- `PluginState` holds only `chunks`, not a `format` as well. The
  `PluginRef` beside it in `PluginSlotState` already says the format, and
  two copies of one fact can disagree.
- The `plugins` table's keys are written and read as strings on purpose
  (`plugin::slot_table`). The bundle loader reads through `toml::Value`,
  which hands a key over as a string, and a bare `u32` key refused it. The
  first round-trip test found that.
- The "CLAP stays out" test reads `Cargo.lock`, not each manifest
  (`crates/mooloop-core/tests/plugin_formats_stay_out.rs`). The lock is what
  Cargo resolved, so a dependency renamed with `package =` can't hide from it.
  A manifest's `features = ["clack-host"]` can't cause a false hit either.
- **`EffectKind::ALL`, checked site by site.** Production: the insert menu's
  index-to-kind map (`effect_kind_from_index`), the preset listing, the
  preset-catalogue scan and the factory-bank seed, all in `ui/src/lib.rs`.
  None of them may see `Plugin`, and none does. Every other use is a test
  sweep over the native kinds (latency agreement, id freeze, container
  predicates, factory banks, the mixed-chain round trip, the engine's
  "every kind alters the signal"). `Plugin` has its own tests beside each
  one that matters. `effect_kind_index` gives `Plugin` the next free number,
  15, and no face exists for it until step 08, so a plugin row draws its
  frame and nothing inside it.
- **Not here: the fake plugin end to end with a route and a lane.** Step 02
  asked for a `FakePluginNode` driven through insert, reorder, a route, a
  lane, save, load, bypass and remove, "using the same functions the UI
  calls". Two things are missing before that test can exist. The session has
  no call that inserts a plugin device (that's the `PluginRack`, step 04).
  And a route or lane on a plugin parameter has no address until step 03's
  `ParamOwner::PluginParam`. Without that address it would go through the
  `Effect` arm of the integrity pass and be dropped (MOO-74, C.6). What's
  here instead: the save/load half (`a_song_with_a_plugin_slot_round_trips_exactly`,
  `a_song_whose_plugin_is_missing_loads_and_saves_back_unchanged`), the
  session half (`a_songs_plugin_slots_pass_through_the_session_unchanged`),
  and the engine half (`a_plugin_device_with_no_plugin_passes_the_signal_through`).
  Step 03's test (ids `{7, 1000, 4_000_000_000}` modulated, automated and
  saved) and step 04's rack take the rest. Both step files say so.

## Step 03, recorded 2026-09-23 (MOO-78)

**What landed.** `ParamOwner::PluginParam { device }`, saved as
`owner.plugin_param.device`, with `ParamAddr::plugin_param` and
`ParamKey::plugin_param`. `size_of::<ParamAddr>()` is still 16. Every
exhaustive owner match has a real arm. `ParamAddr::device()` lists every owner
now instead of ending in `_ => None`, so it answers for a plugin device. The
three sites that found a device's lanes and routes by matching `Effect`
(`slot_of`, `lane_drives_device`, `ModRack::forget_device`) read `device()`
instead. So removing a plugin device takes its lanes and routes, the same as
a native one. The two Buffer migrations in `project.rs` still match `Effect`
on purpose: they only concern the Buffer.

**The integrity pass (C.6).** A `PluginParam` address is checked only for its
device: it must be on the chain, and it must be a plugin. Its `param` is never
checked, so a missing parameter is kept. The `Effect` arm, on a channel and on
a bus, now has the `Source` arm's `descriptors().is_empty()` guard.
`a_plugin_parameter_is_never_dropped_for_its_id` pins both.

**What waits for step 07, and why.** The session's descriptor lookups
(`param_descriptor`, `modulation_destination`, `channel_modulation_destination`)
return a `&'static ParamDescriptor`, and a plugin's parameter has no such
thing. For a plugin address they return `None`, so a plugin parameter reads
as "unavailable" in the shelf and the mapping list until step 07 gives the
session the instance's values. The engine's control pass finds destinations
by walking each kind's descriptor table, and a plugin's table is empty. So a
lane or route on a plugin parameter is saved, kept and shown, but it produces
no events yet. Step 07's "nothing changes in `control_events_for_slot`" was
written against the old option 1, and it no longer holds: delivering plugin
automation needs the control pass to walk what is driven rather than what is
described, which is MOO-195.

**Not built here, deliberately.** The `DeviceParams` view, and moving
`descriptor_slots`' callers onto it. Every caller of `descriptor_slots` is
still fed a `&'static` table, and for a plugin that table is empty, so none of
them can allocate by a plugin's id. The view is only needed once something
draws or edits a plugin's parameters (steps 07 and 08), and it belongs there,
built against the real callers rather than guessed at now. Step 03's
four-billion-id test is here in the form that applies today: the address, the
save and the integrity pass all carry `4_000_000_000` and nothing is sized by
it.

## Step 04, recorded 2026-09-23 (MOO-79)

**What landed.** `mooloop_plugin_host::instance`: the `HostedInstance`
trait (the control-thread half, with no format's types in it), `Requests`
and `RequestFlags` (one atomic word, raised from any thread and drained once
a tick), `HostError` and `Lifeline`. `mooloop_session::plugin_rack`:
`PluginRack`, keyed by `PluginSlotId`, plus `Session::service_plugins`, which
the pump calls once a tick before `sync_compensation`, and
`Session::device_latency`. `Session.plugin_rack` holds it. The session now
depends on `mooloop-plugin-host`, and `plugin_formats_stay_out` still holds:
the session names the host crate, never `clack`.

**How it differs from `04-the-plugin-rack.md`.**

- **Teardown uses a lifeline, not a slot id on the reclaim message.** Every
  processor an instance builds holds a clone of the entry's `Lifeline`. A
  removed entry is marked dying and dropped by `PluginRack::collect` once
  it's the last holder. Processors are only dropped on the control thread,
  in `EngineHandle::poll` or with a retired project. So the instance goes
  strictly after its processor, and the engine's reclaim path needed no
  change. That covers a whole-project install too, which the step file
  didn't mention: `replace_project` closes the rack.
- **A restart is swapped in by slot.** `effect_resource_key` gives a plugin
  device its slot id as the resource key, so `ReplaceEffect` from a
  restart that arrives after the device was removed finds a different key
  and does nothing. That is the step's own suggestion, and
  `a_restart_requested_after_removal_does_nothing` holds the rack's half.
- **Latency is one lookup, not a second path.** `mooloop_core::chain_latency_with`
  is `chain_latency` with each device's own latency supplied by the caller.
  `Session::latency_plan` passes `device_latency`, which asks the rack for
  a hosted plugin (0 when it's missing) and the kind for everything else.
  `sync_compensation` already derives and diffs the plan every tick, so a
  latency change needs no event handling at all.
  `compensation_follows_a_plugins_reported_latency` pins it.
- **Not here: the engine's own latency reads.** The engine's install path
  (`chain_latency` in `render.rs`) and a container's run latency still read
  the kind, so they see 0 for a plugin. On install the engine compensates
  as if every plugin were the placeholder, and the session's next
  `sync_compensation` corrects it per target. A plugin *inside a container*
  is not corrected, because the dry-path delay a container holds is sized at
  install. That is step 06's, where a real processor first reaches a chain.
- **Not here: waiting on close and exit.** The rack can say how many
  entries are dying (`PluginRack::dying`), but nothing waits on it with a
  timeout yet, and no driver is stopped first. Nothing hosts a plugin until
  step 06, which inserts the first instance and owns that wait.
- **Blocker 7 still stands.** Every structural edit reinstalls the project
  from the document, which rebuilds each plugin device as a placeholder and
  retires the hosted processor. Step 06 has to hand the rack's processors to
  the install rather than letting `build_effect` rebuild them.
- **The counting-`GlobalAlloc` check.** `soak_tests.rs` still passes, but
  nothing in this step creates, destroys, rescans or serializes a plugin on
  the audio thread, because nothing reaches the audio thread except a
  processor. Step 06's real processor is where that check has something to
  catch.

## Step 05, recorded 2026-09-23 (MOO-80)

**What landed.** `mooloop_plugin_host::scan`. Every new or changed `*.clap`
on the search paths is loaded only in a child process, the shipped binary
itself run as `mooloop --scan-plugin <path> --deadline-ms <n>`, which
`crates/mooloop-app/src/main.rs` checks for **before logging**, so the child
writes no log, reads no settings, and opens no audio client and no window.
The child prints one JSON `ChildReport` between two marker lines, because a
plugin may print to stdout while it loads. The parent (`scan::scan`, blocking,
run by the app on a `plugin-scan` thread after the window is built) kills a
child at `scan-timeout-s` (default 10) and records each file in
`PluginCache`, `<config>/plugins.toml`, keyed by canonical path with mtime and
size. **A failure is cached like a success**, so a plugin that crashes or
hangs costs one child per change of the file, not one per startup.
`UiSettings.plugins` (`extra-paths`, `scan-timeout-s`, `scan-on-startup`)
exists; its Preferences page is step 08. The contract step 06 reads
(`PluginCache::load`, `plugins()`, `resolve(&PluginRef)`, `ScannedPlugin`) was
announced on the push's contract board before landing.

**The test that pins answer 4.** `plugin-host/tests/scan.rs`,
`a_crashing_or_hanging_plugin_costs_one_child_and_is_never_relaunched`: the
test plugin copied as `…crashes-on-scan.clap` aborts inside its entry's
`init`, and as `…hangs-on-scan.clap` never returns from it
(`mooloop_test_plugin::CRASHES_ON_SCAN`, `HANGS_ON_SCAN`). Both are scanned,
with a good copy, a zero-byte file and a shell script, and the test process
survives to find four plugins, `crashed`, `timed-out` and two `load`
failures, reload them from disk, and launch **zero** children on the second
scan. `app/tests/scan_child.rs` runs the real `mooloop` binary as the child
with every directory it could write to pointed at an empty scratch
directory, no display and no JACK, and asserts it answers and leaves the
directory empty.

**How it differs from `05-the-scanner.md`.**

- **A shell script that sleeps does not hang a scan.** The step's test asked
  for one as the timed-out case, but `dlopen` refuses a text file at once, so
  it is a `load` failure. The hang has to happen inside a real library's
  initialiser, which is why the test plugin grew the two names above.
- **Failure kinds are six, not one `Failed`:** `launch`, `load`,
  `incompatible` (another CLAP version, no plugin factory, or a factory that
  lists nothing), `crashed`, `timed-out` and `bad-output`. That is #27's
  "distinguish scan failure, incompatible plugin, load failure". A plugin the
  factory lists but cannot create is kept with its `error` set rather than
  failing the file.
- **The child creates each plugin once, never activates it,** to read what
  only an instance says: audio port channel counts, note ports and whether it
  has a GUI. Step 06's stereo-in/stereo-out check can read `audio-inputs` and
  `audio-outputs` without loading anything.
- **The child never unloads the library.** The entry is leaked and the
  process exits, so a plugin that crashes in `deinit` does not turn a good
  report into a failed scan. It has its own deadline (twice the timeout plus
  five seconds), so a hung child outlives a parent that quits mid-scan by
  seconds, not forever.
- **`resolve` returns the `ScannedPlugin`, not a `PathBuf`;** its `path` is
  the file. The newest version wins by the numbers in the version string,
  then search-path order, and differing copies are logged.
- **Not here: progress in the window.** Nothing in the window reads the cache
  until step 08, so the scan reports to the log, not through
  `slint::invoke_from_event_loop`. Step 08 adds the progress and the
  "rescan all" button (`PluginCache::clear_failures` exists for it).
- **The test-only child.** A test in the plugin-host crate cannot launch
  `mooloop`, so the crate has a `mooloop-scan-child` binary that calls the
  same `run_child_from_args`. It is never packaged; the packages carry
  `mooloop`, which is the child.

**Adam's plugin folders.** See the MOO-80 closing comment for the scan of
`/usr/lib64/clap` on his laptop.

## Step 06, recorded 2026-09-23 (MOO-81)

**What landed.** `mooloop_plugin_host::clap`: `ClapInstance` (the
`HostedInstance` for a CLAP plugin), `ClapProcessor` (its `AudioNode`), and
`ClapOpener`, which finds a `PluginRef` in the scanner's cache and rereads the
cache whenever a scan rewrites it. `PluginOpener` is the neutral trait
behind it, so the rack's tests use a fake. The rack (`session/src/plugin_rack.rs`)
now hosts what a song names. `Session::service_plugins` opens every slot a
device names that has no instance. It swaps a processor into every plugin
device that lacks one, with `ReplaceEffect` keyed by the slot, and pulls a
stale one back first. `Session::insert_plugin_effect` is the session path:
mint a slot, insert the device, open the plugin, and install its processor,
or the placeholder and the reason. An export renders hosted plugins
(`OfflineRenderer::render_with_plugins`, fed by
`Session::export_plugin_processors`). The app sets the CLAP opener at startup,
exports with plugins, and waits for the rack at quit (`AppUi::retire_plugins`).

**The one rule the rack runs on.** CLAP allows **one processor per
instance**: `activate` hands it out and `deactivate` wants it back. Step 04's
`HostedInstance::restart` assumed the new processor could be built while the
old one ran, and CLAP cannot do that, so `restart` is gone. What replaces it
is one rule: *every live instance whose processor is not out gets one, on the
next tick after its lifeline is alone.* That covers four cases:
- a song opening, where the install built placeholders;
- an install the carry plan did not carry, where the old processor comes
  back with the retired generation;
- a restart, where the rack swaps the placeholder in to pull the processor
  back;
- a new sample rate, which is a restart for every plugin.

A processor the engine keeps refusing is rebuilt at most three times in a row
(`MAX_ATTEMPTS`), so a disagreement between document and engine cannot turn
into an activate-deactivate every 8 ms.

**How it differs from `06-a-headless-clap-effect.md`.**

- **Blocker 7 was half-solved already, and `replace_project` undid the
  other half.** The carry plan (MOO-137) moves a plugin device's live
  processor across a structural install when its row is unchanged. But
  `Session::replace_project`, which every paste, move, delete and undo goes
  through, closed the whole rack, so the carried processor's instance was
  marked dying. That left a processor running with no instance that could
  restart it or report its latency. `PluginRack::retire_except` keeps an
  instance when the incoming song records the same plugin in the same state
  in its slot, which a structural edit always does because it snapshots the
  session. It retires the rest. The parameter list and the pinned ids don't
  count, since they are what the song remembers *about* the plugin. `a_structural_install_keeps_the_hosted_plugin_live` pins it.
  An install that does rebuild the device, say an undo of its bypass, goes
  through the one rule above: the placeholder plays for a tick or two, then
  the processor is swapped back, with its state (the instance never went).
- **The engine's own latency reads were not changed for live playback.** The
  engine installs placeholders and compensates as if every plugin were zero,
  and the session's `sync_compensation` corrects it per target on the next
  tick, as step 04 left it. An export never runs a pump, so
  `RenderState::host_plugins` swaps the processors in and re-derives the plan
  with their latencies (`install_compensation_with`). The container case,
  where a plugin inside a Chain or a Layer has its dry ring and branch
  alignment sized for zero latency, was filed as MOO-212 and is fixed: one
  list of rings (`mooloop_core::container_rings_with`) is sized with the
  real latencies by the interface after an edit, by the session when a
  plugin's latency arrives or changes (`Session::resize_plugin_containers`),
  and by `host_plugins` for an export
  (`container_tests::a_hosted_plugins_latency_sizes_the_container_around_it`).
- **Buffers are copied, never passed in place.** `clack-host` takes input and
  output as separate `&mut` slices, so in place would need aliasing. Two
  stereo copies of at most `MAX_BLOCK_SIZE` frames per block are the price.
  Scratch is allocated at activation. The event buffer is filled in order and
  never sorted (`EventBuffer::sort` allocates). The plugin's own parameter
  output goes into an `rtrb` ring of 1024; overflow is counted
  (`ClapInstance::dropped_param_events`), and step 07 reads it.
- **Ports: exactly one stereo input and one stereo output.** "Main input and
  output stereo" alone would admit a sidechain port, and CLAP expects a
  buffer for every declared port. Sidechains are in the plan's "Deliberately
  not", so any extra port is refused as `Incompatible`. The scan cache
  already says so without loading anything (`audio-inputs = [2]`).
- **Activation uses `mooloop_dsp::MAX_BLOCK_SIZE` (8192) as the maximum
  block,** not the driver's. The executor never hands a node more than that,
  and it holds across a buffer-size change with no reactivation.
- **One CLAP threading deviation, and it is written down.** `stop_processing`
  belongs on the audio thread, but a processor leaving a chain gets no more
  calls there. So `deactivate` stops it on the main thread, which is what
  `clack-host` does for a processor that was dropped while started.
- **An export uses second instances.** The live processors are the engine's,
  and one instance cannot have two. `Session::export_plugin_processors`
  captures each live plugin's state, opens a second instance with it on the
  control thread, builds its processor, and leaves the instance in the rack's
  graveyard until the export drops the processor.
- **The quit wait re-enters the event loop.** The pump owns the
  `EngineHandle`, so once the window has gone `AppUi::run` sets a deadline
  (2 s) and runs `slint::run_event_loop_until_quit` again. The pump then only
  pulls processors back, polls, and collects, and quits when the rack is
  empty. An instance still held at the deadline is leaked, never dropped
  under a processor that may be running.
- **Not here: saving captures state only when the plugin says it changed.**
  A plugin's `state.mark_dirty` captures its state into the song. The
  window's Save does not ask every plugin (`Session::capture_plugin_states`
  exists, and exports use it). Nothing in the app can change a plugin's state
  before step 07 gives it parameters, so this is step 07's.

**Found and filed.** MOO-211 (Sequencing): the same song through the executor
at 64 frames and through the export path at 512 frames landed some notes one
frame apart, with no plugin involved. It was fixed the same day (`184d0de3`),
and the step's parity test now holds a 64-frame callback against the
512-frame export directly. MOO-212 (Effects): the container case above.

**The tests.** Engine (`engine/src/plugin_host_tests.rs`, with the in-repo
test plugin):
- export == executor at 512 frames and at 64, to the sample;
- the export is deterministic, and differs from the song with the plugin
  missing;
- a 64-frame plugin latency comes out as the whole mix 64 frames late in an
  export;
- 64 channels, each with the test gain, and a processor swapped mid-run,
  allocate and free nothing per block at 64 and 1024 frames.

Session (`session/tests/clap_effect.rs`): the case, headless. Insert through
the session, export twice, save, reopen present (same export), reopen missing
(placeholder within 1.5e-8, slot kept byte for byte). The rack
(`plugin_rack.rs`): the one rule, restart, rate change, missing then found,
the zombie, insert, export instances, and quit.

## Step 07, recorded 2026-09-24 (MOO-82)

**What landed.** A hosted plugin's parameters are driven by lanes and routes,
live and in an export, at the control rate; the window's Save asks every
plugin for its state; the plugin's own parameter changes are read; and a
plugin's own edit is an undo step.

- **The control pass walks what is driven, for a plugin.**
  `EffectChain::control_events_for_slot` (`engine/src/render.rs`) has a
  plugin branch, `plugin_curves`. It collects the ids that a route in the
  channel's rack or a lane under the playhead names on this device
  (`ModRack::destinations`, the new `Sequencer::visit_automation_targets_at`),
  once each, into a fixed array, and resolves each against the running
  processor's own list (`AudioNode::hosted_param`, answered by `ClapProcessor`
  from a table sorted at activation). That is MOO-195's shape for the one kind
  that cannot do without it; the native branch is unchanged, and MOO-195 can
  fold it in without undoing anything. An id the plugin does not have (it is
  missing, or a rescan took it) drives nothing, and its lane or route is kept.
- **A lane writes the value; a route writes an offset.** A lane's normalized
  value becomes the plugin's plain units (linear, a stepped parameter on a
  whole position: `HostedParam::plain`) and reaches the plugin as
  `Event::ParamValue`, which the adapter turns into a CLAP param-value event
  at the same frame. A route is an **offset**, `Event::ParamMod`, turned into
  CLAP's parameter modulation: the plugin owns its value, and its own GUI may
  move it under the route, so the route rides over whatever the plugin holds
  rather than over a base this side would have to keep in step. The
  processor zeroes an offset whose route has gone (CLAP's modulation holds
  until changed). `ControlCurve` gained a `kind` (`Value`, `Offset`) and the
  default `apply_curves` emits `ParamMod` for an offset row; natives only
  ever get `Value` rows. The policy is one function both sides ask:
  `ModDestinationDescriptor::for_plugin_param` (continuous and modulatable).
- **A knob edit is the ordinary `SetEffectParam`**, carrying the plugin's id
  and plain value; `RenderState::set_effect_param` queues it for a plugin
  device unless a lane is writing that parameter (a route does not hold it
  back: it is an offset). `Session::set_plugin_param(row, index, normalized)`
  is the face's path, by **dense index**; `plugin_param_id` /
  `plugin_param_index` are the only conversions, and an id never crosses as
  an `i32` (`session/src/plugin_params.rs`). The test plugin's `Nudge` has id
  `4_000_000_000` and goes through the knob path, a route's policy and a
  lane's address and back whole.
- **The plugin's own changes are read.** `HostedInstance` gained
  `param_value`, `drain_param_events` and `dropped_param_events`
  (`PluginParamEvent` moved to `instance.rs`). The rack keeps each live
  plugin's values by dense index, read from the plugin when it opens or
  rescans and updated from its reports each tick. Nothing it reports is sent
  back to it.
- **A plugin's own edit is one undo step** (the orchestrator's condition).
  A gesture that ends, a run of lone values gone quiet for half a second, a
  `mark_dirty`, or a value the session sent it, marks the slot edited. The
  pump then takes a snapshot, calls `Session::capture_plugin_edits`, and
  records the two as "Plugin Edit". Why it must be a step: undo installs a
  whole snapshot and a snapshot carries each plugin's state, so an edit
  captured with no step of its own would be reverted by undoing an *earlier*
  edit. `a_plugins_own_edit_is_one_undo_step_and_survives_an_unrelated_undo`
  pins it: nudge, an unrelated edit and its undo keep the same instance
  still nudged, and undoing the plugin's step reopens it at its old gain.
  MOO-81's `StateDirty` path, which captured and dirtied with no step, is
  gone into this one.
- **Save captures every plugin.** The window's Save calls
  `capture_plugin_states` before it snapshots. A plugin that fails to save
  keeps its last good state.
- **A refused state opens with the defaults and is kept.** The rack tries the
  saved state, and when the plugin refuses it opens it with none; the song
  keeps the refused bytes (no capture overwrites them) until the plugin is
  edited, and the device reports why. Added and removed ids since the song
  was saved are logged when it opens.
- **The adapter cuts a block at every parameter change.** Found with the
  LSP filter: it reads its parameters once per process call, so with its
  cutoff automated the export at 512 frames and playback at 64 differed by
  0.017 while the test plugin, which applies each event at its frame, was
  bit-identical. `ClapProcessor::process` now makes one call per piece
  between parameter events. The engine's control ticks sit on a 32-frame
  grid every block size shares, so the pieces are the same whatever the
  callback, and a block with nothing moving is still one call. CLAP allows a
  host to split a block, and a plugin that is sample-accurate hears the same
  thing either way. That took the difference to 0.00037. The rest was sleep:
  between hats the plugin's input is silent, the engine skipped it, the
  skipped blocks' values never reached it, and it woke gliding from a stale
  cutoff after a sleep whose length depends on the block size. A processor
  that carried a parameter change in its last block is now never at rest
  (`ClapProcessor::driven`), so a lane or route keeps its plugin awake. What
  remains differs only in subnormals: the executor flushes them and an export
  does not (MOO-223, Engine).
- **Offline == realtime.** `mooloop_engine::live_check::play_through_executor`
  (the `test-support` feature, `#[doc(hidden)]`; not API) plays a song
  through the real executor with no driver, swapping processors in down the
  command ring as the rack does live.

**How it differs from `07-parameters-and-state.md`.**

- **A route is not base-plus-offset on this side.** The step said values
  travel in plain units and "automation and modulation need no
  plugin-specific code". A lane does travel that way. A route cannot: the
  base is the plugin's, so it travels as CLAP's non-destructive modulation.
- **No gesture wrapping of a UI drag yet.** Step 08 draws the face, and a
  drag's begin and end belong to it; `set_plugin_param` is the value half.
- **The full ring is a logged number, not `DeviceTelemetry`.** The count
  lives with the instance on the control thread
  (`HostedInstance::dropped_param_events`), where the rack reads it each
  tick and logs each growth; routing it through the audio thread's telemetry
  would copy a number the control thread already has.
- **Hidden parameters** are refused by `set_plugin_param`; a route or lane
  on one is not refused, because only a face hides.
- **Presets are not here.** The plugin device preset (`effect-plugin`
  envelope) is Effects' and Document's, filed as MOO-222.
- **The Slint half of the four-billion id** is step 08's: this step pins the
  session side of it, where the index is made and turned back.
- **Save during an open gesture** captures the state mid-gesture; the step
  the pump records when it closes then holds the rest. Not worth a flush.

**The tests.** Engine (`engine/src/plugin_automation_tests.rs`): a lane on
the test gain lands on every control tick (each tick's gain checked against
the lane at its first frame) and the executor at 64 and 512 frames is the
export to the sample; an LFO route offsets the plugin's own -6 dB, holds per
tick, goes both ways, and plays the same live; an offset whose route went is
zeroed; the plugin's own gesture comes back on its ring and a full ring is
counted. Session (`session/tests/plugin_params.rs`): the undo case above; a
knob edit sends the id and closes as one edit when quiet; a refused state
plays the defaults and is kept; an automated plugin saves, reopens, goes
missing, is saved, comes back, and exports the same file. `plugin_params.rs`
unit tests: the id above `i32::MAX` through the knob and route paths; lanes
only on automatable parameters the song knows.

**The real plugin** (`examples/clap_automation_case.rs`, run on the laptop
against `/usr/lib64/clap/lsp-plugins.clap`, `in.lsp-plug.filter_stereo`,
its `Frequency` lane falling from 0.9 to 0.3 of its range across a two-bar
loop, on the hats' channel): all six measurements passed on 2026-09-24.
- Each of the twelve identical hats was darker than the last against the
  same song with the lane held flat (high-pass energy ratio 1.000, 0.397,
  0.172, 0.069, 0.006 ... 0.000).
- Deterministic, and saved and reopened with no repairs, lanes and slot
  intact.
- Reopened present: an identical export. Reopened missing: the dry loop
  within 1.5e-8, slot and lanes kept.
- The executor at 64 and at 512 frames against the export: equal to below
  the smallest normal float, which took the two adapter changes above
  (0.017, then 0.00037, then subnormals only; MOO-223).

LSP reports its stepped parameters (filter type, slope) with a range of
0..1 and two positions, and names every position by `value_to_text` on that
range. The host rounds a stepped lane to a whole position, as CLAP says a
stepped parameter's values are, so a lane on one of LSP's could only reach
its first and last choice. Nothing in this step automates one; it is noted
for step 08's face, which will show these parameters.

## Step 09, recorded 2026-09-24 (MOO-84)

**What was left to do.** MOO-56 had already made the strip's source one
boxed `SourceNode` with an `InstallSource` command, so the plan's `hosted`
field, `active_source` arms and silent-`None` dispatch had nothing to attach
to. What the step still asked for, and what landed:

- **The persisted variants.** `DeviceKind::Plugin` (`plugin`, label
  "Plugin"), `GeneratorParams::Plugin(PluginSlotId)` and
  `ChannelSource::Plugin(PluginSlotId)` (`source.type = "plugin"`, `state` =
  the slot). The slot's plugin, parameters and state live in `Project.plugins`
  as a plugin effect's do. `DeviceKind::Plugin` has no native table
  (`descriptors()` is empty), and its default block names
  `PluginSlotId::UNASSIGNED`.
- **`mooloop_dsp::HostedSource`**, the `SourceNode` that wraps a node the
  engine did not build: the slot it plays and an optional processor. Without
  one it renders silence, which is both the moment before the rack swaps the
  processor in and the missing-instrument placeholder. With one it clears the
  bus and hands the processor every event the channel carries (notes, chokes,
  parameters) at its own frame. `build_source`'s `Plugin` arm builds it
  empty.
- **The processor moves, not the source.** `StructuralCommand::HostSourceProcessor
  { channel, slot, node: Option<_> }` puts a processor into the channel's
  hosted source, or `None` pulls it out, through
  `SourceNode::host_processor` (native sources refuse). It lands only when
  the channel is still running the hosted source for that slot, the way
  `ReplaceEffect` is keyed by a plugin effect's slot; otherwise the processor
  comes straight back as `StructuralReclaim::HostedProcessor`. Moving the
  processor rather than replacing the source keeps the channel's slot one
  node and one kind across the swap.
- **The rack hosts sources by the same rule** (step 06's "every live
  instance whose processor is not out gets one"): a channel playing a slot
  counts as naming it (`named_plugin_slots`), an install goes into its source,
  a pull-back takes the processor out and leaves silence, and an export's
  `host_plugins` and `live_check::play_through_executor` fill sources too.
  `Session::set_plugin_source(channel, plugin, handle)` is the session path,
  the counterpart of `insert_plugin_effect`: the channel changes source as a
  source change does (a name nobody typed follows the device), the plugin is
  minted a slot and opened, and a hosted source arrives with its processor
  already in it, or silent with the reason kept.
- **A strip is carried only onto its own slot.** The carry plan compares
  projects, and two hosted sources are the same kind whatever they play, so
  `carry_strips_from` also compares the slot.
- **The interface**: `device_kind_to_int`/`from_int` give `Plugin` the number
  8, after the eight, so no kind moves; `SOURCE_KINDS_IN_PICKER_ORDER` leaves
  it out on purpose until step 08 has a browser to choose a plugin with
  (main's condition on the ack); preset directories would be `plugin`.

**How it differs from `09-a-boxed-channel-source.md`.** No ninth field and
no `InstallSource { slot }`: MOO-56 overtook both, as the plan's 2026-09-23
note foresaw. `SetChannelSource` still names only a kind, so a hosted source
built from it names no slot and takes the first `Plugin` block it is sent;
the session's own path never goes that way, it installs the finished source.
`source_base` for a hosted source is its slot, not the kind's defaults, so
pushing the base back names the same slot.

**Not here, and where it went.** A restart or rate change pulls a sounding
instrument's processor out with no fade (the effect swap's MOO-213 hold does
not cover sources); a plugin source's latency is not compensated; a strip an
install does not carry is rebuilt silent and gets its processor back a tick
or two later, as the rack's rule does for effects, with the voices it held
lost (as a native strip's are); a generator preset of a plugin channel would
carry only the slot number; and the source's parameters have no address yet
(MOO-74's `PluginParam { device }` needs a `DeviceId` for the source slot,
which is step 10's). Step 10 (MOO-85) is next and takes the notes.

**The tests.** Engine (`engine/src/plugin_source_tests.rs`, a fake
instrument: a cosine per note id): a pattern played through the export path
and through the executor at 512 and 64 frames is the same to the sample,
every note starts a whole number of steps after the first and lasts its own
length exactly, and with no processor the channel is silent; switching to the
sampler and back, pulling the processor out, offering another slot's and
putting one back allocate and free nothing in the callback, and each
displaced box comes back on the reclaim ring; a hosted source is carried only
onto its own slot. `mooloop-dsp`'s `hosted_source.rs` unit tests. Session
(`plugin_rack.rs`): `set_plugin_source` installs the hosted source with its
processor, the snapshot says `ChannelSource::Plugin`, a song opened from it
has the processor swapped in by slot, a restart pulls it out and puts the
next one back; a missing plugin makes a silent source with the reason kept.
Project: `a_song_whose_source_is_a_plugin_round_trips` (no repairs, equal,
the second save byte-identical).

## Found after step 07: the swap fades (MOO-213, 2026-09-24)

The rack's `ReplaceEffect` between a processor and the placeholder used to
switch in one sample. The test gain at -12 dB stepped by 0.19 against the
continuity family's bound of 0.008. Now:

- **The executor holds a plugin swap** until the slot has faded out of the
  path, the same way it holds a removal (`RenderState::plugin_slot_ready_for_swap`
  over `effect_slot_vacated`: about 35 ms, 100 ms at most, and at once for a
  slot that has never rendered). The slot then fades back in with the new
  node. An export's `host_plugins` swaps before anything renders and is
  never held, so exports are unchanged. A swap the chain would refuse is not
  held and fades nothing.
- **A restart keeps the chain's timeline.** The pull-back placeholder is as
  late as the plugin it replaces (`PluginPlaceholder::with_latency`, at the
  instance's current latency, which is also what `device_latency` compensates
  for). `replace_if_kind` keeps the live dry ring when the incoming one is the
  same length, and a latent incoming node runs its latency at the dry path
  before it fades in (`EffectSlot::rejoin_hold`), so it has caught up with the
  ring it fades against.
- **Not covered:** a swap between different latencies, which is a missing
  plugin found late or a restart that changes the latency. The channel's
  timeline moves by the difference, as it does for any latency change, and
  the dry ring starts empty.
- **Cost:** each swap holds the command ring for about 35 ms. A quit that
  pulls back many plugins does it one after another, so at 57 or more
  sounding plugins it runs past the two-second quit wait, and the
  instances still out are leaked, as the wait already allows.

Pinned by `continuity_tests::swapping_a_hosted_plugin_for_its_placeholder_and_back_is_continuous`
and `swapping_a_latent_hosted_plugin_…` (64 frames).

## Step 08, recorded 2026-09-24 (MOO-83)

**What landed.** A plugin goes in a chain from the window. The join's menu
ends in **Plugin…** (a `PluginTypeRow`, not an `EffectTypeRow`: it inserts
no kind of its own, and `effect_preset_menu.rs` counts `EffectTypeRow`s as
kinds), which opens the browser's third tab, PLUGINS, aimed at that join.
The tab lists the scanner's cache, re-read whenever the tab opens (that is
how a scan finishing after startup reaches the window), one row per plugin
id, the copy `PluginCache::resolve` would open. A double-click, Enter or a
drop calls `Session::insert_plugin_effect` through a small `CommandSink`
over the window's two command queues (`plugin_ui::QueuedSink`), and it is
one undo step. The face, `plugin-device.slint`, draws every parameter the
plugin does not hide as the shared knob, with the plugin's own text; it is
drawn by a predicate (`slot.is-plugin`), like a container, because its
parameters are not a kind's table. A missing or failed plugin keeps its face
from the list the song remembers, greyed, with the reason as a badge.

**How it differs from `08-the-plugin-face.md`.**

- **The face pages rather than pins.** Every visible parameter, in the
  plugin's order: up to six on one unit, twelve a page on two, with `<` `>`.
  Pinning, the sidebar list and the module grouping are MOO-229, with the
  Preferences page and the rescan actions.
- **Every parameter is a knob.** A stepped one snaps to its positions. The
  segmented selector for eight or fewer positions is MOO-229, and LSP's
  stepped parameters need care there: they report two positions on 0..1 and
  name more (step 07 above).
- **No modulation ring, and no naming a parameter from its knob** for a lane,
  a route or MIDI learn; nor the missing-parameter drawing in the lane menu
  and on the shelf. MOO-228.
- **The drag's undo is the pump's step, held for the gesture.** Step 07 left
  "a drag's begin and end belong to the face". The shared knob already
  brackets every drag, wheel notch, reset and typed value with
  `Gesture.begin()`/`end()`, so the pump's "Plugin Edit" capture now waits
  while a gesture is open (`record_finished_plugin_edits`). A drag that
  pauses past the rack's quiet time is still one step. No CLAP gesture
  event is sent to the plugin: CLAP's gesture events are the plugin's to
  report, not the host's to send.
- **A face follows the plugin every pump tick** (`refresh_plugin_faces`):
  each slot keeps one parameter model for its life, updated row by row, and
  the plugin is asked for a value's text only when the value moved. A
  republished model would rebuild the knobs and drop a drag.
- **Instruments, second commit, after step 09 (MOO-84) landed.** The
  add-channel menu's **Add Plugin…** (its own `MenuRow` after the
  `SourceKinds` rows, so `SOURCE_KINDS_IN_PICKER_ORDER` is unchanged) opens
  the PLUGINS tab; an instrument picked there adds a channel and calls
  `Session::set_plugin_source` on it, as one undo step, named after the
  plugin. It plays no notes until step 10, and the row says so. The CLAP
  opener still refuses anything but one stereo input and one stereo output
  (`ClapOpener::open`, `check_ports`), so the test sine and any instrument
  with no audio input make a channel whose plugin is refused, with the
  reason in the status bar: step 10 decides which port layouts a source
  takes. `MenuRow` is an accessible button now, so a menu row can be found
  by role and pressed (`an_instrument_from_the_add_channel_menu_…`).

**The tests.** `ui/src/plugin_ui_tests.rs`, in the real window through
`window_probe.rs`, with the handlers `AppUi::new` wires (`plugin_ui::wire`)
and the in-repo test gain found through a scanner cache: the join's
"Plugin…", the tab's rows (the instrument offered but not as an effect, a crashed file
greyed with its reason), a click that only selects and a double-click that inserts;
the face's Gain knob reading the plugin's "0.0 dB"; a wheel turn inside a
gesture recorded as one "Plugin Edit" only after the gesture closes, however
long it waited; the command carrying the plugin's id; then saved, reopened
with the same gain on the face, and exported at that gain against the dry
loop. A missing plugin's face says so and its knob sends nothing.
`ui/tests/plugin_formats_stay_out.rs` holds the window to the neutral types,
now that `mooloop-ui` depends on the host crate.

## The test plugins

CI cannot install third-party plugins, so step 01 builds **an in-repo CLAP
test plugin** with `clack-plugin`: a gain effect with a latency switch, a
note-driven sine instrument, and a mode that has no GUI and one that
declares a GUI. Every step's automated tests use it.

Listening and manual acceptance use free Linux plugins from more than one
vendor. The candidates are Surge XT (instrument, effects, GUI), LSP, Dexed,
ChowDSP, the free-audio `clap-plugins` reference set, and an Airwindows
build (no GUI). Step 01 records which of them actually ship a Linux CLAP
build today.

## Deliberately not in this plan

These are kept from #10, plus one addition.

- Multi-output instruments, sidechain inputs, and port layouts other than
  stereo. (Sidechain waits for the typed-edge work `FOCUS.md` names.)
- Note expression and MPE, and routing notes that a plugin generates.
- Graph-wide plugin delay compensation beyond what the chain already does.
- Crash isolation while processing.
- Embedding a plugin GUI inside the rack.
- **Windows.** It is low priority and has no step. Nothing in the neutral
  types assumes a platform, so adding it later is a matter of search paths,
  a window handle, and CI.
