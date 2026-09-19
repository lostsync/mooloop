# Plugin hosting — plan status

Linear: [MOO-11](https://linear.app/mooloop/issue/MOO-11/clap-plugin-hosting-effects-instruments),
filed before the docs/plans-into-Linear migration and not yet rewritten to
mirror this file step-for-step the way MOO-5/16/30 do. The GitHub issue
numbers below (`#10`, `#26`-`#30`) predate Adam's move away from GitHub
issues; MOO-11 is the live tracking issue.

**Written 2026-09-16. Nothing has landed.** Adam asked for it directly:
*"let's go ahead and plan out how we'll add CLAP support. Later we'll add
VST/3 and AU. instrument support as well."* `SCOPE.md` already put CLAP in
for 0.2.0 (item 9). This plan is outside the `FOCUS.md` sequence for the same
reason `coreaudio-driver/` is: Adam asked for it.

GitHub #10 and #26–#30 were written before this plan and describe the same
order. This plan replaces their ordering and keeps their "done when" lists.
Each step says which issue it closes.

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
- `PluginState { format, chunks: Vec<(String, Vec<u8>)> }`. It holds chunks,
  not a single blob, because VST3 saves component state and controller state
  separately. CLAP writes one chunk.
- `PluginSlotId(u32)`, `Copy`, minted **per project**, not per chain.
  `DeviceId` is minted per chain ("every chain mints its ids from zero"), and
  a per-chain key would have to be renumbered whenever a channel moves.

### Two halves per plugin

`clack-host` already splits a plugin into two halves, and this plan follows
that split.

- **`PluginInstance`** stays on the control thread, which is the UI thread
  (`ui/src/lib.rs:12364`). It owns the loaded library, the parameter info,
  state save and load, the GUI, and `on_main_thread`.
- **`PluginProcessor: AudioNode + Send`** is what goes through
  `StructuralCommand::InstallEffect` (`engine/src/lib.rs:201`; the citation
  read `:175` until 2026-09-19, when it had drifted), the way every
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

A plugin parameter's saved address is
`ParamAddr { scope, owner: Effect { device } | Source, param }` with the
**plugin's own `u32` id** in `param` (`core/src/modulation.rs:64`). That
field needs no change. Arrays sized by id (`descriptor_slots`,
`session/values.rs:125`) become sparse-safe. A dense per-instance index
exists only in session and UI.

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
  variants with permanent `serde(rename)`s. All of them stay `Copy`
  (`effect.rs:2663,3128` derive `Copy`, and so does `GeneratorParams`,
  `generator.rs:877`).
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

## Blockers (verified in source on 2026-09-16)

Items 1–4 are `SCOPE.md` §5's list. Items 5 and 6 were found while writing
this plan.

1. Parameter descriptors are static per kind (`effect.rs:128`). The id-sized
   arrays assume small, dense ids, and `EffectSlotRow` has fixed `p0..p9`
   (`main.slint:211`). **Step 03.**
2. `EffectParams` is a closed enum and it is `Copy`. **Step 02**, through
   the side table.
3. Latency comes from the effect kind. **Step 04.**
   Added 2026-09-18 from `reports/fable-2026-09-18.md`: compensation is not
   the only per-install table this touches. `AudioTapBank` is **rebuilt whole
   on every install** (`engine/src/render.rs:3379`, deliberately, so an
   offline render that runs no pump still gets its tap plan). A plugin's
   latency is known only after activation and can change while it runs, so a
   latency change that today implies an install implies a tap-bank rebuild
   too. Both need the identity work of `plans/archive/channel-identity/` step 05
   before a plugin's latency can move without tearing the graph down —
   the same prerequisite blocker 7 records for step 06.
4. `ChannelStrip` holds its eight generators as concrete fields
   (`engine/src/render.rs:2189`; the citation read `:2154` until 2026-09-18,
   when it had drifted) and has no slot for a boxed source. **Step 09.**
   Note delivery to those eight is a closed match calling three different
   method shapes rather than one trait method, and unifying it is worth
   doing while every source is still native --
   `reports/fable-2026-09-18.md`, Plan D.
5. Once a node is installed, the control thread has no handle to it, and
   nothing lets the audio thread ask for main-thread work. **Step 04.**
6. The effect menu (`device-rack.slint:172-196`, 14 hard-coded rows) and the
   source popup (`main.slint:3619`) are not driven by data. **Step 08.**
7. **Found 2026-09-17, and not solved by this plan.** A channel paste, delete
   or move rebuilds the whole `RenderState` through `install_project`
   (`LOOSE_ENDS.md`, "Every structural edit stops the song"). `PluginSlotId`
   stops a move from *renumbering* plugins, but not from tearing down every
   plugin processor in the song and loading it again. Solved by
   `plans/archive/channel-identity/` step 05, which **must land before step 06
   here**.

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
  (`crates/mooloop-engine/src/lib.rs:201`) and `ReplaceEffect` (`:219`)
  already move a `Box<dyn AudioNode + Send>` over the ring, and the displaced
  occupant leaves through the reclaim ring rather than being dropped on the
  audio thread — the rule is stated on `InstallEffect` itself and enforced in
  `crates/mooloop-engine/src/executor.rs:181-189`, which checks the ring has
  room *before* applying the edit. Blocker 4 and step 09 are a new field on
  `ChannelStrip`, not a new path. (The report cited `:189,208` and the
  Architecture section above said `:175`; both predate the one-executor
  refactor, and `:175` is corrected there.)
- **A CLAP main-thread callback can run inside a pump tick.** The 8 ms pump
  (`PUMP_INTERVAL_MS`, `crates/mooloop-ui/src/lib.rs:138`; the timer at
  `:12623`, its closure running to `:14043`) waits on nothing: all ten of its
  channel drains are `try_recv`, and the body holds no lock, no blocking
  `recv`, no `join` and no sleep. **One correction to the report:** the body
  is not free of *I/O*. Four `settings.save()` calls write a small TOML
  synchronously (`:12645`, `:13383`, `:13404`, `:13424`), on user-driven
  events rather than every tick. That is bounded and waits on no other
  thread, so the conclusion stands — but a callback drain added here shares a
  tick with a file write and should not be written as if the tick were
  I/O-free.

Harder than this plan assumes — though the second claim is wrong as the
report stated it:

- **`format_version` is an exact-match gate, and the passes beside it are the
  migration story.** Three sites refuse anything that is not `FORMAT_VERSION`
  outright: the bundle loader (`crates/mooloop-project/src/lib.rs:995`), the
  preset lister (`:1142`) and `validate_envelope` (`:1199`). The report read
  the first two at `:982,1118`. Its "one bespoke fixup" is now **four**, all
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
  `mooloop-session`, including `session.rs:457`, in a crate the table does not
  list at all. Treat 25 as the floor.

### The format-migration question is closed

**Resolved 2026-09-17, no action needed**, in the same report's "Outcome".
A plugin's state is opaque bytes that the plugin versions itself, so mooloop
never reads inside one; `plugin-hosting/` keeps the format at version 1 and
adds `Project.plugins` with `serde(default)`, as the Saving section says.
That is what the tree already does four times over: every one of the four
passes above backfills defaulted fields on the way in without moving
`FORMAT_VERSION`, and
`a_song_with_no_channel_identities_loads_with_its_positions`
(`crates/mooloop-project/src/lib.rs:1464`) is the test that says so in as many
words. **Do not reopen this as a version-bump question.**

## Steps

| Step | What | Closes | Rung | State |
| --- | --- | --- | --- | --- |
| 01 | Spike: `clack-host` loads and runs a CLAP; an in-repo test plugin | — | new crate only | not started |
| 02 | The neutral contract: types, `Project.plugins`, TOML state, a fake plugin end to end | #26 | core, project | not started |
| 03 | Parameters belong to an instance | #26 | core, session | not started |
| 04 | `PluginRack`, main-thread requests, latency known at runtime | #26 | engine, session | not started |
| 05 | The scanner, out of process, with its cache | #27 | plugin-host, app, settings | not started |
| 06 | A headless CLAP effect in a chain | #27 | plugin-host, session | not started |
| 07 | Parameters, automation, modulation and state round-trip | #28 | session, project | not started |
| 08 | Plugin browser, the menu row, and the face for plugins without a GUI | #28 | **UI build**, drafted with `slint-sketch` | not started |
| 09 | A channel source that is a boxed node | #29 | core, engine, session | not started |
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
