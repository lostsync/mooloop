# Mixer and Signal-Slot Design

Status: proposed v0.1 design, August 2026.

This document defines the mixer, groups, auxiliary sends, and routing model
Mooloop is growing toward. It is the product and architecture target for this
surface; `CURRENT.md` describes the older fixed master-plus-16-bus prototype
that exists today. `AUDIO_ARCHITECTURE.md` remains the contract for compiling
editable routing into a realtime-safe executor.

The target is a capable mid-size live console inside a pattern instrument,
not a half-built REAPER clone. It should make ordinary grouping, effects
returns, and rerouting quick and understandable now, while preserving one
coherent path to greater routing freedom later.

## The decision

Mooloop has one general **signal slot** primitive. In the mixer, a track,
channel, bus, group, and send return are all signal slots with different
starting roles and UI affordances, not distinct audio-engine species.

A signal slot may:

- own an optional source generator and its sequenced note lane;
- receive the summed output of any number of other slots;
- host an ordered insert chain;
- have a fader, stereo balance, mute, meters, name, colour, and output route;
- send a tapped copy of its signal to zero or more other slots; and
- feed the master directly or through other slots.

The master is a special signal slot: it receives signal, owns inserts and
output controls, but has no source, no sends, and no output route. It cannot
be deleted.

This is deliberately more unified than traditional console language. The rack
can still call a source-bearing signal slot a *channel*, the mixer can call a
summation-oriented one a *bus* or *group*, and a convenient auxiliary target a
*send return*. Those words describe a role, not a different implementation.

## What a new project contains

The blank-console template starts with:

```text
Master
Track 1 … Track 8       eight empty source-capable signal slots
Send A, Send B           two empty signal slots marked as send returns
```

`Track 1` through `Track 8` route to Master. `Send A` and `Send B` also route
to Master; they contain no effect by default and receive no signal until a
send level is raised. The starter-kit template assigns its four generators to
the first four tracks instead of introducing a second, hidden channel bank.

`+ Track`, `+ Bus`, and `+ Send` create more signal slots. They choose useful
defaults but do not create a different graph type:

| Action | Creates | Default route | Default presentation |
| --- | --- | --- | --- |
| `+ Track` | source-capable empty slot | Master | Track N |
| `+ Bus` | receive-only empty slot | Master | Bus N |
| `+ Send` | receive-only empty slot | Master | Send A/B/N; offered as a send target |

A user can change a slot's presentation later without changing its audio.
For example, a drum group is normally created with `+ Bus`, but an unused
track can become a group by removing its source and renaming it. A send return
is only a marked bus; it can receive a normal output route too.

There is no user-facing count ceiling on tracks, buses, returns, effects, or
send controls. Project installation may refuse a graph whose measured memory
or realtime cost cannot be prepared on the current machine; that report must
name the resource and the requested use. A small, silent count cap is not an
acceptable substitute.

## Signal flow and send semantics

Each slot produces one stereo signal for a block. The signal has this order:

```text
optional generator + summed incoming routes
                |
                v
           insert chain
                |
       +--------+---------+
       |                  |
       v                  v
pre-fader sends      fader / mute / stereo balance
                          |
                   +------+-------+
                   |              |
                   v              v
             main output      post-fader sends
```

The signal at an insert is the sum of the source and all incoming main/send
routes. It is then processed once by that slot's inserts. This makes a group
compressor, a reverb return, and a normal source track ordinary instances of
the same rule.

Every send has these fields:

```text
source slot ID
target slot ID
level                 linear internal value, displayed in dB
tap point             pre-fader or post-fader
enabled               explicit on/off; a zero level is still a valid setting
```

The v0.1 behavior is precise:

- A **pre-fader** send is tapped after inserts but before the source slot's
  fader, main mute, and stereo balance. Its own enabled state and level still
  apply. This is useful for a cue-like effect feed or a deliberately constant
  parallel path.
- A **post-fader** send is tapped after fader, mute, and stereo balance. It is
  the default and has the familiar console behavior: pulling down or muting a
  track also pulls down its reverb/delay feed.
- A send's level is applied after its chosen tap. Send level must be smoothed
  like every other audio gain control.
- A slot's main output is its post-fader signal multiplied by its output-route
  gain of unity. The main route is not a hidden send and has no separate gain
  in v0.1.
- The destination sees all main-route and send-route contributions as ordinary
  summed inputs. It cannot tell why an input arrived.

Every non-master slot can create sends. `Send A` and `Send B` merely appear
first in the target menu, so the fast console workflow is one click away while
the data model is not constrained to it. A source can send to a group; a group
can send to a return; a return can feed a group, provided the completed graph
is acyclic.

## Routing rules

The editable graph has two audio-edge kinds:

1. Each non-master signal slot has exactly one **main output** route.
2. A signal slot has zero or more **send** routes.

Both are audio edges. Both contribute to the destination's signal. Both are
included in dependency ordering. Their different meanings are only the source
tap point and their UI presentation.

All ordinary audio routing is a directed acyclic graph ending at Master. This
includes a send: `Track 1 -> Send A -> Master` is a normal two-edge audio
path. The editor refuses a main or send route that would create a cycle and
shows why that destination is unavailable. It never accepts a route and then
silently changes it behind the user's back.

Feedback is not a loophole in this rule. It is a later, explicit delay-bearing
device or edge with a stated delay, gain safety, persistence contract, and UI.
The current block's accidental previous-bus audio is never feedback.

The main target menu contains Master and every legal signal slot. The send
target menu contains the same legal targets, with slots marked Send shown
first. The selected slot itself and every slot downstream of it are disabled
with a short explanation such as “would feed back into Drums.”

## Editable model

Names below are architectural, not a demand to preserve current Rust type
names verbatim.

```rust
type SignalSlotId = u32;
type SendId = u32;

struct SignalSlot {
    id: SignalSlotId,
    name: String,
    presentation: SlotPresentation, // Track, Bus, Send, Master
    source: Option<ChannelSource>,
    note_lane: Option<NoteLane>, // present for a Track even before it has a source
    inserts: Vec<EffectSlotState>,
    mixer: SlotMixerState,
    main_output: Option<SignalSlotId>, // None only for Master
    sends: Vec<AuxSend>,
}

struct AuxSend {
    id: SendId,
    target: SignalSlotId,
    level: f32,
    tap: SendTap, // PreFader | PostFader
    enabled: bool,
}
```

`source: None` is an empty, bus, or return slot. A Track slot reserves its
note lane even before it has a source, so choosing or replacing a source never
discards notes. A source-bearing slot remains free to receive other slots as
well. That permits a musically useful hybrid slot, but the default UI does not
push people toward it.

Slot IDs and send IDs are stable identities, never row positions. Inserting,
reordering, hiding, or deleting a strip therefore cannot change note,
automation, route, selection, or preset targets. Structural edits prepare a
new project/plan generation off the audio thread and swap it at a block
boundary; live high-rate values address these IDs directly.

The persisted project has no `MAX_SLOTS`, `MAX_SENDS`, or small fixed bus-bank
field. The render plan owns the bounded, prepared arrays/vectors for the
specific project generation. Its resource estimate includes at least active
slots, connections, inserts, channel voices, graph-compensation storage, and
buffer-device memory. Refusal is explicit and leaves the active generation
unchanged.

## Compiled render plan

The current mixer has the right broad shape—editable data compiled away from
the callback—but its `[u8; MAX_BUSES]` destination permutation models one
output per bus. It must be replaced, not stretched, before sends land.

The control thread builds a render plan with:

- normalized stable slot and send IDs;
- validated main and send audio edges;
- a topological slot order, sources before every consumer;
- per-destination input-mix operations;
- buffer ownership/assignment for every active slot and tap;
- cumulative latency at every input, plus prepared compensation delays; and
- diagnostics for rejected edits, resource refusal, and legacy-file repair.

The callback executes this finished plan only. For each slot in its compiled
order, it clears/prepares its input buffer, produces its source if present,
sums scheduled upstream buffers, processes inserts, emits the applicable
pre-fader send contributions, applies output state, and emits its main and
post-fader contributions. It allocates nothing, traverses no editable graph,
and never validates a route.

Parallel sends make latency compensation a prerequisite, not a polish item.
At every summing point, the plan computes the longest upstream arrival and
preallocates delays for shorter arrivals. A fully wet return and a dry main
path must remain time-aligned if they later meet. `AudioNode`'s existing
reported latency and internal dry-path alignment are the starting point;
graph-level compensation must precede user-visible sends.

True sidechain inputs are not sends. A sidechain adds a dependency without
mixing its signal into the consumer's main input, and needs a typed auxiliary
port in `AudioNode`. The plan must model that later as a third edge category,
not overload `AuxSend` or borrow another slot's audio buffer.

## Mixer interface

The mixer is a horizontal strip work surface, not a routing spreadsheet. It
uses the existing fixed-height modules and faders from `UI_DESIGN.md`; dynamic
strip count scrolls horizontally rather than compressing the controls.

```text
 [MASTER] | [TRACK 1] [TRACK 2] ... [BUS DRUMS] | [SEND A] [SEND B] | + Track + Bus + Send
```

- Master is pinned at the left and visually distinct.
- Tracks and buses share strip geometry. A small role label/icon identifies
  `TRACK`, `BUS`, or `SEND`; names remain user-controlled.
- Send returns appear after a divider because that is a useful console
  convention, not because the signal engine treats them specially.
- Each strip shows, in one stable vertical order: role/name, input/activity
  indication, insert count/device shortcut, main destination, the two
  preferred-send controls, mute/solo, meter, and fader/balance.
- `Send A` and `Send B` are always visible as compact fader/knob pairs on a
  strip. If either default return was removed, its control becomes an
  `Assign send` affordance. A `Sends…` affordance opens the complete dynamic
  list. Added sends do not make every strip wider or move its fader.
- Clicking a strip selects that signal slot and points the existing lower
  device rack at its inserts. A source slot additionally exposes its source
  face and note editing; an empty/bus/send slot exposes only the device rack
  and slot controls.
- Main destination is a long/dynamic target menu, not an enormous selector
  bank. Disabled destinations stay visible with their cycle reason.
- Adding/removing/reassigning slots and routes are undoable project commands.
  Parameter drags remain high-rate commands, as they do today.

The rack retains its fast source-focused view. The mixer must not require a
user who just wants to sequence a kick to first understand buses. Routing
starts at the useful default (main to Master, zero sends), and more capability
reveals itself only when the user asks for it.

## Metering, mute, solo, and deletion

Each slot exposes a post-insert input meter and a post-fader output meter.
The slot strip's primary meter shows audible post-fader output; the device rack
continues to show individual device input/output. Send meters are deferred:
they can be added once they materially improve diagnosis without turning every
strip into a patch bay.

Mute silences only the slot's post-fader main output and post-fader sends.
The slot and its inserts continue processing so tails decay. Pre-fader sends
continue, by definition. This behavior is shown in the tooltip and is why a
send has its own enabled control.

Solo is part of the v0.1 mixer pass, rather than a decorative button. It is
an AFL-style monitor tap: when one or more slots are soloed, their own
post-fader/post-balance signals are summed into the Master monitor feed in
place of the ordinary Master input. The slots otherwise continue through the
normal graph, so their tails and sends keep running. To listen to the result
of a drum group or a reverb return, solo that group or return; v0.1 solo does
not try to infer an arbitrary downstream listening path. Solo therefore does
not change routing, allocation, or stored project state. Solo-safe and
cue/control-room routing are later work.

Deleting a non-master slot is an explicit structural operation. If it has
incoming main routes or sends, the confirmation names them and offers an
explicit replacement destination (Master by default). It then removes the
slot's outgoing sends and reassigns inbound routes to that chosen destination.
There is no invisible orphan repair in an interactive edit; undo restores the
whole structural snapshot. Loading malformed legacy data may still repair to
Master with a visible diagnostic.

## Persistence and migration

This requires a new project-format version. The old v1 model stores positional
`ProjectChannel` entries plus a fixed master-and-16-bus bank; it cannot
represent stable slot identities or parallel sends faithfully.

When loading v1:

1. Create Master.
2. Convert every v1 channel to a source-bearing Track slot, preserving source,
   notes, inserts, gain, pan, mute, and display name.
3. Create a Bus slot only for a legacy bus that is semantically used: it has
   an incoming channel/bus route, inserts, or non-default name/mixer state.
   Omit unused default buses rather than carrying the old 16-strip ceiling
   forward as empty clutter.
4. Map each legacy channel/bus output to its converted target; a default,
   omitted bus maps directly to Master, which is level-neutral under the
   existing bus balance law.
5. Assign stable IDs, validate/compile the graph, and expose any repair in the
   load report. Saving writes only the new version.

Kit and channel documents should preserve a source slot's own inserts and
mixer state, not duplicate an entire project routing graph. A future reusable
bus/return preset is a separate document type; it must not accidentally bring
in unrelated track routes.

## Delivery sequence

This design does not override `FOCUS.md`; command history, modulation, and the
buffer device remain the current order. When the mixer/sends pass begins, keep
it as vertical slices:

1. **Dynamic slot project model.** Stable IDs; project v2 migration; one
   main output per slot; no user-visible count bank. Preserve current audible
   routing through the new compiler and swap path.
2. **General graph plan and latency compensation.** Dynamic compiled DAG,
   buffer/input-mix operations, diagnostics, and impulse/null tests. No send
   controls yet.
3. **Slots in the mixer.** Blank-console template, add/remove/rename/reorder,
   source and bus roles, main route picker, device-rack selection, and undo.
   This stage is useful by itself: it replaces the 16-bus bank with a coherent
   console.
4. **Aux sends and returns.** Add Send A/B defaults, pre/post taps, send
   targets, level smoothing, routing diagnostics, persistence, and the mixer
   strip affordances. Demonstrate delay/reverb-style returns audibly.
5. **Solo and refinements.** Audible monitor solo, meter details, slot/bus
   presets, and later stem export where it follows naturally.

Each stage swaps whole prepared state for structural changes. Do not try to
grow a callback-owned vector because someone clicked `+ Send`.

## Done means

The v0.1 mixer pass is complete when all of these are true:

- A blank project visibly has Master, eight empty tracks, and two empty send
  returns; a starter kit occupies ordinary tracks in that same console.
- A user can create, name, reorder, group, and delete signal slots without
  changing unrelated note lanes or routes.
- A user can route Kick and Snare to `Drums`, insert compression on `Drums`,
  then route it to Master.
- A user can add delay to `Send A`, raise post-fader sends from two tracks,
  and hear the return follow those track faders; switching one send to
  pre-fader has the documented different result.
- Invalid cycles are explained before an edit commits. Loading a malformed
  project reports any repair and still opens safely.
- Parallel dry/wet paths remain latency-aligned in realtime and offline
  renders at several block sizes.
- Adding, removing, or rerouting slots/sends never allocates, frees, locks,
  or validates topology in the audio callback.
- The mixer is visually checked with the software-rendered snapshot workflow
  at desktop and narrow widths, including a project with more strips/sends
  than fit onscreen.

## Explicitly deferred

- true auxiliary sidechain inputs and ducking effects;
- deliberate feedback edges and feedback safety controls;
- external audio inputs, hardware inserts, and JACK port routing in the mixer;
- cue/control-room buses, solo-safe, solo-in-place variants, and talkback;
- VCA/folder groups, surround/multichannel slots, and plugin pin routing;
- patch-cord/canvas routing UI; and
- a promise of REAPER-equivalent routing in v0.1.

Those are not reasons to keep a small fixed bus bank. They are separate
features with additional signal, timing, or interaction contracts.
