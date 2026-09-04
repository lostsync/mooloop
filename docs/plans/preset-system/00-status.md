# Preset system revisit — plan status

**The opening decision is made. Steps 01 to 04 are written and ready to run
unattended.** Queued on its own merits, independent of `docs/NODE_MODEL.md`.

## The decision — Adam, 2026-09-04

**The unit of a preset is a device, with relative addressing.** The specific
form, not the general one, on this plan's own argument that it is not wasted
work if a fragment format later supersedes it.

What that means in practice, and why it can run ahead of `FOCUS.md`'s
sequencing:

- The gap being filled is the **effect-level preset**, one rack row. Generator
  presets already cover the source slot; there is no effect preset at all,
  and that is what was asked for.
- An `EffectSlotState` contains no route and no `EffectTarget`, so it carries
  **no absolute addressing to get wrong**. The rescoping problem cannot arise,
  which is what makes this form safe to build before the fragment question is
  settled.
- The manifest records **what the bundle contains**, not just what it is, so a
  later fragment reader can tell a one-row preset from a run of rows. That
  record is the condition this plan set on going specific first, and step 01
  treats it as non-optional.
- `PresetSummary.kind` widens from `DeviceKind` to a three-class
  `PresetKind`, which is the **structural half of problem 3** — the flat,
  taxonomy-free list — fixed without building any browser.

Still queued behind DS-01, and unchanged by this: the browser, the taxonomy
surface, and the factory-content mechanism. Those want two factory banks to
design against, and DS-01's step 09 ships the second.

Unlike the node direction, this has a concrete trigger: Adam asked for
device-level presets and an earlier agent delivered something else. The gap is
real, it has already cost one piece of work, and it will cost the next factory
bank the same way.

## What exists today

Two granularities, and nothing between or beside them:

| Preset | Payload | On disk |
| --- | --- | --- |
| Generator | bare `ChannelSource` | `presets/generators/<kind>/` |
| Channel | `ChannelSetup` — source, rack, modulation | `presets/channels/` |

There is **no effect-level preset at all**: no `presets/effects/`, no
per-device save for a rack row. An eight-effect rack row you like cannot be
kept except by saving the whole channel it happens to sit in.

## What is wrong

**1. There is no device-level preset.** This is what was asked for. A device
is the unit a musician thinks in — "that filter setting", "that reverb" — and
it is the one granularity missing. Generator presets come closest but only
cover the source slot, so no effect can ever be saved alone.

**2. The granularity does not match the unit of musical meaning.** The ML-M1
factory bank ran into this directly and the finding is recorded in
`docs/plans/mono-synth-v2/00-status.md`:

> A generator preset is a bare `ChannelSource` with nowhere to put a
> `ModRack`, and Sequence Bleep is an S&H LFO routed to cutoff — it is nothing
> without one.

So a six-patch instrument bank had to ship as *channel* presets, dragging
along everything a patch did not need, and landing in the channel menu beside
unrelated device kinds. The patch was not a channel; it was a source plus the
modulation that made it mean something, and no granularity described that.

**3. The browser has no taxonomy.** Channel presets appear in one flat list
alongside device kinds. Recorded as a known cost when the bank shipped. Adding
any further preset class to that same undifferentiated list turns a small
annoyance into a real one, so the taxonomy should be fixed in the same pass
rather than after it.

**4. Factory content has one mechanism, and it is a first-run seed.**
`seed_mlm1_bank` writes patches into the user's directory once, guarded by
`.ml1-factory-v1`, after which they are ordinary user presets. That was the
right small choice for one bank — nothing in the browser, loader, or on-disk
format had to learn about a second class of preset — but it means factory
content cannot be updated, and a renamed device leaves already-seeded patches
carrying the old label. That happened: patches seeded before the ML-M1 rename
still read `ML-1`.

## What is already solved, and should not be re-solved

**Fragment portability.** A `ModRoute` named its destination channel
absolutely, so a channel preset saved from channel 3 kept modulating channel 3
when loaded onto channel 0. `rescope_modulation` runs on the channel-preset
load path and fixes it. Any new preset granularity inherits this problem the
moment it can contain a route, and inherits the solution with it. Kits are
unaffected because their channels land on the indices they were saved from.

## The question the design has to answer

**What is the unit of a preset?** The current answer is "a whole channel, or
a bare source", and both are wrong for the common case. The candidates:

- **Device** — one rack row or the source slot, with its parameters. What was
  asked for. Cannot express a patch that depends on modulation.
- **Device plus its modulation** — the ML-M1 bank's actual shape.
- **Rack fragment** — an ordered run of rack rows with a declared boundary,
  droppable anywhere the boundary fits. The `NODE_MODEL.md` shape, and a
  superset of the other two.

The third subsumes the others and is the only one that survives the node
direction, but it is also the only one that needs a boundary contract to
exist. A device preset can ship without one. **Whether to build the general
form now or the specific form first is the decision this plan opens with, and
it is Adam's.**

Worth noting: nothing forces that choice today. A device-level preset built
with relative addressing and an explicit record of what it contains is not
wasted work if a fragment format later supersedes it.

## Deliberately out of scope until the above is settled

- Preset browsing UI beyond fixing the flat list.
- Tags, ratings, search.
- Sharing or importing preset packs.
- Migrating the existing seeded ML-M1 bank. Those are ordinary user presets
  now; leaving them alone is a valid answer.
