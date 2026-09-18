# 03 — Capture

## Adam's answers (2026-09-17, amended 2026-09-18)

- **A take is started by the sampler's own record button**, not by the
  global record-arm (decision 7). Any number of channels may be recording at
  once (decision 6), each from its own audio input.
- **It starts on the next bar** (decision 9). If the transport is stopped,
  the record press starts it, and the take begins at the next bar line; the
  press-to-bar wait is the pre-roll.
- **It ends when it is stopped, or at its clip length** (decision 8). With
  clip mode off it runs until the record button is pressed again or the
  transport stops -- loop passes don't end it or split it, so it can be much
  longer than its pattern. With clip mode on it ends by itself, sample-exact,
  when its length has been recorded.
- **Takes go to a recordings folder** until the project is saved.

## Build

**On the audio thread**
- **A take is a per-channel state machine**, driven by a command from the
  sampler face: `Idle` → (record pressed) `Waiting { starts_at }` → (the
  block containing the next bar line) `Recording { frames, limit }` →
  (stopped, transport stopped, or `limit` reached) `Idle`. The bar line and
  the clip limit are both frame positions inside a block, so a take starts
  and ends sample-exact rather than on a block boundary. The waiting state is
  also what the face draws as the pre-roll.
- While `Recording`, copy the channel's source buffer -- whichever seat
  `AudioInputRouting` resolves that channel's audio input to -- into its
  capture ring, and nothing else.
- **The copy runs once, after the whole block has rendered**, not inside the
  strip order. Every source buffer -- `ChannelStrip.bus`, a track's
  `BusStrip.bus`, `master()`, and from step 01 the input bus -- still holds
  that block's audio at that point, so one read site serves them all and the
  audio graph's order does not change. That is the reason capture needs
  nothing from `AudioTapBank` or the outlet machinery.
- **Check where a channel's buffer sits relative to its compensation delay
  before writing the read.** A take of one channel and a take of the master
  should line up with each other; if `ChannelStrip.bus` is read before
  `compensation`, the channel's take is early by that many frames.
- **The ring:** a `rtrb` of `[f32; 2]` frames. The engine handle allocates it
  at arm time, off the audio thread, sized for about ten seconds, and it
  reaches the renderer through a structural command. It returns through the
  reclaim ring when the take ends. One ring per recording channel.
- **Start event:** the first frame of a take emits
  `EngineEvent::CaptureStarted { channel, tick, frames }`. With a bar-line
  start, `tick` is that bar line. `tick` is the
  position in the pattern, computed the same way the MIDI record fix computes
  it (`fix/midi-routing-and-record-wrap`), so notes and audio agree on where a
  pass starts.
- **Latency:** an app source has none to correct, because it is read in
  the same block that produced it. A hardware source (step 01) does:
  subtract the driver's capture latency plus its playback latency from the
  start position, so a take lines up with what the performer heard. JACK
  reports both; Core Audio reports what cpal exposes. Record which latencies
  are and are not accounted for. Only the start position differs between
  the two; the ring, the drain and the file are the same.
- **End event:** a stop press, the transport stopping, or the clip length
  being reached emits `CaptureEnded { channel, frames }`. The loop point is not an end: the ring keeps
  filling across the wrap, and the take is one continuous file.
- **Length:** because a take can run for minutes, the ring only has to cover
  the drain thread's worst stall, never the whole take. The file is what
  grows. At about 22 MiB per stereo minute at 48 kHz (`BUFFER_ENGINE.md`),
  loading a long take back as a sample is the real cost. Step 05 shows how
  long the take is while it records.
- **Overflow:** a ring the drain has not kept up with drops frames and counts
  them. The count reaches the session, and the take is marked damaged. No
  silent gaps (`AUDIO_ARCHITECTURE.md`: overflow is visible to the sender).

**Off the audio thread**
- A drain thread started at arm time pulls from the ring and writes a 32-bit
  float stereo WAV with hound, into the recordings folder: `<data dir>/recordings/`, one file per take,
  named by date and time plus the channel's name.
- It also keeps a running peak summary, so the take's waveform can be drawn
  while it records without reading the file back.
- This thread is the only place that does file I/O for a take. It also
  finalizes the WAV header when the take ends.

## What an install mid-take does to the ring

Named here because `reports/fable-2026-09-18.md` found the step did not say.
The ring reaches the renderer through a structural command, and a project
install replaces the whole renderer
(`crates/mooloop-engine/src/executor.rs:226`). Nothing in `InputState`
(`crates/mooloop-engine/src/lib.rs:574-586`) carries it, so **as written,
any structural edit during a take silently stops the capture**: the incoming
renderer has no ring, the drain thread sees a producer that never writes
again, and the take ends without a `CaptureEnded` — the failure mode this
plan's overflow rule exists to avoid, arriving by a different door. Today
every structural edit also stops the transport, so a take cannot survive one
anyway; `channel-identity/04` removes exactly that, which is when this
becomes reachable.

Two ways out, and this plan takes the second:

1. The ring travels in `InputState`. One more hand-carried field in the list
   that has needed patching three times this month (record arm, then routing
   twice), and it is the wrong shape besides: the ring belongs to a channel,
   and an edit that renumbers channels is precisely what the carry has to
   survive.
2. **Step 03 waits for `channel-identity/05`.** Once a strip is matched by
   `ChannelId` and moved into the new generation rather than rebuilt, the
   capture ring is per-channel state on a carried strip and survives the swap
   for the same reason the strip's delay tail does. A take then ends when the
   channel is deleted, which is the rule a user would predict.

So `channel-identity/05` is a prerequisite for this step, not only for
`plugin-hosting/06`. If step 03 is nevertheless built first, it must state
what an install does to an open take and make it a counted, visible end
rather than a silent one.

**Met, 2026-09-18.** `channel-identity/05` carries a channel's strip across
an install when its id and setup match, and `incremental-structure/02` does
the same for a track's. Hang the ring on the *recording* channel's strip.
The source can be a different channel or track, and the routing names it by
id, so a move of either end re-resolves the seat and the take keeps going.
Deleting the recording channel ends the take. Deleting its source should end
it too, visibly, rather than let it keep recording silence.

## Test

- **Install mid-take:** with `channel-identity/05` landed, a structural edit
  during a take carries the ring with the strip and the take continues across
  the swap, with no gap in the written file and no overflow counted.
- **Engine:** armed and playing, a known source yields the same frames in the
  ring, and a start event at the right tick on the first pass and the second
  pass. No allocations and no frees in any block. Run it for each kind of
  app source: a channel, a track and the master.
- **Resample equals render:** capturing the master for N blocks gives the
  same samples as an offline render of the same N blocks. That is the
  cheapest proof that the read site is in the right place.
- **Resample in place:** a sampler channel recording its own output keeps
  playing the old sample until the take ends.
- **Overflow:** a drain that never reads produces a counted overflow and no
  panic.
- **Drain:** frames written to the ring become a WAV whose samples match them
  bit for bit.
- **Offline:** exporting never arms capture.

## Docs

- `AUDIO_ARCHITECTURE.md`: the capture ring and its overflow rule.
- `OPERATIONS.md`: the recordings folder.
