# Step 04: Aux In

The consumer, and the first thing in this plan anybody can hear. A generator
kind whose sound is another channel's published audio outlet.

## What it is

`DeviceKind::AuxIn`, `GeneratorParams::AuxIn(AuxInParams)`, serialized as
`aux_in`, taking a channel's source slot like any other generator. Its
parameters are few and every one of them is descriptor-addressed from the
start, because the argument that a device can ship without a descriptor table
has now been checked twice and lost twice:

| Id | Control | Kind |
| --- | --- | --- |
| 0 | Source Channel | stepped, `None` plus every channel that publishes audio |
| 1 | Source Outlet | stepped, the source channel's declared audio outlets |
| 2 | Level | continuous, the one thing worth automating |

Source Channel and Source Outlet are structural and modulation-ineligible —
they change the compiled graph, and a modulation route onto one would mean
recompiling the schedule from the audio thread. Level is an ordinary
continuous parameter and a modulation route or an automation lane reaches it
like any other.

**The stepped mapping is wire format.** `docs/FOCUS.md` step 2 turned this up
the hard way on the v1 drum synth: an automation lane persists a stepped
parameter's index, so an index-to-value mapping that lives in the UI is a
second copy of something the project file depends on. The mapping lives in core
beside the enum, once.

## What it is not

It is not a mixer send, and the difference is worth stating because the face
will invite the confusion. A send is a second output from a strip, summed
somewhere else, and the strip keeps its own path. Aux In is an *input*: the
producing channel does not know it is being read, its own output is unchanged,
and nothing about its routing moves. Two channels can subscribe to the same
outlet and neither affects the other.

It is also not a router. One subscription, one channel, one outlet. A device
that could sum four taps would be a mixer, and the thing that makes this worth
building is the edge underneath it rather than the breadth of the device on
top.

## Persistence

A new `DeviceKind` variant and a new `GeneratorParams` variant. Old projects
have neither and load unchanged; the format's defaulted-field rule in
`PROJECT_FORMAT.md` applies to the subscription, which defaults to `None` —
an Aux In with no source is silent rather than invalid.

The subscription names a **channel index**, and channel indices move: channel
delete and paste renumber every channel-scoped address, and the integrity pass
in `mooloop-session` already repairs stranded and dangling ones. An Aux In
subscription is one more channel-scoped address and goes through the same pass
rather than growing a repair path of its own.

## The face

Small, and it should be. A source picker, an outlet picker, a level knob, and
the status line the outlet descriptor already knows how to write — the tap
point, in the words `OutletTap::status` returns, so `pre-level` is visible
before somebody discovers it by wondering why a muted oscillator is audible.

The outlet picker offers the source channel's *audio* outlets only. That is
`OutletDomain` doing the refusal structurally rather than the picker
remembering a rule, which is what the domain exists for.

A refused subscription — a cycle, or a source channel that has changed to a
generator publishing nothing — shows why, from `graph.refused()`. Step 02
keeps that state inspectable precisely so this face has something honest to
draw instead of silence.

## Done when

- A channel set to Aux In, subscribed to ML-P8's `Osc 3` on another channel,
  is audible — with `Osc 3` muted in ML-P8's own mix, which is
  `poly-synth-v2/` step 06's acceptance case and the reason `PreLevel` exists.
- Its parameters are descriptor-addressed, and Level takes a route and a lane.
- A project with an Aux In saves, reloads, and sounds identical.
- Deleting the producing channel leaves the subscription inspectable and the
  consumer silent, rather than pointing at whichever channel inherited the
  index.
- The face refuses to offer a control outlet as an audio source, and says
  where the signal is tapped from.
