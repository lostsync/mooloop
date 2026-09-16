# Scope

Status: the remaining set, drawn 2026-09-14, at Adam's request for *"some kind
of an accounting of what is actually left to be done."* Scope answers from Adam
the same day: **0.2.0 is the line**, CLAP is in, and the sampler needs key
zones.

Every other planning document here answers a narrower question, and answers it
well. None of them answers this one, because the finish line was never drawn:
`VERSIONS.md` said outright that **"`1.0` has no target yet"** — it has since
been deleted as part of the 2026-09-14 documentation trim, having never been
read by the one person it described releases for. This document
draws it, sizes what stands between here and there, and sorts everything
recorded anywhere into *in* or *out*.

**The line is 0.2.0, not 1.0** — Adam's call, and the honest number. §9 records
how releases actually get cut here, and what the nearer **0.1.4** carries.

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

## 1. What 0.2.0 means

> **mooloop is 0.2.0 when sound can get *in*, the program can be driven without
> the mouse, and its devices are ones you would choose rather than tolerate.**

That is the honest reading of Adam's thirteen. Today mooloop can build a song
from patterns, arrange it, mix it through a real console, and render it out —
all of which is done. What it cannot do is accept audio or a controller,
capture a performance, or be operated from the keyboard. Those are not polish;
they are the difference between an instrument and a demo of one.

The fourth thing 0.2.0 has to settle is not a feature: **whether Buffer stays.**
See §7. Because the line is 0.2 rather than 1.0, that decision now falls
*inside* the release rather than after it — the old ladder gave it 0.3.0 all to
itself.

---

## 2. The list, sized

Adam's thirteen, plus a fourteenth he added on 2026-09-14 (key zones).

Grouped by *kind of work*, not by the order they were listed. The grouping is
the point: these are four different activities and scheduling them as one list
is what makes the set feel endless. Two items — 13 and 14 — outgrew a table
cell once Adam said what he actually wanted from them, and have their own
sections at the end.

### Tier A — last mile. The thing exists; finish it.

| # | Item | Where it stands | What is actually left |
| --- | --- | --- | --- |
| 1 | **Left sidebar** | Built and shipped 2026-09-13. `ui/channel-sidebar.slint:45` (251 lines), mounted `main.slint:2643`, width + visibility persisted (`settings.rs:493-495`). Carries NAME, COLOR, and track SENDS; IN and CH went live with item 2 on 2026-09-15 and OUT stays inert because MIDI output does not exist (`channel-sidebar.slint:221-244`). Channel **and** track colour persist (`channel.rs:141`, `mixer.rs:149`) and draw on the rack plate, the rack wash, the mixer strip and the playlist rows. | No action id and no shortcut — it toggles only from a status-bar chip (`main.slint:1295-1297`). Mute, volume, pan, bus routing, meter, source picker and channel preset are still only in the rack row (`main.slint:3048-3210`). A channel is renamable in two places (sidebar, and the DEVICE CHAIN toolbar at `main.slint:3549-3557`). No test covers it. |
| 7 | **Browser sidebars** | Both tabs work. Samples: `main.slint:6284-6608` over a flattened tree (`BrowserRow`, `main.slint:169-190`; FS walk `session/src/browser.rs`), with preview, an autoplay arm and its own trim. Presets: `PresetSlot` (`lib.rs:2466`), `scan_preset_catalog` (`lib.rs:12505`), ~99 factory patches shipping — ML-M1 6, DS-01 17, ML-P8 8, and 5–7 for each of 13 effect kinds. | **No keyboard at all** — there is no `FocusScope` anywhere in the browser subtree and no row-selection state; hover is the only row state. No search or filter. No drag-and-drop to the rack. No preset preview. Category and tags are display-only strings, so there is no taxonomy *navigation*. No bank for Sampler, DrumSynth, MonoSynth, PolySynth or Aux In. One perf wart: expanding a folder re-`read_dir`s synchronously on the UI thread (`lib.rs:12412-12455`). |

### Tier B — real engineering, from zero.

| # | Item | Where it stands | Size |
| --- | --- | --- | --- |
| 2 | **MIDI I/O** | **In, as of 2026-09-15.** `plans/midi-control/` landed the whole control layer and its interface. Below the interface: per-channel input and an Omni-or-1-16 channel filter on `Channel`, persisted (`midi.rs`); the engine routing notes by them with the selected channel as a fallback (`render.rs`); transport messages decoded and followed (Start/Continue/Stop/Song Position — clock still dropped on purpose); control changes forwarded to the control thread and mapped onto any `ParamAddr` through the same write the on-screen control uses, with pickup takeover, the three relative-encoder conventions, toggles and learn (`core/control.rs`, `session/midi.rs`); and note capture into patterns. Both drivers tag messages with a port, though **JACK has only one** — every hardware source is merged onto `midi_in`, so under JACK two keyboards are told apart by MIDI channel, not by port. `BufferMidiMap` is still dead code. **Out does not exist** — `jack_driver.rs:185-201` registers `out_l`, `out_r`, `midi_in` and nothing else. | **The 0.1.4 half is complete as of 2026-09-15**, interface included: the sidebar's IN and CH rows, a record-arm button, LEARN beside the transport, and a mapping editor with the transport list on Preferences > MIDI (`plans/midi-control/04-interface.md`). What is left is not construction -- **none of it has been run against a keyboard**. **MIDI output stays 0.2.0.** Issue #9 turned out *not* to be a prerequisite: port selection needed a port list and a message tag from each driver, which is two small per-driver methods rather than a trait. `docs/CONTROL_SURFACES.md`. |
| 3 | **Audio input** | Nothing. JACK registers no input port; `build_input_stream` appears nowhere; `Executor::process` (`executor.rs:118`) has no input parameter. `archive/ARCHITECTURE_REVIEW.md` confirms: *"No capture path and no media pool."* | Medium. The shape to copy already exists: `AuxIn` (`core/src/aux_in.rs`, `dsp/src/aux_in.rs`) is "a level and a copy", and `AudioTapBank` (`render.rs:53`) already hands a consumer channel a buffer someone else filled. A hardware input is one more pre-filled buffer in that bank and needs no ordering, having no producer. Issues #7, #22. |
| 5 | **Audio + MIDI recording** | **MIDI capture landed 2026-09-15** and was as cheap as this row predicted. `EngineCommand::SetRecordArmed` arms it; armed *and* running, the renderer remembers where a note landed on the playhead and reports the whole note when the key comes up, through `EngineEvent::RecordedNote` on the existing ring. Length is measured in frames rather than ticks, so a note held across the loop point reports how long it was held instead of a negative number. `Session::record_note` writes it into the pattern, on the channel the routing sent it to rather than the selected one, trimmed to the pattern's end. The record-arm button landed beside play and stop the same day. Audio capture is still nothing. | Audio capture is gated on item 3. |
| 8 | **Full-size browser** | Does not exist. The view set is closed at five (`PaneViews`, `main.slint:393-405`). | **Small, and the cheapest win on the list.** A sixth `PaneViews` entry, one `ViewSlot`/`PaneToolbar` block, a `view.pane-browser` action, a `VIEW_COUNT` bump (`settings.rs:498`) and a settings migration. The `BrowserRow` model and row rendering transfer unchanged; what a full-size view adds is column layout, selection and search — which item 7 wants anyway. Machinery: `ViewSlot` (`main.slint:443`), `PaneToolbar` (`:462`), `PaneTabs` (`:485`), `slot-rect` (`:2797`), `LayoutSettings` (`settings.rs:463-496`); tests at `tests/panes.rs`, `pane_drag.rs`, `dock_resize.rs`. |
| 9 | **CLAP effects + instruments** | Zero code. No dependency, no scaffolding, no scanner. Only intent in prose (`node.rs:3-7`). **Planned 2026-09-16: `plans/plugin-hosting/`**, which also outlines VST3 and AU after CLAP. | **The largest item on the list by a wide margin**, and six concrete blockers sit in front of it — the four in §5 and two more in the plan's `00-status.md`. Issues #10, #26–30. |

### Tier C — already built under another name. What is missing is a gesture, not a subsystem.

| # | Item | Where it stands |
| --- | --- | --- |
| 4 | **Audio routing** | **This is the most finished area in the codebase.** A topologically compiled bus graph with cycle refusal (`compile_bus_graph`, Kahn's, `mixer.rs:552`); sends that route a copy of a track to another track with per-edge latency compensation (`AuxSend` `mixer.rs:211`, `compile_latency` `:834`); and typed audio edges with device outlets, refusals-as-values and compiled ordering (`compile_audio_graph` `mixer.rs:1113`, `core/src/outlet.rs`, `AudioTapBank` `render.rs:53`). ML-P8 publishes 7 audio taps; DS-01 and Aux In publish too. **What is genuinely missing is sidechain** — a dependency edge that schedules a producer without summing it — plus mid-chain send taps (`SendTap` has two positions, both post-chain, `mixer.rs:177-201`) and `OutletTap::Output`, which is declared and then refused as `TapIsLate`. Read `plans/archive/typed-audio-edges/` before building any of it. |
| 4b | **MIDI routing** | Separate item, and it is Tier B: there is **no MIDI graph at all**. One input port, decoded once, routed by a single `AtomicU8`. |
| 6 | **Resampling** | The offline render engine is done and reusable: `OfflineRenderer::render` (`offline.rs:104`), WAV via hound and MP3 via mp3lame. Critically, **`render_blocks` (`offline.rs:176`) already takes an arbitrary `FnMut(&[f32], &[f32])` sink** — so "render to a sample" is that closure filling a `Vec` and building `SampleData` (`dsp/src/sampler.rs:65-69`), installed through the existing `EngineHandle::load_sample` (`engine/lib.rs:580`). What is missing is only a **scope smaller than the master bus** (`offline.rs:186/195` reads `state.master()` and nothing else), and `RenderState` already has a full solo model to build one from (`install_solo`, `render.rs:3072`). Small. |
| 13 | **Master bus comp** | Promoted out of this tier on 2026-09-14 — Adam wants it to be its own thing, with its own laws. **See §2.1.** |

### Tier D — taste, not engineering. These need Adam's ears, not a plan.

| # | Item | Where it stands |
| --- | --- | --- |
| 11 | **Reverb** | Nothing is owed. It is an eight-line FDN with a normalized Hadamard matrix, four Schroeder input diffusers, prime-tuned lines, per-line in-loop damping solved for a target RT60, independent incommensurate line modulation, two orthogonal output taps and a mid/side width control. Zero TODOs in the file. Its documented limits are all deliberate: the input is **mono-summed** before the network, so the stereo image is synthetic; damping compounds per trip; the low cut is on the input and not in the loop. The one fragile thing is `OUTPUT_REFERENCE = 0.188` (`reverb.rs:122-129`) — a *measured* constant that will drift if the network is ever retuned, held only by one gain-structure test. **Reverb is the only Tier-D item with no reference measurements**, so this is a pure character call or a new measurement run. |
| 12 | **EQ** | **Done and closed 2026-09-15** (`plans/archive/eq-v2/`). Steps 01-03 landed on the 14th and 15th and step 04 was declined, so the plan archived at 03. Every fault this row used to list as verified is fixed: a shelf designs its slope from its Q instead of ignoring it (a private copy of the core Q law was deleted with it); the pass filters draw, because the producer/consumer stride disagreement that hid them is gone and the curve is now the bank's own coefficients evaluated rather than a shape drawn to resemble them, so the fixed 1.6 exponent went with it; the stale-band-for-a-frame defect closed when an `EqSpec` global took the last range out of `eq-device.slint`; and the local proportional-Q reimplementation is gone. Two things it leaves: **the top 46% of a shelf's Q knob still does nothing**, because `shelf_slope` clamps where the cookbook's radicand goes negative while the same id must serve the band as a bell (`LOOSE_ENDS.md`), and two listening passes are owed. Its **parameter model was the CLAP blocker in miniature** and step 01 resolved it — see §5. |

Item 12 has its **measurements already**. `REFERENCE_MEASUREMENTS.md`'s
protocol was run on 2026-09-10 across 25 plugins and
`spikes/preamp-measure/RESULTS.md` §3 covers EQ: an EQ's nonlinearity belongs
*inside* the filter network rather than after it, because the Massive Passive
makes 36 dB more third harmonic than any post-EQ shaper can reach.

### 2.1 Item 13 — the master bus compressor

Adam, 2026-09-14: *"we've created a really good feeling console mix
experience… I'm totally cool with us using the master strip to house the bus
comp — that's only natural — but it needs to be more prominently displayed in
the rack, with a nice meter, and it should be running its own algos, aimed at
like SSL, 2500, and idk Massive Passive or something. Turning it on should
feel special. We have the measurements to do it (and to show that they're not
the same as the channel comps)."*

**The measurements do not merely permit this — they make the case.** The
cleanest split in the entire reference run falls exactly on the axis that
separates a bus compressor from a channel one:

| Unit | Element | Programme dependence | Detector | Note |
| --- | --- | --- | --- | --- |
| UAD LA-2A | opto | **3.6×** | peak | the channel character |
| Waves CLA-3A | opto | **2.4×** | peak | the channel character |
| UAD 1176LN | FET | **2.5×** | peak | two-stage attack: t63 2.0 ms, t90 514 ms |
| UAD SSL G bus | VCA | **1.0×** | peak | attack 0.31–21.4 ms, release 77 ms–3.4 s (Auto) |
| Waves API-2500 | VCA | 1.0× | **RMS** | attack **floors at 3.8 ms**; its two fastest markings are fiction |
| UAD dbx 160 | VCA | 1.0× | **RMS** | "over easy" knee — shallowest onset of the nine |

Read the programme-dependence column. **Opto and FET units hold 2.4–3.6× more
gain reduction at 500 ms after a long note; every VCA unit is at 1.0×, with no
programme dependence at all.** That is precisely Adam's *"show that they're not
the same as the channel comps"*, and it is a difference of **law**, not of
values — which is the console plan's standing rule (*a voicing selects laws,
never values*) landing exactly where it was designed to.

So the master comp's three voicings are distinguished by **detector shape and
timing law**, the two things the run resolves best — and all three are
measured:

- **SSL G bus** — peak VCA, no programme dependence, fast available attack, a
  marked-vs-measured release ratio of about 3×, and an Auto mode measuring
  3.4 s. Third-harmonic-dominant (−34.8 dB h3 at 60 Hz).
- **API-2500** — RMS VCA. Its character is that it *ignores peaks*: 8.3 dB of
  reduction on a tone against 4.4 dB on same-peak bursts. The **3.8 ms attack
  floor is the unit**, not a limitation to model around.
- **Vari-mu — the UAD Fairchild 670.** Settled 2026-09-14: Adam meant vari-mu,
  not Massive Passive (which is an EQ, RESULTS §3, and cannot voice a
  compressor). It is measured, and it is a third *law* rather than a third set
  of numbers — it has **no separate attack and release at all**, only six
  coupled time-constant positions, measuring at about a third of their
  published values:

  | position | attack t63 | release t63 | published |
  | --- | --- | --- | --- |
  | 1 | 1.52 ms | 104 ms | 0.3 s |
  | 2 | 1.94 ms | 306 ms | 0.8 s |
  | 3 | 3.54 ms | 909 ms | 2 s |
  | 4 | 7.10 ms | 1793 ms | 5 s |
  | 5 | 3.52 ms | 1760 ms | 10 s |
  | 6 | 1.56 ms | 1426 ms | 25 s |

  **Positions 5 and 6 are the programme-dependent ones, and the table shows
  it**: the attack gets *faster* again while the release stays long, which is
  the two-stage behaviour those positions exist for. Detector reads peak
  (10.4 dB on a tone, 9.8 on same-peak bursts).

  This has a design consequence the other two do not: a vari-mu voicing
  sensibly presents **six positions rather than two knobs**, so the master
  comp's control surface is not fixed across its voicings. That is a face
  question to settle early, and it is exactly the kind of thing the console
  plan's rule protects — a voicing selecting *laws* can legitimately change
  which controls mean anything, as long as no control lies about its range.

**What has to be built**, over what exists (`strip.rs:758-798` `process_comp`,
one implementation shared by every track, with a voicing-driven ratio bend and
a programme-dependent release):

1. **Its own laws.** A master voicing selects detector shape (peak vs RMS), the
   timing law including a floor where the reference has one, the knee, and
   crucially **no programme dependence** — the opposite of what the channel
   strip's comp does. This is a separate law table, not new values in the
   existing one.
2. **A real meter.** Today gain reduction is a 9 px lamp on
   `StripSectionHead` scaled to 12 dB (`strip.slint:661-669`), plus a 72 px
   `DynamicsCurveDisplay` that only appears in wide mode. That was the right
   call for a channel — *"a scope would have told you the same and costs 9px
   instead of 80"* — and it is the wrong one for the master.
3. **Prominence in the rack.** Master is currently drawn as just another
   `BusStrip`, identical to every other track.
4. **"Turning it on should feel special."** A taste requirement, and the one
   part that needs a listening pass rather than a spec.

**Note it is not the same item as the master safety limiter.** `OutputStage` is
gain/pan/mute only (`render.rs:1794-1822`) and nothing bounds the live signal —
the console's `sin`/`asin` ceiling is forced off on master by construction
(`render.rs:3093-3096`). A bus compressor is a musical device; a safety limiter
is engineering. Both are in for 0.2; they are separate rows.

### 2.2 Item 14 — sampler key zones

Added 2026-09-14. Adam: *"sampler v2… a lot of that is done. I would say we
should at least get key zones, if not layers."*

He is right that a lot is done — stretch, slicing and the commit path shipped
in 0.1.2. **Key zones are not**, and this one is structural rather than a last
mile. The sampler holds exactly **one** `SampleData` plus a `root_note`
(`dsp/src/sampler.rs:65-69`), and pitch is one subtraction from it
(`sampler.rs:668-669`). There is no zone, layer, or mapping concept anywhere in
`mooloop-core` or `mooloop-dsp`.

What a zone set crosses:

- `SamplerParams` and `ChannelSource::Sampler` grow a mapped set where there is
  now a single handle.
- `PROJECT_FORMAT.md`'s defaulted-field rule applies — a v1 song has one
  sample and must keep loading as a single full-range zone.
- `EngineHandle::load_sample(channel, Arc<SampleData>)` (`engine/lib.rs:580`)
  becomes a set install, and the structural-reclaim path has to return N
  buffers rather than one.
- `CAPACITY_POLICY.md` applies: no small cap on a user-facing collection.
- Some editor is required even at the minimum. **#17** is the full mapping
  workspace and stays out; zones need only a key-range editor.

**Velocity layers are the "if not layers" half** — the same structure with a
second axis, and they should be designed for now and built only if 0.2 has
room. GitHub **#16** covers both; take its key-range half.

Everything else in the Sampler v2 tree stays post-0.2 (§4).

---

## 3. Required for 0.2.0, but not on Adam's list

These are not additions to the scope. They are already-recorded work that his
thirteen sit downstream of, or that the definition in §1 forces.

- ~~**The keyboard pass**~~ — **landed 2026-09-14**
  (`plans/archive/interface-iteration/` step 04), which closed and archived
  that plan. Items 1, 7 and 8 depended on it and are unblocked: the focus
  model they needed is `Surface` in `actions.rs`, and the browser tree is
  navigable without the mouse. `archive/ROADMAP.md`'s account of the focus defect is
  still stale; the real remaining defects are listed in §8.
- **Ship 0.1.4 first.** `v0.1.3` was tagged 2026-09-08 and `main` is **167
  commits** past it — the whole console pass, the channel strip, sends, solo
  in place, the left sidebar, colours and three days of correctness work.
  Adam has since defined what 0.1.4 carries beyond that backlog: the mixer at
  a stopping point, the macOS menubar, and configurable MIDI input. **§9.**
- **File the finished plans.** `plans/README.md` lists `pane-layout/`,
  `ui-consistency-pass/`, `mono-synth-v2/`, `session-layer-extraction/`,
  `preset-system/` and `coreaudio-driver/` as done or done-but-for-a-
  judgement, each held out of `archive/` by housekeeping rather than by work.
  Free, and it changes what the board looks like.
- **A safety limiter on the master output** — see item 13. Nothing currently
  bounds the live signal.
- **The Buffer's tempo-change bug** (§7). This one is a defect, not a
  decision, and it is real.
- **74 loose ends** in `LOOSE_ENDS.md`, across 12 groups. Not all are 0.2
  blockers; the ones that are get picked per polish pass rather than enumerated
  here.

---

## 4. The freeze line

### In for 0.2.0

All thirteen, with three amendments that the sizing above forces:

- **Item 5 splits.** MIDI recording is in; audio recording follows audio input
  and is in only if item 3 lands early.
- **Item 13 grows.** It is no longer "a compressor exists" — it is a master bus
  compressor with its own laws, its own meter and its own presence (§2.1),
  *plus* a separate master **safety limiter** and a lookahead decision.
- **Item 11 is a listening session**, booked like one — not a plan directory.
- **Item 14 joins**: sampler key zones (§2.2), Adam's addition on 2026-09-14.

Plus, from §3: the keyboard pass, the plan filing, and the Buffer tempo bug.

**Minus whatever 0.1.4 absorbs** (§9): configurable MIDI input, the macOS
menubar, and the mixer's stopping point come off this list by shipping
earlier, not by leaving scope.

### Out, explicitly

- **The Sampler v2 tree** — 13 GitHub issues (#13–#18, #20, #31, #33–#38).
  The single largest block of recorded work in the project, and Adam's
  thirteen does not mention it. `FOCUS.md` already says not to treat it as the
  default pick. **Post-0.2**, except for **#16's key-range half** (§2.2),
  which Adam pulled in on 2026-09-14, and except where a defect blocks
  something in §2.
- **Everything in `archive/ROADMAP.md`'s "Later, Not Scheduled"** that item 2 does not
  pull in: MIDI output is now *in* (item 2), but controller mapping beyond it,
  multiple time signatures and tempo maps, stem and bus export, groove
  extraction, the text/algebraic pattern view and the node-based patcher are
  all out.
- **`egui-view-layer/`, `theming/`, `pattern-bank-floor/`,
  `device-registry/`** — parked by `FOCUS.md` on 2026-09-12, each with a
  recorded reason and a recorded unpark condition. Note that `theming/` gets
  *cheaper to defer and more expensive to do*, and that its real argument is
  accessibility: the working type size is 7–11px and is not adjustable. That
  reason does not expire, so it is out for 0.2 and should not stay out
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

   keyboard pass (landed 2026-09-14) ───────┬──► item 1  sidebar (last mile)
                                            ├──► item 7  browser sidebars
                                            └──► item 8  full-size browser

   eq-v2 step 01 (instance-scoped params) ──► item 9  CLAP

   item 3 audio input ──► item 6 resampling (only for capture-to-sample;
                                 render-to-sample needs nothing)

   RESULTS.md §4 (already measured) ──► item 13 master bus comp
   PROJECT_FORMAT defaulted-field rule ──► item 14 key zones
```

Everything else is independent and can be handed to a parallel session:
item 4's sidechain, item 6's render-to-sample, item 11, item 12's steps 02–04,
**item 13, item 14**, the release, the plan filing.

Items 13 and 14 are worth naming as parallel work specifically: neither waits
on the backend boundary or the keyboard pass, both have their inputs already
(measurements for 13, a defaulted-field precedent for 14), and both are the
kind of self-contained device work that ends in something you can listen to.

### The two gates, in detail

**The I/O backend boundary gates three items.**
`crates/mooloop-engine/src/driver.rs` is the entire audio-driver abstraction —
40 lines — and it is **deliberately not a trait** (`driver.rs:1-7`). JACK and
Core Audio are chosen by `cfg`. So every I/O extension must be written twice,
once per driver, or that decision is revisited first. Two GitHub issues
already name the fix (**#7**, **#8**/**#9**) and neither is on `FOCUS.md`,
`archive/ROADMAP.md` or in any plan directory. This is the cheapest-now item on the
whole board and it gates roughly a third of it.

**`eq-v2` step 01 gated CLAP and landed 2026-09-14.** A CLAP plugin hands the
host N independent parameters with stable ids and no context. The EQ was the
one native device whose parameter model a host could not express: six
parameters covered seven bands and two pass filters, resolved through an
`EQ_PARAM_TARGET` that was **itself automatable**, so a lane on it changed
which band every other EQ lane referred to. It is fifty per-band ids now, and
the rehearsal answered its question -- the model holds, and the face did not
have to change to carry it, because a face showing one band at a time is a
*view* and the selection belongs in the view. What it did **not** settle is
item 1 below, which is the part that is actually about plugins.

**The four things standing between here and CLAP**, all verified. The
2026-09-16 plan (`plans/plugin-hosting/00-status.md`) adds two: nothing on
the control thread can reach a node once it is installed, and the effect
menu and source popup are hard-coded rather than data-driven. It also records
Adam's direction that **VST3 follows CLAP and AU comes last**, and that Linux
is the audience.

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
| `PRODUCT.md` non-goals | "A plugin host before its own instrument and sequencing model is coherent" | Item 9 is in for 0.2 — Adam, 2026-09-14: *"I do think CLAP is important."* The instrument model *is* coherent; the parameter model is what is not, and §5 makes that the gate. |
| `FOCUS.md` "deliberately not now" | Plugin hosting deferred | In, behind `eq-v2` step 01. |
| `FOCUS.md` on MIDI | *"build the setting, let it stay inert"* | Item 2 lit those rows up on 2026-09-15, and `FOCUS.md` was rewritten to say so. |
| `archive/ROADMAP.md` "Later, Not Scheduled" | MIDI output, MIDI recording, controller mapping | Output and recording are in; general controller mapping stays out. |

---

## 7. The one question that changes what 0.2.0 is

Adam's item 10 is *"do something with the buffer device/idea or just remove
it."* That is the first time deletion has been on the table for the feature
`PRODUCT.md` calls **"the proposed differentiator"** and that the old version
ladder gave an entire release (0.3.0) to deciding.

**Setting the line at 0.2 pulls this forward.** The old ladder let the Buffer
question wait for its own release; it now has to be answered inside the one
being frozen, because item 10 is on the list that defines it.

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

1. ~~**`archive/ROADMAP.md:20-26` is wrong about the focus defect.**~~ **Corrected in
   `archive/ROADMAP.md` on 2026-09-14.** It had said the window has a single root
   `FocusScope` that eats keys, so shortcuts need a click on the background
   first. Fixed 2026-09-07: the scope surrounds the UI, control scopes reject
   keys they do not own, `ToolButton` deliberately rejects Space, and
   `tests/first_click.rs` pins all of it.
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
6. **The real keyboard defects are not the one `archive/ROADMAP.md` names**, and they
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

## 9. Releases

**How releases actually happen here**, stated by Adam on 2026-09-14 and
recorded because this document was briefly built on the opposite assumption:

> *"I don't really care at all about what `VERSIONS.md` says. I've never read
> it. I've been releasing when it felt like enough changes had accumulated to
> justify a release, basically."*

`VERSIONS.md` was **deleted on 2026-09-14** on the strength of that. It was a
record kept after the fact that nobody consulted, and `JOURNAL.md` already
carries the narrative while `CURRENT.md` carries the facts. An earlier draft of
this section proposed an elaborate renumbering to resolve a collision between
its `0.2.0`/`0.3.0` milestones and the freeze. The collision was never real,
because nothing was driving off those milestones.

### 0.1.4 — the open threads, tied down

Set by Adam, 2026-09-14. This is **not** the freeze; it is the release that
closes what is currently in flight, and it is close.

| Thread | State |
| --- | --- |
| **The mixer section at a good stopping point** | *"almost"* — the console pass closed 2026-09-11 and was played the same day. The studio listening pass is the outstanding piece (`FOCUS.md`, "waiting on Adam"). |
| **A little more macOS support — the menubar** | See below. New, and not previously recorded anywhere. |
| **MIDI input** | See below. |

**The macOS menubar.** mooloop draws its own in-window menu bar
(`ui/menubar.slint`), which is right on Linux and wrong on macOS, where the
menu belongs in the system bar at the top of the screen. There is **no native
menu integration anywhere in the tree** — no `NSMenu`, no platform branch, no
mention in `plans/coreaudio-driver/`. The Slint menu components are
deliberately generic and know nothing about which actions exist, and the action
registry (`actions.rs`) already separates an action's identity from its
surface — so the menu structure is nameable independently of how it is drawn,
which is what a native bar needs. Sizing this is a first step, not a given.

**"MIDI input"**, read in context, meant the **configurable** half rather than
the decode path, and it **landed on 2026-09-15** (`plans/midi-control/`). A
channel picks its port and its channel filter from the sidebar, the choice is
persisted, a controller maps to any parameter, and the transport takes
gestures. OUT stays inert: **MIDI output is still 0.2.0** (§2 item 2).

The decision this was expected to force did not arise. Port selection is
per-driver and `driver.rs` is not a trait (§5), so the question was whether
issue **#9** had to come first. It did not: what port selection actually needs
is a port list and a message tag from each driver, which is two small
per-driver methods rather than a trait. Both drivers have them; the Core MIDI
one has never been compiled, because it is behind a macOS `cfg`.

### 0.2.0 — the freeze

Everything in §4's "in" list that 0.1.4 does not absorb. The version number is
Adam's to pick when it gets there; nothing here depends on it.

---

## Maintaining this

Rewrite it when the freeze line moves, not when an item lands — a row going
green belongs in the owning plan's `00-status.md`. When every row in §4's "in"
list is closed, this document has done its job, and what shipped belongs in
`JOURNAL.md` — written after the fact, which is how releases are recorded here.
