# Preset system revisit — plan status

**Steps 01 to 04 ran on 2026-09-04, on branch `feat/device-presets`.** The
effect-level preset exists end to end: format, directory, session path, and
the rack row's save and load controls. Every effect kind ships a factory bank
of five to seven patches. What building it taught is under "What the run
found" below. Adam ran it on 2026-09-04 and confirmed it works, after the
second pass recorded at the foot of this file; the branch landed on `main`.

Queued on its own merits, independent of `docs/NODE_MODEL.md`.

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

## What the run found — 2026-09-04

Step 04 asked three questions of the specific form. The answers, from the
code that now exists rather than from the plan:

**The `contains` list bought exactly what was claimed, and no more.** An
effect preset's manifest reads `contains = ["effect_params"]`, the loader
refuses any entry it does not know before it parses the document, and
`list_presets` leaves such a bundle out rather than offering it and then
refusing the click. That is enough for a fragment reader to tell a one-row
preset from a run of rows: a fragment will carry entries this reader has
never seen (`effect_chain`, `mod_routes`, whatever the boundary contract
names), and today's reader steps aside cleanly. It is *not* enough to
describe a fragment — ordering and boundaries have no place in a flat list of
content classes, and should not: they belong in the fragment's own document,
with `contains` naming that a document of that shape is present. So the
first real cost of going specific first is nil, provided the fragment format
adds an entry rather than redefining `effect_params`. The generator and
channel envelopes were left without a `contains` list, since nothing has
ever written one for them and an empty list is accepted; a fragment format
that wants to inspect those too will have to add it then.

**Nothing in step 02 reached for a route.** The one place that wanted more
than a row was the *pending save*, not the format: a dialog opened from row 1
must still save row 1's device after the rack is reordered underneath it, so
`PresetSaveTarget::Effect` rides the same `SlotRemap` routes and lanes ride.
That is a position-versus-identity problem in the session, and the format
never sees it. Loading, meanwhile, replaces the slot's state in place and
leaves its identity alone, which has a consequence worth stating: **a preset
loaded into a modulated row keeps its modulation.** The LFO aimed at the
filter's cutoff is still aimed there after "Acid Squelch" lands, because the
route names the slot and the slot did not move. The ML-M1 bank's complaint —
a patch that is nothing without its modulation — does not recur for effects
in the same shape, because an effect preset never claimed to carry the
modulation in the first place; the row it lands on brings its own.

**The directory-per-kind layout holds, and a fragment simply lives
elsewhere.** `presets/effects/<kind>/` makes a mismatch impossible to offer,
which is right for one row and right for nothing wider. A fragment spans
kinds by definition, so it gets a directory of its own when it exists;
nothing here has to move.

**Plainly: a stepping stone, not a detour.** Every part of it — the
`EffectSlotState` payload, the `contains` record, the `PresetKind` taxonomy,
the per-kind directories, the session's slot-following save target — is
something a fragment format would keep or extend, and none of it would be
torn out.

Two things the run left that the plan did not anticipate:

- **The factory-content mechanism was extended after all.** Adam asked for a
  factory bank for every device in the same instruction that ran these
  steps, and that outranks step 02's "do not". So `seed_effect_bank` seeds
  each kind's directory once behind a `.factory-v1` marker, the same shape as
  the ML-M1 seeder and with the same limit: it cannot update a shipped patch.
  The marker is per directory rather than global, so a kind added later gets
  its bank on the next launch without touching the ones already written. The
  banks are authored blind, as data in `mooloop_core::effect_factory`, and
  the tests only prove they are in range, distinct from the defaults, and
  survive the disk; whether they *sound* like their names is the listening
  pass.
- **Instrument banks were not authored here, on purpose.** "A factory bank
  for them all" was read as every *effect* kind, because every instrument
  that wants a bank already has a plan step for one that ends in a listening
  pass: the ML-M1's shipped, ML-P8's is `poly-synth-v2/07`, DS-01's is
  `drum-synth-v2/09`. Writing those blind here would pre-empt that work with
  patches nobody had heard. The three superseded generators (`DrumSynth`,
  `MonoSynth`, `PolySynth`) are not worth a bank, and a sampler preset is
  nothing without a sample to ship with it. Every generator can already save
  and load presets; what they lack is content, and the content has owners.
- **The rail's next/previous preset buttons were removed rather than left
  disabled.** Stepping needs a notion of "the preset this row currently
  holds", and two controls that can never do anything are worse than none.
  The header label added in the second pass below is that notion in its
  weakest form — it says where the settings came from, not that they still
  match — so stepping remains a browser question and waits with the browser.


## Why this was queued at all

Unlike the node direction, this has a concrete trigger: Adam asked for
device-level presets and an earlier agent delivered something else. The gap is
real, it has already cost one piece of work, and it will cost the next factory
bank the same way.

Everything below this line is the problem statement the decision was made
from. It is unchanged, and it is still the reason the steps look the way they
do.

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
exist. A device preset can ship without one.

Nothing forces that choice today, because a device-level preset built with
relative addressing and an explicit record of what it contains is not wasted
work if a fragment format later supersedes it.

**Answered at the top of this file: the first, on exactly that reasoning.**
Steps 01 to 04 build it, and step 04 is where the reasoning gets tested
rather than restated.

## Deliberately out of scope, decision or no

- Preset browsing UI beyond fixing the flat list.
- Tags, ratings, search.
- Sharing or importing preset packs.
- Migrating the existing seeded ML-M1 bank. Those are ordinary user presets
  now; leaving them alone is a valid answer.

## The first cut did not work — 2026-09-04, second pass

Adam ran the branch and reported two things: **presets did not load when
chosen from the menu**, and the loaded preset's name should show in the
device header.

The first was a design fault, not a typo, and it is worth recording because
the shape of it is general. The load was routed through the *document*
pipeline — resolve on a worker thread, hand back through the document
channel, apply in the pump — copied from how a channel or generator preset
loads. That pipeline exists because those documents carry samples that must
be decoded off the UI thread. An effect preset is a few hundred bytes of TOML
that references no audio at all, so the thread bought nothing, and the round
trip cost the one guarantee a rack edit needs: that the row named by the
click is still the row the edit lands on when it finally arrives.

The load is now synchronous in the callback and queued as an ordinary
`ProjectEdit`, which is what every other rack mutation already does — the
same path as add, remove and reorder. `LoadTarget::Effect` is gone.

**The lesson to carry:** an effect preset is a *rack edit*, not a document
load. It was filed under the wrong verb, and everything that followed from
that was wrong in the same direction.

That was the reported symptom's most likely cause, and it was not the cause.
The load path was genuinely wrong and is genuinely better now, but the menu
was broken for a second, independent reason, and only a test found it.

**The menu row closed the popup before invoking its callback.** Closing a
`PopupWindow` destroys the repeater item whose handler is still running, so
the call never landed: the menu opened, drew correctly, highlighted on hover,
and did nothing. The insert menu two buttons up does the same close-then-call
and works, because its rows are written out rather than repeated — a static
child survives what a repeated one does not. `close-policy` already dismisses
the popup on that click, so the explicit close bought nothing and cost
everything.

`crates/mooloop-ui/tests/effect_preset_menu.rs` drives the rail with real
pointer events and keeps the insert menu beside it as a control. It was worth
the build it cost: three rounds of reading the Rust — which was correct —
found nothing, and the first pointer test failed immediately. **A control that
opens a window is not verified until something has clicked it.**

The list is also scrolled now, mirroring `MenuField`, because unlike the
insert menu's fixed twelve entries this one fills from disk and would
otherwise grow past the window.
