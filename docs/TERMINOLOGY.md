# Terminology

The words this project uses for the things a signal passes through, and what
each one means here. Settled by Adam, 2026-09-09, after the mixer's vocabulary
had drifted:

> *"i tried not calling them busses later and it confused the shit out of
> everyone."*

So this document exists to be the answer rather than to be re-derived. It
governs interface strings, documentation and, over time, identifiers.
`CURRENT.md` describes behaviour; this describes what to call it.

## The three words

**Channel** — a slot in the **sequencer**. It has a source device, its own
device rack, and note and automation lanes. This is a drum-machine's word for
it and it is the right one: a channel is a thing you program.

**Track** — a column in the **mixer**. It has a fader, pan, mute, its own
device rack, a device strip, sends, and one output. Every channel has one, and
there can be tracks that no channel feeds.

**Bus** and **send** are **roles, not types.** They are what a track is being
*used as*, decided entirely by routing:

| If a track… | it is being used as |
| --- | --- |
| is fed by a channel | an ordinary track |
| is fed by other tracks' outputs | a bus |
| is fed by other tracks' sends | a send (an effects return) |
| is fed by some of each | all of them at once, which is allowed |

**There is no `+ Bus` and no `+ Send`.** Adam: *"i want the mixer to be like
reaper's in that there are just...tracks. if you set it up as a send, it is a
send. if it is a bus, it is a bus. i dont really want to have to make an
fx/aux channel specifically. it just isn't needed."*

This retires `MIXER_PLAN.md`'s three creation buttons and the *signal slot*
name it proposed for the unified thing. The unification was right; the word
for it is **track**.

## Why the two are not one word

In a conventional DAW "track" and "channel" are near-synonyms, because every
track gets a fader. Mooloop separates them because it has **two device racks,
and they mean different things** — the Maschine arrangement, and the reason
the mixer exists at all:

- **A channel's rack is part of the instrument.** Adam's model: *"if all of
  the parts of the program were real devices in a room, the ones from a
  channel's rack are like plugged in and 'captured to tape' — part of the
  instrument signal."*
- **A track's rack is glue and post-processing.** *"…generally. you could
  still throw buffer on the drum buss or whatever."* — so it is a convention,
  not an enforcement. Nothing refuses a device in either place.

Keeping two words keeps that distinction sayable. UA Luna and Harrison Mixbus
are the reference points for the console half; Reaper is the reference for
"they are all just tracks".

## What the code currently calls things

The code has not caught up, and renaming it is a mechanical change nobody
should do in the middle of a feature. Today:

| Code | This document |
| --- | --- |
| `Channel`, `ProjectChannel`, `MAX_CHANNELS` | channel — correct already |
| `MixerBus`, `BusSetup`, `MAX_BUSES`, `EffectTarget::Bus` | **track** |
| `compile_bus_graph`, `CompiledBusGraph` | the track graph |
| `bus` on a channel (its destination) | the channel's track |

`MASTER_BUS` is the one that can stay: the master *is* a bus in the ordinary
sense, and every desk calls it that.

## Two words that are not roles

**Group** is not a fourth kind of thing. Grouping some channels means routing
them to the same track. That track is then being used as a bus, and it is
worth naming "Drums"; nothing else about it differs, and **a grouped
channel keeps its own track**, its fader and its sends. An earlier draft of
`docs/plans/console/` had grouping *create* a track and its members *leave*
the mixer — that is a spreadsheet's idea of a console and it is wrong.

**Analog sum** is a track's switch, not a channel's, for the same reason the
two words are separate: the console being modelled puts its Channel stage on a
mixer strip, and a mixer strip here is a track. Several channels on one track
reach it linearly and the track encodes their sum. Adam, 2026-09-09: *"the
summing thing for now is tracks-only."*

**Strip** means the vertical run of controls a track draws — preamp, EQ,
compressor, sends, fader. It is a face, not an object. See
`docs/plans/console/THE-STRIP.md`.
