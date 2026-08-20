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

Reference examples are in `reference/img/helm.webp`, `malstrom.jpg`,
`phase4.png`, `polysynth.png`, `subtractor.jpg`, and `thor.jpg`. They are not a
palette or skin specification. Do not copy their branding, skeuomorphism, or
color. Study their grouping, density, control choice, and use of area.

Annotated failure cases are in `reference/img/mooloop-1.png` through
`mooloop-5.png`. Treat the annotations as bug reports.

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

## Source Editor Layout

### Header

The lower editor begins with one 30-36 px channel row:

`[Source | Notes | Playlist] [channel name] [channel preset browser/actions]`

The generator begins with one 30-36 px row:

`[source type selector] [generator preset browser/actions]`

These controls stay attached to what they operate on. There is no duplicate
preset browser at the opposite edge of the panel.

### Sampler

- The waveform owns the wide upper module.
- Tune, voice, filter, amp envelope, and lo-fi form one aligned lower module
  row that fills the available width.
- Voice and retrigger use visible mode selectors when the option count remains
  small.
- The amp graph and A/D/S/R controls form one module.
- Faders are preferred where they make the lower row fill cleanly and improve
  comparison.

### Drum synth

- Drum family and character selectors occupy one compact header row.
- Shared controls and voice-specific controls form a rectangular grid with a
  common outer width.
- A sparse voice such as hat must use an intentional module composition; it
  must not become a narrow card followed by empty framed area.
- Character controls are one-click selectors, never dropdowns.

### Mono synth

- Oscillators use a repeated module template with identical geometry.
- Waveform is a visible selector bank, not a dropdown.
- Amp, filter, and LFO modules align to the oscillator grid.
- Sparse modules expand a useful graph/control or share a row; they do not end
  in blank bordered rectangles.
- LFO waveform is a visible selector bank.

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
- Was the result inspected from a software-rendered screenshot rather than
  accepted from code alone?

If any answer is wrong, the UI is not done.
