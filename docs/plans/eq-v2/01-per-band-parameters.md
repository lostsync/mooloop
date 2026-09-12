# 01 — Per-band parameters

Give every band and both pass filters their own parameter ids, so the EQ is
addressable the way a plugin's parameters will have to be. This is the step the
CLAP argument forces; the rest of the plan is an EQ that is merely better.

## What it retires

`selected_target` stops being a parameter and becomes what it always was: which
control set the face is showing. It stays persisted -- reopening an EQ should
return to the band you were shaping -- and it stops being reachable by an
automation lane, which is the thing the current model cannot make sense of.

`get` and `set` stop resolving through it. Each id addresses one field of one
band, the way `strip_band_param` already does.

## The id space

Mirror the strip rather than inventing a layout:

```
EQ_BAND_BASE + band * EQ_BAND_STRIDE + field    // field: on, freq, gain, q, kind
EQ_PASS_BASE + pass * EQ_PASS_STRIDE + field    // field: on, freq, q, slope
```

Choose the stride with room to append a field without renumbering -- the strip
uses four for four fields and `synth_osc_param` uses ten for five, and the
oscillator's spare room is the one that has been wanted. Take ten.

**Ids 0..6 are spent.** `effect.rs` states the rule about itself -- never
renumber a shipped id, append instead -- so the old six plus `EQ_PARAM_Q_PROFILE`
stay spent whatever happens to them. The options are to retire them (a lane
written against one stops resolving) or to keep them as aliases for band
`selected_target`, which preserves the incoherence this step exists to remove.
**Retire them.** v0.1.3 offers no cross-version guarantee, `load` refuses a
`format_version` it does not recognise outright, and keeping a working alias to
a model we are deleting is how the model survives.

Bump `FORMAT_VERSION` if and only if the persisted *shape* changes.
`EqParams`'s fields are unchanged by this step -- only the ids that address them
-- so the deciding question is whether an existing lane's `ParamAddr` still
means what it did, and it does not.

## Cost and knock-ons

Seven bands x five fields plus two pass filters x four is 43 parameters, against
seven today.

- `descriptor_slots` sizes the modulation arrays by max id + 1, so a stride of
  ten puts the EQ's array at ~90 floats per slot. Measure it before assuming it
  is free; `block_cost.rs` is the tool and the control pass has been the
  expensive half before.
- The face's `modulation-allowed[N]` indexing is by descriptor id and follows
  automatically.
- `param_id_freeze_tests` needs its EQ row rewritten, which is the point of that
  test: the rewrite is where somebody confirms the ids are where they meant.
- `slint_face_agreement.rs` pairs a knob to a parameter by the index its
  modulation overlay reads, so it follows the face without edits.

## Acceptance

- A lane on band 3's frequency moves band 3's frequency, whatever the face is
  showing.
- No parameter's meaning depends on another parameter's value. Assert it: every
  id's `get` is unchanged by writing `selected_target`.
- `EQ_PARAM_TARGET` is gone from the descriptor table, and selecting a band is
  not an automatable act.
- `every_effect_stepped_position_reads_back_as_itself` still passes with no
  exclusions.
