# Transport discontinuity status

**All four steps landed 2026-09-20**, the day the plan was written. The directory came out of Adam reporting that moving around
the app makes the audio glitch, and it covers both the bug he heard and the
mechanism whose absence caused it.

Linear: [MOO-57](https://linear.app/mooloop/issue/MOO-57/selecting-a-pattern-chokes-every-voice-on-every-channel-including-in).

## What the investigation established, so it is not re-derived

- **The symptom is a choke, not a rebuild.** Adam's first guess was the
  renderer being rebuilt, the residual glitch `incremental-structure/` closed.
  It is not: `on_pattern_selected` (`crates/mooloop-ui/src/lib.rs:7186`) sends
  a POD `EngineCommand::SetCurrentPattern` and installs nothing.
- **`SetCurrentPattern` sets `seeked`** (`crates/mooloop-engine/src/render.rs:4507`),
  which makes the block loop call `release_all_voices(0, …)` (`render.rs:5427`),
  which pushes `Event::Choke` onto **every live channel** at offset 0
  (`render.rs:2880`).
- **`Choke` is two different things.** MonoSynth, PolySynth, ML-M1 and ML-P8
  implement it as `release_all()` — a held note stops being held, which is
  exactly what Adam described. Sampler, DrumSynth and DS-01 implement it as
  `choke()`, a few-millisecond hard fade. Both land at offset 0 on every
  channel at once, so they are coherent in the sum.
- **In Song mode it is pure loss.** Every read of `Sequencer::current` that
  reaches playback is inside a `PlaybackMode::Pattern` arm —
  `schedule_pattern` (`sequencer.rs:802`), `schedule_once` (`:656`),
  `automation_lane_at` (`:523`), `has_automation_at` (`:471`). Song mode
  schedules from playlist placements and never reads it. In Song mode
  `current` reaches exactly two things: where a recorded note goes
  (`recording_tick`, `sequencer.rs:374`) and what the editor draws.
- **`restore_lanes_left_behind` is a no-op in Song mode**, not a second
  fault: `covering_pattern_at` ignores `current` there and the tick has not
  moved, so nothing is handed back. It is wasted work on the audio thread.
- **In Pattern mode the discontinuity is genuinely owed.** Pattern position is
  `wrap_tick(global_tick, current_pattern.length_ticks())` (`sequencer.rs:969`),
  so switching to a pattern of a different length really does move the
  playhead. (Written expecting step 02 to stop it being owed. It does not --
  the debt is the changed note source rather than the moved playhead, which
  step 02 established by getting it wrong first. Step 03 is what retires it.)
- **`on_pattern_selected` is the only navigation gesture that reaches the
  audio thread.** `on_channel_selected`, `on_bus_selected` and
  `on_device_selected` send nothing. `on_automation_lane_selected` does send,
  and is an edit rather than navigation — step 04 renames it for that reason.

## What Adam settled, 2026-09-20

**Scope: the fix and the general mechanism**, not the fix alone. Asked whether
the plan should stop at the bug, he took "fix + the general mechanism":
deferred commands, and a real reset/discontinuity hook on `AudioNode`.

**Behaviour: none of the three options offered.** He was asked to choose
between decoupling the view from playback, queueing the switch to a musical
boundary, or keeping the instant switch and narrowing the choke, and rejected
the framing:

> *"im unsure i like any of these options. its really only an issue in song
> mode. when i switch patterns and the song is playing, that's a view-only
> operation, but it does change the 'active' pattern, like if i were to start
> recording, that's where it goes, and i'd be able to edit the pattern, etc. i
> just want to be able to move around the app freely without having audio
> issues."*

That is the right reading and it is a better rule than any of the three. The
active pattern stays one shared thing — view, edit target, record target — and
the engine's job is to **charge only for what actually changed**. Song mode
changes nothing that is scheduled, so it costs nothing. This is why step 01 is
an engine change and not a UI change: the command still has to be sent,
because `recording_tick` needs it.

## What Adam settled next, 2026-09-20

Both questions this file left open were answered the same day, in one line:
*"immediate and yes."*

**A Pattern-mode switch stays immediate.** Step 02 still builds the deferred
command class -- it is the missing granularity and other work wants it -- but
the pattern selector does not adopt it, and the Pattern-mode release stays.
That keeps the FL convention mooloop already has, and it means step 02 is no
longer what retires the last choke; nothing does, because in Pattern mode
under a running transport the release is genuinely owed.

**A switch with the transport stopped stops releasing.** That is the "yes",
and it settles the one thing step 01 proposed against a comment already in the
tree. The reasoning is recorded at the test rather than only here:
`switching_the_viewed_pattern_while_stopped_leaves_an_audition_alone`.

## What step 01 changed

`Sequencer::set_current_pattern` answers `bool` — whether the selection
actually moved — and `RenderState::apply_command` pays the discontinuity only
when that is true, the mode is `Pattern`, and the transport is running. Four
cases that used to cost every sounding voice on every channel now cost
nothing: a Song-mode switch, a switch with the transport stopped, a
re-selection of the pattern already current, and a selection past the end of
the bank.

The Pattern-mode release under a running transport is unchanged, and its
existing test still pins it.

**The two debts the command owes do not share a condition**, and the first
draft had them sharing one. The voice release is conditional on the transport
running; the lane restore is not, because `process` resolves lanes while
stopped on purpose -- *"so that a knob does not jump the moment you press
play"* (`render.rs:5509`). Behind one condition, a paused switch off an
automated pattern would have left the destination parked at whatever the
curve last resolved, which is the exact hazard `restore_lanes_left_behind`
was written for in the first place. Caught on a re-read of the diff, and
`switching_off_an_automated_pattern_hands_the_knob_back_while_stopped` now
pins it.

## What step 02 changed

`MusicalEdge` and a deferred command class: sent with
`CommandSink::send_deferred`, held in renderer state, resolved to an absolute
tick on arrival, and applied between spans at the edge. `Transport::advance_looped`
cuts the block at that tick, and the cut is deliberately **not** a jump --
only a loop fold owes the release.

**Two things the plan had wrong, found by building it.**

The block was not already split at musical edges. `advance_looped` cut at the
loop end and nowhere else, and the per-span scheduling pass ran only under a
loop; the cut had to be added rather than reused.

And step 02 does not retire the Pattern-mode choke, which the draft claimed it
would on the grounds that a switch at the pattern end does not move the
playhead. It does move it -- `wrap_tick(768, 512)` is 256 -- but the real
error is deeper: **the release is owed because the note source changed, not
because the playhead moved.** Implemented the other way, with `seeked`
conditional on the position moving,
`switching_pattern_while_playing_releases_the_sounding_voices` failed at once
with the voice still sounding at 0.2519. Two patterns of one length leave the
playhead exactly where it was and still strand the note-off, because it lives
in the pattern that stopped being scheduled. Reverted; step 01's rule stands.

Releasing *only* the stranded voices needs per-voice knowledge, which is step
03's hook. Nothing before then retires the last choke.

**No caller yet.** The pattern selector stays immediate, so nothing in the
app defers anything; the mechanism is exercised by tests alone until a
gesture wants it.

## What step 03 changed

`AudioNode::on_discontinuity(Discontinuity)`, defaulted to nothing, said by
the engine at a seek, a loop fold, a stop and a Pattern-mode switch. Delay,
modulation, reverb and plate opted in and clear what they are holding on a
seek; every one of them declines a program change, because time is still
continuous there. Aux In and the retained-audio buffer decline outright, in
writing.

The distinction the hook exists for shows up inside modulation: its own
`reset` restarts the LFO, and `on_discontinuity` deliberately does not. Free
-running state has to arrive at the same phase whether or not the transport
moved, exactly as it does across a sleep, or a bounce stops matching a take.

**The voice path was not migrated.** `release_all_voices` still synthesises
`Event::Choke` for a seek. The hook makes the alternative expressible, which
is what the plan claimed for it, but changing what a seek does to held notes
is a behaviour change worth asking about rather than bundling into a contract
addition. Step 02's overhanging-note case is now expressible and still not
implemented.

**Audible change:** reverb and delay tails no longer ring across a seek.

## What step 04 changed

The rule in `AUDIO_ARCHITECTURE.md` and `CURRENT.md`,
`automation-lane-selected` renamed to `automation-lane-opened` because it is
an edit, and `scripts/dupe-audit navigation-sends`, which reports zero.

**The zero needed an allowance the plan did not account for.**
`on_pattern_selected` still sends `SetCurrentPattern` -- step 01 was an engine
change and left the send deliberately, since `recording_tick` reads the active
pattern -- so a check for "a selection handler that sends" would report it
forever. It reports a selection handler that sends anything *else*, which is
the shape the bug had.

Written before the rename and run against that tree, where its first draft
found nothing because it matched `EngineCommand::` constructors and the
automation handler sends a command a session method built. Matching the
channel instead reported both handlers the plan predicted.

## Steps

| Step | What it does | Cost |
| --- | --- | --- |
| 01 | **Landed 2026-09-20.** `SetCurrentPattern` owes a discontinuity only when it changes what is scheduled | small, standalone, fixed the report |
| 02 | **Landed 2026-09-20.** A command class that lands at a musical boundary | medium; no face change after all, and no caller yet |
| 03 | **Landed 2026-09-20.** `AudioNode` can be told time moved, and which kind | four devices opted in; the voice path still uses `Choke` |
| 04 | **Landed 2026-09-20.** Navigation must not reach the audio thread — the rule, and a guard | small |

### Two things step 03 left, both closed 2026-09-21

**A fold arrived as a seek.** `Discontinuity` had `Seek`, `Stop` and
`ProgramChange`, and the renderer sent `Seek` for both a seek and a loop
fold, so a device could not decline one without declining the other -- and
what it costs is audible: every delay, reverb and plate ring in the project
is cleared on every lap, so no repeat and no tail ever crosses the loop
point (`reports/fable-2026-09-21.md`, finding 2). `Discontinuity::LoopFold`
now says which it is; every device that cleared on a fold still clears on
one, so **nothing sounds different yet**, and whether a given device should
keep its tail across a fold is Adam's call, one `match` arm at a time.
`a_fold_reaches_installed_nodes_as_a_fold` holds the kind. A fold is a Song
mode loop range turning the playhead back; Pattern mode wraps inside the
sequencer without the transport jumping, so it does not reach the hook at
all.

**The hook stopped at the outer device.** `RenderState::on_discontinuity`
fans out to every strip's source and chain, and a device that *contains* a
device has to forward it again: ML-P8's finishing chorus is a
`ModulationEffect`, the same type that opted in at step 03, and nothing was
calling it (finding 4). One override, one test. The rule is now written in
`docs/AUDIO_ARCHITECTURE.md` under the node contract, because the plugin
slot is the same shape at a larger size.

**And the cost was measured, because the finding estimated it.**
`block_cost::loop_fold_cost` times sixteen channels of delay + reverb +
plate at a 512-frame block, ten seconds of blocks, with the one-bar loop
range on and off -- so the difference is the fold and not the arrangement.
Release, container, 2026-09-21:

```
  loop        median ns   max ns
  off           2445709  4563766
  on            2350587  4683882
```

**No spike.** A fold lands about every two seconds there and the worst block
differs by 120 us, which is 2.6% and smaller than the spread between the two
medians -- one of which is *lower* with the loop on. The finding's arithmetic
(768 KB per delay line, times every tail-holding ring) is right about the
bytes and wrong about what they cost: a `fill(0.0)` over a ring the CPU has
been reading every block is a fast linear write, not a stall.

Two caveats keep this a floor rather than the worst case. The arrangement is
silent -- samplers with no notes -- so the reverb and plate rings hold little
and some of their pages may never have been touched; and one run of one
machine, at a block already costing 2.4 ms, has little resolution left for a
100 us effect. What it does settle is that the fold is not the thing to fix
first. (The report asked for this in `docs/REFERENCE_MEASUREMENTS.md`; that
document is about measuring reference *hardware*, and every other
`block_cost` figure lives in the plan that commissioned it, as this one now
does.)

Step 01 stands alone and is worth landing on its own. Steps 02 through 04 are
the general mechanism and are ordered by dependency: 03 is much easier to
specify once 02 has established what a discontinuity *is*, and 04's guard
wants 01's tests to exist first.

## Where this sits against FOCUS.md

Outside the sequence, like `coreaudio-driver/` and `plugin-hosting/`, because
Adam asked for it directly. Step 01 is a bug fix he feels every session and
should not wait for the sequence; the rest can.
