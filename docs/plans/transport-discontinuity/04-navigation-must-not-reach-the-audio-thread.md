# 04 — Navigation must not reach the audio thread

Read `00-status.md` first. This step writes down the rule the other three
serve, and leaves something behind that can notice when it stops being true.

Adam's own words are the rule, and they are broader than the bug:

> *"i just want to be able to move around the app freely without having audio
> issues."*

## The rule

**Looking at something is not an edit and must not reach the audio thread.**
Selecting a pattern, a channel, a bus, a device, a mixer track or a pane
changes what is drawn and what the next edit will address. None of it changes
what is scheduled, so none of it may cost a voice, a parameter jump or a
block of work in the callback.

Where a navigation gesture *does* have to inform the engine — the active
pattern is also the record target, so `SetCurrentPattern` must still be sent —
the engine charges for what changed, not for the fact that a command arrived.
That is step 01.

This belongs in `docs/AUDIO_ARCHITECTURE.md`'s Control Plane section, beside
the sentence about what may cross the boundary, because it is a statement
about the boundary rather than about the UI.

## Where the tree stands today

Audited 2026-09-20. One violation, and it is the bug:

| Callback | Sends to the engine? | |
| --- | --- | --- |
| `on_pattern_selected` (`lib.rs:7186`) | yes — `SetCurrentPattern` | the violation; step 01 |
| `on_channel_selected` (`lib.rs:8187`) | no | |
| `on_bus_selected` (`lib.rs:8508`) | no | |
| `on_device_selected` (`lib.rs:10027`) | no | |
| `on_automation_lane_selected` (`lib.rs:7944`) | yes — `open_automation_lane` | **not navigation** |

The last row is the interesting one. It sends, and it is correct to send,
because opening a lane is an edit: it mutates the document, marks it dirty and
records undo history. It is named `…_selected` anyway, which makes it
indistinguishable from navigation to anything reading the source.

**Rename it to `on_automation_lane_opened`.** The session method it calls is
already `open_automation_lane`. The rename costs a callback name in
`main.slint` and its Rust handler, and it is what makes the check below
capable of being clean.

## The guard

A `scripts/dupe-audit` check — a lead generator, not a gate, like the other
six: report any `window.on_*_selected` handler whose body sends on the engine
command channel.

Two things AGENTS.md already establishes about a check like this, and both
apply directly:

- **Write it before the fix, against the unfixed tree.** The `bar-arithmetic`
  lesson: a check written after its fix is shaped by what its author already
  knows about, and one written before it is shaped by the tree. Run it first;
  it should report `on_pattern_selected` and `on_automation_lane_selected`,
  and if it reports neither it is not looking at the right thing.
- **One permanent false positive is worse than a narrow check.** Without the
  rename above, `on_automation_lane_selected` is a hit forever, and a check
  that is never clean stops being read. The rename is not cosmetic — it is
  what lets the check have zero as its expected answer.

After step 01 and the rename, the expected result is zero hits, which puts it
in the same category as `one-sided-test`: a regression guard rather than a
lead generator, and its clean run is its answer.

## What it does not do

It does not try to catch the general case by reading code. The check matches
a naming convention and a channel, and a gesture that violates the rule under
a different name will not be seen — the `repeated-line` limit, restated: a
rename hides a copy from a byte search completely. What it catches is the
shape this bug actually had, which is a new selection callback being wired to
the engine by somebody who did not know the rule.

`docs/CURRENT.md` gets the user-visible half of the rule in the same commit:
moving around the app does not interrupt what is playing.

## What landed, 2026-09-20

The rule is in `AUDIO_ARCHITECTURE.md`'s Control Plane section, beside the
sentence about what may cross the boundary, and its user-visible half is in
`CURRENT.md`. `automation-lane-selected` is `automation-lane-opened` in
`main.slint` and `on_automation_lane_opened` in the handler, with the reason
written at the callback. `scripts/dupe-audit navigation-sends` is the ninth
check and reports zero.

**The plan expected zero hits after step 01 and the rename, and that was
wrong.** Step 01 was an engine change: it made the engine charge only for what
changed and deliberately left the send in place, because `recording_tick`
needs the active pattern. So `on_pattern_selected` still sends, and a check
with no allowance for it would report it forever -- the permanent false
positive this file warns against, built in from the first run.

So the check reports a selection handler that sends **anything other than its
one sanctioned command**, with the allowance named and reasoned at the
allowlist. That is a sharper rule than "does not send" anyway: it is the shape
the bug actually had, a new selection callback wired to the engine by somebody
who did not know the rule.

**Run it first is not a formality.** The first draft matched
`EngineCommand::` constructors and reported *nothing* against the unfixed
tree, where the plan said there were two. `on_automation_lane_selected` sends
a command a session method built -- `tx.send(command)` -- so nothing matched.
Matching the channel instead of the constructor reported both, exactly as
predicted. A check written after its fix would have been written against a
tree with nothing to find and would have looked just as clean.
