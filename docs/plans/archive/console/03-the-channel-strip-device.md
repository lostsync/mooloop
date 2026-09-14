# 03 — The channel strip

> **Rewritten 2026-09-11 as the work order.** The file it replaces proposed a
> composite `EffectKind` you insert on a channel or a bus, and then carried
> two rounds of amendment saying that it is not one. The amendments are right
> and are folded in here rather than stacked on top: **the channel strip is
> not a device, it is what every track has.** Everything the old file said
> about reuse and about the order of work survives, because that part was
> never about where the strip lives.
>
> Read [`THE-STRIP.md`](THE-STRIP.md) first — Adam's mockup, the four
> voicings, the three drawings, and the 92px arithmetic. This file is what to
> build, in what order, and what the acceptance is.
>
> The original is not kept at the bottom. Its opening sentence is the only
> thing in it that is not also here, and `00-status.md` records what the
> mistake was.

## What it is

Four sections on every track, all four out by default:

```text
  pre in / drive     the voicing's input stage: harmonics, tilt, slew
  eq in              four bands — high shelf, high mid, low mid, low shelf
  comp in            threshold, ratio, in trim, attack, release, knee,
                     w/d mix, makeup
  polarity           one multiply, and the cheapest thing on the mockup
```

over one strip-wide **voicing**: **Moo, Grip, Punch, Iron**.

A track, not a channel. That is the same ruling analog sum already landed on
-- Adam, 2026-09-09: *"the summing thing for now is tracks-only"* -- and it
has the same reason: the console this models puts its Channel stage on a
mixer strip, and mooloop's mixer strip is a track. Several channels reaching
one track are a group, and the track's strip is what a desk would put across
that group.

**The master gets one.** It is a track, a mix-bus compressor is a real thing
to want, and nothing about the sections depends on feeding something. Analog
sum is the single exception on the master and it is an exception for a
mechanical reason -- an encode there would go into a sum nothing decodes --
which none of these four share.

## Where the parameters live

`MixerBus` gains one field, `strip: StripParams`, defaulted on load so every
saved song opens with four sections out and `Moo` selected -- which is
bit-identical to the file it was saved from. That is the same shape
`console: bool` already has, and `docs/PROJECT_FORMAT.md`'s defaulted-field
migration.

| Section | Parameters |
| --- | --- |
| voicing | `voicing` (Moo / Grip / Punch / Iron), strip-wide |
| drive | `pre_in`, `drive_db` |
| eq | `eq_in`, four × { `frequency_hz`, `gain_db`, `q`, `kind` } |
| comp | `comp_in`, `threshold_db`, `ratio`, `in_trim_db`, `attack_ms`, `release_ms`, `knee_db`, `mix`, `makeup_db` |
| output | `polarity` |

Twenty-six numbers and five switches. They travel to the engine as
`EngineCommand::SetStripParam { bus, param, value }` -- POD, one variant, one
`u32` id -- and arrive with the project through `load_project` like every
other per-track value. `StripParams::set` / `get` and a
`StripParams::descriptors()` table are what the id means, so a face is held
to the same descriptors an effect's is (see *Verification*).

**The drive section has no output trim**, unlike `EffectKind::Preamp`, and
the mockup is why: it draws `pre in / drive` and one knob. The preamp device
needs its own trim because it can sit anywhere in a chain; the strip's drive
has the comp's `in trim` and the fader downstream of it, both visible on the
same face.

## The rule that keeps the knobs honest

A voicing selects *laws*, never values. **Nothing a voicing does moves a
number a knob shows.**

This is the rule the first draft of this file got wrong, in a way worth
stating because it sounds harmless: it had a voicing choosing "EQ band
frequencies and Q", which means the 3 kHz on the face is not the frequency
being boosted. Four voicings would then be four sets of lying knobs. What a
voicing supplies instead is everything on the signal path that **no knob on
the face shows**:

| Section | What the voicing owns | What the knobs own |
| --- | --- | --- |
| drive | harmonic profile, tilt depth and corner, slew limit | drive in dB |
| eq | the Q law: constant, or proportional (a boost narrows) | frequency, gain, Q, bell/shelf |
| comp | the curve above the knee, and programme dependence | everything the mockup draws |

The EQ's Q law is the one place a voicing touches a knob's meaning, and it is
precedent rather than an exception: `EqQProfile` already exists in
`mooloop-core` and `EqEffect::effective_q` already applies it to the
seven-band EQ. It is also *drawn* -- `EqResponseDisplay` plots the response
that is running, not the response the knobs would imply -- which is the
condition that makes it honest.

The same condition covers the comp: `DynamicsCurveDisplay` draws the gain
computer's real curve, so a voicing that bends the line above the knee shows
the bend. A voicing that changed the attack time under a knob reading 10 ms
would have nowhere to show it, which is why none of them does.

### The four voicings, as tables

`mooloop_dsp::preamp::PreampVoicing` is already the drive half and its
harmonic profiles are **measured** (2026-09-10, 106 units, `spikes/preamp-measure/`).
This step adds the other two thirds:

| | drive | EQ Q law | curve above the knee | programme dependence |
| --- | --- | --- | --- | --- |
| **Moo** | transparent, skipped | constant | the literal ratio line | none |
| **Grip** | `GRIP_PREAMP` (SSL 4000 G+, tilt 10 dB) | proportional | literal, firm knee | slight |
| **Punch** | `PUNCH_PREAMP` (UA 610-A, tilt 5 dB) | proportional | eases above the knee -- loud without clamping | moderate |
| **Iron** | `IRON_PREAMP` (EMI TG12345, tilt 14 dB, slew) | constant | steepens above the knee, vari-mu | strong |

Programme dependence is a second, slower release stage blended with the
knob's: the classic auto-release, and the reason a desk compressor does not
pump on material that has both a kick and a vocal in it. Strength is the
voicing's, the release *time* stays the knob's.

**`Moo` is the null case, exactly.** Both sections out and `Moo` selected is
bit-identical to no strip -- `MOO_PREAMP.is_transparent()`, a literal ratio
line, no programme dependence, and `db_to_linear(0.0) == 1.0`. That is a test,
not a tolerance, and it is what entitles the strip to exist on all 256 tracks.

## Where the strip sits in the chain

`THE-STRIP.md` left this open: *"Where the pinned strip row sits in a track's
chain by default, and therefore whether a track's own devices run before or
after its EQ and compressor."*

**Decided here: at the head.** `pre → eq → comp → the track's own devices →
fader`, and the reasons are in that order:

- the drive stage is an **input** stage; a preamp after an insert is not one;
- a track's rack is *"glue and post"* (`README.md`, decision 2), and post
  means after;
- the thing people put last on a track is a limiter, and a strip compressor
  behind it would be working on a signal something else has already clamped.

It is **one statement**, as Adam asked: `mooloop_core::mixer::STRIP_PIN`, a
`StripPin` enum with `Head` and `Tail`. The engine branches on it in the bus
block loop and the rack reads it to decide where the pinned row draws. Moving
the pin is editing that constant, and both the audio and the drawing follow.

It costs the latency compiler nothing: biquads, a detector and a memoryless
shaper declare no latency, there is no oversampler (`effects/preamp.rs` says
why), and so `chain_latency` is unchanged and no compensation ring moves.

## Free while it is out

Adam: *"comp and eq yes, every track, but defaults to turned off. control for
this is via the 'eq in'/'comp in' buttons."*

That is a stronger rule than "free when flat" and it needs no threshold to
test against: a section that is out is not run. Three switches, three
branches, and the strip's per-block cost on an untouched project is reading
them.

The strip is **not** boxed and not optional storage. Eight biquads, a
detector, two smoothers and a shaper is a few hundred bytes a track, against
the **128 KB** step 04 measured a track already costing -- so the honest
answer to "what does it cost to have one everywhere" is *nothing worth
measuring in memory*. What must not be paid is **time**, and the `in`
switches are what does not pay it.

`is_at_rest` still matters for the same reason `CompressorEffect`'s does: a
detector frozen mid-release wakes up holding reduction the music stopped
asking for. A strip whose sections are in reports at rest only once its
detector has released and its biquads have run out of state, and
`BusStrip::is_resting` gains that term.

## The three drawings

Settled 2026-09-10, in `THE-STRIP.md`; the arithmetic for 92px is there and
is not repeated.

1. **The paned strip, front.** 92px. Name, meter + fader, the column beside
   the fader (mute, polarity), destination, analog sum, and the turn-over
   button in the lower-right corner.
2. **The paned strip, back.** The same 92px, the same name / meter / level /
   pan, and one page of EQ / COMP / DRV / SENDS. Three knobs to a row at
   92px, so **an EQ band is one row**.
3. **The track's pinned rack row.** `BusDeviceFace` already stands in the
   rack's head slot and already carries the name, fader, pan, mute,
   destination, console switch and sends. The strip's four sections join it.

Two rules, pulling opposite ways on purpose:

- **A page that is not on screen is not built** -- an `if`, not an
  `opacity: 0`. That is what keeps the back face free on 256 tracks.
- **No parameter is reachable from only one drawing.** A control on the rack
  face and not on the back face makes the mixer's state something a user has
  to manage before they can work.

The EQ reads **left to right, top to bottom**: high shelf, high mid, low mid,
low shelf. Say so in the markup. Two columns of knobs invite the assumption
that the columns mean something.

`MixerMetrics.strip-width` is 92px under a name, the way `DeviceRackMetrics`
holds the rack's numbers. The front face, the back face and
`tests/mixer_snapshot.rs` all want it.

**The zoomed console is the fourth drawing and it is not in this step.** The
mixer pane zoomed to the window has the room for every section of every strip
at once, which is the desk; the two paned faces and the rack row already
satisfy the reachability rule, so it is comfort rather than capability. It is
recorded in `00-status.md` with what it needs, and it needs nothing this step
does not build.

## Reuse rather than rebuild

Mostly assembly, and every part exists or is queued to be extracted.

- **The shared `Biquad`.** `docs/plans/archive/adopt-shared-biquad-in-eq/` is that
  plan and this step absorbs it: `effects/eq.rs` still declares a private
  copy of the RBJ cookbook, and the strip must not add a third. Doing the
  adoption first means the strip's EQ is built on the one the preamp already
  uses. The shared copy needs two things the private one has -- `is_at_rest`
  and a sloped shelf -- and neither changes a coefficient.
- **`crate::dynamics`** -- `EnvelopeFollower`, `compressor_gain_db`,
  `time_coeff`, the dB helpers. As **primitives**: Adam's ruling is that the
  strip compressor is *"a totally different comp than the one built in as a
  device"*, so it is a sibling of `CompressorEffect` rather than a wrapper
  around it, and `w/d mix` is a straight parallel balance rather than the
  device host's blend.
- **`mooloop_dsp::preamp`** -- the drive section *is* `Preamp`, unchanged. It
  was built for this and has had a device in front of it since 2026-09-10.
- **`DynamicsCurveDisplay`**, which already draws a gain computer's curve.
- **`EqResponseDisplay`**, which is **private to `eq-device.slint`** and has
  to be extracted. That is the one piece of face work this step has to do
  before it can start drawing anything.

## Order of work

`AGENTS.md`'s device-work order, because the three parts of a device differ by
four orders of magnitude in what it costs to look at them. One commit each:

1. **The biquad adoption.** `effects/eq.rs` uses `crate::biquad::Biquad`;
   the shared one gains `is_at_rest` and a shelf with a slope. Bit-exact --
   the EQ's existing tests are the check, and they are about exactness
   already.
2. **The strip DSP.** `mooloop_dsp::strip`: the EQ bank, the compressor, the
   voicing tables, and the unit that runs all four sections. Unit tests, on
   the laptop, at rung 2.
3. **The model and the engine.** `StripParams`, `STRIP_PIN`, the command, the
   bus block loop, `load_project`, polarity. Engine tests.
4. **The faces.** `EqResponseDisplay` extracted, the 92px front and back, the
   pinned rack row. Iterate with `scripts/slint-sketch`; cross into
   `main.slint` **once**, with every property and callback in that one pass.
5. **The documents**, and the plan's close-out.

## Acceptance

1. A default project renders **bit-identically** to one built before the
   strip existed: four sections out, `Moo`, polarity off.
2. `eq in` with every band flat is audible as nothing and measurable as
   nothing; a band boosted 12 dB at 1 kHz raises a 1 kHz sine and leaves
   100 Hz alone.
3. A shelf switched to bell changes the response and keeps its frequency.
4. `comp in` with a threshold under the signal reduces gain; `w/d mix` at 0
   is the dry signal exactly; makeup restores level without moving the
   detector.
5. Each voicing's drive measures its stated harmonic profile at the operating
   level -- which `harmonics_hit_their_stated_target_at_the_operating_level`
   already asserts, so what this step adds is that the *strip* reaches it.
6. A voicing with programme dependence releases slower after a long loud
   passage than after a short one, at the same knob setting.
7. Switching a voicing **rebuilds** the stage rather than retuning it, for
   the reason `PreampEffect::rebuild_voicing` gives -- running one voicing's
   filter state through another's curve is neither voicing -- and a section
   that is out is not touched by the switch at all.
8. `STRIP_PIN` at `Head` runs the strip before the track's devices; flipped
   to `Tail` it runs after, and the rack draws the row in the other place.
   One test, both values.
9. Idle-skip equivalence: the same project rendered with `skip_idle` on and
   off is sample-identical, with the strip in on some tracks.
10. Every strip parameter is reachable from the paned back face **and** from
    the rack row, and the face agrees with `StripParams::descriptors()`.

## Verification

`slint_face_agreement.rs` holds a face to its descriptor table, and
`every_effect_linear_readout_agrees_with_its_table` holds a readout to its
range. Extend both rather than adding a parallel check: those tests exist
precisely so a new face cannot ship a range spelled twice, which
`AGENTS.md` names as this codebase's characteristic fault.

`cargo test -p mooloop-dsp` while building the DSP, `-p mooloop-engine` for
the block loop, and rung 4 on the box once before the commit that closes the
plan.

## Not in this step, and why

Named here so none of it is discovered as an absence.

- **Solo.** Drawn on the mockup, and a monitor tap rather than a control:
  `mooloop-core` has no solo state at all and `docs/archive/MIXER_PLAN.md` specifies an
  AFL-style second output path. It is the largest unbuilt thing on the
  mockup and it is its own step. A dead solo button is not drawn.
- **The zoomed console.** Above.
- **Automation and modulation of strip parameters.** A lane's target is an
  `EffectTarget` plus a slot plus a param id, and the strip is not a slot. It
  is reachable -- the ids exist and are stable -- but it is a change to the
  addressing scheme rather than a rider on this feature.
- **A strip preset.** The preset system's unit is a device
  (`docs/plans/preset-system/`), and a strip is not one. The voicing is the
  thing worth recalling and it is one value.
- **The voicing's own EQ curve.** `06-preamp-modelling.md` asks for a broad
  presence lift on `Iron` and a top-end bump and rolloff, as fixed voicing
  curves. Not built, for one reason: it would make the strip's drive and
  `EffectKind::Preamp` two different stages under one voicing name. When
  somebody measures a curve, it belongs in `PreampVoicing` where both get it.
- **Sends on the back face's SENDS page** are the sends that already exist,
  drawn where `THE-STRIP.md` puts them. The drill-down tap points are step
  05's stage 2 and are unaffected.
