# Workflows

A **workflow** is a procedure that is run again. A [plan](../plans/) is work
that lands once and then moves to `archive/`; a workflow does not land, it
recurs, and what it accumulates is a record of what each run found.

The distinction is worth keeping because the two fail differently. A plan goes
stale when its steps are done and nobody moves the directory. A workflow goes
stale when the codebase moves under its instructions -- the procedure still
reads plausibly, still produces output, and is quietly looking for something
that is no longer the shape of the problem. So every workflow here carries a
**Runs** section naming what it found and when, and a run that finds nothing
is worth recording too: it is the only evidence the procedure still works.

| Workflow | What it is for |
| --- | --- |
| [rust-slint-boundary/](rust-slint-boundary/) | Finding and fixing values, ranges, ids and policies that are stated on both sides of the Rust/Slint boundary with nothing holding them together. |

## Running one

Workflows are not on `FOCUS.md` and do not interrupt it. Reach for one when
you have half an hour and no particular task, when you are about to touch a
shared constant, or when something has just gone wrong in the shape the
workflow describes.

A workflow's stages are passes over the same material, not steps that complete.
Expect to go back: the fixing stage routinely turns up material the finding
stage missed, because a fix is the first time anybody reads the surrounding
code closely.

## Writing one

A workflow earns its place when a procedure has been carried out by hand more
than twice and the expensive part was **knowing where to look and what to
distrust**, not the typing. If the expensive part was the typing, write a
script and put it in `scripts/` instead -- `scripts/dupe-audit` is what that
looks like, and the workflow beside it is the half a search cannot reach.

Ground every claim in something that happened. A workflow written from first
principles is a plausible essay; one written from a run is a list of the
places the thing actually hides.
