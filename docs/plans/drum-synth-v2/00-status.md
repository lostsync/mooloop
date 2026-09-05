# DS-01 plan status

**Every step is in except step 07's published outlets, which are blocked
rather than skipped.** Adam played the device and its bank on 2026-09-04 and
closed step 09 on 2026-09-05. The device exists and plays: a new `Ds01` generator kind
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

Step 08 built the face. It first fitted on one screen by making the scopes the
envelope editor, and was rebuilt on 2026-09-04 into the six pages it ships as,
for the reason ML-P8's face was rebuilt the day before: a face that fits by
shrinking its controls has not fit. See "The face was rebuilt" below.

Step 09 shipped the kit: seventeen patches, seeded once into
`presets/generators/ds01/`, and the same patches the DSP acceptance test
asserts.

Read `01-what-ds01-is.md` for what the device is; the steps are done.

**The directory stays out of `archive/` for one reason:** step 07's published
outlets, which wait on the shared device-outlet mechanism ML-P8's step 06
waits for. Nothing else in the plan is unbuilt.

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

## Evidence for step 09, before it was played

Step 09's listening needed ears. Its central claim did not:
**one architecture reaches every drum type, from the controls rather than from
hand-tuning.** `one_architecture_reaches_a_kit` in `mooloop_dsp::ds01` answers
that mechanically. Thirteen patches — sub, kit and DnB kicks, tight and deep
snares, rimshot, clap, tom, closed and open hats, cowbell, clave, zap — each
sounds, stays bounded, ends, and is a different sound from every other, and
between them they span more than eight times in length and four times in
brightness.

Two of the plan's own structural claims are tested beside it:

- **The toms are one patch at three tunings.** Its ring is the same length at
  every tuning while its pitch tracks, which is what deriving the resonator
  from a decay *time* rather than a Q buys. If that had failed it would have
  been a step 04 bug found here.
- **A ghost hit is a different sound, not a quieter one.** Velocity routed to
  amp decay and filter cutoff makes the quiet hit shorter *and* duller as well
  as softer. `09-the-kit.md` calls this the acceptance case for the whole
  instrument, and it is built from two ordinary matrix rows.

**These are not the factory bank.** They are patches reached by reasoning
about the architecture, and nobody has heard them. Their job is to say the
controls reach; whether they sound *good* is exactly what step 09 is for, and
a patch that turns out wrong is a finding about a range or a curve rather than
a case to delete.

## Step 02's acceptance, through the assembled program

Three of step 02's acceptance criteria are only true of the whole program
rather than of the device against its own `process`, and they now have tests
in `mooloop_engine::ds01_tests`:

- **It plays from a pattern**, through the sequencer, the strip and the master.
- **A channel LFO on Filter Cutoff sweeps a hat pattern.** The step calls this
  the case the whole plan exists for, and it is asserted through the real
  modulation rack — an LFO installed in a slot, a route added to a `ParamAddr`
  — rather than by calling the device. v1's drum synth cannot be reached this
  way at all, which is the difference DS-01 was built to make.
- **It renders identically offline and live.** That claim has one mechanical
  meaning: an offline render and a realtime callback differ in exactly one
  thing DS-01 can see, the block size. The same project at 128 frames and at
  1024 is sample-for-sample identical, with and without an automation lane on
  Tone Pitch.

A fourth criterion — **it saves and reloads** — is covered in
`mooloop_project`: a patch with a value from every band round-trips through a
bundle, the on-disk tag `ds01` is pinned, a value out of range is repaired
rather than refused, and a matrix row pointed at something that cannot be
modulated is switched off. That last one is only reachable from a hand-edited
file, since the face cannot author it, which is exactly why the doctor has to
see it.

The offline-and-live criterion rests on a condition worth naming: DS-01 walks its own control
tick from the start of each block, so the grid only lands on the same absolute
frames because every block boundary is a multiple of the control rate. That is
true of every driver buffer size and of the offline renderer's chunking, but it
is a condition rather than a guarantee.

## The review passes

The device was built in one push, so it got a review afterwards — and the face
and the fixes got a second one, since a fix that moves an architecture is
exactly where the next bug hides. Thirteen defects between them, all fixed.
Four are worth carrying forward as things to watch for rather than as
history:

- **A per-hit destination has to be smoothed per hit.** A route to any of the
  four mix levels was inert, because the levels were smoothed on the device
  while the matrix resolved per voice — and `PARAM_LEVEL` is every row's
  default destination, so the first route anyone made was the one that did
  nothing. Anything else that gains a smoother needs the same question asked
  of it.
- **A release with a phase is not idempotent, and a choke is repeated.**
  Transport stop chokes on every stopped block; restarting the fade each time
  meant one longer than the block period never finished. v1's coefficient
  stamping had that property for free.
- **A display that approximates its own DSP will contradict it.** The scopes
  drew the curve control backwards — the fastest decay where the envelope
  produces the slowest — because a power curve was standing in for
  `env::shape`. The real function is three lines.
- **Two handles at the same value are one handle.** Only the later-declared
  one can be grabbed, so Pitch Attack was uneditable — it has no knob
  elsewhere — and every envelope's Attack was unreachable from the default
  patch, where attack and hold are both zero. The pitch envelope's peak is now
  one handle carrying both axes, since when its attack ends and how high it
  got are the x and y of one corner; the rest are held a minimum distance
  apart when they would coincide.

The second pass also found three things that were offered but could not work:
a matrix route to Body Level, because the skip that gated the layer read the
very smoother it gated; and routes to Output HP and Choke Time, which belong
to the device rather than to a hit and so have nothing per-voice to land on.
Those two are out of the matrix's destination list now — a channel route still
reaches them, which is the right level for a device-wide control — and
`DS01_DESTINATIONS` is 47 rather than 49.

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

## The face was rebuilt — 2026-09-04

The first face fitted the device on one screen by making the scopes the
envelope editor: envelope times were handles dragged on a curve rather than
knobs, and what was left over was a 21px dial with an 8px caption at five rack
units. Adam's ruling is that this is the same trade ML-P8's first face made
and he rejected — "might be ok on a 24 inch screen but not 14" — reached from
the other direction, and that DS-01 should spend pages the way ML-P8 now does.

`08-the-face.md` carries the new layout. Four things the rebuild turned up:

- **Merging the display and the editor cost the device typed entry.** At a
  21px dial there is no room for a value field, so every one of the ninety-two
  controls could only be approached rather than typed. The paged face uses
  `KnobStack`, which is a dial and a field, so a number is exact again — and
  one `ds01-text-committed` handler covers all ninety-two, because the
  descriptor id travels with the text.
- **A model-bound control stops following the model once it is touched.** The
  face is indexed by descriptor id, so a knob's value is a *binding* onto a
  model row — and Slint drops a binding at the first assignment to the
  property it feeds, which is exactly what a knob does to itself during a
  drag. Nothing showed it, because the edit handler writes the same value
  straight back. Loading a preset over a face somebody had been turning would
  have left that control behind, which the factory bank makes an ordinary
  action rather than a corner.

  The first fix was wrong in an instructive way: a `changed` handler on a
  private property that re-asserted the model value. It cannot work, because
  a Slint binding is lazy — once nothing depends on that private property it
  is never re-evaluated, so `changed` never fires. `a_dragged_knob_still_
  follows_the_patch` in `crates/mooloop-ui/tests/ds01_face.rs` failed on it,
  which is the whole reason that test was written before the code was
  believed.

  What works is making the control **not write its own value**:
  `ParameterKnob` and `MiniKnob` gained a `controlled` flag that reports the
  change and leaves the property alone. The owner writes the model, the
  binding survives, and the value has one home. It is opt-in because every
  other face two-way-binds its knob to a real property, and a `<=>` binding
  *intercepts* the write and forwards it up rather than being removed —
  `Property::set` in `i-slint-core` is explicit about that, and it is why this
  had never bitten anything before.
- **Column dimming and the fifth rack unit were both paid for by the one
  screen.** Dimming existed to say which of four columns you were reading;
  one layer per page says it without a mechanism, so `focused-column` is gone.
  The fifth unit existed to make four columns fit; DS-01 is four units now,
  the same as ML-P8, and the rack is that much narrower for it.
- **The mod envelope moved to the AMP page, not the MOD page.** It is an
  envelope and it is drawn like one, and eight matrix rows need the whole
  height of a page — 8 x 18px plus a header, against a module's own 4px
  spacing between children, which is what pushed the eighth row off the first
  attempt.

Three shared-widget changes came out of it, all additive: `PickerChip` moved
from `mlp8-device.slint` into `controls.slint` — a second device needed to
pick from forty-seven destinations — `KnobStack` gained a `fill` so a page's
controls can carry its layer's colour, and `ParameterKnob`/`MiniKnob` gained
`controlled`, above.

One more thing the pages exposed rather than caused: **DS-01's values were
formatted in the shared units and read badly at drum lengths.** Every envelope
time is a field now, and `format_param_value` renders seconds with two
decimals — so a 5 ms attack, a 1 ms one and a zero all read `0.00 s`, which is
the whole range step 09's audit is about. `ds01_display_unit` states the unit
once for both the formatter and the parser: a time under a second is
milliseconds, a frequency over a kilohertz is kilohertz, and a route's depth
is a percentage. A typed value means the unit it is written with, and with
none written it means the unit the field was showing — the only rule that is
self-consistent for a field whose unit follows its value.

And three from step 08's first build, which the rebuild did not invalidate:

- **The face is indexed by parameter id, not one property per parameter.**
  Every other device face here declares a property each; DS-01 has ninety-two,
  and ninety-two properties would be a second copy of the parameter table
  maintained by hand — which is the thing this device exists not to have. It
  takes arrays indexed by descriptor id and reports edits as
  `(id, normalized)`, the shape the modulation depths already use, so one
  handler covers every control. Values cross normalized rather than natural,
  because that is the space a route and an automation lane both work in.
- **A source device declares its own width.** It was three rack units for
  every kind; DS-01 was the first to ask for more, the way an effect slot
  declares its units. It asked for five while it was one screen. It is four
  now, which is where ML-P8 also landed, and the mechanism is the part that
  outlived the number.
- **The columns were not equal width.** TONE, NOISE, BODY and AMP took 4, 5, 3
  and 4, because that is how many cells they had. That is the shape of the
  problem the pages solved: a layout whose columns have to be sized by cell
  count is a layout with no slack anywhere, and the next control added to any
  of them takes width from a neighbour.

## What step 08 owed, and no longer does

**The MOD panel.** The three checked-in concepts settled the columns and the
bands and none of them drew it, so the layout for the matrix's thirty-two
controls was genuinely undecided rather than a detail to fill in. The band
carried a labelled empty region where it went, and `00-status.md` recorded it
as the one thing in the plan that wanted Adam before it wanted code.

Pages answered it. The matrix is a page of its own: eight rows of source,
destination, amount and curve, full width, with the two pickers reading from
lists `mooloop_core` labels so the face holds no second copy of either. Source
and Destination are `PickerChip`s rather than cycling chips because nine and
forty-seven are past what clicking through can carry, and Amount and Curve are
ordinary modulation destinations, which is how a channel LFO gets to scale a
per-hit relationship.

The rest of what the step owed is in: every page, the four contours, the
rendered hit, the burst's impulse ticks, and the span that follows the patch,
verified by a software-rendered snapshot of every page and at a four-second
patch. The preview still follows the patch's span rather than a fixed window,
which needed `Ds01::preview_waveform` to take the span and to *lower its
sample rate* as the span grows: it is still the production voice path, clocked
slower, because rendering four seconds at the full rate is a fifth of a second
of arithmetic that `08-the-face.md` says must not happen on the UI thread. It
is debounced on top of that, so a knob drag renders the hit once when it
stops.

The ticks are read from `Ds01::burst_offsets`, which runs the same schedule
the voice runs rather than re-deriving it from the controls: the spread's
compounding and the bound on the total are one implementation, so a drawn
burst cannot disagree with a played one. They get their own axis rather than
the scopes' span, because a twelve-millisecond flam inside a four-second ride
would be four ticks in the first pixel.

## The kit, from step 09

`mooloop_core::ds01_factory` ships seventeen patches and
`mooloop_project::seed_ds01_bank` writes them into `presets/generators/ds01/`
once. Three things that file settled:

- **Generator presets, not channel presets.** The ML-M1 bank is channel-scoped
  because Sequence Bleep is nothing without a channel rack. A DS-01 patch's
  modulation is its own matrix, inside the voice, so there is no rack to carry
  and nothing to re-scope onto the channel it lands on.
- **The bank is the DSP test's fixture.** `one_architecture_reaches_a_kit`
  held its own thirteen patches; it reads `ds01_factory::patches()` now, so
  what ships is what is asserted, and the tom-tuning and ghost-hit cases name
  bank patches instead of rebuilding them.
- **The gate had to be tested for termination, not excused from it.** The Ride
  is the patch that uses the gate, and a gated patch rings for as long as it
  is written — so the acceptance loop sends a note-off to any gated patch
  rather than skipping the "it ends" assertion for the one patch that would
  fail it for a legitimate reason.

### Closed — 2026-09-05

Adam played the device and the bank extensively on the night of 2026-09-04 and
closed the step: "they don't have to be flawless presets. for now we just need
something there, mostly to prove the system works."

That is the step's bar, stated after the fact and worth keeping, because it is
not the bar the plan implied. **Step 09's job was to prove the architecture
reaches a kit from the controls, not to ship a curated bank.** Seventeen
patches that a musician can play, load, and hear the range of do that. Range
tuning driven by taste is deferred to a later, dedicated push at a factory
bank across every device, when the application is complete enough for one to
be worth authoring; the note under "Deliberately not now" in `FOCUS.md` says
so.

The pass raised no range or curve corrections, so the ROADMAP's drum
range-and-scaling item closes against this step as planned. One thing is
carried out of the step rather than closed with it: **whether the default
new-song kit moves to DS-01**, which `09-the-kit.md` deliberately deferred
until after the listening. That is a decision, not work; nothing in the code
waits on it, and v1 `DrumSynth` stays either way.

This also closes the standing listening debt: the last recorded pass before it
was the ML-M1 bank on 2026-08-31, and a stretch engine, a slice mode, most of
ML-P8, and a 12 dB drop in every default had landed since.

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
| `08-the-face.md` | **In.** Six pages, rebuilt from the one-screen face on 2026-09-04 |
| `09-the-kit.md` | **In.** Seventeen patches, shipped, asserted, and played |

`mockups/` holds three rendered face concepts at the real face size, against
the real widgets. They are checked in because they are the argument for a
layout decision rather than notes from making one, and what they settled still
holds: how the device divides, which layers are peers, and that a display
belongs beside the controls that make it.

What they got wrong is that the division had to fit at once. The third concept
reached "DS-01's controls do not fit on one face unless the scopes are the
envelope editor" and took that as a licence rather than as a warning. It is a
warning: a device that only fits by deleting its knobs is a device that wants
a page. See `08-the-face.md`.

Descriptor addressing lands in **step 02**, not at the end. It is the reason
this instrument exists; it does not get to be the last thing anyone gets to.
