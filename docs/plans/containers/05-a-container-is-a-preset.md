# 05 — A container is a preset

The ML-M1 bank's recorded complaint: a device preset cannot carry the
modulation that makes the patch mean anything. A container is where that stops
being true, because a container is a thing with an inside, and modulation that
lives inside it travels with it.

## The manifest entry

`docs/plans/preset-system/04` set a condition on having gone device-specific
first: that a later, wider preset would **add** a `contains` entry rather than
redefine `effect_params`. Confirm that still holds when the entry is written —
`validate_contains` (`project/lib.rs:1014`) refuses an unknown entry outright,
which is the mechanism that makes adding one safe, and `EFFECT_PRESET_CONTAINS`
is the list to extend.

A container preset holds `effect_run`: the container row and its whole run, in
order, with their host settings. `effect_params` stays exactly what it is.

## Identity on load

A preset carries device ids from wherever it was saved. They cannot be trusted
on the way in — the destination chain has its own mint and its own live ids —
so **loading a run re-mints every id and rewrites the preset's internal routes
through that mapping.** This is the same shape as `install_with_id`
(`modulation.rs:1636`) raising `next_source_id` past whatever it was handed,
and it must not be left to collide.

## The second dimension of rescoping

`ChannelSetup::rescope_modulation` (`project.rs:418`) rewrites a route's
*channel* scope on a channel-preset load, and leaves bus-scoped routes alone
because a bus exists independently of which channel loaded the setup. The
brief asks whether a route whose destination is *inside* the container being
loaded is the same problem or a new one.

**It is a new one, and it is the easier of the two.** Rescoping answers "which
channel is this route talking about", where the answer is a fact about the
destination chain. Re-minting answers "which device is this route talking
about", where the answer is a fact about the run being loaded — it is a
renaming inside the preset, decided before the preset touches the chain, and
it cannot fail. They compose: re-mint first, rescope second, and neither needs
to know about the other.

The case worth naming: a route saved inside a container whose destination is
*outside* it. That route cannot be re-minted, because the device it names is
not in the run. It is dropped on load, and the drop is reported the way
`integrity.rs` reports an address that resolves to nothing. Silently keeping
it would aim it at whatever now holds that id.

## Done when

A container holding two devices and a modulator route between them saves as
one preset, loads onto a different channel of a different project, and sounds
the same — with the route driving the same device inside the run, and no route
in the destination chain disturbed. Save it twice onto the same chain and both
copies work independently, which is the test that the re-mint actually
happened.
