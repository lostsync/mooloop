# Mixer

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## The mixer view

- Mixer tracks can be reordered the same way, by dragging a strip's name
  plate; the strips it passes slide aside. The master stays first: its plate
  selects and never drags, and a drop over it lands in seat 1. Everything that
  named the track follows it — channels routed to it, other tracks' outputs
  and sends, automation lanes, modulation routes and MIDI control bindings —
  the device rack stays on the moved track, and the move is one undoable edit.
  The Track menu's **Move … Left** and **Move … Right** do the same one seat
  at a time for the track the rack is editing, and are bindable in
  Preferences > Shortcuts. A control binding also follows a channel reorder,
  insert or delete.

- A mixer sharing the work surface with the step grid, behind the toolbar's
  STEPS/MIXER tab strip. It is a strip per track, master first, and a strip is
  **92 px wide with three faces**. The middle one is what you look at while
  mixing: name plate, live stereo meter, fader, destination, a count of the
  channels feeding it, and the analog-sum switch at its foot. A `‹` and a `›`
  in the strip's bottom row reach the other two -- sends to the left, the
  channel strip to the right -- and the arrow of the face you are on becomes a
  dot, so the row says where you are as well as where you can go. It is one
  strip at a time, so one track can show its EQ while the rest still show
  faders. The name, the meter and the fader do not turn, and neither does the
  column of four small controls beside the fader -- pan, solo, mute, polarity
  -- because an EQ is set by ear while watching what it does to the level and
  a send is set against the fader that feeds it. **A tall enough mixer stops
  paging and draws the whole strip**: given the room, a strip lays out drive,
  EQ, compressor, sends and then the meter and fader with its destination and
  analog sum beneath them, and the two arrows go away because there is
  nothing left to turn to. There is no zoom and no mode -- it is the pane's
  height and nothing else, so a mixer that is given a slot of its own or
  dragged taller shows more of every track at once. Below that height, a
  turned strip's fader takes whatever height its sections do not want rather
  than leaving it empty. Clicking a strip's name plate points the
  device rack below at that track, so a chain on a group of channels is built
  with the same gesture as a chain on one channel. Channels name their track
  from a picker in their rack row, beside their other output controls.

## Mixing and routing

- Channel solo, mute, volume, and pan are exposed in the rack row, alongside
  the mixer track the channel feeds: volume and pan as compact knobs, solo and
  mute as one 18px chip split across its middle — yellow above, red below.
  Neither half wears a letter; the colour of the lit half is what says which
  is on.
- **Every mixer move ramps.** A channel's or a track's
  fader, pan or balance, its mute, a solo silencing it or giving it back, and
  a track's polarity all reach the audio through a one-pole lag of 5 ms, the
  one sends use, per sample -- including when a lane or a modulator drives the
  fader, whose control-rate staircase the lag rounds off. A mute is a fade:
  the channel or track goes on rendering, its output and its sends aimed at
  silence, and stops contributing only once both have arrived, about a
  hundred milliseconds later. Polarity crossfades through zero. So does a
  send: switching one off fades it out and lets its delay drain before it is
  held, and moving it between pre- and post-fader fades out, switches tap and
  fades back in, all within about ten milliseconds. A document arriving
  starts at its own values rather than ramping into them, so a bounce's first
  milliseconds are at the levels the song holds.
- **Solo in place, per channel.** A soloed channel silences
  the *other* channels and is heard through its own volume, pan, mute and
  track, exactly as a soloed track is. It is the same ruling one level down
  and a simpler derivation: channels do not feed each other, so there are no
  ancestors to keep audible the way a soloed track's feeders are kept. An Aux
  In reading a silenced channel's outlet still hears it, because a producer
  publishes whether or not it is heard.

  A channel's `solo` is what is stored; what it silences is derived from the
  whole bank every pump tick and never written down. A solo does not touch
  the soloed channel's own mute, and dropping a solo gives every channel back
  whatever its mute said. The name of a channel a solo is silencing dims, the
  way a silenced mixer strip's name does.

  A channel's solo and a track's are separate controls over separate banks:
  soloing a channel says nothing about tracks, and soloing the track a channel
  feeds says nothing about its siblings on that track. `channel.solo` is in
  the shortcut registry with no default chord (`docs/ACTIONS.md`).
- **The mixer is a list of tracks, and a track is made because somebody made
  it.** A new song opens with the master; the starter kit adds `Drums`, with
  its four drum channels grouped onto it. `+` in the mixer
  adds a track, and a track's device face renames it or removes it. Both are
  undoable, and a rename is one undo step however many characters it took.
  Removing one falls anything routed to it back to the master rather
  than leaving it unheard. Adding, removing or moving a track reaches the
  engine as one edit rather than a rebuild of the song: every channel and
  every other track keeps its sounding notes, tails, sends and delay
  compensation. Undo still rebuilds.

  **There is no `+ Bus` and no `+ Send`.** What a track *is* — an ordinary
  track, a bus, a send return — is decided entirely by what routes into it.
  See `TERMINOLOGY.md`.
- Every channel names one mixer track. Tracks carry their own effect chain,
  volume, pan, and mute, and may feed another track. The addressable space is
  the master plus sixteen; strips are materialised per track as a project
  loads rather than preallocated, which `CAPACITY_POLICY.md` measures.
- **Sends.** A track can route a copy of itself to another track, in addition
  to its output. Sends are edited in the channel sidebar ([interface.md](interface.md)) and levelled
  on the mixer strip. The sends area draws exactly the sends that exist and
  scrolls when they outgrow the room — there is no ceiling on how many a
  track has.

  **A send is a route, not a kind of track.** The track at the far end is an
  ordinary track that happens to be fed by sends, which is what makes it an
  effects return; there is no return object and nothing to create. The starter
  kit opens with none: a return is a track with a reverb on it that another
  track sends to.

  Two tap points: **post-fader** (the default, so the send follows the track's
  fader) and **pre-fader** (after the track's devices, before its fader, so it
  holds its level while the fader moves). Both are after the chain, so they
  arrive at the same time. Mute silences a track's sends, pre-fader ones
  included.

  A send is **always linear** — analog sum is what a strip does to its own
  output, and a send is a feed into another strip's input. A send whose target
  already leads back would loop and is refused, greyed in the picker with the
  reason, under exactly the rule the output picker uses. Switching a send off
  is not the same as turning it down: it keeps its level and stays in the
  routing, so nothing re-times.

  Send levels are **smoothed**, per sample, over the same 5 ms every other
  strip-level gain uses (see *Every mixer move ramps*). A send that appears
  mid-song fades in from silence; one that survives a routing rebuild keeps
  the level it had.
- Each send is compensated on its own edge. A producer with a send reaches two
  summing points, which generally arrive at different times and are owed
  different delays, so a track feeding a latency-bearing return waits for it
  on its dry path and stays sample-aligned where the two meet again.
- Any bus may feed any other. The realtime thread never sorts a graph:
  `mooloop_core::compile_bus_graph` normalizes and topologically sorts the
  bank off the audio thread, and the engine walks the resulting
  `CompiledBusGraph`.
- Destinations and their matching render order are one fixed-size compiled
  value, and a track's sends travel with it as one command, so no block can
  render edges against a stale order or a send whose target the order has not
  been told about. A short stored bank is a small mixer and is left as it is.
  Invalid individual routes are repaired to the master by the integrity pass,
  which reports the repair; a send naming a track that is gone is dropped by
  `sanitize_bank` after the load, which reports nothing
  (`docs/LOOSE_ENDS.md`).
- A send orders its target after its source, the same way an output does, and
  a cycle closed through a send is refused the same way one closed through an
  output is.
- **A channel strip on every track.** Four sections -- an input stage
  (`pre in` and a drive), a four-band EQ, a compressor, and a polarity
  switch -- under one strip-wide **voicing**: `Moo`, `Grip`, `Punch`, `Iron`.
  Every section is **out by default**, and out is not "flat": a section that
  is out does not touch the samples, so a project that has never opened a
  strip renders bit-identically to one with no strip at all. That is
  what entitles it to exist on every track rather than being a device
  somebody places; the price of having one everywhere is three booleans a
  block, against a track bank that is capped at seventeen today.

  The EQ's four bands read **left to right, top to bottom**: high shelf, high
  mid, low mid, low shelf. Each is gain / frequency / Q, and each switches
  between a bell and the shelf it is nearest -- the top two to a high shelf,
  the bottom two to a low shelf. A band's Q knob is its Q as a bell and its
  **slope** as a shelf, and a band at exactly 0 dB is not run at all. The
  compressor is its own design rather than the compressor device behind a
  different face: threshold, ratio, attack, release, knee, a parallel `w/d
  mix` (at 0 it is the dry signal exactly) and makeup, with a lamp beside the
  section's header that lights with the gain reduction the block actually
  applied.

  **Frequency is stepped, and the step is what is stored.** A band offers 5,
  7, 7 or 5 positions rather than a sweep, and no position is printed with a
  hertz value, because the hertz is the *voicing's*: `StripEqTable` gives each
  voicing its own list, so Iron's low mid can sit lower than Moo's without a
  label going wrong on three faces out of four. The tooltip and the status bar
  say what the current position is worth. Dragging a point on the rack row's
  response plot snaps to the nearest position by log distance.

  **A voicing selects laws, never values.** It owns the input stage's
  harmonic profile, its tilt and its slew limit (all measured -- see
  `docs/plans/archive/console/06-preamp-modelling.md`), the EQ's Q law, the
  compressor's curve above the knee, and its programme dependence. Nothing a
  voicing does moves a number a knob shows, so the 3 kHz on the face is the
  frequency being boosted whichever voicing is selected; what a voicing
  changes that a knob cannot show is *drawn*, by the response plot and the
  gain-computer curve. `Moo` is the null case exactly: every section in, `Moo`
  selected and nothing set is the same audio as no strip.

  The strip is drawn in two places, and no parameter is reachable from only
  one of them: the mixer strip's own face, and a **pinned 2U row in the
  track's device rack**. Where that row sits in the chain -- before the track's own
  devices -- is one statement, `mooloop_core::mixer::STRIP_PIN`, which the
  engine's block loop reads as well, so the drawing and the audio cannot
  disagree. The row has no insert or remove rails, because it can be neither.

  Polarity is drawn beside mute on both faces and acts at the **top** of the
  track's block, so everything after it -- the strip, the chain, both send
  taps and the fader -- sees the flipped signal.

  Not yet: the strip's own processing -- its drive, EQ and compressor -- is
  not an automation or modulation destination (its fader and pan are), and
  there is no strip preset. `docs/plans/archive/console/00-status.md` says why
  each is separable.
- **Solo in place, per track.** A soloed track silences the *other* tracks,
  and the exceptions are what make it useful: anything that feeds a soloed
  track and anything a soloed track feeds stay audible, followed through
  outputs **and** sends, in both directions -- so soloing a group hears the
  group, and soloing a channel's track hears it through the group it lands in
  rather than in isolation from its own destination. Two solos are both heard.
  Nothing is silenced when nothing is soloed, and the master refuses the
  gesture, because soloing the thing everything reaches means silencing
  nothing.

  It is **solo in place**, not a monitor tap: the silence happens where mute
  happens, at the track's own output, so the mix a solo produces is the mix
  minus everything else rather than a separate path with its own gain
  structure. What is silenced is derived from the whole bank every pump tick
  and diffed like compensation, the audio graph and the console sums -- a
  track's `solo` is what is stored, never the silence -- so adding a send can
  change what a standing solo lets through without anyone pressing anything.
  A solo does not touch the soloed track's own mute: a muted track that is
  soloed stays muted, which is the question "is this the one that is quiet?"
  answered honestly. The button is drawn beside mute in the column that
  survives the turn, and on the strip's rack row.
- **Analog sum.** Any mixer track can be switched to sum into its destination
  through a non-linear encode, decoded at that destination together with
  everything else feeding it that has the switch on. The control is a small
  button at the foot of the strip, set apart from mute, drawing a straight
  line when it is off and a sine when it is on. The master has none, because
  it feeds nothing.

  **A track's switch, and only a track's.** A sequencer channel has none: the
  console this models puts its Channel stage on a mixer strip, and mooloop's
  mixer strip is a track (`TERMINOLOGY.md`). Several channels on one track
  therefore reach it linearly and the track encodes their sum, which is what a
  desk does with a group.

  There is no device to place and no bus to create: every summing point
  decodes, and the master is already one, so two channels switched on glue
  with nothing configured. Nesting needs no special case either — a
  console-on bus encodes at its own output and whatever it feeds decodes it.
  A strip switched on **alone** changes nothing, exactly; the character is
  entirely in the interaction between strips that opted in together.

  Off is the default and is bit-identical to a linear mixer. Switched on, the
  summing law separates a sparse mix and bounds a dense one at +3.92 dBFS —
  see `GAIN_STRUCTURE.md`, which records that ceiling as a deliberate
  exception to "nothing bounds a sample in the live path". A bus's fader sits
  after its decode, so pulling a bus down is level and pulling its feeders
  down is drive.

  Called *analog sum* in the interface and *console summing* everywhere in the
  source and the documents; the technique is the Airwindows Console idea, and
  `mooloop_dsp::console` is where the curve lives.
- Cycles are refused rather than delayed, at the picker (looping destinations
  are shown greyed with the reason), at the command boundary, and on load,
  where a cyclic file gives up **the edges that close the loop** so it still
  opens and plays. A send on the ring goes before an output on it, because a
  dropped send loses what was added where a re-pointed output still carries
  the track's audio; routing elsewhere in the bank is untouched, and each
  removal is named in the log. Feedback routing would mean reading a bus's
  previous block, which is a deliberate feature rather than a fallback and
  needs a latency story this engine does not have.
- A muted bus still processes, so effect tails on it decay rather than freeze,
  but contributes no audio and meters as silent. A muted channel does the same
  once it has faded: its generator is left uncalled, but its effect
  chain is fed silence until it reports at rest, so a delay or reverb tail
  decays under the mute instead of replaying on unmute. A muted channel whose
  chain is at rest costs nothing.
- Per-bus peaks reach the GUI through a shared array of atomics rather than the
  event ring, which the ring's drain rate could not keep up with. The published
  value is a peak hold that only the GUI's read clears, so a transient landing
  between two UI frames is still shown. The channel rack has no meter of its
  own: `ChannelMeter` is drawn on the mixer strip, the device rack's two rails
  and a track's fader row, and nowhere else. **All of them are continuous
  bars with a peak-hold hairline**; the LED-segment form survives only in the
  mockup catalog. Preferences > Appearance > Metering tunes how fast they
  fall, from 30 dB/s down to 3, defaulting to the IEC rate of about 12.

  **Only a meter with a clip latch behind it draws a clip lamp.**
  `ChannelMeter` takes `show-clip`, and the rack's two rails set it false:
  they meter a chain, and a chain has no latch to light or to clear. A track's
  fader row does have one -- the same latch its mixer strip shows, cleared
  from whichever of the two the user clicks -- and shows peak hold from the
  same reading. **The master's toolbar meter is the same latch again**: it
  reads bus 0 through the mixer strip's own ballistics, so either lamp clears
  both and neither can disagree with the other.
- Channels retain the historical constant-power pan law, so existing project
  levels do not jump. Mixer buses use a distinct stereo balance law that is
  unity at centre and never boosts an endpoint; adding centred routing stages
  is therefore level-neutral.

## Latency compensation

- **The mixer is latency compensated**, for every device, native or hosted.
  Every device declares the frames it adds (`AudioNode` reports integer
  processing latency and `EffectKind` declares it without being built), the
  bus tree compiles into a per-producer delay, and each channel and bus waits
  by the difference before it sums — so two channels hitting on the same tick
  land in the same frame even when one carries an oversampled device and the
  other does not. What it removes is the comb filtering that was worst
  exactly when two channels were most alike. The drive costs the measured 15
  frames of its complete 2x interpolate/decimate path, and it also delays its
  internal dry path by the same amount so its own wet/dry control cannot mix
  time-misaligned signals; the Limiter's 96 frames are under the dynamics
  entry in [devices.md](devices.md). Sends are compensated per edge rather than per producer
  (above). Sidechains, which are untrustworthy without compensation,
  are still absent.
  Bypass keeps its device's latency — a bypassed node's signal goes through
  the same delay rather than past it — so A/B-ing an effect A/Bs the effect
  and not the timing. Removing the device is what gives the latency back. A
  hosted plugin's latency is its own, read once it is active: the
  compensation plan asks the plugin for it, live and in an export. A plugin
  *inside a container* sizes the container too: a Chain's dry copy waits for
  it and a Layer's other branches are held back to meet it, resent whenever
  the plugin reports a latency, so a latent plugin in a container sums as one
  copy rather than combing against an early one. A resend at the same length
  keeps the ring that is playing, so it is silent; a real change of latency
  jumps, as any latency change does. The plan is derived from the project
  rather than tracked alongside it, so no edit path can forget to update it,
  and an offline render compiles the same plan as a live one.
- **A channel** feeds exactly one track and cannot author a send of its own.
  The engine's sends are strip-level and a channel's compiles correctly, but
  nothing authors one, because the mixer draws no channel strips for the control
  to live on — that and the tap points below pre-fader are stage 2 of the send
  work. A send has a level and a tap, and no pan and no wet/dry split of its
  own. There are no sidechains: no device takes a key input, and a hosted
  plugin's sidechain port runs on silence. The hardware input reaches the mix
  only as a channel's AUDIO source with MON on ([sampler.md](sampler.md)).
