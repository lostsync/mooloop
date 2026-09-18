# Device registry status

Linear: project [Device registry](https://linear.app/mooloop/project/device-registry-c98b5e8712bd).

Nothing has landed and no steps are written. This directory is a survey,
written 2026-09-11 from the second of two bites taken while adding
`EffectKind::Preamp`, and `README.md` is the whole of it.

## What landed instead, the same day

The fragility that prompted the survey is fixed without a registry, in
`refactor(rack)`:

- `effect_kind_index`, `effect_kind_units` and `device_kind_to_int` are public,
  so a UI test addresses a device the way the application does. Four test
  files held forty-three lines of bare device integers -- twenty-seven effect
  kinds and sixteen `set_source_kind(n)` -- and hold none.
- `the_insert_menu_offers_every_kind` clicks every row of the insert menu and
  requires the answers to be every kind exactly once, rather than clicking the
  last row and requiring it to report `ALL.len() - 1` -- which had been a
  silent demand that the menu list kinds in numbering order.
- `the_menu_and_the_faces_cover_every_kind` is new, and reads both markup
  lists back out of `device-rack.slint` and `main.slint` to check them against
  `effect_kind_index`. A kind with no face arm had been a device that inserts
  and draws an empty frame, and nothing failed.

So a kind can be appended, and put in the menu wherever it reads best, and
the three lists are checked for agreement in about a millisecond.

## Where this sits against FOCUS.md

**Parked, by Adam on 2026-09-12**, under `FOCUS.md`'s "deliberately not now".
Adam asked what it would take, not for it to be built. One piece of it is
exempt and `FOCUS.md` says so: take the face host component if a device step
already has `main.slint` open, but do not open one for it. The survey's
own recommendation is that the first item -- a face host component that takes
the 245 duplicated lines out of `main.slint`'s fourteen face arms -- is worth
doing on its own terms whether or not anything else here ever is, and that
generating markup from a Rust table is possible and not worth what it costs.
