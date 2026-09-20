# Transport discontinuity status

Nothing has landed. This directory was written 2026-09-20, out of Adam
reporting that moving around the app makes the audio glitch, and it covers
both the bug he heard and the mechanism whose absence caused it.

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
  playhead. Step 02 is what stops it being owed.
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

## What is not settled

**Whether a Pattern-mode switch should become queued or stay immediate.**
Step 02 builds the mechanism either way; which one the pattern selector uses
is Adam's call and is asked there rather than assumed. Queueing is the
Elektron/Ableton convention and is what removes the last reason to choke;
immediate is what mooloop does today and is the FL convention.

## Steps

| Step | What it does | Cost |
| --- | --- | --- |
| 01 | `SetCurrentPattern` owes a discontinuity only when it changes what is scheduled | small, standalone, fixes the report |
| 02 | A command class that lands at a musical boundary | medium, needs a face change |
| 03 | `AudioNode` can be told time moved, instead of being handed fake note events | touches every DSP node |
| 04 | Navigation must not reach the audio thread — the rule, and a guard | small |

Step 01 stands alone and is worth landing on its own. Steps 02 through 04 are
the general mechanism and are ordered by dependency: 03 is much easier to
specify once 02 has established what a discontinuity *is*, and 04's guard
wants 01's tests to exist first.

## Where this sits against FOCUS.md

Outside the sequence, like `coreaudio-driver/` and `plugin-hosting/`, because
Adam asked for it directly. Step 01 is a bug fix he feels every session and
should not wait for the sequence; the rest can.
