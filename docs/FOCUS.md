# Focus

Status: active working sequence, rewritten 2026-09-05.

`ROADMAP.md` orders the whole product by dependency. This document is narrower:
it names the active sequence and the work that should not interrupt it. Rewrite
it when that sequence is exhausted; do not let it become a second roadmap.

Read `PRODUCT.md` for the product argument and `CURRENT.md` for the implemented
surface. Source and tests settle any disagreement with either document.

## The line has moved

The previous sequence was ML-P8, then DS-01, then Buffer. **Its first two steps
are done but for one thing, and it is the same thing in both.**

- **ML-P8 plays.** Steps 02 through 05 are in: the three-oscillator network,
  all six directed XMOD routes, sync, the derived sub, coloured noise, both
  envelopes, four filter modes, the feedback loop with drive inside it, its own
  audio-rate LFO and six per-voice sources onto thirty-one destinations, group
  allocation, Unison, Detune, Spread, Drift and a chorus that is off by
  default. Its face spends five pages so the controls can be read.
- **DS-01 plays, and was signed off.** Adam played the device and its
  seventeen-patch bank on 2026-09-04 and closed step 09. One universal
  percussion voice reaches a kit from range and patches rather than mode
  branches, every parameter is descriptor-addressed, and the six-page face
  holds all ninety-two of them at a readable size.
- **What both are waiting on is one mechanism.** ML-P8 step 06 and DS-01 step
  07 both publish device outlets. The control half is built — `mooloop_core::outlet`
  is the vocabulary, ML-P8 declares fourteen outlets and publishes its seven
  control values, and a route can name one. The audio half needs the typed
  auxiliary audio edges `AUDIO_ARCHITECTURE.md` describes and that do not
  exist. Two plan directories are held out of `archive/` by that one gap.

So the sequence below starts by clearing the debt those two instruments left,
and only then moves to the interface, which is where Adam's standing list has
accumulated.

## The rule

**Prefer changes that produce a musical decision over changes that merely add
capacity.**

The engine already has more breadth than the instrument has identity. A new
source type, graph abstraction, effect, or routing primitive is not progress by
itself. Each step below must end in something that can be played, heard, saved,
reopened, and rendered through the ordinary UI.

Step 3 is interface work, and it gets a sibling rule: **an interface change is
judged by whether something that already exists becomes easier to reach, not by
how much new surface it adds.** A new pane that shows what a menu already
showed is not progress either.

## The sequence

### 1. Publish device outlets, and close two plans

Finish `docs/plans/poly-synth-v2/` step 06 and `docs/plans/drum-synth-v2/`
step 07. They are the same work seen from two devices, which is why doing it
once is the point.

Two pieces, in order:

- **The control slice's last mile.** The mechanism is built and tested; what
  is missing is the *source picker offering outlets*, so a route onto one can
  be made by hand rather than only by hand-editing a project file. This is
  small and it is the difference between a feature and a file format.
- **Typed auxiliary audio edges.** ML-P8's `Osc 1/2/3`, `Sub`, `Noise`,
  `Pre-Filter Mix` and `Filter` taps are audio-rate ports. The one-block
  control table cannot carry them and downsampling them destroys the reason
  they exist. `AUDIO_ARCHITECTURE.md` describes the edge type they need;
  nothing implements it. This is the one genuine architecture gap left behind
  by the instrument push, and it is also what parallel sends and sidechain
  will want later — build it as an edge type, not as an ML-P8 feature.

This is first because two finished instruments are sitting in `plans/` unable
to be archived on account of it, because the contract gets designed once
instead of twice, and because it is the only item on this list that is
architecture rather than surface.

`drum-synth-v2/` archives the moment its step 07 closes. `poly-synth-v2/` has
one more thing after this: its step 07 is ML-P8's factory bank and listening
pass. ML-P8 has no bank and no recorded listening pass — nothing added since
the ML-M1 bank on 2026-08-31 has been listened to except DS-01. Do that pass at
the end of this step rather than deferring it again; it is the only thing
between ML-P8 and being finished.

Done when: a route can name a device outlet from the picker; audio outlets obey
their declared rate and latency contracts; ML-P8's bank has been played;
`poly-synth-v2/` and `drum-synth-v2/` both move to `docs/plans/archive/`.

### 2. Give the v1 drum synth automation support

Adam's ask, 2026-09-05, in his words: *"og drumsynth was simple but honestly
sounded pretty good. why has simply updating it for automation support never
been on the table?"*

It has not been on the table because of a recorded argument, and **that
argument does not survive being checked.** The doc comment on `DeviceKind::descriptors` said
`DrumSynthParams` is a mode-union whose flat descriptor table would hand out
ids whose meaning changes with the Mode switch. It is not a union. It is a flat
struct of twenty-one named fields (`synth.rs:70`), every one of which means
exactly one thing forever: `kick_start_hz` is the kick sweep start whatever the
Mode switch says, and `drumsynth.rs:7` states outright that the other modes'
knobs are *retained* so switching modes never loses settings. Mode selects
which fields are audible, and it is latched per voice at note-on
(`drumsynth.rs:216`).

That leaves a real but much smaller objection: a route onto `kick_start_hz`
does nothing while the device is in Snare mode. That is an audibility gate, not
an addressing bug, and it is the same situation as a route onto a bypassed
effect — which the application already permits everywhere.

So this is close to what the old note called it before the correction:
sixteen continuous fields, one descriptor table, the same shape every other
generator already has. `render_sample` already takes `params` by value per
sample, so there is no structural obstacle on the DSP side either.

Do not scope-creep it into DS-01. The v1 device stays what it is — three modes,
simple, and by Adam's ear good — and gets addressable parameters, nothing else.
Do not delete it: it is the only source that cannot be modulated today, and
after this step that sentence stops being true, which also removes the last
standing reason to hurry it out of the tree.

Done when: `DrumSynthParams`' continuous fields are descriptor-addressed with
stable ids, a modulation route and an automation lane both reach them, old
projects load unchanged, and the inert-while-out-of-mode case is documented
rather than special-cased.

### 3. The 1.0 interface shell

One push, mockup-driven. `reference/img/mooloop-1.0-mockup.png` is the target
Adam drew; treat it as the argument for the layout, not as a pixel spec. Four
things, and the first one gates the other three:

- **Keyboard and focus.** *"keyboard is still wonky. you often have to click
  into a background area to make shortcuts work, even spacebar."* The window
  has exactly one `FocusScope` (`main.slint:1253`) reached by
  `forward-focus`, so anything inside that takes focus — a text field, a
  focusable touch area — swallows the key before the dispatcher sees it, and
  the fix is getting focus back that the caret should not have kept. Spacebar
  is play/stop. **This blocks playing, which makes it the one item here that
  may also interrupt the sequence** (see below).
- **A left channel sidebar.** Channel name, track colour, input channel, and
  the rest of the per-channel settings that today are scattered across the
  rack row and nowhere. Note that **track colour does not exist at all** — no
  field, no persistence — so this crosses `PROJECT_FORMAT.md`, and the
  defaulted-field rule there applies.
- **Move and redesign the modulation rack.** The shelf is 1,539 lines under
  the device rack; the mockup puts modulation in a right-hand panel with its
  own tabs. Read `MODULATOR_SYSTEM_SPEC.md` before moving it: the assign
  gesture and the destination policy are contracts, and a relocation must
  keep them. The mockup also draws its modulator as a *tracker*, which is the
  same idea `IDEAS.md` has been holding since before this list — decide
  whether that is one design or two before building either.
- **The browser earns its panel.** Keyboard navigation in the sample browser,
  and preset browsing beside samples. The preset half is not new work waiting
  on a decision: `docs/plans/preset-system/` already decided a preset's unit
  is a device, both instrument banks now ship, and the browser was explicitly
  the thing waiting for them.

Done when: every shortcut in the registry fires from anywhere it sensibly
should, a channel's name and colour are set and saved from the sidebar,
modulation is reachable from its new home without losing an existing gesture,
and the browser can be driven and can load a preset without the mouse.

### 4. Turn Buffer into a composition workflow

Unchanged, and still the honest product test.

The retained-audio engine, insert position, collision policy, gestures,
automation addresses, and UI face already exist. The remaining question is not
more Buffer DSP; it is whether a musician can deliberately route a source into
Buffer, sequence a transformation, understand the active head, and keep the
result as part of a project without relying on debug controls or a hidden MIDI
mapping.

It sits last because its value is a workflow judgement, and that judgement is
better made against finished instruments and an interface that can be driven
than against neither.

Done when: a project can generate or load sound, capture it continuously at a
chosen insert point, sequence an audible jump/reverse/repeat transformation,
show what the read head is doing, survive save and reload, and render the same
result offline. If that workflow is not materially better than bouncing a
sample and loading it again, record why before expanding the device.

## Fixes that may interrupt the sequence

Take a fix immediately when it blocks hearing, playing, saving, loading, or
rendering the active step; threatens realtime safety or project compatibility;
or is a small regression in the surface being touched. Record larger adjacent
work instead of folding it into the current branch.

- **The focus caret eating shortcuts qualifies on its own terms.** Step 3 owns
  the proper fix, but spacebar not starting the transport blocks *playing*,
  which is the first clause of the rule above. If a step-1 or step-2 branch
  trips over it, fix it there and note where.
- **The stretching-polyphony cap is not enforced anywhere.** `StretchPool::new`
  builds a reader for every one of the sampler's sixteen voices and nothing
  limits how many stretch at once, although the contract in #13 names four.
- **`poly-v1-mono-mode` is one step and unblocks a deletion.** It is the only
  thing keeping `DeviceKind::MonoSynth` alive, and that deletion is what lets
  `MlM1` take the plain name. The held-note stack it needs already exists.

## Deliberately not now

**More effect kinds or a broad effect-polish pass.** The 2026 effects-feedback
pass is complete and archived. The rack has twelve effects and a common host. A
step may fix a concrete defect it exposes; the suite does not need more
breadth.

**More modulator kinds, or raising the slot count.** Five kinds, eight slots,
durable identity, and a measured price per slot are enough. Raising
`MAX_MODULATORS_PER_CHANNEL` is now a one-line decision — which is the point of
the capacity work, not an invitation to make it. Note that step 3 *moves and
redesigns* the modulation rack; that is a relocation and a layout, not a
licence to add kinds while it is open.

**The text-label-to-icon pass, and colour scheme support.** Both are on Adam's
list as of 2026-09-05 and both are wanted. Both are also polish over a shell
that step 3 is about to move, so doing either first means doing it twice. The
colour work in particular has somewhere real to land — Appearance already
derives the whole palette from three seeds plus roundness and contrast — and
that is exactly why it can wait for the panes to stop moving. `ENHANCEMENTS.md`
holds both in Adam's words, including the pywal/wallust half.

**Parallel sends, sidechains, and plugin delay compensation.** Compensation is
required before parallel paths are trustworthy. Step 1's typed audio edges are
the shared prerequisite; build the rest when sends or sidechain become the
actual product task.

**Broad arrangement and recovery work.** Playlist clip manipulation, explicit
loop ranges, autosave, crash recovery, and richer missing-sample relinking
remain important. They do not interrupt this sequence unless one becomes
necessary to preserve its work.

**A curated factory bank.** Every device that ships presets ships them to prove
its architecture reaches its range from the controls, and that is the only bar
they are held to — Adam's ruling closing DS-01's step 09 was that *"they don't
have to be flawless presets. for now we just need something there, mostly to
prove the system works."* Authoring content by taste, across every device, is a
deliberate later push. Do not fold it into a device step, and do not hold a
device step open waiting for it.

**Metronome, plugin hosting, MIDI configuration, and the graph editor.** None is
required to prove the active workflows. Note that step 3's channel sidebar
draws a MIDI input row: build the *setting*, and let it stay inert, rather than
letting the sidebar pull MIDI configuration forward.

## Working discipline

Keep one audible acceptance case per branch and run it through realtime,
persistence, and offline rendering where the change crosses those boundaries.
The plan files define the order inside a plan; update their status as steps
land rather than duplicating implementation notes here.

Keep branches small enough to listen to and revert independently. Preserve
stable parameter IDs, conservative project defaults, deterministic rendering,
and the realtime rules in `AUDIO_ARCHITECTURE.md`. `AGENTS.md` governs
worktrees, commits, and verification.

**Listening is a step, not a formality.** The last recorded listening pass was
DS-01 and its bank on 2026-09-04; ML-P8 has never had one, and step 1 ends
with it. Step 2 changes nothing about how the v1 drum synth sounds
and does not need a pass of its own; step 3 changes what can be *done* to a
sound rather than how it sounds, and a moving patch is the only proof it
worked.
