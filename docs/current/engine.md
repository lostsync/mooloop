# Engine

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## The audio path

```text
UI commands -> rtrb queue -> shared render state -> transport + sequencer
                                  |
                                  v
                         timed events/channel
                                  |
                                  v
selected source (sampler / drum synth / DS-01 / v1 mono / ML-M1 / ML-P8 / poly / aux in) -> effect chain -> gain/pan/mute
                                                           |
                                                           v
                                             assigned mixer bus (0-16)
                                                           |
                                        bus effect chain -> gain/balance/mute
                                                           |
                                             (optionally another bus)
                                                           v
                                                master bus (bus 0)
                                                           |
                                            master effect chain -> master bus compressor -> gain/pan
                                                           |
                                                           v
                               output guard: NaN/Inf -> silence, 0 dBFS limiter (no lookahead)
                                                           |
                                                           v
                                  driver output (JACK ports or a Core Audio device)
```

**The output guard** (`docs/GAIN_STRUCTURE.md`) is the last thing
every block passes through, live or exported. A NaN or infinite sample from a
device that blew up leaves as silence and is counted as a latched fault
instead of reaching the speakers, and a zero-latency safety limiter holds the
output at 0 dBFS. It is bit-transparent to a mix that stays under 0 dBFS.
The master's meter reads the mix *before* the guard, so a mix that is over
still lights the clip latch, and a non-finite sample reads as an infinite
peak rather than as silence.

**The Bus Comp** (`docs/GAIN_STRUCTURE.md`) is the
master's own section, drawn in the master's device rack between its inserts
and its fader, where it runs. Three voicings, each a measured unit's law:
**Grip** (the SSL G bus), **Punch** (the API-2500) and **Tube** (the
Fairchild 670), picked with one `◂ GRIP ▸` chip whose arrows or wheel step
through them. Grip and Punch show ratio, attack and release, each a switch
reading the unit's own markings; Tube shows the 670's six-position TIME
instead. Threshold, makeup and wet/dry are shared, and each voicing keeps its
own settings when another is picked. A needle meter reads its gain reduction,
with a held mark. Out, it leaves the mix bit for bit. It is saved with the
master's strip.

**The Bus Comp is also an insert**, in the insert menu after Comp,
so a drum bus, a channel or a container branch can have one. It is the
master section's compressor, not a copy: the same DSP, the same face
(needle, voicing chip, per-voicing knobs) in the accent colour, three
units wide, and the same settings and defaults. Its in/out is the rail's
bypass, so it has no IN switch, and it has no lookahead. Every knob automates
and takes modulation, it reports no latency, and it ships a factory bank of
six (two per voicing). At the end of the master's chain it renders the same
as the master's section at the same settings, sample for sample.

**The safety limiter has no lookahead and no knob**: nothing leaving the
master is late, so monitoring, takes and exports carry no extra delay for it.
A song saved with a lookahead set on the master's Out face opens at no
lookahead, with nothing to repair.

**Inside the graph**, the effect host checks each device's input in
the peak fold it already takes for the meters. A block carrying a NaN or an
infinity has those samples silenced before the device sees them, and it's
counted (`EngineHandle::effect_faults`). The status bar raises a warning the
first time that happens. The reverb, plate, modulation effect, gate,
compressor, limiter and Buffer also clear their own state if a non-finite
value gets in, one pass over the block, as the shared filters do. So a
device that blows up costs one block of silence downstream.

The engine preallocates channel strips, pattern storage, event lists, and audio
buses. A driver-independent render state owns transport, scheduling,
instruments, effects, mixing, and metering. One executor drains fixed-size
commands into that state and publishes position and master peak events, and a
driver adapter hands it buffers: JACK on Linux, Core Audio through cpal on
macOS, chosen at compile time -- or, chosen at run time when that driver will
not open, none. Offline export drives the same render path with no driver at
all.

**With no audio device the app still opens.** When JACK will not
open -- no libjack installed, or no server answering, which the log and the
window tell apart -- the engine runs on a null driver: a thread rendering
512-frame blocks at 48 kHz into nothing, so editing, the transport and the
meters all work and nothing is heard. The window asks "mooloop is running with
no audio" and offers Reconnect. The same question, as "The audio stopped",
appears when a running engine stops being heard: the JACK server shut the
client down (a PipeWire restart does), changed its sample rate, or the
callback has not run for three seconds. Reconnect closes the driver, opens it
again, builds a new engine at whatever rate it now reports, and installs the
open song into it -- samples, routing, mixer -- with the transport stopped and
nothing marked unsaved. Dismissed, the status bar keeps saying so,
and Preferences > Audio > Refresh reconnects. A device that panics in the
audio callback costs a block of silence rather than the audio for the rest of
the session: the block is silenced and counted, the log and the status bar say
so, and the next block renders.

Core Audio has no port graph, so an output target there names a device and two
of its channels. The system default follows whatever the system output is; a
named device that disappears hands playback to the system default, and with
auto-reconnect on, playback returns to the device when it comes back. A new
device or buffer size reopens the stream without rebuilding the engine. The
engine keeps the sample rate the system output had when it started and asks
later devices for the same one.

Under JACK the client asks for the name `mooloop`, and a second instance is
given another (`mooloop-01` by a JACK server, `mooloop-<id>` by
pipewire-jack). Each instance names its own ports from the name it was given,
so two instances each connect, move and reconnect only their own outputs and
MIDI input, and neither offers a mooloop's inputs as an output.

On Linux the engine picks which `libjack` it loads before JACK is first
touched. Debian, Ubuntu and Mint keep pipewire-jack's library off
the default search path, where the name finds JACK2's client library and no
server, so when PipeWire is running, its JACK library is installed, no JACK
server answers, and neither `LD_LIBRARY_PATH` (`pw-jack`) nor `LD_PRELOAD`
names a libjack, the engine opens PipeWire's by its full path and the `jack`
crate is answered with it. Otherwise the default stands. The log names the
file actually loaded, and Preferences > Audio shows it beside the driver's
name. The `.deb` lists `pipewire-jack` first among its libjack alternatives.

Channels render in a compiled order (`ARCHITECTURE.md`), so a producer runs
before any channel subscribed to one of its audio outlets. The channel
modulator tick pass stays in index order and stays a separate loop: a
modulator's phase must not depend on a subscription somebody made on another
channel.

Devices and channels with nothing to do are not rendered. A device says how
long it can still be heard after its input goes silent and whether its own
state has settled; the host stops calling an effect slot whose input has been
quiet longer than that, and stops rendering a whole channel strip when it has
no events, its generator has no voices and has been putting out silence, and
every effect on it would be skipped. Waking is the first block with audio in
it, from the state the device had when it stopped — nothing is reset and
nothing ramps. What a project renders is unchanged either way, at any block
size.

WAV decode, waveform construction, directory scanning and project
installation happen off the audio thread; how a prepared project, a sample or
a displaced node crosses to it and back is `AUDIO_ARCHITECTURE.md`'s.

## The command backlog

- **An edit the engine's command ring has no room for waits instead of
  being lost.** The pump holds what would not fit, in order, with everything
  queued behind it (`mooloop_session::engine::EngineBacklog`), delivering it
  first on a later tick. Two installs waiting together merge into the
  newest, keeping the older one's undo step; a load or a Reconnect clears
  what was addressed to the old song. A backlog lasting a second raises a
  warning in the status bar -- "the audio engine has stopped" when the
  callback is not running, which is what a full ring usually means -- and
  it comes down when the backlog drains.

## Foundations

- `AudioNode` provides one in-place DSP interface for instruments and effects.
- `DrumSynth` (kick/snare/hat), `Ds01` and its one universal percussion voice,
  the v1 `MonoSynth`, the filter/performance-led `MlM1`, the eight-voice
  `MlP8` and its oscillator network, and `PolySynth` use the same timed note
  path as the sampler in realtime and offline renders.
  Their oscillator and envelope types are shared DSP primitives; their voice
  engines are deliberately separate.
- `EventList` carries fixed-capacity, sample-timed NoteOn, NoteOff, generic
  ParamValue, and internal-route-amount events.
- `StereoBus` ownership is centralized in the graph, leaving room for sends,
  groups, sidechains, and buffer taps.
- Musical time is PPQ 96, which exactly represents common subdivisions through
  64th notes and triplet grids.
- Realtime state is preallocated outside the audio callback. The channel and
  effect banks each cover their complete 256-value `u8` address space; these
  are bridge-format boundaries rather than small product caps.
- Sampler playback resamples through a band-limited windowed-sinc reader:
  unity rate is sample-exact, pitching up narrows the kernel's cutoff to
  keep foldback down, and the kernel folds across loop and ping-pong
  boundaries rather than filtering against silence.
- DSP primitives are checked at 44.1, 48, 96 and 192 kHz through one shared
  measurement kit, `mooloop_dsp::testkit`. A NaN or infinite cutoff,
  resonance, drive, Q, gain, frequency or smoothing target lands on the edge
  of its range rather than in a filter's or oscillator's state, where it
  would have made every later sample NaN. A NaN or infinite *sample* -- a NaN
  frame in a decoded file, a blown-up upstream device -- is lost, but it does
  not stay in any SVF, cascade, ladder, biquad or one-pole state, and a delay
  does not feed it back round its loop: the next finite sample is heard.
  The reverb, plate, modulation effect, dynamics, limiter and Buffer clear
  their own state too (described under "Inside the graph" above).

## Samples and rendering

- A loaded sample is immutable in the audio path. The application generates
  audio for itself in three places. A recorded take
  ([sampler.md](sampler.md)) captures a channel (its own included), a track,
  the master or the hardware input, becomes the recording sampler's sample,
  and is copied into the song by a save. A sampler stretch commit is rendered
  on the UI thread and written to the data directory's `renders/` as a sample
  of its own. And the Buffer insert keeps its rolling ring.
- The render graph is independent of the audio driver and supports finite
  offline passes at the engine's sample rate (export, in [files.md](files.md)); MP3 goes through an in-process LAME encoder. Realtime-vs-offline
  null testing is not implemented.
- Replaced sample lifetimes need a deliberate deferred-reclamation design so
  the last large sample allocation can never be freed on the realtime thread.
