# DS-01 plan status

**Steps 02 through 07 are in except step 07's outlets, and step 08 is
mostly in.** The device exists and plays: a new `Ds01` generator kind
beside the v1 drum synth, the tone layer with its wave morph, partial bank and
FM, the noise layer with four colours, a rate reducer and a morphing
state-variable filter, the amplitude and pitch envelopes, and the voice pool
with choke and a Mono retrigger mode. Every one of its parameters is
descriptor-addressed from the first commit, which is the reason the instrument
exists.

Step 03 replaced its two `ExpDecay`s with four real envelopes: one AHD type
with a curve and an optional gate, used for the amplitude, the noise layer and
the mod contour, and once more without its gate half for the pitch. Curve is
the control that earns the step — logarithmic at -1, v1's exponential law at 0,
linear at +1 — and gate mode is what makes a ride that rings for as long as it
is written, sounds v1 cannot make at all.

Step 04 added the body: three tuned resonators struck by the impulse, by the
noise layer, or by a crossfade of the two. This is the layer v1 has no
equivalent of, and Ratio is the whole design in one control — harmonic at 0, so
the layer reads as a tom or a conga, and the ideal circular membrane's modes at
1, where it stops having a pitch and starts having a material.

Step 05 added the burst: one trigger, up to eight impulses, one voice. This is
how DS-01 makes a clap — three or four bursts a few milliseconds apart followed
by a longer one, which no amount of envelope shaping reaches from a single hit
— and once the mechanism exists it is also a flam, a drag, a buzz roll and a
machine-gun fill.

Step 06 added the shape stage and settled the gain contract: Drive into one of
four characters, a bias, a bit reducer, and an output high-pass, with the
device reference documented as a contract rather than left as a constant doing
three jobs.

Step 07 added DS-01's own matrix: eight rows of source, destination, signed
amount and curve, with eight per-voice sources. This is the modulation the
channel rack *cannot* provide — a channel source produces one number per
control tick for the whole channel, and a drum channel can have eight hits
ringing at once, each with its own velocity, its own position in a burst and
its own envelopes.

**Step 07's published outlets are not built, and are blocked rather than
skipped.** See below.

Step 08 built the face: the adopted four-column layout, wired and live in the
rack. Every one of the device's sixty non-matrix parameters is reachable on one
screen — forty-eight as knobs and chips, twelve as handles on the scopes — and
the scopes' span follows the patch, so a 5 ms hat and a 4 s ride both read.

Read `01-what-ds01-is.md`, then finish `08` and work `09`.

Four things step 02 turned up that the plan could not have known:

- **The event-ordering rule already held, and held on purpose.**
  `EventList::push_ordered` has sorted note-offs, then parameter changes, then
  note-ons at equal offsets since retriggering needed it. What was missing was
  the *reason a latching generator depends on it*, which is now written there
  beside a test, because a later reordering would break drum modulation
  silently and no synth test would notice.
- **`soft_ceiling`'s numbers are ML-P8's, not a universal bound.** Its knee and
  asymptote sit at 1.5 and 2.5, calibrated against a voice nominal near 0.7
  with a channel fader still downstream. DS-01 needs a bound that holds *at the
  device output* for every control combination, so it states its own in output
  units and keeps `soft_ceiling` for its actual job, which is catching a
  resonant state-variable filter before any level scales it.
- **Step 02's `SetChannelDs01Params` bridge command was not added, deliberately.**
  Since the plan was written, `SetChannelGeneratorParam` landed for exactly
  this: every entry in the engine's fixed command ring is as wide as its widest
  variant, so shipping a whole parameter struct to move one knob makes every
  unrelated command pay for the largest device. `Ds01Params` will be the
  largest by step 07. Per-knob edits go through the narrow command and
  whole-struct installs through `load_source` on the control thread, which is
  the arrangement ML-P8 already uses.
- **Three parameters are in neither of `01`'s two tables.** Partials, Noise
  Colour and Retrigger are all structural discretes, which is how they escaped
  a pair of tables about what a *sounding* hit follows. `01` says that is a bug
  in `01` rather than a free choice at the call site, so it is recorded here:
  Partials is latched, because a hit does not grow an oscillator halfway
  through; Colour and Retrigger are read where they are used, which step 02's
  own "fine between hits, undefined mid-hit" makes conformant either way.
- **Tune is stepped, and therefore not a modulation destination.** The plan
  marks it "step 1" and does not mark it structural, which reads as a
  contradiction under the rule that eligibility comes from the descriptor
  curve. It is not one: Tune is *which note this drum is*, latched with the
  note itself, and the continuous pitch controls a route wants are Tone Pitch
  and Pitch Depth. The curve gives the right answer for the right reason.

And five from step 03:

- **Attack and Hold are linear, not log.** `ParamCurve::Exponential` is a
  ratio sweep whose bottom is `min`, so it cannot include zero, and zero is the
  property the step calls non-negotiable — "a drum synth whose attack cannot be
  zero is broken". Of the two, zero wins; the taper is left to the control
  surface, which is where a taper belongs. Adding a fourth curve variant would
  change how every device in the program normalizes, for one control.
- **Amp Decay's range moved from step 02's 4 s to the envelope type's 8 s.**
  Its id is unchanged. Step 03 states one range for the decay of one envelope
  type, and having the amplitude one be the odd 4 s would mean the three gated
  blocks were not literally the same block. Nothing had saved a project.
- **The envelope lives in `env.rs`, not in `ds01.rs`.** It is an envelope, and
  that file is where the shared ones are. `AhdShape` is its own small type
  rather than `Ds01EnvParams` so the primitive does not know about the device.
- **The flat top is Hold plus one sample.** The decay's own first sample is the
  peak, which is the same property that makes a zero attack cost the transient
  nothing. Worth stating because it is the kind of off-by-one a later reader
  would otherwise "fix".
- **The pitch envelope has no gate half at all** — no hold, sustain, release or
  gate ids, only 50-53. With the gate off they would be four controls that do
  nothing, which is exactly what `01-what-ds01-is.md` forbids, and a pitch
  envelope that held its excursion for the length of a note is a transposition
  rather than a sweep.

And five from step 04:

- **A two-pole resonator, not the shared biquad in a band-pass.** The step asks
  for the latter with a Q derived from decay and frequency; the property it
  wants from that is "decay is a time in seconds at every pitch", and a
  resonator whose pole radius comes *straight* from the decay time has that
  property by construction rather than by a conversion. Two other things
  settled it: `biquad.rs` has no band-pass constructor, so the shared-biquad
  route meant adding one anyway; and the two-pole form is what makes the two
  excitation gains below derivable.
- **The two excitation gains are derived, not tuned.** The same resonator has
  to be struck and to be driven, and those want opposite scalings: an impulse
  response peaks at `1/sin(w)`, so a strike is scaled by `sin(w)`; a continuous
  excitation accumulates with an RMS gain of about
  `1/(sin(w) * sqrt(2(1 - r^2)))`, so it is scaled by the inverse of that.
  Without the second, an eight-second ring would be forty decibels louder than
  a short one for the same noise going in.
- **Damping is a function of a mode's ratio to the fundamental, not of its
  index.** That makes it literally high-frequency loss: a harmonic body at
  Ratio 0 damps less than a membrane at Ratio 1 at the same setting, because
  its modes are closer together. Damping the index instead would have made the
  control mean something different at each end of Ratio.
- **A trigger does not clear the resonators.** A hit strikes an object that may
  still be ringing, which is what makes a fast pattern on a bell build rather
  than restart — and it is the same property step 05's burst needs across the
  impulses of one hit.
- **The skip clears them.** When the layer is skipped its state is reset, so a
  level returning from zero starts from the next strike instead of resuming a
  ring that was frozen mid-decay.

And four from step 05:

- **Level Step scales the strike, not the output.** It is a strike force, so
  it scales what each impulse excites. Scaling the voice's output instead would
  step the body's ring every time a later impulse changed the level, which is a
  click in a tail that is supposed to carry across the burst — the one thing
  the step says a burst must do.
- **The level sequence is normalized so its loudest impulse is the reference
  one.** A falling step then fades from a full first hit and a rising one
  builds *to* a full last hit rather than past it, so Level Step shapes a burst
  without making it louder than the single hit it replaces. Without that, a
  positive step at eight repeats is a burst that ends 18 dB up.
- **The bound is on the schedule's total, not on one gap.** Eight individually
  legal gaps still add up: a decelerating eight-impulse burst at the top of
  Spacing would place its last hit a minute after its first. The burst ends at
  the impulse that would take the whole schedule past `DS01_BURST_MAX_S`.
- **The mod envelope is not re-fired by an impulse.** The other three are, from
  their current level so impulses add rather than cut each other off. The mod
  contour is the one with no fixed job, and a shape spanning the whole burst is
  more use than eight copies of a short one — which is also what makes it worth
  routing to Burst Index's neighbours in step 07.

And three from step 06, one of which changed `01`:

- **The shaper is after the amplitude envelope, not before it.** `01`'s signal
  path had it ahead; that diagram is now corrected there, as `01` asks. The
  reason is the property step 06 requires of Fold: folding is a function of
  instantaneous amplitude, so the shape of a hit changes across its own decay
  *for free* — but only if the decay has already happened by the time the
  signal reaches the folder. Ahead of the envelope, a tone-only patch presents
  a constant amplitude and every hit folds identically. The same choice is
  what lets velocity reach the colour, which is the taste brief's "reacts to
  level, timing and the source" rather than a fixed percentage of an effect.
- **Soft is `filter::apply_drive`, called rather than re-derived.** "Reproduces
  v1's drive curve" is then an identity a test can assert with `assert_eq!`
  rather than a tolerance, and an old-sounding patch is reachable exactly.
- **Crush rectifies asymmetrically, not fully.** A full-wave rectifier leaves a
  DC pedestal under silence, and the output high-pass removing it afterwards is
  not the same thing as it never being there. Keeping most of the negative half
  out gives the even harmonics and the partly-doubled fundamental without a
  voice whose idle output is -1.

`DS01_BITS_TRANSPARENT` is named rather than left as "the top of the range",
because the identity is load-bearing: the default patch has to reach the gain
reference through a shaper doing nothing at all, and `(x * 32768).round() /
32768` is not `x`.

And four from step 07:

- **A route's curve is neutral in the middle; an envelope's is not.** Reusing
  `env::shape` directly was a real bug, caught by the Burst Index test: that
  function's zero is v1's exponential decay law, which is the right answer for
  an envelope and the wrong one for a route, where the middle of a bipolar
  control has to mean "no shaping". A route at its default curve delivered
  almost nothing until its source was near the top, which reads as a dead
  route rather than a shaped one. `route_shape` keeps the same two ends and
  makes the middle the identity.
- **The control tick had to become a real interval.** It was the gap between
  events, which is enough while everything that moves comes from outside. The
  matrix moves things with nothing arriving — an envelope opening a filter,
  Burst Index walking a pitch across a roll — so a block with one note-on in
  it held the first tick's values for its whole length. `render_range` now
  walks `CONTROL_RATE_FRAMES` at a time and `process` splits on top of that.
- **No default route ships.** The step asks for Velocity to Amp at full
  amount, so the device feels normal unprogrammed — and `Velocity Amount` at
  id 5 already does exactly that, which the same paragraph says stays as the
  plain control for the common case. Shipping both applies velocity twice.
  Step 09's factory patches are the better place to demonstrate the mechanism,
  since they can do it without doubling a control that is already on.
- **A row cannot address the matrix's own band.** Source and Destination are
  stepped and would be refused by the descriptor rule anyway; Amount and Curve
  are not, and a row modulating another row's amount would make the result
  depend on the order the rows happen to be evaluated in. A *channel* route
  still reaches Amount — which is how an LFO scales a per-hit relationship
  without knowing anything about voices — because it is resolved before the
  block rather than inside it, so there is no order to depend on.

## What is blocked

**Step 07's published outlets are not built.** Both halves need infrastructure
that does not exist, and building either one for DS-01 alone would be the
special-case knowledge `COMPOSABLE_DEVICE_UNITS.md` exists to prevent:

- **Control outlets** (`Amp Envelope`, `Mod Envelope`, `Velocity`, `Note`,
  `Gate`, `Trigger`) need a device-outlet modulator kind and the per-channel
  published table `MODULATOR_SYSTEM_SPEC.md` describes. `ModulatorKind` has
  five kinds and none of them is one; that spec lists "Generator outlet" and
  "Device outlet" as *Planned*. ML-P8's step 06 is blocked on the same thing,
  which is the argument for building it once rather than twice.
- **Audio outlets** (`Tone`, `Noise`, `Body`, `Pre-Shape`) need the typed
  auxiliary audio edges `AUDIO_ARCHITECTURE.md` describes, which
  `COMPOSABLE_DEVICE_UNITS.md` explicitly says a device may not bypass.

Nothing about DS-01 blocks them, and DS-01 does not need them: the step's own
rule is that the device makes complete sounds with no channel routes at all,
and the matrix is what delivers that. `Trigger` is the one worth wanting
soonest — it is what lets a kick duck a bass or fire an envelope on another
device without a sidechain graph — and it arrives with the shared mechanism.

And three from step 08:

- **The face is indexed by parameter id, not one property per parameter.**
  Every other device face here declares a property each; DS-01 has ninety-two,
  and ninety-two properties would be a second copy of the parameter table
  maintained by hand — which is the thing this device exists not to have. It
  takes arrays indexed by descriptor id and reports edits as
  `(id, normalized)`, the shape the modulation depths already use, so one
  handler covers every control. Values cross normalized rather than natural,
  because that is the space a route and an automation lane both work in.
- **A source device declares its own width.** It was three rack units for
  every kind. DS-01 declares five, the way an effect slot declares its units:
  three is the width at which "one screen, no pages" stops being true, and the
  concept was drawn at a width the rack did not give a source. ML-P8 spent the
  same problem on a second page instead.
- **The columns are not equal width.** TONE, NOISE, BODY and AMP take 4, 5, 3
  and 4, because that is how many cells they have. Equal columns would have
  meant either a different knob size per column or leaving two of the noise
  layer's controls out, which is the failure the plan refuses.

## What step 08 still owes

The layout, the controls, the four scopes and their handles are in and
verified by a software-rendered snapshot at the default patch and at a
four-second one. Four things are not:

- **The MOD panel.** The three checked-in concepts settled the columns and the
  bands and none of them draws it, so the layout for the matrix's thirty-two
  controls is genuinely undecided rather than a detail to fill in. The band
  carries a labelled empty region where it goes. **This is the one thing in
  the plan that wants Adam before it wants code.**
- **The rendered hit in the AMP scope.** `Ds01::preview_waveform` exists and
  renders through the production voice path; what is missing is the debounced
  plumbing from a parameter edit to a redraw, which `08-the-face.md` is
  explicit must not run per keystroke on the UI thread.
- **Burst impulse ticks** on a short axis inside the BURST section.
- **Focus dimming** — the touched column's scope drawn solid and the others
  quiet.

The face is **not** part of steps 02 through 07. DS-01 is selectable, playable,
automatable and modulatable without one — every parameter is
descriptor-addressed, so the modulation shelf and the automation lanes reach it
whether or not a knob exists — and the rack shows a placeholder saying so.
`08-the-face.md` has three rendered concepts waiting; an improvised panel
before it would be work that step deletes.

The device is called **DS-01**. It deliberately does not join the `ML-*`
family: ML-M1 and ML-P8 are keyboard instruments that share a voice lineage
and a parameter-id convention, and DS-01 shares neither. Its serialized tag is
`ds01`, chosen explicitly rather than taken from a `rename_all` default — the
ML-M1's frozen `ml1` is the reason to pick an on-disk name on purpose the
first time.

## The four decisions this plan starts from

1. **One universal percussion voice.** Not Kick/Snare/Hat modes, and not a
   set of selectable per-voice engines. A single architecture whose controls
   mean something in every configuration. More drum types come from range and
   factory patches, not from new code paths.
2. **A new generator kind beside the existing `DrumSynth`.** The v1 device
   stays, old projects load unchanged, and no migration is attempted.
3. **One drum per channel stays.** The kit is the set of channels. Choke
   groups already work across channels, and each drum keeps its own insert
   rack, modulation rack, mixer strip, and pattern lane — which is more than
   any multi-part drum plugin gives a single drum.
4. **Name: DS-01.**

## Why v1 cannot be extended in place

The presenting symptom was that mod-source assignment could not be enabled for
the drum synth the way it was for the sampler and the synths. The cause is
structural, and it is worth stating precisely because it decides the whole
design.

`DeviceKind::descriptors()` in `generator.rs` returns `&[]` for `DrumSynth`,
and it is the only generator that does. The comment there calls the missing
table "mechanical work rather than a design question." **That is wrong**, and
this plan supersedes it. `DrumSynthParams` is a mode-union: `mode` selects
Kick, Snare, or Hat, and roughly two thirds of the struct is inert at any
moment. `kick_start_hz` means nothing in Hat mode. A flat descriptor table
over that union produces parameter ids whose meaning depends on a discrete
selector, so:

- a modulation route or automation lane can address a live parameter, and then
  silently stop doing anything when the mode changes;
- `mode` itself is the one control that would make those routes meaningful,
  and it is exactly the kind of structural discrete that must not be a
  modulation destination — switching it mid-hit changes which oscillators and
  envelopes exist;
- the ranges are not shared. `kick_start_hz` is 20-1000 Hz, `snare_tone_hz` is
  40-2000 Hz, `hat_hp_hz` is 500-16000 Hz. There is no honest way to give one
  id one curve.

Giving v1 a descriptor table is therefore not mechanical. It requires deciding
that a parameter's meaning does not depend on a mode, which is a different
instrument. Hence DS-01.

## Two things the investigation turned up that the design has to answer

- **v1 is inconsistent about what a parameter change does to a hit already in
  flight, and nobody decided it.** `render_range` re-copies `self.params` at
  every range boundary, so levels, drive, and mix follow parameter changes
  mid-hit. But envelope coefficients, the pitch tracking factor, and filter
  cutoffs are resolved once inside `trigger()` and never revisited. The split
  is an artifact of where the code happened to read the struct. Under
  modulation that becomes user-visible and arbitrary, so DS-01 must publish
  the split as a rule. Step 02 does.
- **Event order inside a block decides whether a route reaches the hit it was
  aimed at.** Modulation arrives as ordinary timed `Event::ParamValue` at the
  32-frame control rate, and `trigger()` snapshots parameters. A route meant
  to shape *this* hit must land before the note-on at the same offset. That is
  a contract between the renderer and the device, not an implementation
  detail, and it has never been written down because no descriptor-addressed
  generator has had a parameter that latches at note-on before.

## Build order

| Step | What it lands |
| --- | --- |
| `02-the-voice-and-the-descriptor-table.md` | **In.** The kind, the params, the ids, the tone and noise sources, and descriptor addressing on day one |
| `03-the-envelopes.md` | **In.** Four AHD envelopes with curve and optional gate |
| `04-the-body-resonator.md` | **In.** The tuned modal layer — toms, rims, bells, clangs |
| `05-the-burst.md` | **In.** Multi-impulse triggering: clap, flam, roll, buzz |
| `06-the-shape-stage.md` | **In.** Drive characters, the output stage, the gain contract |
| `07-internal-modulation-and-outlets.md` | **Matrix in; outlets blocked** on the shared device-outlet mechanism |
| `08-the-face.md` | **Mostly in.** Layout, controls, scopes and handles; the MOD panel, the hit trace, burst ticks and focus dimming remain |
| `09-the-kit.md` | Factory patches, range tuning, and the listening pass |

`mockups/` holds three rendered face concepts at the real face size, against
the real widgets. They are checked in because they are the argument for a
layout decision rather than notes from making one: the adopted layout is
there, and so are the two that were built and rejected. `08-the-face.md`
records why.

The adopted one is Adam's: each layer's scope sits directly under the controls
that make it. It carries a consequence the earlier layouts had hidden —
**DS-01's controls do not fit on one face unless the scopes are the envelope
editor.** Envelope times are handles dragged on the curve, not knobs. A scope
without handles is not a smaller version of this face; it is a different one
that needs a page.

Descriptor addressing lands in **step 02**, not at the end. It is the reason
this instrument exists; it does not get to be the last thing anyone gets to.
