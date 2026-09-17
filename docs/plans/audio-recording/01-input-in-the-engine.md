# 01 — Input reaches the engine

## The boundary question

`SCOPE.md` §5 says the I/O backend boundary (#7) gates this, because
`driver.rs` is deliberately not a trait and every I/O extension otherwise has
to be written once per driver. That is true, and it is still cheaper to do
without #7 for this step:

- **What is shared stays shared.** The shared part is the executor's contract,
  not the driver's. Change `Executor::process` to take `in_l` and `in_r`
  slices. Each driver then only has to find input samples and pass them in,
  which is a few lines per driver.
- **Port listing has a precedent.** `midi_ports()` is already two small
  per-driver methods, and `SCOPE.md` §9 records that this was enough for MIDI
  port selection. `audio_input_ports()` copies it.
- **#7 is about more than this.** It covers device selection, format
  negotiation, and start/stop/status, none of which recording needs.

**Do #7 first if** the Core Audio half below turns out to need more than an
input stream and a ring. That would mean the drivers really differ, and then
the boundary is worth building.

## Build

**Executor**
- `process(midi, in_l, in_r, out_l, out_r)`.
- The input is copied into a preallocated stereo input bus on `RenderState`
  at the start of the block, before any strip runs.
- `process_once_block` and offline export pass silence, so realtime and
  offline rendering stay identical, as `AUDIO_ARCHITECTURE.md` requires.

**JACK**
- Register `in_l` and `in_r` as `AudioIn` in `Graph`, and pass their buffers
  to the executor.
- Connect them automatically to the system capture ports, as `midi_in` is
  auto-connected today.
- `audio_input_ports()` returns one entry for now, named the way
  `MERGED_MIDI_IN_LABEL` is, because what feeds `in_l`/`in_r` is chosen in
  the JACK graph.

**Core Audio**
- cpal has no duplex stream, so input is a second stream on its own thread and
  its own clock. Its callback writes into an `rtrb` ring of frames, and the
  output callback reads from that ring into `in_l`/`in_r`.
- Starting up: fill the ring halfway before the output side reads from it.
- Underrun: output silence for the missing frames, and count it.
- Clock drift between the two devices: the first version counts underruns and
  overruns and exposes them. It does not resample. Write that down as a known
  limitation.
- **None of this has been compiled on this machine.** Mark it unverified in
  the status file until it has been built and run on the Mac.

**Meters**
- An input peak meter per input pair, published the same way bus meters are
  (atomics, not events).

## Test

- The executor: a block with a known input produces that input in the input
  bus, and allocates nothing (`CountingAllocator`, `allocations()` and
  `frees()`).
- Offline: a render is bit-identical before and after this step.
- JACK: a `#[ignore]`d test that needs a running server, as the existing
  driver tests do. Also a manual check: `jack_lsp` shows `mooloop:in_l` and
  `mooloop:in_r`.

## Docs

- `AUDIO_ARCHITECTURE.md`: the input bus, and when it is filled.
- `SCOPE.md` §2 item 3: move it from "Nothing" to what now exists.
