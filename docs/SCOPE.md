# Scope

Status: the remaining set, drawn 2026-09-14, at Adam's request for *"some kind
of an accounting of what is actually left to be done."* Scope answers from Adam
the same day: **0.2.0 is the line**, CLAP is in, and the sampler needs key
zones.

Every other planning document here answers a narrower question. This one draws
the finish line, sizes what stands between here and there, and sorts
everything recorded anywhere into *in* or *out*.

**The line is 0.2.0, not 1.0** — Adam's call, and the honest number. §9 records
how releases actually get cut here.

**What this document is not.** It does not order the work — `FOCUS.md` still
decides what is next, and two documents claiming that job is the failure mode
`FOCUS.md` warns about. It does not restate detail another document owns; each
row points at whoever owns it. The rule is **SCOPE says what and how big; the
linked document says how.**

---

## 1. What 0.2.0 means

> **mooloop is 0.2.0 when sound can get *in*, the program can be driven without
> the mouse, and its devices are ones you would choose rather than tolerate.**

The fourth thing 0.2.0 has to settle is not a feature: **whether Buffer stays.**
See §7.

---

## 2. The list, sized

Adam's thirteen, plus a fourteenth he added on 2026-09-14 (key zones).

Grouped by *kind of work*, not by the order they were listed. The grouping is
the point: these are four different activities and scheduling them as one list
is what makes the set feel endless. Items 13 and 14 have their own sections at
the end.

Items 3 (audio input), 5 (audio and MIDI recording) and 12 (EQ) are done:
`plans/archive/audio-recording/`, `plans/archive/midi-control/` and
`plans/archive/eq-v2/`.

### Tier A — last mile. The thing exists; finish it.

| # | Item | Where it stands | What is actually left |
| --- | --- | --- | --- |
| 1 | **Left sidebar** | Built (`ui/channel-sidebar.slint`); mute/solo, volume and pan are in it as well as the rack row (MOO-8). OUT stays inert because MIDI output does not exist. | OUT, with MIDI output (item 2). |
| 7 | **Browser sidebars** | The tabs work, over one flattened row model (`BrowserRow`), with search, filter, drag to the rack and preset preview (MOO-9). | Category and tags are display-only strings, so there is no taxonomy *navigation*. One perf wart: expanding a folder re-`read_dir`s synchronously on the UI thread. Check Linear before taking either as still open. |

### Tier B — real engineering, from zero.

| # | Item | Where it stands | Size |
| --- | --- | --- | --- |
| 2 | **MIDI I/O** | **In landed 2026-09-15** (`plans/archive/midi-control/`). **Out does not exist** — the JACK driver registers `out_l`, `out_r`, `midi_in` and nothing else. | What is left of the input half is not construction -- **none of it has been run against a keyboard**. **MIDI output stays 0.2.0.** `docs/CONTROL_SURFACES.md`. |
| 8 | **Full-size browser** | Does not exist. The view set is closed at five (`PaneViews`). | **Small, and the cheapest win on the list.** A sixth `PaneViews` entry, one `ViewSlot`/`PaneToolbar` block, a `view.pane-browser` action, a `VIEW_COUNT` bump and a settings migration. The `BrowserRow` model and row rendering transfer unchanged; what a full-size view adds is column layout, selection and search — which item 7 wants anyway. |
| 9 | **CLAP effects + instruments** | `plans/plugin-hosting/`, planned 2026-09-16, which also outlines VST3 and AU after CLAP. | **The largest item on the list by a wide margin.** |

### Tier C — already built under another name. What is missing is a gesture, not a subsystem.

| # | Item | Where it stands |
| --- | --- | --- |
| 4 | **Audio routing** | **This is the most finished area in the codebase**: a compiled bus graph, sends with per-edge latency compensation, and typed audio edges. **What is genuinely missing is sidechain** — a dependency edge that schedules a producer without summing it — plus mid-chain send taps (`SendTap` has two positions, both post-chain) and `OutletTap::Output`, which is declared and then refused as `TapIsLate`. Read `plans/archive/typed-audio-edges/` before building any of it. |
| 4b | **MIDI routing** | Separate item, and it is Tier B: there is **no MIDI graph at all**. One input port, decoded once, and delivered by a per-channel route table (`MidiRouting`, a `Vec<MidiInputRoute>` built on the control thread and swapped in whole). What is still missing is the graph — one port, no merge, no fan-out, no per-device filter. |
| 6 | **Resampling** | **Folded into `plans/archive/audio-recording/` on 2026-09-18** (Adam: *"its the same thing, only the source is different"*): recording with a channel, track or the master as the source. A faster-than-realtime offline *bounce* is a different gesture that nothing has asked for yet. If something does: `render_blocks` (`offline.rs`) already takes an arbitrary `FnMut(&[f32], &[f32])` sink, so "render to a sample" is that closure filling a `Vec`, and what is missing is only a **scope smaller than the master bus**, which `RenderState`'s solo model can build. Small. |
| 13 | **Master bus comp** | Promoted out of this tier on 2026-09-14 — Adam wants it to be its own thing, with its own laws. **See §2.1.** |

### Tier D — taste, not engineering. These need Adam's ears, not a plan.

| # | Item | Where it stands |
| --- | --- | --- |
| 11 | **Reverb** | Nothing is owed (`docs/REVERB.md`). **Reverb is the only Tier-D item with no reference measurements**, so this is a pure character call or a new measurement run. |

### 2.1 Item 13 — the master bus compressor

Adam, 2026-09-14: *"we've created a really good feeling console mix
experience… I'm totally cool with us using the master strip to house the bus
comp — that's only natural — but it needs to be more prominently displayed in
the rack, with a nice meter, and it should be running its own algos, aimed at
like SSL, 2500, and idk Massive Passive or something. Turning it on should
feel special. We have the measurements to do it (and to show that they're not
the same as the channel comps)."*

Built 2026-09-23 (`plans/archive/master-bus-compressor/`, MOO-13);
`docs/CURRENT.md` has what it does. What the scoping settled:

- **Its own laws, not new values.** The reference run
  (`spikes/preamp-measure/RESULTS.md` §4) splits exactly on the axis that
  separates a bus compressor from a channel one: opto and FET units hold
  2.4–3.6× more gain reduction at 500 ms after a long note; every VCA unit is
  at 1.0×, with no programme dependence at all. That is a difference of
  **law**, not of values — the console plan's standing rule (*a voicing
  selects laws, never values*). So the master comp has no programme
  dependence, and its three voicings are three measured units distinguished by
  detector shape and timing law: the SSL G bus (peak VCA), the API-2500 (RMS
  VCA, with its 3.8 ms attack floor) and the Fairchild 670 (vari-mu: Adam
  meant vari-mu, not Massive Passive, settled 2026-09-14).
- **"Turning it on should feel special."** A taste requirement, and the one
  part that needs a listening pass rather than a spec.
- **It is not the master safety limiter.** A bus compressor is a musical
  device; a safety limiter is engineering. Both are in for 0.2; they are
  separate rows. The safety limiter is `mooloop_dsp::output_guard`
  (`docs/GAIN_STRUCTURE.md`, "The output guard"). It does not look ahead, and
  has no knob for it (MOO-217, 2026-09-24).

### 2.2 Item 14 — sampler key zones

Built 2026-09-25 (MOO-14): key ranges and roots per zone, a minimal ZONES
page, the format and the export. `docs/CURRENT.md` has what it does.

Adam, 2026-09-14: *"sampler v2… a lot of that is done. I would say we should
at least get key zones, if not layers."* **Velocity layers are the "if not
layers" half**: their ranges are stored and not played, and they are built
only if 0.2 has room. The mapping workspace (MOO-40) and SFZ import (MOO-41)
build on zones and are in with the rest of Sampler v2 (§4).

---

## 3. Required for 0.2.0, but not on Adam's list

These are not additions to the scope. They are already-recorded work that his
thirteen sit downstream of, or that the definition in §1 forces.

- **The loose ends** in `LOOSE_ENDS.md`. Not all are 0.2 blockers; the ones
  that are get picked per polish pass rather than enumerated here.

---

## 4. The freeze line

### In for 0.2.0

All thirteen, with the amendments that the sizing above forces and the later
additions:

- **Item 5's shape was settled 2026-09-17** (Adam): a take goes into the
  selected channel's sampler, and the channel's one input menu chooses between
  audio and MIDI. No per-track clips.
- **Item 13 grows.** It is no longer "a compressor exists" — it is a master bus
  compressor with its own laws, its own meter and its own presence (§2.1),
  *plus* a separate master **safety limiter**, with no lookahead (MOO-217).
- **Item 11 is a listening session**, booked like one — not a plan directory.
- **Item 14 joins**: sampler key zones (§2.2), Adam's addition on 2026-09-14.
- **Sampler v2 joins, 2026-09-18.** Adam: *"i think i do want v2 in 0.2.0."*
  It is the Linear project **Sampler V2**, with MOO-42 as the umbrella:
  click-free loop seams, legato, the rest of tempo fit, transient detection,
  pattern from slices, loop quantize, the mapping workspace and SFZ import.
  Live loop gestures and per-repeat variation were cancelled the same day
  because Buffer covers them. If variation comes back, it goes on the MIDI
  notes, as a note modulator. This is the single largest block of recorded
  work in the project. Its biggest items are the mapping workspace (MOO-40)
  and SFZ import (MOO-41). The stretching-polyphony cap it left behind is
  MOO-7.
- **Rendering joins, 2026-09-23.** Adam: *"0.2.0"* -- all of it, when asked
  whether to split it. It is the Linear project **Rendering** (MOO-180 to
  MOO-191, MOO-193 and MOO-194): a render writes several files named from a
  folder and a template, with stems from mixer tracks (*"like recording the
  direct out from a console channel"*) and from channels directly, one file
  per pattern, a song, loop or typed range, a wrap tail, 16-bit and dither,
  mono, the file's sample rate, normalizing, auto-bumped names, remembered
  settings and presets. MOO-180 is the one the rest stand on. The pattern
  render is also the groundwork for render-in-place once clips exist, which
  stays after 0.2.0.
- **Song automation joins, 2026-09-30.** Adam, asked whether it is before or
  after the freeze: *"yeah probably before 0.2.0, almost certainly"*. Pattern
  lanes stay as clip automation, and song lanes are added in a panel under the
  playlist. It is the Linear project **Song automation**, planned as
  `plans/song-automation/`.
- **Undo as incremental commands joins, 2026-09-30.** Adam, on MOO-466:
  *"answer is 1 now, 2 by 0.2.0 if not sooner"*. Option 1 is MOO-466: channel,
  track, pattern and preset edits stop rebuilding the engine. Option 2,
  undo and redo applying the difference the same way instead of a whole-song
  install, is MOO-469.

### Out, explicitly

- **Everything in `archive/ROADMAP.md`'s "Later, Not Scheduled"** that item 2 does not
  pull in: MIDI output is now *in* (item 2), but controller mapping beyond it,
  multiple time signatures and tempo maps, groove extraction, the
  text/algebraic pattern view and the node-based patcher are all out.
- **`theming/`, `pattern-bank-floor/`, `device-registry/`** — parked by
  `FOCUS.md` on 2026-09-12, each with a recorded reason and a recorded unpark
  condition. A toolkit swap is out altogether: Adam, 2026-09-22, archiving
  `egui-view-layer/`: *"we can archive. i think QT would be better than egui
  if we do switch toolkits"*. Note that `theming/` gets *cheaper to defer and
  more expensive to do*, and that its real argument is accessibility: the
  working type size is 7–11px and is not adjustable. That reason does not
  expire, so it is out for 0.2 and should not stay out forever.
- **Windows and macOS as release targets** (#23, #24, #25). macOS builds and
  runs for development; signing and notarization are out. `PRODUCT.md` is
  unambiguous that Linux is the platform. **Amended 2026-09-18 (Adam):** from
  0.1.4, every release also attaches an unsigned, ad-hoc-signed Apple Silicon
  `Mooloop.app`, zipped, so the Mac can run a release build without building
  one. It is a convenience build, not a supported target, and there is no
  Intel build.
- **The modulation-rack move and the tracker question** (MOO-159). Adam,
  2026-09-26: it is one idea (automation written in something
  tracker-inspired), and *"something i am interested in doing. i havent
  decided on it"*. Out, and not a question waiting on him: it comes back only
  when he raises it. Song automation shares the pattern lanes' point type and,
  by its last step, their editor, so a tracker-style view would still be one
  view of one data model, not a third editor.
- **A curated factory bank.** Every device ships presets to prove its
  architecture reaches its range; authoring content by taste across every
  device is a deliberate later push.

---

## 5. Dependency order

Not a schedule. Only the constraints that actually exist.

```
   ┌─ #7 backend boundary ──┐
   │  (AudioBackend)        ├──► item 2  MIDI out
   └─ #9 MidiBackend ───────┘
```

Everything else is independent and can be handed to a parallel session:
item 4's sidechain, item 6's render-to-sample, item 11.

### The gate, in detail

**The I/O backend boundary.** `crates/mooloop-engine/src/driver.rs` is
**deliberately not a trait**. JACK and Core Audio are chosen by `cfg`. So
every I/O extension must be written twice, once per driver, or that decision
is revisited first. Issues **#7** and **#9** name the fix. Neither MIDI port
selection nor audio input waited for it: each needed a few small per-driver
methods rather than a trait.

---

## 6. Where this overrides the other documents

Adam's thirteen was given on 2026-09-14 and `PRODUCT.md`'s own precedence rule
is that *"current direct feedback outranks this file."* So where
`archive/ROADMAP.md`'s "Later, Not Scheduled" lists MIDI output, MIDI
recording and controller mapping, output and recording are in, and general
controller mapping stays out.

---

## 7. The one question that changes what 0.2.0 is

Adam's item 10 is *"do something with the buffer device/idea or just remove
it."* **Settled 2026-09-15: Buffer stays a device**, and the rack-end
placement with its own sequencing lane is not ruled out (`BUFFER_ENGINE.md`;
`FOCUS.md`, "Standing judgements"). Whether the
workflow beats bouncing to a sample is a musical verdict, and only Adam can
give it.

---

## 9. Releases

**How releases actually happen here**, stated by Adam on 2026-09-14:

> *"I don't really care at all about what `VERSIONS.md` says. I've never read
> it. I've been releasing when it felt like enough changes had accumulated to
> justify a release, basically."*

**The macOS menubar** (Adam, 2026-09-14) moved out of 0.1.4 unstarted and goes
to whichever release comes next. mooloop draws its own in-window menu bar
(`ui/menubar.slint`), which is right on Linux and wrong on macOS, where the
menu belongs in the system bar at the top of the screen. There is **no native
menu integration anywhere in the tree**. The action registry (`actions.rs`)
already separates an action's identity from its surface, which is what a
native bar needs. It can only be built and checked on the Mac. Sizing this is
a first step, not a given.

### 0.2.0 — the freeze

Everything in §4's "in" list. The version number is Adam's to pick when it
gets there; nothing here depends on it.

---

## Maintaining this

Rewrite it when the freeze line moves, not when an item lands — a row going
green belongs in the owning plan's `00-status.md`. When every row in §4's "in"
list is closed, this document has done its job, and what shipped belongs in
`JOURNAL.md` — written after the fact, which is how releases are recorded here.
