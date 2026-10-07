# Listening

Checks only Adam can make: renders to hear, and things to look at or try in
the app. An agent renders and measures. Whether the result sounds or looks
right is the check. Add an item when a change alters a sound or a look and an
agent can't judge it. Delete the item once it's been heard, and file anything
it turned up in Linear.

**Last heard, on record:** DS-01 and its kit (2026-09-04), ML-P8 and its bank
(2026-09-05), the ML-M1 and its patches and the channel strip's three
voicings (2026-09-11), and `incremental-structure/` (2026-09-18). Everything
since was closed on Adam's 2026-09-22 instruction to treat outstanding passes
as done with nothing heard. **Nothing below has been heard.**

The items are in the order worth playing. Items 3, 23, 24, 25 and 27 change
how existing songs sound rather than adding something new. Most renders build
on the box and pull their files back, and each binary prints what it
measured.

### 1. The layer device's parallel compression

A drum loop on a track whose layer holds a clean branch and a Drive →
Bitcrush branch, with the layer's Mix at 0, 25, 50, 75 and 100% in turn, two
bars each:

```sh
scripts/antibox --no-incremental --pull target/listening/layer-parallel-drums.wav \
  cargo test -p mooloop-engine --lib -- layer_parallel_drums --nocapture
```

Play `target/listening/layer-parallel-drums.wav`. The test prints each
branch's level and each section's. Then build the same thing in the rack from
the list's `+`, and play with S and M.

### 2. Bend, sustain, mod wheel and aftertouch (MOO-126, MOO-128)

On every source, from a real keyboard.

### 3. Saved patches whose sound moved (MOO-112, MOO-119, MOO-123, MOO-145)

The cutoff law is now one law at every sample rate, the resonance taper and
the 24 dB slopes changed, and glide arrives in its stated time. ML-P8, ML-M1
and Acid patches are where a difference would show.

### 4. The insert dynamics (MOO-142)

The Limiter looking ahead at true peaks, the Gate with hysteresis, and the
Compressor with its own Mix.

### 5. Transitions that used to click (MOO-104, MOO-172, MOO-213, MOO-230)

Fader, mute, solo, bypass, preset loads and sampler steals should all be
silent now. So should a hosted plugin's restart. Put the LSP filter from item
7 on a sustained pad at a low cutoff. Then change the latency or oversampling
setting that makes it ask for a restart, or change JACK's sample rate while
it plays. The filter should fade to dry and back with no click.

A hosted instrument's restart fades out with no click too. Hold a pad on a
CLAP instrument and force a restart or a rate change. The held note staying
silent until the next note-on is deliberate. A restart only happens live, so
there's no render. These are the measured versions:

```sh
scripts/antibox --no-incremental cargo test -p mooloop-engine --lib -- hosted_plugin_for_its_placeholder --nocapture
scripts/antibox --no-incremental cargo test -p mooloop-engine --lib -- hosted_instruments --nocapture
```

### 6. The master bus compressor, each voicing (MOO-13)

*"Turning it on should feel special"* is the acceptance case, and only a
listen can pass it. A kick, a bass line and chords a few decibels hot,
rendered with the section out and then with Grip, Punch and Tube in, to float
WAVs with their gain reduction printed beside them:

```sh
scripts/antibox --pull target/master-bus-comp cargo run -p mooloop-engine \
  --example master_bus_comp -- target/master-bus-comp
```

Then try the same on the master's rack.

### 7. A CLAP effect on a drum loop (MOO-81)

LSP's flanger on a two-bar drum-synth loop. It's exported, then the song is
reopened with the plugin present and with it missing. Build on the box and
run on the laptop, which has the LSP plugins:

```sh
scripts/antibox --no-incremental --pull bin/clap_effect_case sh -c \
  'cargo build -p mooloop-session --example clap_effect_case && mkdir -p bin && cp "$CARGO_TARGET_DIR/debug/examples/clap_effect_case" bin/'
bin/clap_effect_case --plugin /usr/lib64/clap/lsp-plugins.clap \
  --id in.lsp-plug.flanger_stereo --out clap-case
```

Listen to `clap-case/dry.wav` against `wet.wav`. `reopened.wav` should match
`wet.wav`, and `missing.wav` should match `dry.wav`. Run it again with
`--id in.lsp-plug.filter_stereo --out clap-case-filter` for the host's own
check. The flanger moves its LFO once a block, so its output depends on the
block size, by its own design. In the app, open `clap-case/clap-case.mooloop`.
The flanger is on channel 1, and it plays live on JACK at any buffer size.

### 8. A CLAP filter's cutoff automated on a drum loop (MOO-82)

LSP's filter on the hats of a two-bar loop, with its cutoff lane falling
across the loop. It's saved, reopened, exported, and played through the
executor the way a callback plays it. Build on the box and run on the laptop:

```sh
scripts/antibox --no-incremental --pull bin/clap_automation_case sh -c \
  'cargo build -p mooloop-session --example clap_automation_case && mkdir -p bin && cp "$CARGO_TARGET_DIR/debug/examples/clap_automation_case" bin/'
bin/clap_automation_case --plugin /usr/lib64/clap/lsp-plugins.clap \
  --id in.lsp-plug.filter_stereo --out clap-automation
```

Listen to `clap-automation/wet.wav` against `flat.wav`, which is the same
song with the lane held at its start. The hats should darken across the two
bars and snap back bright at the loop point, under an untouched kick and
snare. `reopened.wav` should match `wet.wav`, and `missing.wav` should match
`dry.wav`. `live-64.wav` is what the executor played. In the app, open
`clap-automation/clap-automation.mooloop` and play it on JACK.

### 9. ML-P8's Volume under modulation (MOO-214)

A held chord of sines with its Volume pumped by a quarter-note LFO. It should
duck smoothly, with no buzz on the duck:

```sh
scripts/antibox --no-incremental --pull target/mlp8-volume-pump \
  cargo run -p mooloop-engine --example mlp8_volume_pump -- target/mlp8-volume-pump
```

Play `pump.wav`, and `steady.wav` for the same chord unpumped. Then try the
real case: an Envelope gated by the kick, routed to ML-P8's Volume with
negative depth, at an Attack well under 500 ms.

### 10. The sampler's loop seam (MOO-43)

A one-bar synthetic break whose bass tone doesn't fit the bar, so a hard seam
clicks. It's looped two ways, from the top and after a lead-in, at Loop fade
0, 2, 10 and 30 ms:

```sh
scripts/antibox --no-incremental --pull target/sampler-loop-seam \
 cargo run -p mooloop-engine --example sampler_loop_seam -- target/sampler-loop-seam
```

Play `top-0ms.wav` against `top-10ms.wav`, then do the same for the
`lead-in-*` files. From the top, the fade's first millisecond fades the kick
back in, which moves its onset by up to 15 dB under its peak. Listen for
whether that softens the downbeat.

### 11. Fit to tempo, and SYNC off keeping the sound (MOO-39)

A 1.5 s loop fitted to one bar, rendered three ways: synced at 120 BPM,
synced at 90, and frozen at 120 then played at 90:

```sh
scripts/antibox --no-incremental --pull target/sampler-fit-freeze \
  cargo run -p mooloop-engine --example sampler_fit_freeze -- target/sampler-fit-freeze
```

The frozen render should sound like the 120 BPM loop, unchanged. Then, in the
app, turn SYNC off on a fitted loop and change the tempo. The loop's sound
should stay put.

### 12. A lane sweeping Loop start on a grid (MOO-47)

A one-bar break loops for four bars while a lane sweeps Loop start from the
top to the last quarter, first with the grid off and then on sixteenths:

```sh
scripts/antibox --no-incremental --pull target/sampler-loop-grid \
  cargo run -p mooloop-engine --example sampler_loop_grid -- target/sampler-loop-grid
```

Play `off.wav` against `sixteenth.wav`. The first slides through every frame.
The second should step in rhythm, a sixteenth at a time.

### 13. Slices detected on a break (MOO-44)

A two-bar synthetic break with a ghost snare and a bass tone under it. It's
detected at three sensitivities, then chopped with the default sensitivity's
slices played backwards on the sixteenths:

```sh
scripts/antibox --no-incremental --pull target/sampler-slice-detect \
  cargo run -p mooloop-engine --example sampler_slice_detect -- target/sampler-slice-detect
```

Play `chop.wav`. Every slice should start on its hit, with no flam and no
pre-echo. Then, in the app, DETECT a real break, move a marker by hand, and
REPLACE. The moved marker should stay where you put it.

### 14. A sliced break played back from its pattern (MOO-46)

A two-bar break, sliced by detection, with PATTERN writing a note per slice.
It's rendered once as written and once with neighbouring slices swapped:

```sh
scripts/antibox --no-incremental --pull target/slice-pattern \
  cargo run -p mooloop-session --example slice_pattern_case -- target/slice-pattern
```

Play `pattern.wav` against `break.wav`: they should be the same loop.
`reordered.wav` is the chop. Then, in the app, slice a real break, press
PATTERN, then REPLACE, and move a few notes in the roll.

### 15. The Bus Comp on a drum bus (MOO-216)

A two-bar kick, snare and hat loop on a drum bus, rendered for each voicing
three ways: dry, with a Bus Comp insert on the drum bus, and with no insert
and the master's own section in at the same settings (threshold -24 dB, no
makeup):

```sh
scripts/antibox --no-incremental --pull target/bus-comp-insert \
  cargo run -p mooloop-engine --example bus_comp_insert -- target/bus-comp-insert
```

Play `insert-grip.wav`, `insert-punch.wav` and `insert-tube.wav` against
`dry.wav`. Each `master-*.wav` should sound the same as its insert render.
Then, in the app, put a Bus Comp on a real drum bus from the insert menu and
try each voicing.

### 16. A folded device's label (MOO-219): a look

Fold a device with the `<` at the top of its rail. Its name should read top
to bottom in upright capitals, one per line, with "BUS COMP" parted by half a
line. A name too long for the strip ends in stacked dots, and hovering over
the strip shows it whole in the status bar. Also fold a Chain that holds a
folded device, open the Chain again, and check that the inner fold is still
folded.

### 17. A CLAP filter put in a chain from the window (MOO-83)

In the app, open a drum loop, hover over the arrow after its last device,
pick **Plugin…**, type `filter` in the PLUGINS tab, and double-click LSP's
*Filter x2 Stereo* (`/usr/lib64/clap/lsp-plugins.clap`). While the loop
plays, turn the frequency (a page or two in), undo it, redo it, then save,
reopen and export. The export should sound like the last thing you heard, and
the reopened face should show what it showed before. With the plugin
uninstalled or renamed, its face should stay, greyed out, saying it's
missing, and the loop should play dry. The face has only been seen under the
software renderer. The measured version:
`scripts/antibox --no-incremental cargo test -p mooloop-ui --lib plugin_ui`.

### 18. A CLAP instrument playing its channel's pattern (MOO-85)

A one-bar melody of six notes, with the last note held across the loop
point. The in-repo test sine plays it as the channel's source, and it's
exported the way the app exports:

```sh
scripts/antibox --no-incremental --pull target/clap-instrument \
  cargo run -p mooloop-session --example clap_instrument_case -- target/clap-instrument
```

Play `instrument.wav`. Each note should start clean on its step and release
over 50 ms. `missing.wav` is the same song with the plugin missing, and it's
silent. For a real instrument, run `clap_instrument_case <dir> --real <clap
id>` on the laptop. It opens the instrument through
`~/.config/mooloop/plugins.toml` and writes `real.wav`. No installed plugin
makes sound this way yet: LSP's *Sampler Stereo* is silent without a sample
loaded, and its *Trigger MIDI* is refused as a source. Surge XT or Dexed
would be the real case.

### 19. The sampler's mono glide and legato (MOO-45)

A long sine sampled at A3 plays a one-voice line. Its first four notes
overlap, and its last note starts after a gap. It's rendered three ways:
Glide 80 ms with Env trig on Legato, the same with Retrig, and no glide:

```sh
scripts/antibox --no-incremental --pull target/sampler-legato-glide \
  cargo run -p mooloop-engine --example sampler_legato_glide -- target/sampler-legato-glide
```

In `legato.wav`, the overlapping notes should slide into each other with no
new attack, and the note after the gap should start fresh at its own pitch.
`retrig.wav` slides the same way but restarts each note. `none.wav` steps.
The level can't tell Legato from Retrig apart, so the question is whether
Retrig's restart from the top of the sample can be heard. Then, in the app,
set a sampler to one voice, turn Glide up, and play legato on a keyboard.

### 20. A click on an instrument preset (MOO-227): in the app

With autoplay on, open the PRESETS tab and click an ML-P8, an ML-M1 and a
DS-01 preset in turn. Each should play a short phrase without changing the
song or its undo history. A click on a second preset while the first is
playing should cut to it. An effect preset's click plays nothing, by Adam's
ruling.

### 21. Sampler key zones (MOO-14)

Two sine files: the sampler's own at C4, and a zone's at C5, an octave up and
at half the level. The sampler plays its own file up to B3 and the zone from
C4. The song is saved embedded, reopened and exported through the app's own
paths:

```sh
scripts/antibox --no-incremental --pull target/sampler-key-zones \
  cargo run -p mooloop-session --example sampler_key_zones -- target/sampler-key-zones
```

In `key-zones.wav`, C3 and B3 are the louder base tone, then C4 and C6 are
the quieter zone, each at the pitch its key names. B3 to C4 should be a
semitone step across the split, with no jump. Then, in the app, add a zone on
the ZONES page, play across the split on a keyboard, and undo the add.

### 22. The Modulation phaser at its fastest (MOO-235)

The phaser now works out its all-pass coefficients every 8 samples and draws
a straight line between them. The claim is that it sounds the same. A
sustained ML-P8 saw chord goes through a 12-stage phaser at half wet, Depth
100% and Feedback 70%: two bars at 12 Hz (the fastest Rate), then two at
0.5 Hz:

```sh
scripts/antibox --no-incremental --pull target/phaser-sweep \
  cargo run -p mooloop-engine --example phaser_sweep -- target/phaser-sweep
```

Play `phaser.wav` against `dry.wav`. The 12 Hz half should be a fast, even
warble with no grain or buzz riding on it, and the 0.5 Hz half should be a
smooth sweep.

### 23. ML-P8 unison at a constant level (MOO-244)

Turning Unison up should thicken a note, not make it louder. This prints the
level table, for one held A3 on Init Saw at 1x to 8x:

```sh
scripts/antibox --no-incremental cargo test -p mooloop-dsp --release --lib \
  unison_level_table -- --ignored --nocapture
```

In the app, step a held pad through the Unison counts. The level should hold
while the width grows. Adam's songs whose ML-P8s use unison now play quieter
than they were mixed: in `housey-dropout-factory`, "ML-P8 9" is down 2.1 dB,
and in `ok-then`, "ML-P8 1" is down 9.6 dB, "ML-P8 7" 2.3 dB and "ML-P8 15"
1.3 dB. Whether those faders want to come back up is the listen. To render
them:

```sh
scripts/antibox --no-incremental --pull target/mlp8-unison \
  cargo run --release -p mooloop-session --example mlp8_unison_levels -- \
  target/mlp8-unison ~/perf-songs/housey-dropout-factory.mooloop ~/perf-songs/ok-then.mooloop
```

### 24. The Drive after its oversampler changed (MOO-250, MOO-251)

A saw chord, one bar each through the default Drive, Tape Warmth and Hard
Clip, then dry:

```sh
scripts/antibox --no-incremental --pull target/drive-curves \
  cargo run -p mooloop-engine --example drive_curves -- target/drive-curves
```

Play `drive.wav`. Each Drive should sound as it did. The render has no old
build to A/B against, so a song with a Drive that was saved before 2026-09-25
is the real comparison.

### 25. ML-P8's filter at control rate (MOO-246)

A voice's filter coefficients are now worked out every 16 samples and ramped
towards where the cutoff is heading. The claim is that it sounds the same.
Play the fastest filter envelope the device has, a pluck (a 1 ms filter
attack on a resonant 24 dB low-pass), and play Furnace Stab. Furnace Stab is
the one to listen to: its voice feedback turns the smallest change in
rounding into a different waveform. There's no render for this one. The
measured comparison is
`a_control_rate_voice_filter_tracks_the_per_sample_one_closely`
(mooloop-dsp).

### 26. The starter kit (MOO-267)

A new song opens with four DS-01 channels: Machine Kick, Machine Snare,
Machine Hat and Machine Open Hat, voiced as plain 80s drum-machine sounds. A
two-bar beat at 120 BPM, then each hit on its own:

```sh
scripts/antibox --no-incremental --pull target/starter-kit-audition \
  cargo run -p mooloop-engine --example starter_kit_audition -- target/starter-kit-audition
```

Play `starter-kit-80s.wav`, then `kick.wav`, `snare.wav`, `closed-hat.wav`
and `open-hat.wav`. The listen is whether they sound like a drum machine and
whether the balance is right. Every value is in
`crates/mooloop-core/src/ds01_factory.rs`.

### 27. The Modulation device at high feedback (MOO-200)

Past 75% Feedback, the wet output is trimmed so that a feedback resonance
peaks at +12 dB. It used to reach +22 dB at the knob's end (92%). A flanger
at full feedback should still sound like a jet, just not 22 dB louder. A
sustained ML-P8 saw chord at half wet, two bars each through a Flanger at
50%, 75%, 92% and -92%, then a Phaser at 92%, then dry:

```sh
scripts/antibox --no-incremental --pull target/modulation-feedback \
  cargo run -p mooloop-engine --example modulation_feedback -- target/modulation-feedback
```

Play `modulation-feedback.wav`. Songs with a Modulation device above 75%
feedback now play quieter than they were mixed.

### 28. A range export that starts inside a reverb (MOO-239): in the app

Put a long reverb on a channel whose note ends just before bar 2, and export
Range: custom, from bar 2. The file should open inside the reverb's tail, and
a note held across the range's start should sound from the file's first
sample. The measured version:

```sh
scripts/antibox --no-incremental cargo test -p mooloop-engine --lib -- \
  a_range_holds_the_reverb_from_before_it a_note_held_into_a_range
```

### 29. A mono CLAP effect in a chain (MOO-266)

A mono effect is fed `(L + R) / 2`, and its output goes to both sides. So a
centred sound should come out at the level it went in, and a hard-panned one
6 dB down on both sides. LSP's `_mono` plugins are no longer greyed out in
the browser's PLUGINS tab. Put `Filter Mono` on a centred drum loop with the
filter open, and A/B it against bypass: the level shouldn't move. Then do the
same with the loop panned hard left. Offline, it's item 7's binary with a
mono id:

```sh
bin/clap_effect_case --plugin /usr/lib64/clap/lsp-plugins.clap \
  --id in.lsp-plug.filter_mono --out clap-case-mono
```

### 30. The Modulation device's Width and Mix (MOO-245)

Width is a mid/side scale on the wet signal. At 0% both voices fold to the
centre, and at 100% (the default) the mode keeps its own image. The face's
knobs are now five to a row. A sustained ML-P8 saw chord goes through a
chorus at full Spread and half Mix: two bars each at Width 100%, 50% and 0%,
then at full Mix, then dry:

```sh
scripts/antibox --no-incremental --pull target/chorus-width \
  cargo run -p mooloop-engine --example chorus_width -- target/chorus-width
```

Play `chorus-width.wav`. Two questions here, a listen and a look: does 0%
sound like a good mono chorus rather than a comb, and does the face read well
with five knobs to a row?

### 31. The sources' names (MOO-140): a look

- The rack's `+` should offer "Add Gitdum DS-SX", "Add Munotone ML-M1", "Add
  Polyneight ML-P8" and "Add Dominic DS-01".
- A source's header should show the full name.
- The source picker's chip should show the model number, and its menu the
  full name.
- The preset browser should head each source's group with the full name.
- A new channel should be named by its model number ("ML-M1 2").
- An old song's "Drum Synth 1" channel should keep that name.

### 32. Key zones as regions (MOO-463): in the app

Adam's own case: cut three chords out of two guitar loops into one sampler.
Load the first loop, and on SAMPLE set zone 1's start and end around its
first chord. On ZONES, add the second loop, then the first loop again. Pick
each zone on the ZONE strip (or click its name on ZONES) and set its start
and end around its chord. Each key should play only its own chord. Then try
FOLLOW, the LEVEL trim, and a lane on Start. Every zone should move by the
same amount.

### 33. The bevel on everything it was meant for (MOO-153): a look

In Preferences > Appearance, pick **Platinum** and then **Impulse**, with a
song whose rack has a few devices. Besides the buttons, these are now lit
blocks: every knob's cap (the light swaps while you drag it), each device's
plate and header strip, a folded device, every pane (the dock included), the
toolbar, the status bar and both sidebars. Dividers, rails and the insides of
device faces stay flat on purpose. The look decides whether that reads as Mac
OS 8 or Impulse Tracker, or as a bevel too many (the pane edges and the
sidebars are the likeliest culprits). Mooloop's own theme and the other flat
ones should look exactly as before.

### 34. A stretch commit as a render (MOO-375, MOO-370, MOO-394): in the app

Load a break in Slice mode and detect slices over all of it. Drag Start in
past a few markers, fit the break to a bar, and press COMMIT. Every slice
should keep its key, including the ones outside Start and End, and land on
the same hit. Add a slice at a hit the detector missed, drag End in a little,
change the tempo, and press REBAKE. The new slice and the trim should still
be there, on the same hits. Then press REVERT: the original comes back, with
that slice on its hit. Save, quit and reopen: the committed sample should
play exactly as before. The listen is whether a stretch of a stretch sounds
acceptable at ordinary tempo changes.

### 35. Many zones from one file (MOO-464): in the app

Load a file of hits (twelve xylophone notes, say, or a drum run) into a
sampler. Switch to Slice, DETECT, and accept. Then, on ZONES, set FROM to a
key and press ZONES FROM SLICES. Every hit should become its own zone, one
key each, playing at its recorded pitch, with the sampler back in Pitch mode.
The Pitch/Slice switch is greyed out while there are zones. Select a zone and
press DUPE: the copy should land on the next free key. Save and reopen: all
the zones should be there, and the song's folder should hold the file only
once. The questions are whether the workflow feels right, and whether each
hit's tail is cut where you'd expect.

### 36. Pattern clone, clear and delete under a playing song (MOO-466): in the app

In Song mode, with a pad held across a bar and a lane sweeping a filter,
clone, clear and delete patterns while the song plays. Only the edited
pattern should lose a note (in Song mode, the patterns after a clone or
delete can too). Nothing should click or drop out. A filter that a cleared
pattern's lane was sweeping should go back to its knob position. Then undo
each edit. The question is whether it feels instant.

### 37. Adding, removing and moving tracks under a playing song (MOO-466): in the app

Put a long reverb on one track, a send into another, and a lookahead Limiter
on a third, and hold a pad across a bar. While the song plays, add a track,
drag tracks past each other, and remove an empty one. No tail should cut, no
send should drop out, and nothing should click or shift in time. Then remove
a track that has channels on it (they should carry on through the master),
and undo each edit. The question is whether it feels instant.

### 38. A busy 0.1.6 song's modulation, before and after (MOO-511): in the app

Open a 0.1.6 song with modulation on several channels: LFOs on filters, an
Envelope gated by another channel, a Step pattern, a Random, a Math module
reading an LFO. Play it in 0.1.6 and in this build. It should sound the same;
the one change on purpose is that a Step pattern and a tempo-synced Random
now start on the downbeat and follow the song position, as a synced LFO
already did (MOO-373). Then, while it plays, move a channel and paste one:
the other channels' notes should carry on, and an LFO should not restart.
Route an LFO to a track's insert and to a track's fader and listen for both
moving.

### 39. Assigning anywhere (MOO-512): in the app

Add an LFO on one channel and arm Assign. Without leaving Assign, drag a
knob on that channel's filter, then select another channel and drag one of
its inserts, then open a track's rack and the master's and drag an insert on
each. Every knob dragged should show its depth while armed, a dot once
disarmed, and its arc swinging with the LFO while the song plays, whichever
chain the rack shows. With MIDI learn armed, a press should still learn the
knob rather than route it. Then pick another channel in an Envelope's input
and play that channel: the Envelope should open on its notes. Point a Math
module's input at a module seated on another channel and check it follows.

### 40. The modulation pane (MOO-513): look, in the app

Press Ctrl+6. The Modulation pane should appear in the bottom pane. Drag
its tab to the top-left and to the split, and try it zoomed: in each, the
module grid should wrap to the width, the Add list should stay on the
right, and the selected module's knobs and its route list should stay
readable along the bottom. Add an LFO on one channel, select another
channel: the LFO should still be selected. Rename it, route it to knobs on
two channels and the master, and check the route list names each one with
its channel or track. Remove a route there and its dot should leave the
knob. Open a 0.1.6 song with a saved layout: it should open as it was, with
Modulation in the bottom pane. The device rack should have no shelf under
it.


### 41. The patch canvas (MOO-522): look, in the app

Open a 0.1.6 song with modulation, press Ctrl+6 and zoom the Modulation
pane. Its modules should sit in a grid where the old tiles were, each
Envelope, Step and Random gated by a `gate` tag at the left edge naming its
channel, a Math box wired from what it read, and every route as a tag under
its box reading the parameter, the device and the depth. Then build the
prototype's first example: a `gate  Kick 1` tag into an LFO's `retrigger`
(click the inlet and pick the kick), the LFO's outlet clicked to arm it and
Cutoff dragged up, then a Math box set to `* -0.5` wired from the LFO and
armed onto Res. Play: the LFO should restart on every kick and Res should
move against Cutoff. Drag boxes and tags, marquee two and drag them
together, drag a tag's wire off its inlet onto nothing; each should be one
undo. Wire the Math box's outlet back into the LFO's `rate`: the loop should
play, and the wire it closes should carry a small bar at its inlet. Save,
reopen, and everything should be where it was left.

### 42. Typing a box, and the LFO sequence (MOO-523): listen, in the app

In the Modulation pane, double-click empty canvas and type `lfo`, then Enter.
Type three more the same way: `lfo tri 2hz`, `lfo saw 1/8` and `lfo sqr 4hz`.
Then type `counter 4` and `select 4`. Wire the first LFO's outlet into the
counter's `advance` (it stands in for the beat until step 06 brings the
transport in). Wire the counter's `index` into the select's `index`, and the
four LFOs into `a` to `d`. Click the select's outlet to arm it and drag a
filter cutoff up. Play: the cutoff should move in four distinct shapes in
turn, a new one each time the first LFO rises. Type `slew 0.2`, wire the
select into it, click the select's Cutoff tag and press Delete, then arm
the slew onto the cutoff: the change from one shape to the next should
glide rather than jump. Double-click the
counter and retype it `counter 2`: only `a` and `b` should play. Type
`chord min7` somewhere: it should stay, outlined in red, doing nothing, and
save and reopen as it was typed. Typing `*   -.5` should make a box that
reads `* -0.5`.

### 43. Faces, bends and cable activity (MOO-524): look and listen, in the app

Type `lfo` and `lfo tri 2hz`, and open both with the arrow at each box's
right. Each should show its settings as small knobs with readouts; the
shape should click through `sin tri saw sqr rnd`, not slide. Drag the first
LFO's depth down and up: one undo should put it back. Click the second
LFO's outlet to arm it and drag the first LFO's rate knob up, then arm the
first onto a filter cutoff and play: the cutoff's wobble should speed up and
slow down at 2 Hz, and the rate knob's arc should move with it. Drag the
middle of a cable: its run should follow the pointer along one axis, and a
double-click on it should put it back where the canvas routes it. Save and
reopen: the bend and the open faces should be as left. Then play with
Preferences > Appearance > Cable activity on each setting. Off: plain wires.
Subtle: the LFO's cable should glow faintly with its swing, never enough to
pull the eye from the boxes. Full: plainly lit. Is Subtle still too loud?
