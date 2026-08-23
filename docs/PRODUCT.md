# Product Definition

Status: working definition, August 2026.

This document defines what mooloop is trying to become. It should be updated
when product decisions change. It is not a claim that every described feature
already exists; implemented behavior is recorded in `CURRENT.md`.

## Product Statement

Mooloop is a Linux-native pattern instrument for making rhythm-centered music
with samples, generated sound, and retained audio buffers. It should preserve
the speed of an early groovebox while allowing precise timing, detailed note
data, resampling, and destructive-sounding transformations without requiring a
full DAW workflow.

The target musical territory includes IDM and glitch, industrial and
post-industrial beats, hip-hop and related urban production, and drum and bass.
That is a design constraint, not a genre lock.

## The Core Promise

A useful beat should be possible in under a minute. The same beat should then
be able to become structurally and sonically strange without leaving the main
instrument.

The workflow is:

1. Load or generate a sound.
2. Sequence it immediately in a pattern.
3. refine pitch, velocity, duration, timing, and other note data in context.
4. Capture channel audio into retained memory and sequence its playback or
   mutation.
5. Combine variable-length patterns into a song.
6. Export audio or continue work through JACK in another tool.

## Product Pillars

### Immediate Pattern Construction

The rack remains the fastest entry point. A step is never only an on/off bit:
velocity and timing must remain visible enough to shape a groove without
opening a separate editor for every change.

### Expressive Time

The grid is a reference, not a verdict. The event model must support at least
64th-note placement, duration, note-off, microtiming, and consistent lane or
track offsets. Random humanization is not a substitute for authored feel.

The step rack summarizes denser events. For example, a sixteenth-note cell can
show four occupied or empty 64th-note subdivisions while the piano roll edits
the actual events.

### Channels As Device Chains And Memory

A channel is an ordered device chain: one sound source followed by insert
devices and a mixer output. A retained-audio buffer is one insert device, not a
mandatory stage allocated in every channel. It can therefore capture the
signal at any musically useful point in the chain and can be omitted where it
is not needed.

In the buffer device's ordinary state, its read head follows the write head and
the device behaves like a transparent bridge. Patterns and automation can make
that head jump into recent history, change direction or rate, repeat a window,
or otherwise treat the signal reaching that device as sampled material in
realtime.

Controls operate on PCM sample frames, not opaque encoded file bytes. The
source already has to produce discrete samples for the audio graph; the working
buffer keeps those samples addressable after the current process block would
normally be discarded.

This is the proposed differentiator. It is still a product hypothesis and must
pass the insert-device spike in `BUFFER_ENGINE.md` before the buffer contract
is treated as permanent.

### Sample-Centric, Not Sample-Only

The sampler is a first-class instrument, not a file player. It needs the voice,
loop, envelope, pitch, filter, and coloration behavior expected from a serious
groovebox sampler.

Synths are sound sources that participate in the same channel and buffer model.
The first synths should be chosen because they create useful material for
continuous capture and manipulation, not to fill out a conventional workstation
feature matrix.

### One Automation Language

Velocity, pan, sample start, pitch, buffer position, effect parameters, and
future destinations should share a stable parameter-target system. The lower
parameter lane can present unipolar or bipolar values and precise editing; the
sequencer delivers changes sample accurately.

### Pattern First, Song Capable

Patterns have independent lengths. A playlist placement starts a pattern on a
shared song timeline and lasts for that pattern's natural duration unless the
placement explicitly overrides it. Different lengths therefore need no special
case: they may end at different times, cross bar lines, and overlap other
placements.

Pattern mode loops the selected pattern. Song mode schedules placements. Song
loop boundaries are independent of pattern lengths.

### Trustworthy Machinery

Audible instability is a musical option; application instability is not. The
audio callback remains bounded, allocation-free, lock-free, and free of I/O.
State must be visible, undoable where practical, and recoverable after a crash.

## Product Scope

Mooloop should include:

- A channel rack for fast pattern entry.
- A piano roll and aligned parameter lanes for detailed event editing.
- Variable-length patterns and a layered pattern playlist.
- Pattern and Song transport modes with explicit loop ranges.
- A capable sampler and a small set of authored synthesis sources.
- Insertable retained-audio buffer devices with sequencable capture and playback.
- Parameter automation, channel inserts, sends, groups, and useful routing.
- Project and kit persistence with ordinary audio assets.
- Offline WAV rendering and JACK output. Compressed export can be added through
  a proven encoder after WAV rendering is correct.
- Keyboard-driven editing alongside direct mouse manipulation.

## Non-Goals

Mooloop is not trying to be:

- A replacement for REAPER or a general linear recording DAW.
- A clone of FruityLoops, Reason, Maschine, Bitwig, or any one reference UI.
- A plugin host before its own instrument and sequencing model is coherent.
- A modular patching environment as broad as Max/MSP.
- A collection of unrelated effects and synths added to satisfy a checklist.
- A machine that calls random timing jitter "humanization."

## Working Decisions

These are firm enough to build against:

- Native Linux is the primary platform.
- Rust, Slint, and JACK/PipeWire remain the implementation stack.
- Internal musical time uses PPQ ticks. The first UI assumes 4/4, but stored
  note placement is not limited to one sixteenth-note slot.
- The minimum serious placement resolution is 64th notes. The PPQ value of 96
  already represents them exactly at six ticks each.
- Note duration and note-off semantics precede polyphonic synth work.
- Effects and other automation use stable parameter IDs and the existing timed
  event path rather than separate per-feature sequencers.
- Project files should be versioned, inspectable, and git-friendly. Audio
  buffers belong in referenced or project-owned audio files, not encoded into
  a text document.
- WAV is the canonical render target. MP3 is a delivery format, not an engine
  primitive.
- Effects use an ordered device chain and stable parameter IDs. Dynamic musical
  items must not receive small product caps: any realtime storage is prepared
  off the audio thread and bounded only by an explicit protocol or safety
  boundary, documented at that boundary.
- Buffer capture position is determined by where its device is inserted; it
  does not require a separate fixed channel tap point.

## Open Product Questions

These should be answered by prototypes or current UI design, not guessed from
old screenshots or personality notes:

- Is the channel buffer continuously rolling, explicitly recorded, or both?
- Which tap points are essential: source, pre-insert, post-insert, group, or
  master?
- Does the initial buffer voice replace the source, layer with it, or expose
  both as explicit monitor modes?
- How many simultaneous buffer playback heads are musically necessary?
- Which operations must be nondestructive, and which should deliberately
  modify working audio?
- How much live performance behavior matters relative to composition and
  export?
- When should MIDI input and output enter the roadmap?

## Decision Precedence

Current direct feedback and current purpose-built designs outrank this file.
This file outranks old phase labels, competitor screenshots, generated taste
briefs, and speculative comments in source code.
