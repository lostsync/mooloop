# 03 · It runs on the master, is saved, and keeps takes and exports aligned

**Crates:** `mooloop-core`, `mooloop-dsp`, `mooloop-session`,
`mooloop-engine`, `mooloop-project` (tests). Crosses Document's format and two
Engine call sites; both need an ack on the board.

## What it builds

- **The saved field.** `StripParams.master: MasterSectionParams`,
  `#[serde(default, skip_serializing_if = "MasterSectionParams::is_default")]`,
  so a song that never touched it writes nothing new and an old song opens
  with the section out and the lookahead at 0. `PROJECT_FORMAT.md` records it.
  Document's ack.
- **The session.** `set_strip_param` refuses a master-section id on any track
  but the master, so a stale face cannot set a compressor nothing runs.
- **The engine.** The dsp `Strip` owns a `BusComp`. The bus walk (Mixer's)
  runs it on the master after the inserts and the tail-pinned strip, before
  the pre-fader send tap and the fader, and publishes its reduction to a new
  `BusMeters` cell, `take_master_comp_reduction`. `is_resting` asks it, so a
  master in mid-release does not sleep holding reduction.
- **The guard reads its lookahead** from the master's strip at the call site
  in `process_block_inner` (Engine's line, ack needed).
- **Alignment.**
  - The graph's reported latency, `CompiledLatency::total`, includes the
    master's lookahead: it is how far behind the input the ports are.
  - A take from the hardware input starts `input latency + lookahead` after
    its bar line. A take of a channel, a track or the master reads the mix
    before the guard, so it needs no change, and a test says so.
  - An export skips the first L frames it renders and renders L more, so the
    file starts on the bar line whatever the lookahead (`offline.rs`,
    Engine's, ack needed). The tail's at-rest check waits for the ring to
    empty.
  - **MIDI notes recorded while monitoring are not compensated, and that is
    filed rather than half-done** (MOO-209, Sequencing). A player hears the
    master L late and plays L late, exactly as for an audio take; but note
    capture compensates no output latency at all today, not even the
    driver's playback latency, which is usually the larger. Subtracting only
    the lookahead would leave it half-compensated, so the whole correction is
    one decision in Sequencing's code, with `RenderState::output_latency_frames`
    as the lookahead's half of it.
  - **At 0 nothing moves**, pinned through both paths: a live render at a
    256-frame block and an export, each with the section present but at its
    defaults, are bit-identical to the same song without one, and the export
    has the same length and the same first frame.

## The acceptance render

An example, `crates/mooloop-engine/examples/master_bus_comp.rs`, builds a song
with the real `mooloop_core` types (a drum pattern and a bass line, summed
about 6 dB hot at the master), saves it with `mooloop_project::save_song`,
checks `load_bundle` repairs nothing, and renders it offline four times --
section out, and each voicing in -- to float WAVs. It prints each render's
gain reduction against the law, and a test beside it holds:

- **Bypass is transparent**: the section out is bit-identical to a song that
  never had one.
- **Gain reduction follows the law**: each voicing's static reduction on the
  render agrees with `bus_comp`'s own curve at the detector level it saw.
- **Realtime and offline agree**: the same song through `process_block` at a
  256-frame block and through `OfflineRenderer` at its 512-frame block are
  bit-identical.
- **Lookahead**: an export at 3 ms starts on the same frame as one at 0.

Its command goes into `FOCUS.md`'s listening list in the same commit.
