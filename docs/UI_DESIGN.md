# Mooloop Interface Design Language

Status: active design contract, August 2026.

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
4. Generator: source type, generator preset/actions, and generator modules.

A control belongs at the lowest level that owns its state. Generator preset
controls sit beside the source selector, not detached at the far edge of the
editor. Channel presets remain in the channel header because they include the
generator and channel-level state.

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
- Current source devices use 3U. An effect uses only the units its working
  controls require.
- The rack scrolls horizontally. Device internals never compress when the
  application narrows.
- Every device has a 28 px identity header with enabled state, name, kind, and
  size. The host's bypass and wet/dry controls occupy the header's right edge;
  a device face must not add a second copy. Effect faces inherit the shared
  `EffectDeviceShell`, which owns that header and the drag-to-reorder handle;
  a face file contains only its working controls. Controls unique to that device
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
  bypass, presets, insertion, removal, and reorder actions. Its meter pair is
  signal-flow evidence: left is the signal entering the hosted device and
  right is the signal leaving after host wet/dry and trim. A generator is the
  only exception: it has no input meter, only a generated output.
- Every gain trim is the same `TrimKnob` class: dB from unity, −60 dB (−∞) to
  +12 dB, double-click to 0 dB. No gain control reads in percent; dB is the
  unit the values actually mean.

The lower editor retains one channel row:

`[Source | Notes | Playlist] [channel name] [channel preset browser/actions]`

The device-chain row directly below it owns source type and generator presets:

`[DEVICE CHAIN] [source type] [generator preset browser/actions]`

### Channel modulation shelf

The channel's modulation shelf lives immediately below the device rack and is
collapsed by default. Its header is a small `MOD` affordance; opening it shows
existing source chips and an add-source action. It is one shelf for the whole
channel, so a source can target a source parameter, any insert, and the strip
at the same time. Do not place four permanent empty slots in the rack or a
separate modulation page inside every device.

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

A source's own signal inputs belong on its expanded control surface. For an
LFO this begins with `Reset: Free | Note On`; later, the same compact input
picker can list compatible generator, device, Buffer, and cross-channel
outlets such as `Kick / Gate`. This is selection over the channel control
graph, not a second device-local modulation system. It is intentionally
different from Assign: the input picker determines what drives the source,
while Assign determines where that source's output goes.

Sync-capable LFO timing knobs use the compact `O.` pattern: the knob is the
circle and a clickable LED immediately to its right selects transport sync.
When the LED is dark the knob reads continuous time or frequency; when lit it
steps through musical divisions from `4/1` to `1/64T` and shows the division
in the same value field. Rate and fade-in use this pattern. Smoothing and
square-wave pulse width remain ordinary continuous controls; pulse width is
visibly disabled when another waveform is selected.

This interaction has no drawn patch cords. A full route matrix or zoomed-out
graph is deferred expert tooling, not a replacement for the rack and not a
requirement for the first modulation UI.

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

### Mono synth

- `Osc` uses three repeated oscillator strips with identical geometry.
- `Amp/Filter` pairs the graphical amplitude envelope with filter and drive.
- A Mono face does not own a general LFO page. Its common frame exposes the
  channel modulation shelf and the routes that terminate in Mono parameters.
  Any existing device-local LFO controls are transitional and must migrate to
  the channel rack rather than grow into a second modulation system.
- Waveforms remain visible selector banks rather than dropdowns.
- Oscillator plots respond to waveform, tuning, level, and pulse width.

### Response displays

- Filter plots use the state-variable filter's cutoff, resonance, and
  bilinear frequency mapping. The reusable display supports low-pass,
  band-pass, and high-pass even while current instruments expose low-pass.
- Envelope-modulated filters show the base response and the response at peak
  envelope depth without implying that the second curve is a separate filter.
- Lo-fi plots apply the same rounded bit-depth and sample-hold mappings as the
  sampler DSP.

## Rack Actions

Add and remove are commands, not tall parameter modules. Present them as a
compact horizontal action strip or familiar icon buttons with tooltips. Their
dimensions must match the rack row/control scale. Never use two tall blank
columns with tiny `+` and `-` glyphs.

## Responsive Behavior

- Design the desktop module grid first, then define explicit narrow variants.
- At narrow widths, wrap whole modules to a new row. Do not squeeze labels,
  graphs, or buttons until text clips.
- Preserve module internals and control hit targets while wrapping.
- Horizontal scrolling is acceptable for a fixed-format instrument panel when
  wrapping would destroy comparison or alignment.
- Dynamic content must not resize toolbar, rack cells, knobs, or selectors.

## Interaction And Wording

- A knob's label and value drag the same parameter as its knob face.
- Selecting a modulation source only opens its editor. When its explicit
  Assign switch is armed, parameter dragging edits that source's route depth
  while preserving the parameter's base value; the affordance and resulting
  overlay must make this mode obvious.
- Familiar icon buttons receive tooltips; visible prose does not explain the
  interface.
- Tooltips name the musical result or action. They do not narrate the code.
- Long contextual detail belongs in a status bar, not a multi-line hover card.
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
