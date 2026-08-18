# Channel Buffer Engine

Status: product and architecture hypothesis. Not yet approved as a permanent
engine contract.

## Thesis

Each channel owns musical audio memory, not merely an input instrument and an
output mixer strip. A sound source can write into a bounded working buffer;
sequenced playback heads can immediately read regions of that buffer; parameter
events can move record and playback behavior with the same precision as notes.

This should make a short loop of actions possible without stopping transport:

```text
play -> catch -> retrigger -> offset -> reverse -> overwrite -> repeat
```

The point is not automatic freezing. The point is that retained audio is a
normal, visible part of the channel's state and can participate in composition.

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
hypothesis is that source, retained buffer, sequencer, parameter lanes, and
channel routing can be one coherent everyday workflow instead of separate
record, render, edit, and reload modes.

Primary references:

- https://docs.cycling74.com/legacy/max7/refpages/buffer~
- https://docs.cycling74.com/legacy/max7/refpages/groove~
- https://www.elektron.se/wp-content/uploads/2024/09/Octatrack-MKII-User-Manual_ENG_OS1.40A_210414.pdf
- https://www.native-instruments.com/ni-tech-manuals/maschine-software-manual/en/sampling-and-sample-mapping.html
- https://tutorials.renoise.com/wiki/Render_or_Freeze_Plugin_Instruments_to_Samples

## Proposed Channel Model

```text
timed events
    |
    +-> source instrument -> source bus ----+----> source monitor
                                           |
                                           +----> record head
                                                     |
                                                     v
                                              working buffer
                                                     |
timed events --------------------------------> buffer voice(s)
                                                     |
source monitor + buffer voices ----------------------+
                                                     v
                                              insert chain
                                                     v
                                                strip mix
```

The source and buffer paths are distinct even when they share a channel. A
monitor mode makes their relationship explicit:

- `Source`: hear the generator while retaining or recording audio.
- `Buffer`: hear only playback heads reading retained audio.
- `Layer`: hear both intentionally.

This avoids accidental feedback and makes capture state understandable.

## Working Buffer Semantics

The first implementation should have one fixed-capacity stereo buffer per
spiked channel, allocated off the audio thread. Capacity is a project or engine
budget, not an unbounded user allocation.

Required state:

- Empty, armed, recording, and retained states.
- A visible write head and one visible playback region.
- Capture start and length in musical ticks, with a free-time option deferred.
- Explicit source tap. The spike starts pre-insert; later post-insert capture
  must be a deliberate routing choice.
- Defined behavior when a read head reaches a region being written. The spike
  should forbid ambiguous overlap or specify a one-block-old read snapshot.
- Clear, replace, and snapshot operations that never free large memory on the
  realtime thread.

The buffer contents are working audio. Saving a project writes retained buffers
as project-owned WAV assets and references them from the text project file.

## Sequencer Contract

Notes and parameter events may target either the source or buffer playback
voice. The first buffer voice needs:

- Region start and end.
- Playback rate or pitch.
- Forward and reverse direction.
- One-shot and loop behavior.
- Velocity or gain.
- Note duration and release behavior.

The shared parameter system should later expose record enable, write position,
feedback/overdub amount, region selection, read offset, rate, direction, and
loop boundaries. Buffer-specific lanes must not become a second automation
engine.

## Realtime And Memory Constraints

- Allocate and resize buffers only off the audio thread.
- Record with bounded writes and no locks.
- Publish snapshots or replacement buffers through lock-free pointer swaps.
- Defer destruction until the realtime graph and all voices release old data.
- Make the memory budget visible. At 48 kHz, one minute of stereo `f32` audio
  is about 22 MiB; multiplying that silently by 16 channels is unacceptable.
- Offline and realtime render paths must use the same buffer behavior.
- Project save must snapshot coherently without blocking the callback.

## Bounded Spike

Build the spike on one channel before changing every strip:

1. Capture one beat or bar from the current channel source while transport
   runs.
2. Switch explicitly between Source, Buffer, and Layer monitoring.
3. Trigger the retained region from the existing note/event pipeline.
4. Sequence region offset, playback rate, and reverse.
5. Clear or recapture without stopping transport.
6. Save and reload the retained audio with a minimal project document.

Use the existing sampler as the first source if that reduces implementation
risk. A small percussion generator can follow only if it is needed to prove
that generated audio and retained audio form a useful loop.

## Success Test

Keep the design only if a user can make an audible source, capture it, and turn
it into a recognizably different rhythmic part in under a minute, without file
dialogs or leaving the pattern workflow.

It must also pass these engineering tests:

- No allocation, locks, I/O, or large-object destruction in the JACK callback.
- Deterministic capture boundaries across varying block sizes.
- Defined results for transport stop, recapture, tempo change, and project
  reload.
- Buffer contents and monitor mode are always visible.

Reject or revise the thesis if the result is merely a slower version of
render-to-sample, if users cannot tell which signal is audible, or if persistence
and memory costs dominate the musical benefit.
