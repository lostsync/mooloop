# 04 — a track's fader and pan follow a lane

MOO-419, which started this plan. A track's inserts can already be automated
(`render.rs:10074` passes the automation block to a bus chain). **Its fader
and pan can't.**
- The bus walk applies a plain `strip.output.apply(&mut strip.bus, frames)`
  (`engine/src/render.rs:10128`).
- A channel's strip resolves `resolve_strip_segments` (`:3888`) and applies
  them with `apply_segments` (`:4096`), re-aiming its smoothing per control
  subdivision.

Nothing in the session offers a track's strip as a destination either.
`automation_destinations` lists bus inserts but no bus strip
(`session/src/session.rs:800`).

## The change

- **Engine.** The bus walk calls `resolve_strip_segments` with
  `EffectTarget::Bus(i)` and applies the segments as the channel strip does.
  - The base values are the track's own `volume` and `pan` (`core/src/mixer.rs:115`).
  - The modulation argument is `None`, because tracks have no modulation rack.
  - The function already takes the scope as an argument. Check it doesn't
    assume a channel past that.
- **Session.** A track's `STRIP_PARAM_VOLUME` and `STRIP_PARAM_PAN` become
  destinations, for song lanes (step 03's `song_automation_destinations`) and
  for pattern lanes, which may already name a bus (`project/src/integrity.rs:1734`).
- **The knob.** While a lane drives a track's fader, the fader follows it on
  screen and doesn't fight it (`effect_is_driven`, `render.rs:7097`, for the
  strip). The mixer strip shows that the fader is automated, as a channel
  strip does.
- **MIDI mapping** already reaches a track's fader and pan
  (`session/src/midi.rs:736-747`). A mapped controller moving a driven fader
  behaves as it does on a driven channel fader.

Sends and mute stay out (`README.md`, *Not in this plan*).

## Done when

- An engine test beside the channel strip-segment tests (`render.rs`, near
  `:13446`): a lane on `EffectTarget::Bus(n)`'s `STRIP_PARAM_VOLUME` moves the
  bus's gain per control tick, with no zipper. The same for pan.
- A song lane on a track's fader, drawn in the panel, is heard. So is a
  pattern lane on it.
- The track's mixer strip shows its fader moving while a lane drives it.
- MOO-419 closes with a comment naming the commit.
