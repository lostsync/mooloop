# Mooloop — dev journal

*Reconstructed from the commit log. Dates are derived from relative timestamps, so treat them as approximate; a few commits are out of chronological order in the log itself (rebase/cherry-pick noise), and I've placed them by content rather than by position.*

---

## Aug 11 — Phase 0: it builds, it registers, it makes no sound

Scaffolded the workspace as five crates: `mooloop-core` (musical time types, lock-free bridge enums), `mooloop-dsp` (a `Device` trait and a throwaway metronome), `mooloop-engine` (JACK via pipewire-jack, realtime transport, rtrb SPSC bridge), `mooloop-ui` (Slint, dark toolbar, placeholder rack), and `mooloop-app` to wire them together (`b8a3fd1`). Clean build, clean clippy, two passing tests, a live JACK client.

And silence. PipeWire's JACK bridge doesn't auto-wire a new client's outputs to the playback sink, so the ports existed with no links attached. Connected `mooloop:out_{l,r}` to `system:playback_{1,2}` after activation, best-effort with a hint toward the patchbay if it fails (`9ba4dbe`). First of two silence bugs that looked identical from the outside and had nothing in common underneath.

## Aug 12 — Phase 1: a real instrument

Replaced the metronome with a sampler and a 16-step sequencer, FL-style (`5f95c88`). Pattern/Step data model with velocity carried from the start for forward compatibility. ADSR, sample-accurate triggers, start/loop points, three loop modes, linear-interpolated resampling, and a synthesized default kick so it's audible out of the box. Samples publish through an ArcSwap slot so nothing allocates on the audio thread. WAV loading via zenity plus hound. Half-open `[start, end)` step-boundary intervals so blocks don't double-fire or drop a step.

## Aug 17 — Diagnostics, then the bug they found

A "no playback, no VU" report I couldn't localize by staring at it. Rather than guess a layer, built tools that test each one independently (`14f4991`): a headless `engine-selftest` binary driving the full JACK path, `MOOLOOP_AUTODRIVE=1` driving the real Slint callbacks and reporting whether commands actually forward, an offline sequencer→sampler render test, and env-gated debug logging. All of it passed — which is what made the real cause findable.

It was hound. `samples::<f32>()` only reads IEEE-float WAVs; on ordinary 16-bit PCM every sample yields an error, and the decoder was doing `filter_map(Result::ok)`. Every error silently dropped, empty buffer, sampler treats zero-length as instant silence, no message anywhere (`a51b78d`). Decode at native width now, propagate errors loudly, regression tests both ways.

Same day, Phase 2: 16 channels, an 8-pattern bank, pattern-indexed step edits so editing a non-playing pattern persists, per-channel mute (`1881f30`). Then the engine hardening pass that everything since has stood on (`7e58b4b`): preallocated `StereoBus`, a fixed-capacity sample-timed `EventList` in the shape VST3/CLAP/LV2 use, an `AudioNode` trait replacing `Device`, segment-based sampler rendering split at event offsets, epsilon-tolerant boundary detection (`BOUNDARY_EPS = 1e-6` ticks) proven by a test asserting exactly 400 note-ons across 100 bars of irregular block sizes, `frames_played` as integer ground truth, per-channel channel strips summing into a master, xrun counting, and sample loading moved off the UI thread because the OS was marking the app frozen while the file dialog was up.

## Aug 18 — The long day

Notes became tick-addressed (`5a2dc1d`) and the piano roll became expressive (`d6702ce`), with the velocity lane pinned to and aligned with the grid. Wrote down the buffer-centric product direction and the always-running channel buffer model (`f7bb6c2`, `6a7ae8c`) — the thing the whole instrument is eventually for. Channel volume/pan, a layered song playlist, and playback decoupled from whichever view is open (`21ec5de`, `a6f8bf4`, `acb0790`).

The UI stopped being ad hoc. Rack, sampler editor and transport had each grown their own controls, so a knob in one panel didn't read like a knob in another. Established a shared vocabulary (`dbc7d64`): a `KnobFace` worn by both the full labelled knob and the compact rack knob so they can't drift, LED-segment metering coloured by scale position the way hardware does, latching clip indicators, an `EnvelopeEditor` with fixed stage-width budgets so dragging one handle doesn't reflow the others. Adopted it across the app (`ee1650e`), which handed 108px back to the step grid.

Two input bugs, both Slint routing:

- Every control needed two clicks. `FocusScope` consumes the press that focuses it and ignores presses once focused, and the focus scopes were declared after the touch areas — so the first press was eaten and the second fell through (`cc40eab`). Fix: `focus-on-click: false`, take focus explicitly from the touch area. Regression test dispatches real pointer events rather than invoking callbacks, deliberately avoiding accessibility default actions since those bypass the routing at fault.
- Per-cell `TouchArea`s can't support a drag at all: the cell where the press lands keeps the grab. The rack grid moved to one hit area deriving the cell from mouse-x (`aead814`). Paint and stretch report a *target* state rather than toggling, so dragging across mixed steps doesn't invert each one.

Also in `aead814`: the toolbar split into a transport row (state of playback) and an edit row (what the next click does), patterns became a stepper plus a jump menu that costs the same width at two patterns or two hundred, and cells gained an onset mask beside the coverage mask — before that, a ×2 ratchet, a ×4 ratchet and one sustained note all drew as a full cell.

Project bundles landed as a versioned, durable contract with offline export sharing render state (`57efa44` → `5adb7dd`). DrumSynth and MonoSynth DSP nodes were written against the existing `AudioNode` trait but deliberately not wired to anything yet (`ae3e742`). And documented the headless agent output plus the two gotchas that make it silently screenshot the wallpaper (`3b0ca2f`, `c4d45a5`) — software rendering is the right default for looking at the UI, not the live GPU path.

## Aug 19 — Synths become instruments

Generated sources exposed as channel instruments (`32b50e3`), drums made punchier (`77e2320`).

The mono synth clicked constantly and the reason was structural: `Adsr::note_on` reset level to zero, so every note landing while the voice still sounded stepped the output through zero — which, at the default 150ms release, is most notes in a pattern. Attack now starts from wherever the envelope already is, velocity slides in over 5ms, and block-rate parameters that scale the signal directly get a one-pole `Smoothed` (`1ab8f85`). Added the LFO as one shape with a depth per destination rather than a destination selector, bipolar shapes leaving zero rising so a retrigger isn't itself a click.

Piano roll notes became genuinely draggable (`72bf20a`) — the same defect as the rack grid, one layer up: each note's touch areas lived inside a rectangle bound to that note's tick, so the moment a drag updated the model the rectangle moved out from under the cursor and the delta collapsed to zero. Slint's binding-loop checker rejects the self-recursive scan needed to resolve a note from a press position, so the lookup moved to Rust behind a pure callback, scanning back to front so overlaps resolve to the note drawn on top.

Presets for generators and whole channels in a well-known config directory (`9d44591`), global swing (`dcff827`), and a real menu bar built from pieces that know nothing about which actions exist, so adding an item costs one line (`77334ea`).

## Aug 20 — A composition language

Stepped back from features and defined the instrument UI composition language and its layout/selector primitives (`a6e0d52`, `ee32937`), then rebuilt the source editors as instrument panels (`1399e50`) and, after sketching alternatives (`8d83f8f`), as horizontal rack devices (`6855ac2`).

## Aug 21 — Public release, then the effect suite

Prepared the repo for public release: full GPL terms, an honest README about what this experimental prototype is and isn't, songs saved as openable document files (`dc59381` → `06cdb90`).

Two long-standing annoyances got real diagnoses. Dev builds were OOMing on the 16GB workstation, mostly linking mooloop-ui's Slint-heavy test binaries — capped jobs, switched to mold, trimmed debuginfo (`979f276`). And drum decays sounded ~9.2× longer than their labels promised: `ExpDecay::set_time` treated its argument as the raw 1/e time constant, but level only becomes inaudible after ~9.21 of those, which is why a 39ms setting produced a 363ms kick (`e0d6291`).

Then the effects, done in the right order. Design first (`b81fa96`, `a339763`): descriptors as the single source of truth for range and normalization, engine-owned base value plus modulation offset so knobs and modulators stop fighting, and an explicit record of what's deferred so a later pass doesn't mistake absence for oversight. Filter as a vertical slice — core types, DSP node, engine path, UI face (`a9cf6c9` → `2a4f146`). The realtime path is the interesting part: `Box<dyn AudioNode>` isn't Copy and core mustn't depend on dsp, so structural changes ride a second rtrb pair, with displaced boxes returning via `StructuralReclaim` so the audio thread never frees memory.

With one effect proving the plumbing, generalized before adding more (`97139ac`): tagged `EffectParams`, static `ParamDescriptor` tables, one positional param-changed callback instead of one per knob. Drive (four curves, 2× oversampled so harmonics don't fold back as fizz) and bitcrush (deliberately not oversampled — its aliasing *is* the effect) followed cheaply, which was the point.

Also pinned the exact-version Slint 1.17.1 docs into `AGENTS.md` (`3a064d1`), started `CONTRIBUTORS.md` as a sign-in sheet for which model/harness combinations worked on what (`c146e47`), and made verification proportional to change risk (`36f75fd`).

## Aug 22 — Delay, dynamics, and the mixer

The delay's buffer is the buffer device's buffer, so `delayline` landed in dsp as a shared primitive rather than privately inside the delay (`b5c672b`). `ReadHead` deliberately doesn't know about playback rate or direction — the caller passes per-frame offset drift — which is what lets a fixed tap, a repitching tape delay, a reverse window and the buffer device's detached heads all be one type. 4-point cubic Hermite reads, because linear droops and aliases audibly off unity rate. Six controls didn't fit 1U, so faces became width-quantized in rack units.

Which immediately exposed that the rack's ScrollView viewport was the constant 758px — the width with an *empty* chain (`6a19b68`). Anything beyond it was laid out and unreachable, and since the viewport never exceeded the visible width, the view couldn't scroll at all. Wrong since the effect chain shipped; one filter already overflowed it. Bound to the row's intrinsic width, because a constant has to be re-derived every time a device is added, which is exactly how it drifted.

Gate, compressor and limiter share one `dynamics` module (`9aa91fb`) — three detectors and three sets of decibel plumbing would have been three chances to disagree. Detection is stereo-linked on the louder channel. The limiter has no lookahead on purpose: lookahead means latency, and with no plugin-delay compensation in the engine it would shift a limited channel against everything else.

The mixer arrived in three moves. Channels got a bank of buses to feed, with master as bus 0 and buses only routing to lower-numbered buses — acyclic by construction, so the render pass is a single descending sweep with no topological sort in the audio callback (`2c3ee4e`). Effect commands started addressing an `EffectTarget`, so one set of messages serves a channel's chain and a bus's, with the per-slot machinery extracted into a shared `EffectChain`. Pan law normalized to unity at centre while staying constant power, since a signal now crosses several pan stages and the raw law charged 3dB at each. Per-bus peaks publish through a small array of atomics instead of the event ring, holding the peak until the GUI's read clears it so a transient between frames still shows (`4520b4f`). Then the mixer pane, where clicking a strip's name plate points the device rack at that bus — putting a compressor across a group is the same gesture as putting one on a channel (`c679128`).

And then I took the lower-numbered rule back out (`c472721`). It was never an internal necessity; it made the user's mental model carry an implementation constraint. Whoever edits the graph topologically sorts it and the callback executes the schedule — cheap here because every bus owns a permanent buffer, so the whole schedule is a `[u8; MAX_BUSES]` permutation compiled by Kahn's algorithm with no allocation. Cycles are refused at the picker (greyed with a reason, not hidden), at the command boundary, and on load, where a cyclic file flattens to everything-to-master so it still opens and plays.

Third pass at the build problem, and the first one with the right cause (`df2cbce`, `6f1df1c`): an earlier attempt used `nice`, which adjusts CPU scheduling and is ignored entirely by memory allocation and page reclaim. The limit is concurrent linking, not CPU. Capped debug info workspace-wide with split-debuginfo (a test binary went ~450MB → 217MB) and dropped default jobs from six to three; eight, six and four had each hard-locked the compositor.

## Aug 23 — Poly synth and zoom

A polyphonic source: `DeviceKind::PolySynth` with a 16-voice pool, per-voice oscillator/envelope/filter/pan plus global LFO and portamento (`3f520e9`), then its UI face across Osc, Amp/Filter, Mod and Voice pages (`cb0d14c`). Generic device chain controls (`df94176`) and channel clipboard actions (`3e08096`).

Noticed I was clicking pitch zoom-in three times before every edit, so the default row height now starts there (`e685af3`). The four zoom buttons collapsed into one horizontal and one vertical `ZoomScrollBar` — thumb pans, end grip zooms around the fixed opposite end, built-in bars off so there's exactly one per axis. The widget itself was rescued from an abandoned selector branch where it was the only thing that hadn't already landed on main (`8665a4f`).

Latest: duplicate loop-boundary events prevented (`f332b7c`), device frame metering fixed and controls compacted (`8b3a9d2`).

## Aug 23 — The effect container earns its name

The device host was a UI contract with a matching engine shape, but three things said the container wasn't finished. The dry path blended against a latency-introducing device's wet copy with no alignment — drive reports 15 frames, so its own dry signal combed against itself. Bus effect slots were dead glass: `DeviceMeters` had no address space for them, and the rack polled the selected channel's chain even while editing a bus. And the trims read in percent of a 0..2 range, which put unity at "200%" and geared the drag wrong, while the source device's output trim sat at a frozen "80%" because the window property behind it was only ever restated on channel selection (`a82bed0`, `c58f3fb`).

The alignment fix is a stereo integer ring per slot (`DryAlign` in mooloop-dsp), built from the node's `latency_frames()` at install time and reclaimed with the node on removal. Bypassed slots keep feeding their ring so the delay history survives a bypass round-trip. Meters gained a target-index space: channels 0..256, buses above them, one polling loop for both.

Trims moved to dB from unity — −60 dB (−∞) to +12 dB, one `TrimKnob` class for the effect in/out, the rack-row volume and the source output trim, all storing linear gain in the wire and project file. Linear-in-dB needed no custom curve; dB is already the logarithm of gain. Channel volume widened to the same +12 dB ceiling so identical-looking knobs behave identically.

Then the faces (`a65bd0f`). Seven files each carried a copy of the header strip, the drag handle, and — on six of seven — a hidden rectangle of bypass/remove buttons that rendered nothing and wired nothing. All of it collapsed into `EffectDeviceShell`; a face now declares its name and unit count and its controls arrive as `@children` under the shell's header. Net −307 lines. A new effect kind is a face of controls plus DSP wired to the container; no chrome of its own to write or drift.

## Aug 23 — Piano roll opens up, the sampler catches up

A lint pass closed out the effect-container work (`8877380`): named-field struct literals in place of mutate-after-`default()` in the poly synth tests, and a `RemoveEffect` branch rewritten as `.map()` instead of an `if let`/`else None`.

The piano roll's pitch axis had quietly been capped at C2–C6 (`piano-low-note`/`piano-high-note` = 36/84, a 49-row canvas) since the zoom work landed — anything below C2 was simply unreachable, the same class of bug as the 758px rack viewport a day earlier: a constant sized for the common case with no way to reach past it. Widened to the full MIDI range 0–127, with every pixel extent (`piano-content-h`, the key-row loops, the zoom scrollbar's fraction math) re-derived from a new `piano-note-count` property instead of a second hardcoded `49` (`e45a37a`). The viewport now opens on C6 at the top so the working register doesn't move, but nothing stops the user scrolling past it.

That freed the vertical `ZoomScrollBar` for the pitch axis, so both zoom button pairs came out entirely: the horizontal bar under the grid already worked this way, and pinch-style `Ctrl`+wheel (`Shift` to target time instead of pitch) covers the case a scrollbar-drag doesn't (`f7e36a0`).

Then a full pass on the sampler, framed as its own session (`7e99e34`):

- A new channel no longer synthesizes a kick into its sample slot — it starts genuinely silent, `SampleReference::Empty` (`87ce723`). `Builtin { id }` stays only so a project saved before this change still sounds the way it did; the engine's default sample pool went from "every slot pre-loaded with a shared kick" to "every slot `None`."
- The waveform view gained the same `ZoomScrollBar` already proven on the piano roll's time axis, re-binning from the actual sample data for whatever range is visible rather than stretching a fixed 256-bin overview — the previous view was too coarse to place a loop point precisely (`79fe51f`).
- Start/End/Loop Start/Loop End became exact frame-index fields (`SampleField`, drag/scroll plus `Ctrl`/`Shift` for single-frame steps) next to the waveform markers, which were pixel-precision only (`e09ceca`).
- Coarse/Fine tune moved from full-width `ParameterFader`s to a `MiniKnob` pair — matching the mono/poly synths' own coarse/fine convention — freeing a row for a "C4 · 261.6 Hz" readout, since "+3 st / +40 ct" doesn't say what pitch that actually is (`e11328d`).
- A `PlayheadMeters` atomic array, shaped like the existing `BusMeters`/`DeviceMeters` (latest-value-only, no backlog), lets the audio thread publish each sampler voice's position once a block; the UI's 8ms pump timer draws one faint line per active voice, so layered retriggers show as several moving lines (`fa33fcd`).
- The new tune readout then got clipped at the bottom of the sampler's fixed 240px content budget — the knob column had grown taller than the fields column beside it. Dropped the redundant "Coarse"/"Fine" captions (the tooltip already names each knob) to bring the row back under budget, and extended the screenshot regression to actually stage sample frames, the tune label, and playhead positions so this class of clip gets caught next time (`ae7af0f`).

Last: the channel-settings save validator still checked volume against the old `0.0..=1.0` unity range from before trims moved to dB, so a channel legitimately turned up toward the new +12dB ceiling failed to save with a generic "invalid mixer values" error and no way to tell which field or why. Rewrote validation field-by-field — each check now names its parameter and reports the allowed range in the error, and the volume bound follows `MAX_LINEAR_GAIN` instead of a stale literal (`b1230c4`). README updated to say plainly that normal use can't fail a save anymore, and a malformed file identifies its exact channel/parameter/range in the error dialog instead (`38acde7`).

---

## Patterns worth noticing

**Hardcoded constants drift; derived ones don't.** The 758px viewport, the 220px pattern strip with 190px of hole, the fixed 5px note edge zone that ate a minimum-width note, the forwarded-command threshold of 29 that had overcounted the baseline, the piano roll's C2–C6 range hardcoded as a bare `49` in half a dozen places. Every one was correct on the day it was written; a stale range check in the save validator (checking volume against `0.0..=1.0` after the trim ceiling moved to +12dB) is the same failure one layer over, in validation instead of layout.

**Slint input routing is where the bugs live.** Focus scopes eat the first press. `TouchArea.pressed` only tracks the left button, so right-drag-to-clear cleared exactly one step. Per-element hit areas can't survive a drag that moves the element. Three different symptoms, one lesson: test by dispatching real pointer events, because invoking callbacks directly passes even when routing is completely broken.

**Two silence bugs, no shared cause.** Missing JACK links; swallowed decode errors. The diagnostics harness built to chase the second one is still earning its keep — autodrive now covers the whole effect command surface.

**Write the primitive once when the second user is already visible.** `delayline` for the delay and the buffer device. `dynamics` for three effects. `EffectChain` and `OutputStage` for channels and buses. `KnobFace` for two knob sizes. Descriptor tables instead of a callback per knob.

**Constraints get relaxed as soon as they're cheap.** The lower-numbered bus rule lasted about a day, and removing it took a topological sort over fixed-size arrays. Worth asking of the remaining rules which ones are still load-bearing.

**Deferrals are recorded with reasons.** Limiter lookahead waits on plugin-delay compensation. `ParamAddr`, inter-channel data and audio sidechain are named as deliberate absences. The README states what the mixer deliberately is *not* yet.

## Open threads

- Modulation groundwork exists; nothing drives it yet.
- The retained-audio buffer device in `docs/BUFFER_ENGINE.md` — the primitive is built and proven, the device isn't.
- Mixer: inserts only, no sends, sidechain, solo, stem export, or bus renaming.
- No plugin-delay compensation, so no lookahead anywhere.
- Undo and clipboard are menu placeholders waiting on a command layer.
