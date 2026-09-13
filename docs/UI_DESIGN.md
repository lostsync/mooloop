# Mooloop Interface Design Language

Status: active design contract, September 2026.

This document defines how mooloop's interface is composed. It exists because
locally reasonable controls do not automatically make a coherent instrument.
Current direct feedback and annotated mooloop screenshots outrank older UI
decisions. New work must follow this document unless a purpose-built design
explicitly replaces part of it.

## Design Goal

Mooloop should read as a compact musical instrument: dense, bounded, quickly
scannable, and comfortable to manipulate repeatedly. It is not a settings
application and should not look like a collection of form fields.

The strongest ideas in the current references are structural rather than
decorative:

- Instruments are tiled from rectangular functional modules.
- Related controls share an edge, baseline, heading, and visual field.
- Graphs, faders, meters, keyboards, and routing diagrams make useful use of
  module area.
- A small finite choice is visible and selectable in one click.
- The entire panel has a deliberate silhouette. Controls do not trail off into
  unexplained bordered space.

## Composition Grammar

The interface has four levels. Their ownership must remain visible.

1. App chrome: menu, transport, global timing, master state.
2. Work surface: rack, notes, playlist, mixer.
3. Channel header: selected channel identity and whole-channel preset/actions.
4. Generator: source type and generator modules.
5. Device: the hosted device's own parameters, presets, and host actions.

A control belongs at the lowest level that owns its state. A preset's unit is
a device, so a device's save and load sit on that device's own rail --
generator and effect alike -- rather than in a toolbar above the chain, where
a control that belongs to one device would look like it belongs to the row.
Channel presets remain in the channel header because they include the
generator and channel-level state, which no single device owns.

The modulation rack and its routes are channel state. A common device frame
may expose that system because it is where the user reads the channel's signal
flow, but the frame does not make a modulation source belong to that device.
Device faces own their parameters; the channel owns the control signals that
can reach them.

## Module Grid

An instrument body is a row or grid of modules, not a free canvas.

- Use a 4 px base unit. Ordinary gaps are 4 or 8 px.
- Outer editor padding is 8 px.
- Module padding is 6 or 8 px, chosen once per row.
- Modules in one row share their top and bottom edges.
- Reuse a small set of row heights. Prefer 96 px for one compact control row,
  160 px for a graph plus controls, and 224 px for a dominant editor.
- Adjacent modules use 4 px gaps. Do not simulate layout with large invisible
  rectangles.
- A module title is 9 px uppercase or compact title case and occupies a fixed
  12 px line at the top-left.
- A module's width is intentional: fixed grid span, proportional stretch, or
  content width within a larger unframed band. Never inherit an arbitrary
  viewport width by accident.

### The rectangle test

Before accepting a panel, outline every visible module. The outlines should
form a small number of clean rectangles with aligned edges. A staircase of
unrelated content-width cards fails this test. So does one full-width card with
half of its interior blank.

Empty space is valid only when it is:

- an unframed work surface;
- the plotting area of a graph, envelope, waveform, meter, or routing view;
- reserved for content that changes size at runtime; or
- deliberately allocated to a module that stretches with the window.

Empty bordered space is a defect. Fix it by changing the module grid, changing
the control type, or giving the area a useful display. Do not hide it by merely
shrinking every card to a different width.

## Control Selection

Choose controls by the shape of the decision, not by whichever widget already
exists.

| Value | Preferred control | Avoid |
| --- | --- | --- |
| 2-6 fixed modes | segmented selector or radio bank | dropdown |
| waveform/filter shape | icon or short-label selector bank | dropdown |
| binary state | toggle, checkbox, or power button | two-item dropdown |
| compact continuous value | knob | text field with arrows |
| related continuous values that must align | short faders | uneven knob row |
| envelope stages | graph plus aligned A/D/S/R knobs or faders | detached graph and controls |
| precise integer | stepper or drag value | oversized +/- buttons |
| long/dynamic set | menu or searchable browser | dozens of visible buttons |
| file/preset choice | browser/menu with previous/next where useful | segmented selector |

Knobs are not the default answer to every parameter. Faders create strong
baselines, expose relative values, and can occupy width that would otherwise
become dead space. Use them for envelopes, mixer levels, and related control
banks when that improves the module geometry.

Dropdowns are reserved for genuinely long or dynamic option sets. Oscillator
shape, filter mode, drum family, retrigger mode, and similarly small fixed sets
must be visible one-click choices.

## Alignment

- Controls in a row share knob centers or fader tracks.
- Labels in a row share a baseline.
- Value readouts in a row share a baseline and use stable dimensions.
- Section dividers span the module content height; they do not stop or start at
  arbitrary points.
- Graph handles must lie on the rendered envelope or curve.
- Device plots derive from the parameters and transfer functions used by the
  audio path. Static decorative waveforms and filter curves are not valid
  substitutes for parameter feedback.
- A compact control beside tall controls must be intentionally centered or
  placed in a labeled sub-row. It must not leave an accidental blank quadrant.
- Dynamic visibility must not move unrelated modules. Reserve a stable grid
  cell or replace content within the same bounds.

## Density And Hierarchy

Use contrast and spacing to show hierarchy, not floating cards within cards.

- The editor background is the work surface.
- Generator modules use one surface level and a restrained border.
- A graph may use a darker plotting field inside its owning module.
- Selected modes use the accent. Accent is state, not decoration.
- Module titles are quieter than parameter labels; parameter labels are quieter
  than values that need active reading.
- Avoid isolated tiny controls surrounded by large dark fields.
- **A list of things a user makes draws exactly the ones that exist, and
  scrolls.** It does not reserve empty bays for a number somebody drew once,
  and it does not shrink its rows to fit more in. Adam, 2026-09-09, on the
  sends area his own mockup had drawn as four bars: *"i drew 4 sends bc that's
  how many fit in my drawing. if there are no sends, we wouldnt show any. we're
  not limiting to 4… if we gain more than will fit, that area should scroll."*
  A track with no sends draws a line saying so, which is smaller than one empty
  bay would be. Note that a scroll bar drawn *over* the viewport's right edge
  will swallow the rightmost control in a row -- reserve for it in the row's
  padding, and test the reachability with a click rather than an invoke.

The source editor should feel like one instrument front panel. It should not
look like several cards dropped into the center of a page.

## Theme Tokens

The palette has three user-set seeds, and every token is derived from them in
`settings::derive_palette`:

- **Base** seeds all neutrals: background, panel, the three surface levels,
  border, and the three text weights. A light base flips the ramp, so light
  schemes work without a second code path.
- **Accent** is state: selection, focus, and meters in their safe range.
- **Alert** is attention: warnings, meter headroom, out-of-range readouts.

Only a true clip uses the fixed destructive red; it is not user-set, because a
clip must never blend into a chosen palette.

Two scalars retune the derived result live: **contrast** scales every neutral's
distance from the base, and **roundness** scales the shared corner radii.

A component must not write its own hex color or literal corner radius. Use
`Theme.*` colors and the `Theme.radius-xs/sm/md/lg` tokens; anything hardcoded
is invisible to Preferences > Appearance. Pill shapes stay local geometry
(`height / 2`), since they track their own bounds rather than the radius scale.

## Device Rack Layout

The lower source editor is an ordered horizontal device rack. Signal flows
left-to-right from one source device through zero or more insert devices. The
source is not a special full-width page: it uses the same rack chrome,
alignment, and height contract as effects.

- Device faces have one fixed 268 px height.
- Width is quantized in 220 px units with 4 px inter-device gaps. Half-unit
  widths are valid for compact effects.
- A source device declares its width the same way an effect does, and for the
  same reason: 3U for the sampler, the two v1 synths and the ML-M1; 4U for the
  ML-P8 and the DS-01, which spend pages rather than one dense screen and
  need the fourth unit to hold three modules of 34 px dials without shrinking
  one; 2U for Aux In, whose whole content is a source, an outlet and a level,
  and which at 3U would be empty rather than generous. A bus's output stage
  stands in the same position at 2U, and the track's pinned channel-strip row
  takes 2U beside it -- three until 2026-09-11, when the EQ's response plot
  moved into the room the input stage was not using and the third unit turned
  out to have been margin. An effect uses only the units its working
  controls require, declared once in `effect_kind_units`
  (`mooloop-ui/src/lib.rs`) rather than in each face: 1U for filter, drive,
  preamp, bitcrush, limiter, plate, and Buffer; 2U for gate, compressor, EQ, and Mod;
  3U for delay and reverb. A face that outgrows its width takes another unit;
  it does not compress.
- The rack scrolls horizontally. Device internals never compress when the
  application narrows.
- Every device has a 28 px identity header with enabled state, name, kind, and
  size. When the device came from a preset the header names it beside the
  device name, elided rather than allowed to push the kind tag off the end.
  The host's bypass and wet/dry controls occupy the header's right edge;
  a device face must not add a second copy. Effect faces inherit the shared
  `EffectDeviceShell`, which owns that header and the drag-to-reorder handle;
  a face file contains only its working controls. A reorder drag is visible
  while it happens: the face follows the pointer, its origin stays as an
  outline, and the rows in between animate aside to open the gap it will land
  in. The landing is the row under the pointer, taken from that row's own
  bounds rather than from a nominal one-unit pitch. Controls unique to that device
  begin below the header. The common frame also owns a compact `MOD n` route
  summary for routes terminating in the device. It can show source pills where
  a count is too opaque and opens the channel's modulation shelf or a
  device-filtered route inspector; it never creates a device-local modulator.
- Signal direction and insertion points remain visible between devices.
- A device with more controls than one face can hold uses stable internal
  pages. Switching pages never changes device dimensions or moves neighboring
  devices.
- A face is a working surface, not a dump of every parameter. Each page must
  still expose a coherent musical operation rather than an arbitrary subset.
- The shared host owns input and output metering, input and output trim, wet/dry,
  bypass, presets, insertion, removal, and reorder actions. Presets are the
  same two rail buttons whatever the device is, so a generator's bank is
  reached exactly where an effect's is. Its meter pair is
  signal-flow evidence: left is the signal entering the hosted device and
  right is the signal leaving after host wet/dry and trim. A generator is the
  only exception: it has no input meter, only a generated output.
- Every gain trim is the same `TrimKnob` class: dB from unity, −60 dB (−∞) to
  +12 dB, double-click to 0 dB. No gain control reads in percent; dB is the
  unit the values actually mean.

The device rack is one of five **views**, and a view has exactly one toolbar
row, led by its slot's tab strip:

`[DEVICES NOTES PLAYLIST] | [DEVICE CHAIN] [source type] ··· [channel name field] [channel preset browser/actions]`

This used to be two stacked rows — a slot header carrying the switcher, the
channel name and the preset browser, and a device-chain row under it. They
merged because a slot header cannot hold per-view controls once a view can be
moved between panes, and because the shared row was already asking which page
was open in order to know whether to draw the preset browser, which is this
document's own stated symptom for a control in the wrong place.

The channel preset browser is on this view and no other. A channel preset is
the channel's sound, and this is the view whose subject is the channel's
sound; on the piano roll it was noise beside a stretch.

The channel name is a **field** here and a label on the piano roll, and that
asymmetry is the same rule: a name is edited where its subject is edited. A
track's name sits on its device face for the same reason, rather than on the
mixer strip, which stays compact. A bus showing in this slot keeps the label,
because its own face already carries the field.

**A rename field is fed one way and reports through `edited`.** A Slint
`TextInput`'s `text` is an ordinary property, so typing into it does not
update a binding — it replaces it. A field bound `text <=> input.text`
tracks what the application pushes right up until the first keystroke and
never again, which looks like "renaming is broken" while every store behind
it is correct. `NameField` therefore drives its input from a `changed`
handler and never writes back to `text`; `current-text` is what is actually
in the box, and is what a test should read.

### The back of a device

**Every device has a second face, and it costs nothing.** A face is a fixed
268 px by N units, and that rectangle is already allocated whether or not
anything is drawn in it. Turning a device over reuses it. `Mixer Strips`
below specifies the turn-over for the channel strip, where it was worked out
first; this is the same mechanism generalised, and the mechanism is the cheap
part.

Adam, 2026-09-10: *"its a whole space we could have for every device pretty
much at no cost... i dont know exactly what we'd spend it on yet but its
something we have."* So this section records the space and the rules that
keep it safe to use. **What goes on it is deliberately not decided here.**

**It is a space, not a picture of one.** Reason's rack back is a direct
emulation -- you turn the rack round and patch physical cables between real
jacks, and the skeuomorphism is the feature. That is not what this is for.
Nothing here needs to look like the back of anything; it is simply the other
side of a rectangle we are already paying for.

The rules that make it usable:

- **The back is for what is *set*. The front is for what is *played*.**
  Anything reached for while listening belongs on the front, and a device
  that puts a performance control on its back has mis-sorted it. Candidates
  for the back are per-device configuration that does not earn front-panel
  space, macro controls, and options that change what the front face *means*
  rather than what the signal does.
- **A back-face control is not a hidden control, because a face is only a
  drawing.** A parameter's identity is its `ParamDescriptor`, and automation,
  modulation and presets address it by id through `EffectKind::descriptors`
  -- none of which knows or cares which face draws the knob. So moving a
  control to the back costs it nothing in addressability: it stays
  automatable, modulatable, and saved. This is the property that makes the
  back cheap to spend, and it is worth not breaking.
- **A device with nothing on its back does not grow a turn-over button.** The
  affordance is evidence that there is something behind it. A rack where
  every device offers a turn and most of them turn to nothing teaches people
  not to turn any of them.
- **A face that is not showing is not built.** An `if`, not an `opacity: 0`,
  exactly as the mixer's back face already requires -- a rack holds many
  devices and an unbuilt face is what keeps the second one free.
- **The turn is per device, not global.** Same reasoning the mixer strip
  gives: you turn one device over to compare it against what its neighbours
  are doing on the front.
- **The identity header survives the turn.** A face sharing nothing with the
  one it replaced reads as a different device arriving rather than as this
  device, turned round.
- **The transition spends `Motion.duration` and `Motion.curve`**, like every
  other animation in the rack. Slint 1.17 has no 3D transform, so a literal
  card flip is an x-scale through zero with the faces swapped at the
  midpoint; a cross-dissolve with a small slide is equally available.

**The first real candidate, when one is wanted:** the preamp's
spectrum-deviation display, and anything else that answers "what is this
device doing to my signal" rather than "what do I want it to do". A meter is
not a control, it wants room, and it is exactly the kind of thing a front
face cannot afford at 1U.

### Piano roll gestures

The roll's header reads left to right as pane, mode, grid, then selection:

`[DEVICES NOTES PLAYLIST] | [SEL DRAW PAINT SLICE ERASE] [SNAP] [interval] | [tick/note/vel] [length] | [VEL AUTO] ··· [channel name]`

Length is a musical division, not a tick count, and setting it applies to the
whole selection. Beside the picker is a readout of the exact value, because an
unsnapped drag can land on a length no division names and the picker alone
would hide it.

The tool leads because it changes what every other gesture means. The row
clips rather than widening the window; keys 1-6 reach the tools and the snap
toggle when it is narrow.

| Gesture | Result |
| --- | --- |
| Drag a note's body | Moves the whole selection by that delta |
| Drag either note edge | Changes every selected note's length by that delta |
| Alt + drag a note edge | Stretches the selection in time about its other edge |
| Drag empty grid (Select) | Marquee, catching what it overlaps |
| Double-click empty grid | Creates a note and drags its length |
| Right-drag | Erases what it crosses, in any tool |

One drag is one undo step regardless of how many frames it took.

Modifier roles are remappable in Preferences > Shortcuts rather than fixed in
the grid, because a window manager can claim a chord and leave the gesture
dead with no visible cause. The roles compose — holding copy and snap
override together does both — and where two overlap by design, the more
specific one wins.

Pressing a note that is already selected does not collapse the selection --
that press is nearly always the start of dragging the group. The collapse is
deferred to release and only happens if nothing moved. Selection is shown by
tinting the notes themselves, with no frame drawn around them: a frame has to
be drawn somewhere, and where it was drawn was on top of the notes at the
selection's edges.

### Playlist gestures

The playlist's canvas header is two strips over one timeline, and they are
separate because the two gestures are: a loop is a **section** and is dragged
out, a playhead is a **position** and is dragged along. One strip for both
would have made every drag ambiguous and forced a modifier onto whichever of
them lost the argument.

| Strip | Gesture | Result |
| --- | --- | --- |
| Loop strip (thin, top) | Drag | Loops every snap unit the drag crossed |
| Loop strip | Click | Loops the one unit clicked |
| Loop strip | Right-click | Clears the loop, points and all |
| Bar numbers | Click or drag | Moves the playhead, snapped |

Both ends of a loop drag snap down and the range runs to the end of the last
unit touched. That is what makes a click loop the bar clicked rather than
nothing, and it is why the strip needs no separate handles: the section is
re-dragged rather than resized.

The section is drawn in the strip whether or not looping is live, dimmed when
it is not, because switching a loop off keeps its points and a strip that went
blank would say otherwise. Only a live loop tints the lanes below.

The playhead is drawn whenever the playlist is in song mode, running or not.
It used to appear only while playing, which was defensible when there was no
way to move it and is not now: a position that can be aimed has to be visible
to aim.

### Channel modulation shelf

**Its location is under review as of 2026-09-05.** Adam wants the modulation
rack moved and redesigned into its own panel; `reference/img/mooloop-1.0-mockup.png`
puts it on the right with its own tabs. `FOCUS.md` parks the move until one
question is settled -- whether the mockup's tracker and `IDEAS.md`'s automation
tracker are one design or two -- because the relocation is a layout and that is
not.
What follows describes where it is today and, more usefully, the two rules that
a move must carry with it — one shelf for the whole channel, and no fixed row
of permanent empty slots.

The channel's modulation shelf lives immediately below the device rack and is
collapsed by default. It is pinned to the bottom of the editor dock rather
than living inside the rack's horizontal scroll: it is one surface for the
whole channel, so following the chain's width put its module grid and Assign
button off-window once the chain grew past a few devices. Its header is a small `MOD` affordance; opening it shows
existing source chips and an add-source action. It is one shelf for the whole
channel, so a source can target a source parameter, any insert, and the strip
at the same time. Do not place a fixed row of permanent empty slots in the
rack or a separate modulation page inside every device: the grid's rows follow
the capacity constant and scroll, so the number is not a layout decision.

Selecting a source tile opens its larger control surface without changing what
ordinary parameter gestures mean. A separate **Assign** switch arms the
selected source. Legal destination controls then receive a subtle assignable
state, and dragging a normal control creates or changes route depth without
changing that control's base value. Its normal value display remains the base;
an overlay or second arc communicates modulation excursion. Switching source
tiles while Assign is active moves the assignment focus to the new source;
turning Assign off restores base-value editing. A small marker on a parameter
opens its incoming-route inspector. The inspector is destination-first and
should be sufficient for ordinary review and removal.

Both the compact tile and expanded source face are parameter-derived previews,
not generic type icons. An envelope face follows its effective attack, decay,
sustain, release, and amount (including tempo-synced stage durations); a
zero-time attack therefore has a vertical leading edge while a long attack
has a visible ramp. An LFO face follows waveform, phase, amount, fade-in,
smoothing, and pulse width. These are deterministic previews of the configured
signal, not phase-locked telemetry from the audio thread.

A source's own signal inputs belong on its expanded control surface. For an
LFO this begins with `Reset: Free | Note On`. The gate-driven envelope exposes
an explicit channel-note input picker, so `Kick notes → Envelope → Sampler
position` is possible before generators publish typed outlets. The channel
choice is an adapter for the future `Kick / Gate` outlet, not a competing
routing language. Input selection is intentionally different from Assign: the
input picker determines what drives the source, while Assign determines where
that source's output goes.

Sync-capable source timing knobs use the compact `O.` pattern: the knob is the
circle and a clickable LED immediately to its right selects transport sync.
When the LED is dark the knob reads continuous time or frequency; when lit it
steps through musical divisions from `4/1` to `1/64T` and shows the division
in the same value field. LFO rate/fade-in and envelope attack/decay/release use
this pattern. Smoothing and square-wave pulse width remain ordinary continuous
controls; pulse width is visibly disabled when another waveform is selected.

The compact rack does not require drawn patch cords. A later expanded graph may
draw and edit the same typed inlet and destination edges when that makes a
complex patch easier to read; cables are a visualization of existing routes,
not a separate engine or a replacement for the rack.

### Sampler

- `Sample` keeps file navigation, waveform, trim/loop markers, root note,
  reverse, loop mode, and tuning together.
- `Voice` keeps playback/retrigger/polyphony/choke behavior beside the
  amplitude envelope.
- `Tone` keeps filter/drive and lo-fi processing together.

### Drum synth

- Drum family and character remain one-click selectors on the face.
- Shared controls, a voice-shape display, and the selected voice's parameters
  fill one stable face without internal paging.
- Kick, snare, and hat use the same outer geometry even though their parameter
  counts differ.
- The voice-shape display is a deterministic preview rendered through the
  production drum voice and reduced to waveform min/max bins.

### Synth faces

The four synths do not share a layout, because they are not the same
instrument. What they share is the rule below them: waveforms and filter
models are visible selector banks rather than dropdowns, and every oscillator
plot responds to waveform, tuning, level, and pulse width.

- **v1 mono and v1 poly** page as `Osc` / `Amp/Filter` / `Mod`, with poly
  adding `Voice`. `Osc` uses three repeated oscillator strips with identical
  geometry; `Amp/Filter` pairs the graphical amplitude envelope with filter
  and drive.
- **ML-M1** pages as `Osc` / `Amp/Filter` / `Perf`. `Perf` is the page that
  makes it a distinct device: note priority, legato and glide, and accent.
- **ML-P8** pages as `OSC` / `NETWORK` / `FILTER` / `AMP` / `ML-P8 MOD`. It was
  one screen until 2026-09-04, and that is the layout lesson worth keeping:
  sixty-nine parameters on one 884x240 face fit only at a 20px dial and a 9px
  caption, which is a smudge on a 14" laptop. **A face that fits by shrinking
  its controls has not fit.** Pages cost a click and buy a dial you can read.
  NETWORK is the source-by-destination grid with a page to itself — rows are
  sources, columns the oscillators they reach, the diagonal is an oscillator
  on itself, and a MIX column carries the levels, because a level is a route
  to the output. Its cells are `ParameterKnob` with `show-dial: false`, not a
  second draggable control, so arming a modulation source changes what a cell
  means exactly as it changes what any other knob means. On AMP, eight is
  instrument information rather than a control — nothing on the face can
  change the pool — so the number that moves beside the Unison selector is the
  derived one: how many notes are left to play.
- **`KnobStack` is the knob a device page reaches for**, and it exists because
  the other two each give up one half of what a page needs. `ParameterKnob`
  stacks a caption and a value around a large dial, but the value is
  read-only. `KnobField` makes the value typed into, but lays label, dial and
  field in a *row* — 130px wide once the dial is legible, so four of them do
  not fit in a quarter of a face. `KnobStack` is stacked *and* typed into, and
  its dial is a real `ParameterKnob` with its own captions turned off, so
  there is one implementation of what a drag, a wheel, a double-click and an
  armed modulation source do. A timing control adds `show-sync` for the shared
  `O.` LED rather than dropping to a smaller widget for it.

No synth face owns a general LFO page. The common frame exposes the channel
modulation shelf and the routes that terminate in that device's parameters.
The v1 mono and poly faces still carry device-local LFO controls; those are
transitional and must migrate to the channel rack rather than grow into a
second modulation system. A synth's *authored* modulation — per-voice
envelopes, the ML-P8's oscillator network — is part of its synthesis contract
and stays where it is.

### Response displays

- Filter plots use the state-variable filter's cutoff, resonance, and
  bilinear frequency mapping. The reusable display supports low-pass,
  band-pass, and high-pass even while current instruments expose low-pass.
- Envelope-modulated filters show the base response and the response at peak
  envelope depth without implying that the second curve is a separate filter.
- Lo-fi plots apply the same rounded bit-depth and sample-hold mappings as the
  sampler DSP.

## Mixer Strips

The mixer is a row of fixed-format strips that scroll rather than compress.
`docs/MIXER_PLAN.md` owns what a strip contains; these are the rules that
decide its shape. Settled 2026-09-10.

- **A mixer strip is 92 px wide, from one named metric.** Not 62, and not a
  literal in four files. The width is set by the widest row that must not
  wrap, measured in real controls: an EQ band is freq / gain / q, a `MiniKnob`
  is 22 px, and three of them with gutters need 74 px of content. 62 px leaves
  54 px, which is two knobs, which is why the same section used to need a
  wider face than the one it lived on. Pick a strip's width by doing that
  arithmetic, not by choosing a number that looks narrow and paging around it.
- **A strip that needs more controls gets another face, not more height.** A
  strip's height is its fader's, and the fader is the one element on it with a
  floor -- so an area that toggles open below the fader spends the only
  dimension that cannot give. Turning the strip over changes nothing's size.
- **Where there is height, the strip stops paging -- and that is a
  measurement, not a control.** Added 2026-09-12. `MixerMetrics.full-height`
  is what the whole arrangement needs, stated as the sum of the parts the
  paged face already has to name, and a pane with that much room draws drive,
  EQ, comp, sends and then the meter and fader with the destination under
  them, in the order the mockup stacks them, with no arrows because there is
  nothing left to turn to. Adam: *"i'd like this to basically just be
  responsive design -- if the mixer is big enough, it shows everything."* The
  interface has no zoom for this and no mode to be in: the same pane in the
  same place shows more because it is taller, which is the one form of
  progressive disclosure that costs a user nothing to discover.
- **There are three faces and the fader is the middle one.** Sends to the
  left, the strip to the right, reached by a `‹` and a `›` in the strip's
  bottom row. Two, not one: a single cycling button makes the user press it
  and find out, where a pair says which way each goes before it is pressed.
  The arrow of the face being shown **becomes a dot** -- the same idle-dot
  idiom as an in-and-out switch -- so the row reads as a position indicator
  rather than as two buttons that might both do something.
- **The turn-over controls are per strip, not global.** The reason to look at
  one track's EQ is usually to compare it against what its neighbours are
  doing, and a mixer that turns over all at once takes that away.
- **The name, meter, level, pan, solo, mute and polarity survive the turn.**
  An EQ is set by ear while watching what it does to the level, and a face
  sharing nothing with the one it replaced reads as a different panel arriving
  rather than as this strip, turned round. The four small controls live in one
  column beside the fader -- pan above solo, mute, polarity -- which is what
  buys the room to keep them on every face rather than only on the front.
- **The mixer's strip face carries no scopes.** The EQ curve and the
  compressor's transfer are on the rack row's wide face, where there is width
  to draw them at a size worth reading. A 84 px plot is a decoration that
  costs the control under it; leaving it out is most of why the three
  sections fit stacked with no tabs. Adam, on the mockup: *"no scopes on the
  mixer -- that's partly the point."*
- **A plot does not have to live in the section it draws.** On the rack row
  the EQ's response sat above its own four band rows and did not fit: 72 px
  of plot and 200 px of rows in a 224 px face drew the fourth band below the
  panel. The plot moved to the column under DRIVE, whose one knob and voicing
  bank leave most of a column empty, and got taller in the move. What decides
  where a display goes is where the room is, not which heading it belongs
  under -- a section owns its *controls*.
- **Size a panel from its contents, not from a share of the slack.** Two
  panels on `horizontal-stretch: 1` split what is left over, which gave the
  strip's EQ 232 px and its compressor 212 px for clusters 96 px and 76 px
  wide. The knobs were centred and still read as misplaced, because a small
  huddle in the middle of a box twice its width looks like a mistake wherever
  it actually sits: Adam, *"the area itself seems pinned to the left of the
  channel strip, so the buttons are off center."* Measure the widest row that
  must not wrap, spell that as the width, and give the stretch to the one
  panel that has something to do with extra room -- here the compressor,
  whose curve widens.
- **A face that is not showing is not built.** An `if`, not an `opacity: 0`:
  exactly one of the three is constructed at a time, which is what keeps the
  other two free on every track in a large project.
- **No parameter is reachable from only one presentation.** The paned strip's
  strip face, the track's pinned row in the device rack, and the zoomed console
  strip are one parameter set drawn three ways. A control that exists in only
  one of them makes the mixer's own state something a user has to manage
  before they can do the work. Built 2026-09-11 for the first two, and
  2026-09-12 for the third, which needed nothing new as
  `docs/plans/archive/console/00-status.md` predicted -- it is the same four
  components in a taller column, and it arrives by the pane being big enough
  rather than by a zoom.
- **A strip control declares no range of its own.** Every knob on the channel
  strip takes its minimum, maximum, default, curve, name and unit from the
  descriptor table the engine reads, handed to the markup once at startup as
  the `StripSpec` global -- so no control on the face has a range to drift
  from. This is the stronger version of what `slint_face_agreement.rs` does
  for the device faces, which mirror a range and are checked for divergence
  afterwards; here there is no second copy to check. `tests/strip_face.rs`
  holds the table it installs to `StripParams::descriptors()` and fails if a
  bound is ever spelled on a control in `strip.slint` "to make it clearer".

  What a face may still hold is a *display's* convention, and the distinction
  is worth keeping: `EqResponseDisplay` reports a dragged point normalized
  over its own axes, so the markup inverts those axes to turn one back into
  hertz and decibels. That is not a parameter range and should not be handed
  over as one -- the frequency axis is deliberately wider than any single
  band. But such a number can *coincide* with a range or a floor, and then
  moving one silently stretches a drawing rather than breaking it, so the
  coincidence is asserted rather than commented on.
- **Size says importance.** On a strip's sections the control a user reaches
  for first is drawn larger than its neighbours: an EQ band's gain over its
  frequency and Q, the compressor's threshold and ratio over its attack,
  release, knee, mix and makeup. Two knob sizes, from two named metrics, and
  the row order matches -- the big one first. This is the only ranking the
  face gets, and it is why no section needs a heading per control.
- **Draw the thing when the thing is a shape.** Each EQ row is identified by
  a bell, a high shelf or a low shelf, not by the letters `HS`, `HM`, `LM`,
  `LS` -- and the band's own type switch toggles between the two shapes it can
  be, which is the same drawing pressed rather than a word. Where a control
  is a quantity and has no shape (a threshold, an attack) a single-letter
  caption stands in, and it is understood to be a mnemonic rather than a
  label: Adam, on the earlier `G F Q` header, *"the letters -- they're hard to
  read anyway, tooltip and statusbar are gonna be the user's friend
  regardless."* So the caption is never the only place a control is named --
  the tooltip carries the value and the status bar the explanation, per
  **Interaction And Wording** below -- and a caption that has to be taught is a sign
  the thing wanted an icon.
- **A stepped control shows a position, and the voicing owns what the
  position is worth.** The EQ's frequencies are 5, 7, 7 and 5 selectable
  positions per band, unlabelled, because a printed hertz value would have to
  be one voicing's -- and Iron wanting 2.2 kHz where Moo wants 2.5 kHz would
  make the label lie on three faces out of four. The project stores the
  position; `StripEqTable` turns it into hertz; the tooltip and the status bar
  say which hertz it currently is. This is the same rule as *a voicing selects
  laws, never values*, arrived at from the other end: the number the knob
  shows must be one no voicing can move.

  **A stepped control also takes a shorter throw.** Every knob in the app
  crosses its range in 150 px of pointer travel, which is right when there is
  a value to resolve between two settings and wrong when there is not: five
  frequency positions over 150 px is 37 px of drag for one of them, which
  Adam hit immediately -- *"it is too hard to move the freq knobs with the
  mouse pointer."* `MiniKnob.travel` is that distance, and a stepped
  parameter sets it to 14 px a stop off its own descriptor, so a band that
  gains a position gains the travel for it.
- **Where the strip's processing sits in a track's chain is one statement.**
  `mooloop_core::mixer::STRIP_PIN` decides both when the engine runs it and
  where the rack draws its pinned row, so the drawing cannot say one thing
  while the audio does another. The pinned row has no rails, because it can
  be neither inserted nor removed and offering those affordances would be
  offering gestures that do nothing.
- **The transition spends `Motion.duration` and `Motion.curve`.** They are
  what the device rack's slide-aside and the dock's extent already animate on.
  Slint 1.17 has no 3D transform, so a literal card flip is an x-scale through
  zero with the faces swapped at the midpoint; a cross-dissolve with a small
  slide is equally available. What is not available is a second set of timing
  numbers.

## Rack Actions

Add and remove are commands, not tall parameter modules. Present them as a
compact horizontal action strip or familiar icon buttons with tooltips. Their
dimensions must match the rack row/control scale. Never use two tall blank
columns with tiny `+` and `-` glyphs.

## Side Panels

Two panels flank the work area, and they are deliberately one mechanism: the
**channel sidebar** on the left and the **browser** on the right.

- **A panel is in flow, always, and hides by animating its width to zero.**
  Not an `if`: a panel that leaves the tree snaps the layout and cannot
  animate. The content sits in a clipped child so the panel can reach zero
  width without its controls spilling, and the resize grip stays *outside*
  that clip, because `clip` cuts pointer events along with pixels.
- **A grip rides the edge it moves.** Each pointer event folds its own offset
  into the current width rather than replaying from the press, and a bound
  that swallows a move re-anchors the grab — otherwise reversing direction
  spends the overshoot before anything happens. The left panel's grip widens
  rightward and the right panel's leftward; that sign is the only difference
  between them.
- **Two panels cannot each clamp against the whole window.** At 1000px a pair
  that each allowed itself 400px would leave 200 for the editor, so each
  measures its ceiling against the window *minus its sibling*. That is one
  function, `sidebar-ceiling`, and not a constant.
- **What a panel shows is the selection, not a copy of it.** The channel
  sidebar edits whatever `selected-channel` names and holds no selection of
  its own. It reuses the rename callback the `DEVICES` toolbar already has,
  because a second way to rename a channel is a second thing to drift.
- **A control drawn for a setting that does not exist yet is disabled, and
  looks it.** The sidebar's MIDI rows are the standing example: MIDI is
  decoded and routed and configurable nowhere, so the rows say what the panel
  will hold without claiming to hold it. A disabled field mutes its value as
  well as its background — a greyed box with black text reads as live.

## Responsive Behavior

- Design the desktop module grid first, then define explicit narrow variants.
- At narrow widths, wrap whole modules to a new row. Do not squeeze labels,
  graphs, or buttons until text clips.
- Preserve module internals and control hit targets while wrapping.
- Horizontal scrolling is acceptable for a fixed-format instrument panel when
  wrapping would destroy comparison or alignment.
- Dynamic content must not resize toolbar, rack cells, knobs, or selectors.
- **Spend spare height on the control that has a floor, not on the one that
  scrolls.** A pane taller than its content has to give the surplus to
  something, and the default -- whichever child happens to carry the stretch
  -- is usually a scrolling page, which cannot spend it and turns it into
  empty space. State what a fixed page wants, let it stop there, and the
  slack goes to the fader, the meter or the plot that reads better bigger.
  The mixer strip had this backwards until 2026-09-12: a turned strip's
  sections page took the room and its fader stayed at its minimum.

## Toolbars

- **A view has exactly one toolbar row, and its slot's tab strip leads it.**
  A strip that exists only to hold a switcher is chrome; a toolbar carrying
  controls for a pane that is not showing is worse, because it looks like it
  applies to what is on screen; and *two* stacked rows, where the upper one
  has to ask which of the lower one's pages is open, is both at once. The row
  is 30px on `Theme.surface` with 24px controls, whichever slot the view is
  in — a pane's toolbar should not change shape when the pane moves.
- **The tab strip is the part of that row that never clips.** It is the way
  out of the pane. A dense row clips its own controls at a narrow width
  instead, which is what the piano roll's already did.
- **The status bar's layout chips read in the screen order of the regions
  they toggle**, by each region's left edge: the channel sidebar at `x 0`,
  the dock below it, the split at the divider, the browser at the right
  sidebar's edge. The row's arithmetic counts the chips rather than stating
  how many there are, because a fourth chip arriving is exactly when a
  hardcoded three stops being true — which is what happened on 2026-09-13. Their glyphs share one
  outline, and what separates them is **fill, not position** — a docked panel
  appears and disappears and is drawn solid; a split is two editors and is
  drawn as two empty halves. Two rules 2px apart are the same square at 16px.
- **A gesture with no affordance needs a menu that names it.** The tab drag
  and the double-click to zoom are both invisible, so a tab's right-click menu
  lists them: it is where the control says what can be done to it, and it is
  what lets the gestures stay gestures once they are learned. A menu row
  elsewhere is not a substitute when it acts on "the current thing" rather
  than on the thing under the pointer.
- **A view moves by dragging its tab, and a drop says which pane, not where
  in a sequence.** So the feedback is a tint over the target pane, where the
  device rack animates its rows aside — a reorder has to answer *where in the
  order*, a pane drop only *which pane*. A drag that is not allowed refuses at
  the grab rather than snapping back at the end.
- **A pane fills the window on a double-click of its active tab.** The
  maximise gesture a title bar has, on the control that names the pane, which
  costs no chrome at all — a button per slot would be three buttons for a mode
  entered rarely and left immediately. The state is not hidden: the zoomed tab
  takes the full accent, the other slots are gone from the screen, and the
  status bar says how to get back. `Esc` leaves, and loses to every dialog.
- **A view declares an intrinsic height or it stretches.** A device face is a
  fixed 268px, so `DEVICES` declares one and nothing else does; that single
  fact is what makes the dock's divider live on some views and not others.
  Do not write a condition naming a view where the view can state a fact.
- **A divider closes what it is dragged out of existence.** Both dividers use
  one idiom: a 1px line taking `Theme.focus` on hover, a grab zone beside it,
  moving-origin drag arithmetic because the grip travels with the edge it
  sets, and re-anchoring when a bound swallows a move. The vertical one adds
  double-click-to-even, and dragging it to either bound folds the split away.
- **A setting that belongs to a pane lives in that pane's header, once.** A
  control that has to ask which pane is open in order to know which value it
  is editing is in the wrong place -- that question is the symptom.
- **A row of buttons is for a set that is fixed and small.** A button per
  member of a set that grows -- one per instrument, one per effect -- does
  not survive the set growing. Use `PickerChip`; `WIDGET_INVENTORY.md`
  records what it is for.
- Two switchers for two panes look the same. Both are `SegmentedControl`.

## Interaction And Wording

- A knob's label and value drag the same parameter as its knob face.
- Selecting a modulation source only opens its editor. When its explicit
  Assign switch is armed, parameter dragging edits that source's route depth
  while preserving the parameter's base value; the affordance and resulting
  overlay must make this mode obvious.
- Familiar icon buttons receive tooltips; visible prose does not explain the
  interface.
- **A tooltip is a value or a label. Nothing else.** `"Freq"` and `"22 kHz"`
  are tooltips; *"Sweep the frequency — Ctrl+drag for slow"* is a status-bar
  hint. A tooltip that is a sentence covers the control beside the one being
  read.
- Long contextual detail belongs in the status bar, not a multi-line hover
  card. Two channels reach it, and which to use is decided by where the
  control lives:
  - **`StatusHint.show` / `.clear`** (`theme.slint`) for a control anywhere.
    A knob, fader, `ToolButton`, `ToggleButton`, `SelectorBank`,
    `StepperField` or `MenuField` publishes automatically: a knob sends its
    `tooltip` (which is why a knob's tooltip may be a sentence — it is never
    rendered as one, the value is), and a button-shaped control sends its
    separate `hint` while its `tooltip` stays the label.
  - **`root.hover-hint`** for the surfaces defined in `main.slint` itself —
    the playlist, the piano roll, the automation lane — which can write the
    window's own property directly and take priority over the global.
  Do not thread a `hover-hint` property up through a device face. That was
  tried, exactly one face grew it, and every other sentence in the program
  stayed in a tooltip.
- **A mode or a preset that goes for a familiar sound is named for the sound,
  not for the hardware.** Adam's instruction, 2026-09-09, naming the channel
  strip's four voicings: *"dont reference these by name, can use a 'character
  name' that sorta describes what we've gone for in the sound."* So the strip
  mode reads `Moo / Grip / Punch / Iron` rather than naming three consoles.
  This is a standing rule for shipped strings, and it is also the more useful
  label: a character name says what to expect from a control the user has not
  used before, where a brand name only helps someone who already owns one.
- The first click acts. Focus acquisition must not consume it.

## Agent Acceptance Checklist

Before committing UI work, answer all of these:

- Can every visible control be assigned to a named owner and module?
- Do module outlines form clean rows or a clean grid?
- Is any bordered region mostly empty?
- Is a dropdown representing six or fewer fixed choices?
- Would a fader, graph, or meter use the available area better than another
  knob?
- Do repeated modules use the same dimensions and control order?
- Are graph points on their rendered lines?
- Are labels, centers, values, and dividers aligned?
- Does the desktop view look composed at 960x760 and at the real app width?
- Does the narrow view wrap or scroll without overlap or clipped text?
- Does every device retain the fixed rack height and an intentional unit width?
- Is signal order legible without opening a menu or inspector?
- Can a reader tell which devices receive modulation, open the channel shelf,
  and inspect a destination's incoming routes without treating a source as a
  property of one device?
- Was the result inspected from a software-rendered screenshot rather than
  accepted from code alone?

If any answer is wrong, the UI is not done.
