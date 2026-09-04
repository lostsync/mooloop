# 04 — What this leaves for the general form

Read `00-status.md` and whatever steps 01 to 03 actually recorded. This is
half an hour of writing at the end of the run, and it is what makes the night
count for something beyond the feature.

## The question this answers

`00-status.md` opens on a decision — device, device-plus-modulation, or rack
fragment — and Adam settled it on 2026-09-04: **device, with relative
addressing**, on the plan's own argument that it is not wasted work if a
fragment format later supersedes it.

That argument is a claim, and steps 01 to 03 are the test of it. So write down
what was actually learned:

- **What the `contains` list bought.** Step 01 records `["effect_params"]` in
  the manifest. Now that the format exists, is that enough for a fragment
  reader to distinguish a one-row preset from a run of rows, or does it need
  ordering and boundary information the current record has no place for? If
  the latter, say so — that is the first real cost of having gone specific
  first, and it is much better known now than assumed later.
- **What an effect preset could not express.** The ML-M1 bank's complaint was
  that a patch is a source *plus the modulation that makes it mean
  something*. An effect preset has the same shape of problem the moment a
  rack row is modulated. Did anything in step 02 want to reach for a route?
- **Whether the directory-per-kind layout holds.** It makes a kind mismatch
  impossible, which is right for one row. A fragment spans kinds by
  definition. Does `presets/effects/<kind>/` become an obstacle, or does a
  fragment simply live somewhere else?

## The thing to say plainly

Whether the specific form turned out to be a stepping stone or a detour.
`00-status.md` asserts it is the former. If the night's work says otherwise,
that is the most valuable sentence in this directory and it should be written
without softening.

## Sequencing, which has not changed

`docs/FOCUS.md` queues the rest of this plan behind DS-01, because DS-01's
step 09 ships a second factory bank and the taxonomy and browser want two
banks to design against rather than one. Nothing in steps 01 to 03 touches
either, which is why they could run first.

So this directory does not become the active sequence. Update `FOCUS.md`'s
paragraph to say the device-level half is delivered and what remains is the
browser, the taxonomy surface, and the factory-content mechanism — all still
waiting on DS-01.

## Done when

`00-status.md` records the decision and what building against it taught,
`FOCUS.md`'s preset paragraph is accurate, and the remaining work is named
precisely enough that the next session does not have to re-derive it.
