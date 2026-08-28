# Channel Buffer Engine

Status: product and architecture hypothesis. Not yet approved as a permanent
engine contract.

## Thesis

A retained-audio insert device owns musical audio memory. Any channel may place
one in its ordered device chain, including before or after EQ, saturation,
delay, or other processing. The device continuously writes the PCM frames that
reach its input into a bounded circular working buffer. One or more read heads
immediately turn that recent history back into the device's output. Parameter
events can move those heads with the same precision as notes.

In normal operation, a read head follows the write head at a defined minimal
latency, so the stage behaves like a transparent connection. It becomes an
instrument when the head detaches: it can jump backward, reverse, change rate,
repeat a window, hold, scrub, or return to live following. Everything is
therefore sampled while it is generated without requiring a separate record
gesture.

This should make a short loop of actions possible without stopping transport:

```text
play -> hear live -> jump into history -> reverse -> repeat -> return live
```

The point is not automatic freezing or conventional render-to-sample. The point
is that short-term retained audio can be inserted at the useful point in a
signal chain and its read behavior can participate in composition. Channels
that do not use the device pay no buffer-memory cost.

The existing engine already renders instruments into an `f32` stereo bus for
each JACK block. That scratch bus is cleared on the next callback, so it only
contains the present. The proposed stage extends the lifetime of those same
sample frames in a circular buffer. It does not add a vector-to-bitmap
conversion or manipulate encoded WAV bytes.

## Adjacent Ideas

The ingredients are established:

- Max/MSP separates named `buffer~` memory from objects that record, read,
  loop, index, and transform it.
- Elektron Octatrack gives each track a recorder and recorder buffer that can
  capture internal or external signals and feed playback machines.
- Maschine can sample internal sources without stopping the sequencer.
- Renoise can render instruments, tracks, patterns, or selections to samples
  for tracker-style manipulation.

Mooloop should not claim to invent realtime resampling. The narrower product
hypothesis is that an always-running source buffer, sequencer, parameter lanes,
and channel DSP can be one coherent everyday workflow instead of separate
record, render, edit, and reload modes.

Primary references:

- https://docs.cycling74.com/legacy/max7/refpages/buffer~
- https://docs.cycling74.com/legacy/max7/refpages/groove~
- https://www.elektron.se/wp-content/uploads/2024/09/Octatrack-MKII-User-Manual_ENG_OS1.40A_210414.pdf
- https://www.native-instruments.com/ni-tech-manuals/maschine-software-manual/en/sampling-and-sample-mapping.html
- https://tutorials.renoise.com/wiki/Render_or_Freeze_Plugin_Instruments_to_Samples

## Proposed Device-Chain Model

```text
timed events -> source -> EQ -> buffer device -> delay -> strip mix
                                  |      |
                                  |      +-> read head(s) -> device output
                                  +--------> write head -> circular memory
```

The default read mode is `Follow`: the primary read head tracks newly written
samples and the device passes its input transparently at its defined latency. A
manipulation temporarily changes that relationship. `Jump`, `Loop`, `Reverse`,
`Rate`, `Hold`, and `Return Live` are read-head behaviors, not file-editing
modes. Ordinary insert bypass provides the safety comparison.

## Working Buffer Semantics

The first implementation should have one insert instance with a fixed-capacity
stereo circular buffer, allocated off the audio thread. Capacity is a project
or engine budget, not an unbounded user allocation. The write head advances
while input reaches the device and wraps without user intervention.

Required state:

- A visible write head, read head, history span, and active read window.
- Follow state versus detached/manipulated state.
- Read offset behind the write head, region length, direction, and rate.
- A sample-accurate `Return Live` operation.
- Defined behavior when a read head reaches the write head. The spike should
  keep a small protected distance or read a defined prior sample.
- Optional freeze, clear, and snapshot operations that never free large memory
  on the realtime thread.
- Explicit position in the ordered device chain. Moving the device changes
  what signal it captures without introducing a separate tap-point model.

The rolling history is working audio and need not all become a project asset.
If an arrangement depends on a frozen or detached region, project save writes a
coherent snapshot as a project-owned WAV asset and references it from the text
project file.

## Sequencer Contract

Notes continue to target the source. Parameter events control the buffer stage
and its read heads. A future channel may also trigger buffer regions as pitched
voices, but the first proof is manipulation of the continuously running signal.

The first read head needs:

- Follow or detached state.
- Offset behind the write head and window length.
- Playback rate.
- Forward and reverse direction.
- One-shot, loop, hold, and return-live behavior.
- Gain and short crossfades at discontinuities.

The shared parameter system should later expose read offset, window length,
rate, direction, repeat, hold, return-live, freeze, and possibly write feedback
or overwrite behavior. Buffer-specific lanes must not become a second
automation engine.

## Future Control Outlets

The first Buffer spike does not require modulation outlets. Once its read-head
behavior is musically proven, it may publish named, normalized channel control
signals such as playhead position, distance from the write head, window phase,
amplitude, transient state, or slice state. These are inputs to the
channel-owned modulation rack described in `MODULATION_PLAN.md`, not
device-local LFOs and not values sampled back out of the display.

Every outlet must declare its signal semantics, control rate, and latency; a
consumer reads it through the ordinary source-to-`ParamAddr` route path.
Waveforms, collision indicators, and other UI telemetry remain observation
only. Do not expand the first Buffer spike to implement this list.

## Realtime And Memory Constraints

- Allocate and resize buffers only off the audio thread.
- Record with bounded writes and no locks.
- Publish snapshots or replacement buffers through lock-free pointer swaps.
- Defer destruction until the realtime graph and all voices release old data.
- Make the memory budget visible. At 48 kHz, one minute of stereo `f32` audio
  is about 22 MiB; multiplying that silently by every addressable channel is
  unacceptable. Buffer devices are opt-in inserts, sized only when present.
- Offline and realtime render paths must use the same buffer behavior.
- Project save must snapshot coherently without blocking the callback.

## Bounded Spike

Build one insert instance before expanding the device count:

1. Continuously write the device input into a short stereo ring.
2. Pass live audio through a following read head with no surprising coloration
   or timing shift.
3. Detach the head and sequence read offset, window length, rate, reverse, and
   return-live actions.
4. Crossfade discontinuities enough to prevent accidental clicks while still
   allowing intentionally abrupt edits.
5. Freeze a useful window and restore it with a minimal project document.

Use the existing sampler as the first source if that reduces implementation
risk. A small percussion generator can follow only if it is needed to prove
that generated audio and retained audio form a useful loop.

## Success Test

Keep the design only if a user can make an audible source and turn its rolling
recent history into a recognizably different rhythmic part in under a minute,
without arming a recorder, opening file dialogs, or leaving the pattern
workflow.

It must also pass these engineering tests:

- No allocation, locks, I/O, or large-object destruction in the JACK callback.
- Deterministic write/read-head behavior across varying block sizes.
- Defined results for wraparound, read/write proximity, transport stop, tempo
  change, and project reload.
- Buffer history and head states are always visible.

Reject or revise the thesis if the normal Follow state cannot behave like a
trustworthy source-to-DSP bridge, if manipulation is merely a slower version of
a stutter effect, or if persistence and memory costs dominate the musical
benefit.
