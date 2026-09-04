# Mooloop — dev journal

*The early entries were reconstructed from the commit log; dates there are derived from relative timestamps, so treat them as approximate. A few commits are out of chronological order in the log itself (rebase/cherry-pick noise) and are placed by content rather than by position. From the modulation-grid entry onward it is kept live — a dated section per arc of work, not per commit.*

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

## Aug 24 — A real preferences dialog, then EQ, reverb, and modulation

Appearance prefs had been a single-purpose dialog; folding Audio settings into it as a second tab would have meant a second bespoke dialog shape. Rebuilt it as a general two-pane `PreferencesDialog` — a page nav list down the left, page content on the right — with Appearance as its first page (`76634ce`). JACK device controls landed as the second: driver select (JACK only, ALSA later), an output-device picker read from the live JACK port graph, buffer size, a read-only sample-rate readout, and auto-reconnect for when the port graph changes underneath the app. The JACK surface is its own Slint component rather than a driver-agnostic one, because JACK's client API has no sample-rate setter and buffer size is server-wide, not per-client — a future ALSA page is a sibling, not a rewrite, since the two drivers expose genuinely different controls (`35b4084`). Settled a few days of build/doc loose ends alongside it: portable Cargo target output, linker requirements documented, dev setup instructions trimmed (`4d36c04`, `9d94853`, `ee8a1fd`), and a later pass tightened the dialog's own layout now that it had two real pages to balance (`73aca0f`).

Two of today's doc commits are process, not content: an `AGENTS.md` exception letting an untracked Markdown file under `docs/` be committed alongside doc work without asking each time (`fdfff4e`), and preserving a couple of Adam's own working notes — dockable-pane and step-shading ideas for `ENHANCEMENTS.md`, a `SHORTCUTS.md` stub (since consumed, now `docs/archive/SHORTCUTS.md`) — that had been sitting uncommitted (`37f46d3`). Also today: the journal's own do-not-edit guard came off, since keeping it updated live turned out to be fine (see the note at the top of this file — no commit hash, just Adam's call).

The effect suite grew three devices in one day. Seven-band parametric EQ: peak/shelf/pass biquads per band plus dedicated high/low-pass filters, sample-timed coefficient updates, no allocation in `process` (`4a7cf6f`). Then a compact allocation-free `SpectrumAnalyzer` — a fixed Goertzel bank over a mono sum, 48 log-spaced bands, computed once per 2048-sample hop and only when a device display subscribes — so a device host can show a spectrum without ever handing PCM to the UI thread (`f6ffc1a`). Convolution reverb followed: an IR player first, a room generator second, meeting at one `StereoIr` boundary so a future WAV/AIFF loader is just another producer of the same type. Rooms combine a small image-source early-reflection model (shape controls reflection density) with a material-filtered deterministic diffuse tail; the direct sound is deliberately absent since the host already owns dry/wet blending (`385a765`). Preparing a room's FFT partitions runs off the audio thread on a worker that coalesces control edits for 80ms, and the resulting prepared resource carries a fingerprint so a slot refuses a stale swap — a generic mechanism meant to serve future resource-backed devices, not just reverb (`16c7b48`). And a unified modulation effect: chorus, flange, ensemble, and ADT as policies over one short fractional stereo delay line, plus a phaser sharing the same LFO and parameter contract even though its all-pass cascade is a different signal path underneath — one device rather than four, because a user thinks of them as one knob-turn apart (`76c8730`).

Then the CPU jankiness Adam had been noticing predated all of this and finally got a real diagnosis: recursive DSP state — filter feedback, envelope followers, parameter smoothers — decays asymptotically toward zero and lingers in subnormal float range on the way there, and unflushed subnormal arithmetic can be an order of magnitude slower on x86. That reads as constant, unattributable load rather than one hot function, which is exactly what made it hard to pin down. Fixed at the source: set MXCSR's FTZ/DAZ bits once per realtime callback (MXCSR is per-thread, so this can't happen once at construction), with snap-to-zero epsilons in `Smoothed` and `EnvelopeFollower` as defense in depth so correctness doesn't depend on the hardware flag alone (`0921c85`).

## Aug 24 — A plans workflow, and paying down what it found

Also today: `docs/ARCHITECTURE.md` plus a mermaid diagram map the application's crates and control flow at a glance, separate from the detailed audio-core contract (`8fb8bd3`). And a new habit — dated, numbered plan documents under `docs/plans/<topic>/NN-step.md`, moved to `docs/plans/archive/` once done — started today with `reduce-ui-pump-overhead`, `amortize-reverb-partition-cost`, `auto-offline-idle-devices`, `extract-mid-level-dsp-blocks`, `reduce-audio-jack-buffer-size`, and `share-dsp-primitives` (`1818169`, `029d2b0`, `5cbfcfe`). Several landed the same day they were written:

- The UI pump was invalidating Slint state it hadn't actually changed: a meter redraw fired on every ballistics tick even when the lit-segment count was unchanged, and the document title got rewritten on every transport command instead of once when dirty state actually flipped. Both now compare against what's already displayed before writing (`735270e`), then the plan itself moved to `archive/` (`ab8709e`).
- `EffectChain`'s realtime pass looped over all `MAX_EFFECTS_PER_CHANNEL` slots per channel and per bus regardless of how many were actually populated. Added a `bound` — one past the highest occupied slot, maintained incrementally on install/remove/swap rather than rescanned every block — so the render cost is proportional to the chain that's actually there (`1fd6a3c`).
- JACK's buffer size had never been set explicitly, so the engine just inherited whatever quantum PipeWire's JACK shim happened to be running — 1024 frames / ~21ms on this workstation, which read as generic sluggishness with no attributable cause. Defaulted to 256 frames as a safe middle ground; 128 becomes reasonable once the render-cost plans above land fully (`f33bd28`).

Then the parameter-smoothing pass `share-dsp-primitives` opens with: `Smoothed` existed and only `MonoSynth`/`PolySynth` used it, so every continuous effect parameter stepped the signal at each block boundary instead of ramping — zipper noise on a filter sweep, a click on a drive/delay/dynamics gain change. Wrapped the audible continuous parameters per effect (filter cutoff/resonance, drive's four controls, delay's feedback and damping coefficient, modulation's depth/feedback/spread/tone/color, the compressor and limiter's gain-shaping controls), left the genuinely discrete ones alone (bitcrush's bit depth and downsample rate — the steppiness *is* the effect), and added a discontinuity regression test per changed effect so a future effect can't skip smoothing silently. EQ and reverb stayed deliberately unchanged, with the reason recorded in-file rather than left implicit (`41bf343`).

That plan's second step swept the codebase for filters that were the same primitive under different names — confirmed by reading each site, not assumed. `OnePoleLp` (matching the existing `OnePoleHp`'s shape), `AllPass` promoted out of modulation's private copy, `Biquad` plus its RBJ-cookbook designers promoted out of EQ, and a shared `scale::hz_from_normalized` for the `20 * (max/20)^x` log-frequency mapping duplicated across four DSP files — landed unused first so a bisect could separate "the primitive is wrong" from "the adoption is wrong" (`6f5a4e4`), then adopted one call site at a time: `OnePoleLp` gained a precomputed-coefficient constructor for delay's damping, which smooths a coefficient directly rather than a cutoff-Hz control (`4e4f541`), all three one-pole tone/damping filters switched over bit-identical (`6e8af0b`), the four log-frequency call sites collapsed onto the shared helper (`11241e1`), and modulation's phaser/chorus swapped its hand-rolled all-pass and LFO for the shared ones — the LFO swap needed a new `Lfo::peek_offset` to read a second tap of the same cycle at a runtime-variable offset without disturbing the accumulator, since stereo spread is smoothed continuously now rather than fixed at construction (`4fd91e5`, `56e9ada`). Adopting the smoothed cutoff moved a test's assumptions with it: a filter-bypass test measured energy in the same block a cutoff change was queued in, which only worked while cutoff snapped instantly — now it renders one block to let the new 3ms ramp settle before measuring (`ce32d17`).

The plan's third step was explicitly to *attempt* collapsing the sampler's own inline SVF and ADSR onto the shared `crate::filter::Svf` and `crate::env::Adsr`, then measure and decide rather than trust the existing comments. Both stay separate, for sharper reasons than before: the shared `Svf` recomputes its coefficients (including a `tan()`) every call, while the sampler computes once per stereo frame and ticks both channels — calling it per-channel would double that cost, and a synthetic benchmark confirmed roughly 2x, consistent across runs. The shared `Adsr` preserves envelope level on retrigger by design (avoids a click when a synth voice retriggers over its own release tail); the sampler's `trigger()` always hard-resets on a voice-steal, because restarting a sample from its start is the intended behavior for a stolen voice, not a legato retrigger — adopting `Adsr` as-is would have silently changed that. No code changed, only the header comments, which now state the measured/reasoned cause instead of just the fact (`5d82121`).

Last two of the day: both the EQ band handle and the reverb capture-point dot re-centered under the pointer mid-drag because their position was bound to the value it controlled, turning the drag into a sign-flipping recurrence — the same pitfall already worked around on the mixer fader. Switched both to the same delta-from-press-point pattern (`f0da9d5`). And keyboard shortcuts became reassignable: a named `ActionSpec`/`KeyChord`/`ShortcutTable` registry that transport, file, edit, view, channel, and pattern operations all dispatch through, so a future console or MCP server has one seam to hang off instead of each growing bespoke wiring; `main.slint`'s key handling collapsed from a hardcoded chain to generic decode-and-dispatch, and a Preferences > Shortcuts page does (re)binding with conflict handling. Manual testing caught a real bug along the way: a bare Ctrl keypress reports the same key code as Ctrl+Q in Slint's key model, so every Ctrl-chorded shortcut was misfiring quit until standalone modifier presses were explicitly rejected (`faa66eb`).

## Aug 31 – Sep 1 — The modulation rack becomes a grid

The modulation shelf existed but only had two source kinds and no way to grow.
Adam pulled the rest in explicitly: the rack becomes the power plant of the
app, a grid of small modules each a discrete control-signal device. Four
commits, in the order the plan (`docs/plans/modulator-modules/`) asked for.

First the foundation refactor (`c6f32eb`): modulators adopt the descriptor/
`get`/`set` paradigm effects already had, so the UI glue collapses from
per-field plumbing to one `param-changed` verb — net −351 lines — param edits
become undoable, and sources become deletable. Then three new kinds on top of
it (`2c57d66`): step, random, and math, each a descriptor table plus a tick,
which is the whole point of having done the refactor first. Two things
resolved themselves in the doing. The slot-order rule needed no machinery:
`outputs` already holds last tick's value everywhere the evaluation pass has
not reached, so a math module reading a lower slot sees this tick and one
reading itself or a higher slot sees the previous — self-reference bounded by
the module's own output clamp rather than by a cycle check. And Random kept
the LFO's tempo-syncable free rate rather than the division-only clock the
plan described, because it is a promotion of that LFO's hidden sample-and-hold
and dropping the free rate would regress anything migrating off the waveform.
One bug the new kind exposed immediately: every Random module on a channel
drew the same numbers (`25a8ac4`).

Then the grid itself (`f90d36c`, `626264f`, `e749cf9`): capacity from four to
eight, and durable `ModSourceId`s on routes. The reorder hazard turned out not
to be the routes — those resolve by identity — but the math module's
`input_slot`, a slot reference the user never sees; `move_module` remaps it
through the permutation.

## Sep 1 — Capacity stops being a memory argument

`docs/plans/modulator-capacity/` set out to make raising the module count a
constant edit rather than a hunt through layout code. Its first version
measured only the modulator arrays, concluded control outputs were the biggest
line, and drew the wrong conclusion. Measuring the whole render graph
(`ffbaa98`) said otherwise: 42.8 MiB before a project existed, of which
modulation was one percent and `EffectChain` was 140 KiB per channel —
because `MAX_CHANNELS` and `MAX_EFFECTS_PER_CHANNEL` are both the `u8` index
space, so the graph reserved the *product*: 65,536 effect slots, each with a
320-byte pending queue, for a project that will populate a few dozen.

So the lesson inverted. Capacity is not expensive; **dimensioning by ceilings
is**, and it is expensive whether or not the number ever moves. Boxing the
effect slot state took the graph to 11.6 MiB (`7a131d7`); materializing
channels from the project instead of reserving 256 of everything took a
sixteen-channel project to 1.1 MiB (`2815585`); addressing rack edits one fact
at a time took `EngineCommand` from 936 to 136 bytes, so the command ring
stopped growing with capacity at all (`5554cce`). Both ceilings are untouched
throughout — a project may still have 256 channels of 256 effects, it just
does not pay for them in advance. The arithmetic is pinned in a test rather
than left in a commit message, because the number was invisible at every
individual definition and only appeared when they were multiplied.

`EngineCommand::AddChannel` did not survive: adding a channel allocates, so it
is structural by nature, and leaving it POD on the realtime ring would have
meant it silently did nothing in exactly the case it was needed.

## Aug 30 – Sep 2 — The sampler learns to stretch, then to slice

A spike first (`c1626f4` → `ae05137`), which is the point: WSOLA and a phase
vocoder implemented against the same `Stretcher` trait, shaped like the
sampler's real situation — an immutable resident sample, a region, an optional
loop, a pull-style render into whatever block the executor asked for — and
scored on synthetic fixtures generated from a seeded PRNG so runs are
byte-reproducible and no audio enters the repository. `spikes/time-stretch/RESULTS.md` holds
the numbers; WSOLA won, and the trait shape was itself part of the finding.

Then the unit the spike chose (`e530062`), a grain mode because the artifact is
the point (`446a8b0`), composition with transposition (`03d237c`), state the
sampler does not pay for when stretch is off (`549ef0b`), and persistence with
repair (`7789c61`). Tempo-fitted loops freed pitch from duration (`6bfb447`),
and a sounding voice retunes live rather than on its next trigger (`8b9d361`).

Slicing followed on the same buffer machinery (`f41aea7`, `a42e9aa`): a marker
list normalised on load rather than refused, a slice per note, backwards if
asked. The interesting piece is the commit (`46a759f`, `15345fe`): a committed
stretch stores its *spec*, not its audio — mode, resolved ratio and grain, the
region fractions, and the markers the editor held — so loading decodes the
source as usual and re-renders. That is what keeps a project text-sized, and
it is also the known hole: nothing checks the source file, so a sample
replaced on disk re-renders new audio under the old spec.

## Sep 2 — ML-P8, and a mockup tool with one catalog

The ML-P8 is a new device beside the retained v1 poly, not a migration of it.
Its parameter ids are their own namespace starting at zero, and its descriptor
table lives in `mlp8.rs` rather than `generator.rs`, because the shared
`SYNTH_PARAM_*` ids exist for Mono and Poly being the same voice with a
different count and this device is not that voice (`da4a658`). The oscillator
network, sub and noise landed first (`f2de216`), then the two envelopes, the
multimode filter, and the per-voice feedback loop (`060f684`).

Three things the plan could not have known, all from step 02 and 03:

- **The sync BLEP made aliasing worse until two mistakes were fixed.** The
  step height has to be measured on the *naive* waveform, and the oscillator's
  own cycle-boundary residual has to stand down for the sample after a reset.
  Neither is visible without building it, and neither shows up in a test that
  looks for energy in a high band — a hard-synced oscillator is exactly
  periodic at its master's rate, so every alias product folds back onto the
  master's own harmonic grid. The test compares harmonic magnitudes against an
  eight-times-oversampled render instead.
- **Clearing the feedback loop on `restart()` was not enough.** That only runs
  for a fresh slot, and stealing a sounding voice deliberately keeps its
  oscillator phases; it was keeping the loop with them.
- **"Skip an oscillator nothing reads" needed a caveat.** The skip is decided
  once per block from target levels, but levels are smoothed — so a level knob
  reaching zero un-needs an oscillator while its smoother is still
  milliseconds from silence, and skipping it there replaces the ramp with a
  step.

The face draws the voice as what it is: a source-by-destination grid, rows are
sources, columns the oscillators they reach, the diagonal an oscillator on
itself (`0b57044`). A mix level went into the same grid rather than beside it,
because a level is a route to the output (`249ff65`). The cells are
`ParameterKnob` with `show-dial: false` rather than a second draggable
control — arming a modulation source changes what every gesture *means*, and a
hand-rolled cell would have been a second implementation of that contract.

Beside that, the mockup tool collapsed onto one catalog (`79b57a0`,
`179d183`), which made an audit possible: exported widgets with no catalog row
show up in an UNCATALOGUED group, so the standing list of what the tool cannot
compose with maintains itself. Three fixtures that existed only to render
controls — the control gallery, the widget sheet, the rack row — were retired
in favour of placing a control in the tool (`60e4d4d`, `bb7db8b`, `9401988`).
The converse list, UI patterns that recur with no component behind them at
all, became `docs/WIDGET_INVENTORY.md` (`4f4957d`).

And the build loop got faster where it hurt most: `scripts/slint-sketch`
type-checks a scratch `.slint` against the real widgets in ~0.05s and
screenshots it in ~0.2s, where `cargo build -p mooloop-ui` is about four
minutes for any edit at all, because rustc recompiles the whole generated
module either way (`b87d51b`). `slint-viewer` is deliberately not a workspace
dependency; the build never refers to it.

## Sep 2 — Addresses follow their device

The last one is the kind of bug that is obvious in hindsight and invisible
until something moves. A modulation route and an automation lane name their
destination by effect slot, and a channel by its index. Reordering, inserting
or removing an effect left every route and lane pointing at the old number, so
the LFO drawn on a filter's cutoff started driving whatever slid into that
slot and the filter went dry. Deleting a channel from the middle did the same
one level up.

`mooloop_core::structure` states each structural edit once as a permutation —
`SlotRemap` for a chain, `ChannelEdit` for the channel list — and runs it over
everything that stores a position: the matrix, every lane in every pattern,
and the lane the editor is showing. The UI model and the engine apply the same
table for the same command, so neither can point at a different device than
the other. `SwapEffectSlots` became `MoveEffect`, a pointer rotation the
engine follows with the same retarget, and a removed device takes its routes
and lanes with it. Effect add, move and remove became undoable; they were the
only rack edits that were not (`0c60828`). The same hole existed one level
down inside the rack — a route aimed at a modulator's own parameter — and
closed the same way (`c1765e9`). The integrity pass repairs what earlier
versions left in saved songs.

---

## Patterns worth noticing

**Hardcoded constants drift; derived ones don't.** The 758px viewport, the 220px pattern strip with 190px of hole, the fixed 5px note edge zone that ate a minimum-width note, the forwarded-command threshold of 29 that had overcounted the baseline, the piano roll's C2–C6 range hardcoded as a bare `49` in half a dozen places. Every one was correct on the day it was written; a stale range check in the save validator (checking volume against `0.0..=1.0` after the trim ceiling moved to +12dB) is the same failure one layer over, in validation instead of layout.

**Slint input routing is where the bugs live.** Focus scopes eat the first press. `TouchArea.pressed` only tracks the left button, so right-drag-to-clear cleared exactly one step. Per-element hit areas can't survive a drag that moves the element. Three different symptoms, one lesson: test by dispatching real pointer events, because invoking callbacks directly passes even when routing is completely broken.

**Two silence bugs, no shared cause.** Missing JACK links; swallowed decode errors. The diagnostics harness built to chase the second one is still earning its keep — autodrive now covers the whole effect command surface.

**Write the primitive once when the second user is already visible.** `delayline` for the delay and the buffer device. `dynamics` for three effects. `EffectChain` and `OutputStage` for channels and buses. `KnobFace` for two knob sizes. Descriptor tables instead of a callback per knob. `OnePoleLp`, `AllPass`, `Biquad`, and `scale::hz_from_normalized` for the four filter/frequency-mapping duplicates found once someone actually went looking (`6f5a4e4` → `11241e1`).

**Constraints get relaxed as soon as they're cheap.** The lower-numbered bus rule lasted about a day, and removing it took a topological sort over fixed-size arrays. Worth asking of the remaining rules which ones are still load-bearing.

**A ceiling costs nothing; dimensioning by it costs everything.** Nothing about `MAX_CHANNELS` or `MAX_EFFECTS_PER_CHANNEL` being 256 was wrong. Reserving their *product* before a project existed was, and it cost 42.8 MiB that no individual definition made visible — the number only appeared when they were multiplied. The fix left both ceilings exactly where they were.

**Names are the thing that has to be reachable, not values.** `Osc.phase` was private with no reset and no wrap event, which is why `COMPOSABLE_DEVICE_UNITS.md` used it as its live counter-example. Hard sync for the ML-P8 needed all three, and adding them was mechanical *because the value already existed as a field*. The habit is cheap precisely when it looks unnecessary.

**A position is not an identity.** Routes and lanes named a device by its slot and a channel by its index, and every structural edit silently repointed them. Stating each edit once as a permutation and running it over everything that stores a position is the fix; so is minting durable ids for modules so a grid reorder means nothing at all.

**Deferrals are recorded with reasons.** Limiter lookahead waits on plugin-delay compensation. `ParamAddr`, inter-channel data and audio sidechain are named as deliberate absences. The README states what the mixer deliberately is *not* yet.

**A comment that states a fact can outlive the reason for it.** The sampler's inline SVF and ADSR carried comments justifying why they weren't the shared primitives; asked to actually attempt the merge and measure rather than trust the comments, both conclusions held but for different, sharper reasons than what was written (`5d82121`). Worth periodically re-deriving *why*, not just re-reading it.

**Unattributable CPU load has more than one hiding place.** The gradual jankiness Adam kept noticing turned out to be denormal floats in recursive DSP state going unflushed (`0921c85`) — a different failure shape than the JACK buffer size silently inheriting the server's quantum (`f33bd28`) or `EffectChain` scanning empty slots (`1fd6a3c`), but all three read identically from the outside: no hot function, just slower than it should be.

## Open threads

As of 2026-09-04. Four of the previous six closed; what replaced them sits
one level further out.

- **The Buffer's product question.** The device is built and is an ordinary
  insert. Whether routing a source into it and sequencing the result beats
  bouncing to a sample is untested, and that is what `docs/FOCUS.md` step 3
  and `VERSIONS.md`'s 0.3.0 are for.
- **MIDI input is wired to nothing.** A JACK port, a decoder, and a
  `BufferMidiMap` the render state will apply — with no caller installing one,
  and no controls on the MIDI preferences page.
- **Mixer: inserts only.** No sends, sidechain, audible solo, stem export, or
  bus renaming. `MIXER_PLAN.md` is the design and needs a project format
  version to land.
- **No plugin-delay compensation**, so no lookahead anywhere, and no parallel
  paths that can be trusted to stay aligned.
- **Realtime hygiene is reasoned, not measured.** No allocation-detector
  harness around the callback; the buffer spike's last acceptance test is open
  on exactly that.
- **The tooltip audit.** The status bar exists and about forty sites feed it;
  deciding per control which half of the rule it falls under has not happened,
  and the sampler face is not plumbed in at all.
- **The v1 mono synth cannot be deleted** until its channels have somewhere to
  land, which is the poly mono/legato toggle in
  `docs/plans/poly-v1-mono-mode/`. Until then the picker shows six sources
  where the plan wants five.

Closed since the last audit: modulation drives real destinations through five
module kinds; the buffer device exists; the reverb is an FDN and the IR player
is gone (a convolution device may return as its own thing, not as a mode);
undo and the clipboard are real, behind one action registry.
