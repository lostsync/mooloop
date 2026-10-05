# 05 — the contract, rewritten

The documents that say modulation belongs to a channel are rewritten to say
it belongs to the song, so the next agent does not push back on Adam with
the old rule. Markdown only.

## `MODULATION.md`

- **Status** (top): song-wide, with the date and Adam's words from
  `README.md`.
- **Decisions** (`:18-52`): the diagram's `CHANNEL (ownership)` becomes the
  song. *"A channel owns its modulation sources and routes"* becomes *"The
  song owns its modulation sources and routes"*. The device-local paragraph
  stays as it is. *"Patch cords and a full graph editor are deferred"*
  stays, with a pointer to the Song Patch direction as a proposal.
- **Channel collection** (`:141-160`): rewritten as the song collection,
  with the capacity rule (no cap a user meets; preallocated and grown by
  replacement) in place of `MAX_MODULATORS_PER_CHANNEL`.
- **Inter-device and inter-channel data** (`:498`): a route crosses channels
  and reaches tracks; sources name their channel.
- **Scope boundaries** (`:669-691`): remove *"general cross-channel/global
  routing"* from the exclusions.
- **Acceptance criteria** (`:693`): *"A source on one channel can target ...
  on that channel"* becomes any destination in the song; *"follows no
  implicit cross-channel edge"* stays true, because every edge is now
  explicit.

## Elsewhere

- `docs/current/modulation.md`: what the app does now.
- `PROJECT_FORMAT.md`: written in step 01; check it reads as one section.
- `UI_DESIGN.md`, *Channel modulation shelf* (`:496`): the pane.
- `FOCUS.md`: remove the modulation-rack move from *Deliberately not now*.
- `SCOPE.md` §4: move it from *Out* to *In*, dated, in Adam's words.
- `TERMINOLOGY.md`: *module*, *route*, *input* as the pane uses them.
- `IDEAS.md` and `ENHANCEMENTS.md` entries on the rack move: mark done, and
  point at the Song Patch proposal for what is still open.

## Done when

- No document under `docs/` (outside `archive/` and `JOURNAL.md`) says a
  channel owns modulation, or that cross-channel modulation is out.
- Every path and symbol the rewritten sections cite exists (`rg` each one).
- The plan directory moves to `archive/` and the Linear project to
  Completed, in the same change.
