# Unison groups

The largest structural change in this plan. It replaces voice allocation, and
voice allocation is where a polysynth's bugs live.

## The decision

**Unison is a real voice multiplier, not an oscillator-detune macro.** A
played note owns a *group* of physical voices that share note and gate state
and receive symmetric detune and pan offsets. Effective note polyphony is
`floor(physical_voices / unison)`.

The macro alternative — detuning the three oscillators harder — is cheaper and
wrong. It cannot produce the density of eight independently drifting voices,
and it consumes the oscillator section that Poly's identity depends on.

## What is wrong today

Allocation is per-note and flat. `select_voice` (`polysynth.rs:195`) finds a
free slot or steals the lowest `age`; `note_off` releases every voice matching
the `event_id` (`polysynth.rs:242`) — which is already group-shaped and is the
one piece that survives unchanged.

`apply_params_to_voices` (`polysynth.rs:147`) handles a polyphony reduction by
setting `active = false` on out-of-range slots. That cuts a sounding voice
dead with no release — an audible click today, and with groups it becomes a
partially-killed note. Fix it in this step: out-of-range voices get a fast
release (`STOP_RELEASE_S`, as `release_all` uses), not an instant kill.

## Do this

### 1. Group allocation

A group is `unison` consecutive-or-not physical voices sharing an `event_id`.
Since `event_id` is already the identity used by `note_off`, the group is
implicit — no separate group table is needed, and adding one would be state
that can disagree with the voices.

What has to change:

- **Allocation is all-or-nothing.** `select_voice` becomes
  `select_voices(count)`, returning `count` slots. Prefer free slots; make up
  the shortfall by stealing.
- **Stealing operates on groups.** Steal the oldest *group* — every voice
  sharing the oldest `event_id` — not the `count` oldest individual voices.
  Stealing half of an 8× unison note is the specific failure the spec calls
  out, and it is what a naive implementation does.
- **`age` is per-group.** Every voice in a group gets the same `age`, so
  `min_by_key` over ages picks a whole group cleanly.
- If `unison` exceeds the polyphony limit, clamp: allocate what exists rather
  than dropping the note.

### 2. Detune and pan within a group

Symmetric spread around the note. For a group of N, member `i` gets a
normalized position in `[-1, 1]`, the same shape `voice_pan` already computes
(`polysynth.rs:34`):

- **Detune** scales that position into a cent offset. The knob is 0-100%
  mapped to a musically bounded spread — tune the maximum by ear; do not
  expose cents directly.
- **Pan** places group members across the field, scaled by Spread.

This is where Spread's meaning changes. Today it pans by *physical slot index*
against the polyphony count (`voice_pan(voice_index, polyphony, spread)`), so
which side a note comes from depends on which slot the allocator happened to
pick — a note is on the left because it was played fourth. With unison groups
the useful semantic is: **Spread pans the members of a group across the
field**, so a unison note is wide and a chord is centred-but-detailed.

At `unison = 1` there is no group to spread, so fall back to today's
slot-based panning to keep 1× behaviour recognizable. Say so in the tooltip's
status-bar text. If listening says a chord should also spread by note, revisit
and record it here.

Group-member offsets stack with the per-voice drift offsets from step 02.
Drift is the slot's fixed character; detune is the group's deliberate spread.
They are independent and both apply.

### 3. Parameters

| Field    | Kind / range              | Default | ID |
|----------|---------------------------|---------|----|
| `unison` | 1 / 2 / 4 / 8, stepped    | 1       | 42 |
| `detune` | 0-1                       | 0.0     | 43 |

`polyphony` keeps ID 15 and its 1-16 range; `spread` keeps ID 16. Unison as a
four-value stepped descriptor rather than a free 1-8 integer: 3× and 5× unison
are not musically interesting and the constraint keeps the allocator simple.

### 4. UI

VOICE page, Allocation section, beside the existing Polyphony stepper: a
Unison selector (`1×` / `2×` / `4×` / `8×`) and a Detune knob. Show the
effective note polyphony — `floor(polyphony / unison)` — as derived text next
to the stepper; without it, "why can I only play two notes" is a support
question.

## Done when

- 2×, 4×, and 8× consume exactly that many physical voices per note.
- Stealing takes a whole group. Test: 8 voices, 4× unison, play three notes —
  the third steals the first entirely and the second is untouched.
- NoteOff releases the whole group.
- No orphans: changing unison, polyphony, or both mid-chord leaves no voice
  active without a group and no voice cut off without a release. Assert that
  after a polyphony reduction, previously-sounding voices are releasing rather
  than instantly silent.
- Detune at 100% on an 8× unison note produces a measurably wider pitch spread
  than at 0%, without changing the perceived fundamental.
- Spread across a group produces a stable stereo field and does not change
  pitch or total gain. Extend `spread_pans_voices_outward` to the group case.
- `unison = 1` with `detune = 0` is bit-identical to the pre-step build.
- No allocation in group selection.
