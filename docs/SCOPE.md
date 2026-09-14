# Scope

Status: the remaining set, drawn 2026-09-14, at Adam's request for *"some kind
of an accounting of what is actually left to be done."*

Every other planning document here answers a narrower question, and answers it
well. None of them answers this one, because the finish line was never drawn:
`VERSIONS.md` says outright that **"`1.0` has no target yet."** This document
draws it, sizes what stands between here and there, and sorts everything
recorded anywhere into *in* or *out*.

**What this document is not.** It does not order the work — `FOCUS.md` still
decides what is next, and two documents claiming that job is the failure mode
`FOCUS.md` warns about. It does not restate detail another document owns; each
row points at whoever owns it. The rule is **SCOPE says what and how big; the
linked document says how.**

Every status below was read out of the source, not out of another document.
That is deliberate: this codebase's named recurring fault is *"a claim the
source no longer supported"*, and a scope document assembled from other
documents would be exactly that. Six such stale claims turned up in the
writing and are listed at the end.

---

## 1. What 1.0 means

> **mooloop is 1.0 when sound can get *in*, the program can be driven without
> the mouse, and its devices are ones you would choose rather than tolerate.**

That is the honest reading of Adam's thirteen. Today mooloop can build a song
from patterns, arrange it, mix it through a real console, and render it out —
all of which is done. What it cannot do is accept audio or a controller,
capture a performance, or be operated from the keyboard. Those are not polish;
they are the difference between an instrument and a demo of one.

The fourth thing 1.0 has to settle is not a feature: **whether Buffer stays.**
See §7.

---

## 2. Adam's thirteen, sized

Grouped by *kind of work*, not by the order they were listed. The grouping is
the point: these are four different activities and scheduling them as one list
is what makes the set feel endless.

### Tier A — last mile. The thing exists; finish it.

| # | Item | Where it stands | What is actually left |
| --- | --- | --- | --- |
| 1 | **Left sidebar** | Built and shipped 2026-09-13. `ui/channel-sidebar.slint:45` (251 lines), mounted `main.slint:2643`, width + visibility persisted (`settings.rs:493-495`). Carries NAME, COLOR, and track SENDS; the MIDI IN/OUT/CH rows are drawn inert on purpose (`channel-sidebar.slint:221-244`). Channel **and** track colour persist (`channel.rs:141`, `mixer.rs:149`) and draw on the rack plate, the rack wash, the mixer strip and the playlist rows. | No action id and no shortcut — it toggles only from a status-bar chip (`main.slint:1295-1297`). Mute, volume, pan, bus routing, meter, source picker and channel preset are still only in the rack row (`main.slint:3048-3210`). A channel is renamable in two places (sidebar, and the DEVICE CHAIN toolbar at `main.slint:3549-3557`). No test covers it. The MIDI rows light up when item 2 lands. |
| 7 | **Browser sidebars** | Both tabs work. Samples: `main.slint:6284-6608` over a flattened tree (`BrowserRow`, `main.slint:169-190`; FS walk `session/src/browser.rs`), with preview, an autoplay arm and its own trim. Presets: `PresetSlot` (`lib.rs:2466`), `scan_preset_catalog` (`lib.rs:12505`), ~99 factory patches shipping — ML-M1 6, DS-01 17, ML-P8 8, and 5–7 for each of 13 effect kinds. | **No keyboard at all** — there is no `FocusScope` anywhere in the browser subtree and no row-selection state; hover is the only row state. No search or filter. No drag-and-drop to the rack. No preset preview. Category and tags are display-only strings, so there is no taxonomy *navigation*. No bank for Sampler, DrumSynth, MonoSynth, PolySynth or Aux In. One perf wart: expanding a folder re-`read_dir`s synchronously on the UI thread (`lib.rs:12412-12455`). |

### Tier B — real engineering, from zero.

| # | Item | Where it stands | Size |
| --- | --- | --- | --- |
| 2 | **MIDI I/O** | **In works**: one hardcoded JACK port (`jack_driver.rs:28`) auto-connecting every hardware source, midir on macOS, decoding NoteOn/NoteOff/CC/PitchBend (`midi.rs:89`) and playing the selected channel through `keyboard_channel` (`render.rs:2615`). **Out does not exist** — `jack_driver.rs:185-201` registers `out_l`, `out_r`, `midi_in` and nothing else. No MIDI clock (`0xF8` is explicitly dropped, `midi.rs:122`). No port selection, no settings persistence, no learn. The one controller-mapping system in the tree, `BufferMidiMap`, is **dead code**: `set_buffer_midi_map` (`engine/lib.rs:747`) has no caller in the workspace. | Medium, **after the boundary** (§6). Issue #9. |
| 3 | **Audio input** | Nothing. JACK registers no input port; `build_input_stream` appears nowhere; `Executor::process` (`executor.rs:118`) has no input parameter. `ARCHITECTURE_REVIEW.md` confirms: *"No capture path and no media pool."* | Medium. The shape to copy already exists: `AuxIn` (`core/src/aux_in.rs`, `dsp/src/aux_in.rs`) is "a level and a copy", and `AudioTapBank` (`render.rs:53`) already hands a consumer channel a buffer someone else filled. A hardware input is one more pre-filled buffer in that bank and needs no ordering, having no producer. Issues #7, #22. |
| 5 | **Audio + MIDI recording** | Nothing. `EngineCommand` has no `Record` (`core/src/bridge.rs:42-68`); live keys reach the block's event list and are never written back to a pattern. | **Split it.** MIDI capture is cheap and needs no driver change — `press_key`/`release_key` (`render.rs:4212/4228`) already know note, velocity, block offset and channel, and the event ring back to the UI already exists (`executor.rs:46` → `session/engine.rs:477` → `NoteEvent` through `ProjectEdit`). Audio capture is gated on item 3. |
| 8 | **Full-size browser** | Does not exist. The view set is closed at five (`PaneViews`, `main.slint:393-405`). | **Small, and the cheapest win on the list.** A sixth `PaneViews` entry, one `ViewSlot`/`PaneToolbar` block, a `view.pane-browser` action, a `VIEW_COUNT` bump (`settings.rs:498`) and a settings migration. The `BrowserRow` model and row rendering transfer unchanged; what a full-size view adds is column layout, selection and search — which item 7 wants anyway. Machinery: `ViewSlot` (`main.slint:443`), `PaneToolbar` (`:462`), `PaneTabs` (`:485`), `slot-rect` (`:2797`), `LayoutSettings` (`settings.rs:463-496`); tests at `tests/panes.rs`, `pane_drag.rs`, `dock_resize.rs`. |
| 9 | **CLAP effects + instruments** | Zero code. No dependency, no scaffolding, no scanner. Only intent in prose (`node.rs:3-7`). | **The largest item on the list by a wide margin**, and four concrete blockers sit in front of it — see §6. Issues #10, #26–30. |

### Tier C — already built under another name. What is missing is a gesture, not a subsystem.

| # | Item | Where it stands |
| --- | --- | --- |
| 4 | **Audio routing** | **This is the most finished area in the codebase.** A topologically compiled bus graph with cycle refusal (`compile_bus_graph`, Kahn's, `mixer.rs:552`); sends that route a copy of a track to another track with per-edge latency compensation (`AuxSend` `mixer.rs:211`, `compile_latency` `:834`); and typed audio edges with device outlets, refusals-as-values and compiled ordering (`compile_audio_graph` `mixer.rs:1113`, `core/src/outlet.rs`, `AudioTapBank` `render.rs:53`). ML-P8 publishes 7 audio taps; DS-01 and Aux In publish too. **What is genuinely missing is sidechain** — a dependency edge that schedules a producer without summing it — plus mid-chain send taps (`SendTap` has two positions, both post-chain, `mixer.rs:177-201`) and `OutletTap::Output`, which is declared and then refused as `TapIsLate`. Read `plans/archive/typed-audio-edges/` before building any of it. |
| 4b | **MIDI routing** | Separate item, and it is Tier B: there is **no MIDI graph at all**. One input port, decoded once, routed by a single `AtomicU8`. |
| 6 | **Resampling** | The offline render engine is done and reusable: `OfflineRenderer::render` (`offline.rs:104`), WAV via hound and MP3 via mp3lame. Critically, **`render_blocks` (`offline.rs:176`) already takes an arbitrary `FnMut(&[f32], &[f32])` sink** — so "render to a sample" is that closure filling a `Vec` and building `SampleData` (`dsp/src/sampler.rs:65-69`), installed through the existing `EngineHandle::load_sample` (`engine/lib.rs:580`). What is missing is only a **scope smaller than the master bus** (`offline.rs:186/195` reads `state.master()` and nothing else), and `RenderState` already has a full solo model to build one from (`install_solo`, `render.rs:3072`). Small. |
| 13 | **Master bus comp** | **It already exists, twice.** Master is bus 0 and is an ordinary `BusStrip` (`render.rs:1864-1912`) carrying (a) a full 256-slot insert chain — so a `Compressor` or `Limiter` device can be dropped on master today via `EffectTarget::Bus(0)` — and (b) a channel strip whose compressor has threshold, ratio, attack, release, knee, **parallel mix** and makeup, with programme-dependent release (`strip.rs:66-79`), pinned ahead of the rack (`StripPin::Head`, `mixer.rs:65-71`). **What is actually missing is a master *output* stage**: `OutputStage` is gain/pan/mute only (`render.rs:1794-1822`), and nothing bounds a sample in the live path — the console's `sin`/`asin` ceiling is off on master by construction (`render.rs:3093-3096`). So the real item is a **safety limiter and a decision about lookahead**, not a compressor. |

### Tier D — taste, not engineering. These need Adam's ears, not a plan.

| # | Item | Where it stands |
| --- | --- | --- |
| 11 | **Reverb** | Nothing is owed. It is an eight-line FDN with a normalized Hadamard matrix, four Schroeder input diffusers, prime-tuned lines, per-line in-loop damping solved for a target RT60, independent incommensurate line modulation, two orthogonal output taps and a mid/side width control. Zero TODOs in the file. Its documented limits are all deliberate: the input is **mono-summed** before the network, so the stereo image is synthetic; damping compounds per trip; the low cut is on the input and not in the loop. The one fragile thing is `OUTPUT_REFERENCE = 0.188` (`reverb.rs:122-129`) — a *measured* constant that will drift if the network is ever retuned, held only by one gain-structure test. **Reverb is the only Tier-D item with no reference measurements**, so this is a pure character call or a new measurement run. |
| 12 | **EQ** | Has a written plan (`plans/eq-v2/`) and **nothing has landed**. It is the weakest device in the tree and its faults are verified, not reported: a **shelf silently ignores its Q** (`eq.rs:88-89` calls `shelf()` where `Biquad::shelf_slope` exists, is tested, and is what the strip uses); the **pass filters never draw** because the producer writes 7 bands × 5 floats then appends HP and LP at 4 each (`lib.rs:1346-1366`) while the consumer reads `index * 5 + field` (`device-displays.slint:410`), with nothing checking the two agree; the **plot draws shelves at a fixed 1.6 exponent** ignoring Q; clicking a band shows the previous band's values for a frame; and the proportional-Q law is reimplemented locally (`eq.rs:79-82`) instead of using the shared `eq_effective_q`. Its **parameter model is also the CLAP blocker in miniature** — see §6. |

Both 12 and 13 have their **measurements already**. `REFERENCE_MEASUREMENTS.md`'s protocol was run on 2026-09-10 across 25 plugins and `spikes/preamp-measure/RESULTS.md` has sections on EQ and compressors. Two findings bear directly on these rows: an EQ's nonlinearity belongs *inside* the filter network rather than after it (the Massive Passive makes 36 dB more third harmonic than any post-EQ shaper can reach), and compressor programme dependence is the opto/FET signature — opto and FET units hold 2.4–3.6× more gain reduction 500 ms after a long note, where VCA and digital units are within 1%. If the strip compressor is meant to feel like an 1176 or an LA-2A, that ratio is the thing to build.

---

## 3. Required for 1.0, but not on Adam's list

These are not additions to the scope. They are already-recorded work that his
thirteen sit downstream of, or that the definition in §1 forces.

- **The keyboard pass** (`plans/interface-iteration/` step 04) — the last step
  of the only plan in the active sequence, and **items 1, 7 and 8 all depend
  on it.** `ROADMAP.md`'s account of the focus defect is stale; the real
  remaining defects are listed in §8.
- **Cut the release that already exists.** `v0.1.3` was tagged 2026-09-08 and
  `main` is **167 commits** past it. The whole console pass, the channel
  strip, sends, solo in place, the left sidebar, channel and pattern colours
  and three days of correctness work are unreleased and have no `VERSIONS.md`
  entry.
- **File the finished plans.** `plans/README.md` lists `pane-layout/`,
  `ui-consistency-pass/`, `mono-synth-v2/`, `session-layer-extraction/`,
  `preset-system/` and `coreaudio-driver/` as done or done-but-for-a-
  judgement, each held out of `archive/` by housekeeping rather than by work.
  Free, and it changes what the board looks like.
- **A safety limiter on the master output** — see item 13. Nothing currently
  bounds the live signal.
- **The Buffer's tempo-change bug** (§7). This one is a defect, not a
  decision, and it is real.
- **74 loose ends** in `LOOSE_ENDS.md`, across 12 groups. Not all are 1.0
  blockers; the ones that are get picked per polish pass rather than enumerated
  here.

---

## 4. The freeze line

### In for 1.0

All thirteen, with three amendments that the sizing above forces:

- **Item 5 splits.** MIDI recording is in; audio recording follows audio input
  and is in only if item 3 lands early.
- **Item 13 is re-aimed.** A master compressor exists; what ships is a master
  **output stage with a safety limiter** and a lookahead decision.
- **Item 11 is a listening session**, booked like one — not a plan directory.

Plus, from §3: the keyboard pass, the 0.1.4 release, the plan filing, and the
Buffer tempo bug.

### Out, explicitly

- **The Sampler v2 tree** — 13 GitHub issues (#13–#18, #20, #31, #33–#38).
  The single largest block of recorded work in the project, and Adam's
  thirteen does not mention it. `FOCUS.md` already says not to treat it as the
  default pick. **Post-1.0**, except where a defect blocks something in §2.
- **Everything in `ROADMAP.md`'s "Later, Not Scheduled"** that item 2 does not
  pull in: MIDI output is now *in* (item 2), but controller mapping beyond it,
  multiple time signatures and tempo maps, stem and bus export, groove
  extraction, the text/algebraic pattern view and the node-based patcher are
  all out.
- **`egui-view-layer/`, `theming/`, `pattern-bank-floor/`,
  `device-registry/`** — parked by `FOCUS.md` on 2026-09-12, each with a
  recorded reason and a recorded unpark condition. Note that `theming/` gets
  *cheaper to defer and more expensive to do*, and that its real argument is
  accessibility: the working type size is 7–11px and is not adjustable. That
  reason does not expire, so it is out for 1.0 and should not stay out
  forever.
- **Windows and macOS as release targets** (#23, #24, #25). macOS builds and
  runs for development; packaging and signing are out. `PRODUCT.md` is
  unambiguous that Linux is the platform.
- **The modulation-rack move and the tracker question.** Genuinely undecided —
  `IDEAS.md` shows it is a three-way fork (automation events, modulators, song
  automation) and answering it separately is how a project ends up with two
  editors that nearly agree. Out until it is answered.
- **A curated factory bank.** Every device ships presets to prove its
  architecture reaches its range; authoring content by taste across every
  device is a deliberate later push.

---

## 5. Dependency order

Not a schedule. Only the constraints that actually exist.

```
   ┌─ #7 backend boundary ──┬──► item 2  MIDI out
   │  (AudioBackend)        ├──► item 3  audio input ──► item 5b audio recording
   └─ #9 MidiBackend ───────┘

   keyboard pass (interface-iteration 04) ──┬──► item 1  sidebar (last mile)
                                            ├──► item 7  browser sidebars
                                            └──► item 8  full-size browser

   eq-v2 step 01 (instance-scoped params) ──► item 9  CLAP

   item 3 audio input ──► item 6 resampling (only for capture-to-sample;
                                 render-to-sample needs nothing)
```

Everything else is independent and can be handed to a parallel session:
item 4's sidechain, item 6's render-to-sample, item 11, item 12's steps 02–04,
item 13, the release, the plan filing.

### The two gates, in detail

**The I/O backend boundary gates three items.**
`crates/mooloop-engine/src/driver.rs` is the entire audio-driver abstraction —
40 lines — and it is **deliberately not a trait** (`driver.rs:1-7`). JACK and
Core Audio are chosen by `cfg`. So every I/O extension must be written twice,
once per driver, or that decision is revisited first. Two GitHub issues
already name the fix (**#7**, **#8**/**#9**) and neither is on `FOCUS.md`,
`ROADMAP.md` or in any plan directory. This is the cheapest-now item on the
whole board and it gates roughly a third of it.

**`eq-v2` step 01 gates CLAP, and `FOCUS.md` already argues why.** A CLAP
plugin hands the host N independent parameters with stable ids and no context.
The EQ is the one native device whose parameter model a host could not
express: six parameters cover seven bands and two pass filters, resolved
through an `EQ_PARAM_TARGET` that is **itself automatable** (`effect.rs:285-302`),
so a lane on it changes which band every other EQ lane refers to. Fixing that
on a device whose behaviour is already understood is a cheap rehearsal for the
same problem.

**The four things standing between here and CLAP**, all verified:

1. `EffectKind::descriptors()` returns `&'static [ParamDescriptor]` — a table
   per *kind* (`effect.rs:128-145`). A plugin's parameters belong to an
   *instance*, discovered at load, with arbitrary sparse `u32` ids.
   Downstream, `descriptor_slots` sizes modulation arrays as max-id-plus-one
   (`session/values.rs:125`), Slint faces index `modulation-allowed[N]`
   densely, and `param_id_freeze_tests.rs` pins parameters by position.
2. `EffectParams` is a closed serde-tagged enum (`effect.rs:2145-2162`); a
   plugin needs an opaque id plus an opaque state blob.
3. `EffectKind::latency_frames()` is static and asserted to match the node
   (`effects/mod.rs:446-472`); for a plugin it must be discovered at runtime.
4. **Instruments are worse than effects.** `ChannelStrip` holds all eight
   generators as concrete fields (`render.rs:2020-2029`) with `active_source`
   selecting among them. There is no `Box<dyn AudioNode>` source slot, so a
   hosted plugin *instrument* has nowhere to live at all.

The good news is real: `AudioNode` (`node.rs:142-281`) takes a whole block plus
a sorted event list, which is CLAP's own shape, and the install path already
exists — `StructuralCommand::InstallEffect`/`ReplaceEffect` move a boxed node
over a ring and the RT thread never drops it.

---

## 6. Where this overrides the other documents

Adam's thirteen was given on 2026-09-14 and `PRODUCT.md`'s own precedence rule
is that *"current direct feedback outranks this file."* Four documents are
therefore now stale on scope, and are recorded here so the next agent does not
re-park work that has just been asked for:

| Document | Said | Now |
| --- | --- | --- |
| `PRODUCT.md` non-goals | "A plugin host before its own instrument and sequencing model is coherent" | Item 9 is in for 1.0. The instrument model *is* coherent; the parameter model is what is not, and §5 makes that the gate. |
| `FOCUS.md` "deliberately not now" | Plugin hosting deferred | In, behind `eq-v2` step 01. |
| `FOCUS.md` on MIDI | *"build the setting, let it stay inert"* | Item 2 lights those rows up. |
| `ROADMAP.md` "Later, Not Scheduled" | MIDI output, MIDI recording, controller mapping | Output and recording are in; general controller mapping stays out. |

---

## 7. The one question that changes what 1.0 is

Adam's item 10 is *"do something with the buffer device/idea or just remove
it."* That is the first time deletion has been on the table for the feature
`PRODUCT.md` calls **"the proposed differentiator"** and that `VERSIONS.md`
gives an entire release (0.3.0) to deciding.

**This document deliberately does not sort it.** With Buffer, mooloop is a
retained-audio instrument; without it, it is a very good groovebox. The answer
changes §1.

What is worth knowing before answering:

- **The engine is built and is not the problem.** 1146 lines: a bounded ring,
  a cubic-Hermite fractional read head, bit-transparent zero-latency Follow,
  JUMP/REV/STUT, a scrub platter, off-thread resize, collision telemetry,
  persistence, and 16 tests including block-size independence and crossfade
  declicking.
- **The workflow was never tested, and that is the whole outstanding exit
  criterion.** `FOCUS.md` puts the judgement last on purpose: whether this
  beats bouncing to a sample is a musical verdict, and only Adam can give it.
- **Adam has already half-answered it.** `BUFFER_ENGINE.md:10-17`, 2026-08-30:
  making Buffer an ordinary insert device was *"partly the wrong call"*; the
  stated intent is the end of a device rack with its own sequencing lane. So
  "remove it" may really be "the insert shape was wrong", which is a different
  answer.
- **The gestures on the face are an admitted debug surface**
  (`lib.rs:1415-1420`) standing in for a note layer and parameter locks that
  do not exist. Judging the workflow through them judges the wrong thing.
- **One real bug, independent of the decision: a tempo change destroys the
  retained history.** `ResizeBuffers` constructs a new zeroed device and swaps
  it in (`session/engine.rs:501`, `engine/lib.rs:524`). Nothing tests it and
  `BUFFER_ENGINE.md` lists tempo-change behaviour as an unrun engineering
  test. Fix this before any listening pass, or the pass will be judging a
  device that silently forgets.

---

## 8. Stale claims found while writing this

Each verified against source. They are listed because the project's own named
fault is *a claim the source no longer supported*, and six of them accumulated
in nine days.

1. **`ROADMAP.md:20-26` is wrong about the focus defect.** It says the window
   has a single root `FocusScope` that eats keys, so shortcuts need a click on
   the background first. **Fixed 2026-09-07**: the scope now surrounds the UI
   (`main.slint:2087-2088`), control scopes reject keys they do not own,
   `ToolButton` deliberately rejects Space, and `tests/first_click.rs` pins all
   of it. `FOCUS.md` and `interface-iteration/04` both already say so.
2. **`BUFFER_ENGINE.md:40-41`** claims the Buffer is built on the shared
   `mooloop_dsp::delayline` primitive "rather than a private ring." It is not
   — `buffer_device.rs:87-88` is two private `Vec<f32>`s with its own indexing
   and its own Hermite interpolation. `delayline.rs:3-8` and
   `CURRENT.md:930-933` both still assert the Buffer is or will be the shared
   ring's second consumer.
3. **`effects/dynamics.rs:423-429`** justifies the limiter's lack of lookahead
   on the grounds that the engine has no plugin delay compensation. It has had
   it since 2026-09-05. `CURRENT.md:919-929` already flags the rationale as
   expired; the code comment does not.
4. **`FOCUS.md:117`** says "the rack plate draws a channel's colour and nothing
   else does." The mixer strip (`mixer.slint:788`) and the playlist rows
   (`main.slint:5925`, `:6033`) draw it too.
5. **`plans/buffer-implementation/00-status.md` does not exist**, where every
   other plan directory has one — so the one plan whose thesis is undecided is
   also the one with no status.
6. **The real keyboard defects are not the one `ROADMAP.md` names**, and they
   are worth recording because step 04 owns them:
   - **Bare unmodified letters never reach the dispatcher.** The decode ladder
     (`main.slint:2096-2237`) handles Space, arrows, named keys, `Ctrl`+letter
     sentinels and digits 1–6, then rejects everything else at `:2235`. So
     `transport.loop-toggle`, registered against `"l"` (`actions.rs:102`) and
     dispatchable (`lib.rs:5087`), is **unreachable from the keyboard** — as
     is any user rebind to a bare letter.
   - **The rebind capture has the same hole**
     (`appearance-dialog.slint:361-366`), so it cannot even be discovered by
     rebinding. The two ladders are duplicated by hand and the file says so.
   - **Modal dialogs are outside the root scope and handle no keys.**
     Preferences, Save Preset, Export and About are siblings of the focus
     scope, so **Escape cannot dismiss them.**
   - **No surface holds focus for navigation.** The browser tree, channel
     rack, mixer strips and device rack have no `FocusScope` and no
     focused-row concept; the arrow keys are hard-coded globally to channel
     selection (`main.slint:2104-2111`).
   - **No context resolution.** `Ctrl+C` branches on `notes_have_focus()`
     inside the Rust dispatcher (`lib.rs:5101-5125`) rather than on a focus
     model, which is why the device clipboard was pushed onto `Ctrl+Shift+*`.

---

## Maintaining this

Rewrite it when the freeze line moves, not when an item lands — a row going
green belongs in the owning plan's `00-status.md`. When every row in §4's "in"
list is closed, this document has done its job and should be replaced by a
`1.0` section in `VERSIONS.md`.
