# Fix the reverb's wet level and the wet/dry crossfade

## Problem

Adam: "1% reverb feels like 15% if i use the one in the device frame."

The generated impulse response is normalized by **peak**, at
`crates/mooloop-dsp/src/effects/reverb.rs:205-215`:

```rust
// A predictable peak keeps default wet level musical across room sizes.
let peak = /* max abs across both channels */;
if peak > 0.42 {
    let gain = 0.42 / peak;
    ...
}
```

Peak is the wrong measure for an IR. The response is a direct impulse plus
early reflections (`reverb.rs:174-179`) followed by a dense noise tail of up
to two seconds (`:192-203`). Its peak is dominated by the single direct
impulse, but its *energy* — which is what convolution actually applies to
sustained input — is dominated by the tail. Constraining the peak to 0.42
therefore says almost nothing about how loud the wet path is, and for dense
material it comes out many dB above the dry signal it is blended against.

At 1% wet that leaves an audible reverb, which is exactly the reported
symptom. Step 02's measurement 5 confirms the size of the error.

Separately, the host wet/dry blend is a linear crossfade
(`crates/mooloop-engine/src/render.rs:619-622`):

```rust
bus.l[frame] = (self.dry.l[frame] * dry + bus.l[frame] * wet) * trim;
```

For a wet signal decorrelated from the dry — a reverb, a chorus — a linear
crossfade dips about 3 dB at the midpoint. This is a real but much smaller
problem than the normalization, and it does not explain the 1% case; fix it
because it is wrong, not because it is the cause.

## What to do

1. Replace the peak normalization with an energy normalization targeting
   equal wet/dry loudness. Normalize the IR so that
   `sqrt(sum(ir^2))` is 1.0 across both channels, which makes convolution
   approximately level-preserving for broadband input. Verify against step
   02's measurement rather than trusting the algebra — the direct impulse
   and the tail interact with the `tail_level`/`onset`/`envelope` shaping at
   `:200`, and the practical answer may need a fixed offset from unity.

   The existing comment claims a predictable peak "keeps default wet level
   musical across room sizes". That intent is right and energy
   normalization serves it better: a large room and a small room should
   arrive at the same wet level, which peak normalization does not deliver
   because their tails differ far more than their direct impulses do.

2. Do the same audit for `Plate` (`crates/mooloop-dsp/src/effects/plate.rs`)
   — it is a comb/allpass network rather than a convolution, so it has no IR
   to normalize, but its wet output should still be measured against dry and
   corrected if it is off.

3. Switch the host wet/dry blend at `render.rs:619-622` to equal-power:
   `dry_gain = cos(wet * PI / 2)`, `wet_gain = sin(wet * PI / 2)`. Both
   gains are constant across the block, so compute them once outside the
   frame loop.

4. Revisit the per-kind default blends at
   `crates/mooloop-core/src/effect.rs:2198-2209` — reverb and plate open at
   `wet_dry: 0.35`, modulation at `0.5`. Those numbers were chosen against
   the broken wet level; once the wet path is level-matched they should be
   re-picked by ear.

## Constraints

- The IR is built off the realtime thread (it is a prepared resource with a
  `resource_key`, see `crates/mooloop-core/src/effect.rs:1360`), so
  normalization cost is not a realtime concern. Compute it properly.
- Equal-power is correct for decorrelated wet paths and slightly wrong for
  correlated ones — a filter or an EQ at 50% mix will now sum ~3 dB hot
  where linear was right. The host blend is shared by every effect
  (`render.rs:619`), so this is a genuine trade-off. Equal-power is the
  better default because the effects people actually blend are the
  decorrelated ones; note the trade-off in `docs/GAIN_STRUCTURE.md` rather
  than adding a per-effect switch for it now.
- Do not change the dry-path latency alignment (`dry_align`,
  `render.rs:601-607`) while touching this code. A blend fix and a timing
  fix in one commit is not reviewable.

## Verification

Step 02's measurement 5, re-run: default reverb at 100% wet should now be
within a few dB of the dry signal rather than far above it. Add a test that
a 50% blend of a wet path decorrelated from dry preserves energy, which is
what equal-power buys. `cargo test -p mooloop-dsp -p mooloop-engine`.

Then listen at 1%, 10%, and 50% wet and confirm the control feels
proportional across its range. That is the actual acceptance criterion here;
the numbers only prove the cause was removed.
