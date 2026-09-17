# 03 — Capture

## Needs an answer first

Open questions 3 (what a loop pass does) and 5 (where a take lives before the
project is saved) in `00-status.md`.

## Build

**On the audio thread**
- When the engine is armed, the transport is playing, and the input bus is
  routed to a channel, copy the input frames into a capture ring and nothing
  else.
- **The ring:** a `rtrb` of `[f32; 2]` frames. The engine handle allocates it
  at arm time, off the audio thread, sized for about ten seconds, and it
  reaches the renderer through a structural command. It returns through the
  reclaim ring on disarm.
- **Start event:** the first frame of a take emits
  `EngineEvent::CaptureStarted { channel, tick, frames }`. `tick` is the
  position in the pattern, computed the same way the MIDI record fix computes
  it (`fix/midi-routing-and-record-wrap`), so notes and audio agree on where a
  pass starts.
- **Latency:** subtract the driver's capture latency plus its playback
  latency from the start position, so a take lines up with what the
  performer heard. JACK reports both; Core Audio reports what cpal exposes.
  Record which latencies are and are not accounted for.
- **End event:** disarming, stopping, or (under the recommended answer to
  question 3) reaching the loop end emits `CaptureEnded { frames }`.
- **Overflow:** a ring the drain has not kept up with drops frames and counts
  them. The count reaches the session, and the take is marked damaged. No
  silent gaps (`AUDIO_ARCHITECTURE.md`: overflow is visible to the sender).

**Off the audio thread**
- A drain thread started at arm time pulls from the ring and writes a 32-bit
  float stereo WAV with hound, into the recordings folder that question 5
  settles.
- It also keeps a running peak summary, so the take's waveform can be drawn
  while it records without reading the file back.
- This thread is the only place that does file I/O for a take. It also
  finalizes the WAV header when the take ends.

## Test

- **Engine:** armed and playing, a known input yields the same frames in the
  ring, and a start event at the right tick on the first pass and the second
  pass. No allocations and no frees in any block.
- **Overflow:** a drain that never reads produces a counted overflow and no
  panic.
- **Drain:** frames written to the ring become a WAV whose samples match them
  bit for bit.
- **Offline:** exporting never arms capture.

## Docs

- `AUDIO_ARCHITECTURE.md`: the capture ring and its overflow rule.
- `OPERATIONS.md`: the recordings folder.
