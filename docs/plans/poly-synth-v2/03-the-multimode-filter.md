# The multimode filter

## Why this belongs on Poly and not Mono

`Svf` (`crates/mooloop-dsp/src/filter.rs:22`) already computes low-pass,
band-pass, and high-pass from the same state-variable stage —
`next_sample_lp_bp_hp` returns all three
(`crates/mooloop-dsp/src/filter.rs:69`) and Poly currently throws two of them
away, calling `next_sample_lp_hp(...).0` (`polysynth.rs:351`). The capability
is already paid for.

Mono deliberately does *not* get a mode menu, because a clean multimode filter
is the opposite of a character filter. On Poly it is exactly right: a flexible
per-voice tone shaper across pads, brass, strings, and stabs.

## Do this

### 1. Four modes

```rust
pub enum PolyFilterMode { Lp12, Lp24, Bp12, Hp12 }  // default Lp12
```

| Mode | Response                          | Implementation                              |
|------|-----------------------------------|---------------------------------------------|
| LP12 | Open, smooth two-pole low-pass    | The existing single `Svf` stage             |
| LP24 | Classic four-pole poly response   | Two cascaded stable 12 dB stages            |
| BP12 | Nasal, swept textures             | The existing BP output                      |
| HP12 | Thin, brassy, string patches      | The existing HP output                      |

`PolyVoice` owns the SVF state for both stages as concrete fields — enum-based
state with static storage, no heap, no trait objects. The second stage's state
is dead weight in the three 12 dB modes and that is fine; it is two floats.

Match on the mode once per block, not per sample.

### 2. LP24 specifics

Cascading two SVFs is not simply calling the stage twice. Two things to get
right:

- **Resonance placement.** Applying the full resonance to both stages gives a
  peak roughly twice as tall in dB and a filter that self-oscillates far too
  easily. The usual answer is resonance on one stage with the other running
  flat, or a reduced Q on each. Pick one, listen, record it here.
- **Cutoff compensation.** Two cascaded 12 dB stages at the same cutoff have a
  -6 dB point noticeably below either stage's individually. Compensate so that
  the Cutoff knob means roughly the same frequency in LP12 and LP24 — the user
  is comparing modes, not stages.

If a dedicated 4-pole path turns out cleaner than a cascade, that is an
acceptable substitution; the requirement is the response, not the topology.
Note that the Mono plan's step 04 builds a nonlinear ladder — **do not reuse
it here.** Poly's LP24 is clean by design; that difference is the point.

### 3. Stability under per-sample modulation

This is the real risk. Cutoff already moves every sample from the filter
envelope, the LFO, and now keytrack and drift. `Svf` is topology-preserving
specifically so that it stays well behaved when cutoff moves per sample (see
the comment at `crates/mooloop-dsp/src/filter.rs:19`) — a cascade of two must
keep that property, and the HP output in particular is the one that blows up
if resonance and cutoff modulation are handled carelessly.

Switching mode mid-note must not click. Do not reset stage state on a live
switch; let the 5 ms parameter smoothing cover it, and cross-fade only if a
listening test shows a pop. Switching from HP to LP is the worst case, since
the outputs are near-complementary.

### 4. Decide where Drive goes

Per 01, Poly's Drive is a color control and its placement is a listening call,
not a defining requirement. It is post-filter today
(`apply_drive(filtered, drive)`, `polysynth.rs:355`). Try a mild pre-filter
stage; keep whichever sounds better as a gentle color across all four modes,
and **write the decision and the reason into this file.**

Bear in mind BP12 and HP12 remove a lot of energy, so post-filter drive on
those modes is quiet and pre-filter drive is not. That asymmetry is itself an
argument, in whichever direction the listening goes.

### 5. Parameter and UI

| Field         | Kind                   | Default | ID |
|---------------|------------------------|---------|----|
| `filter_mode` | `PolyFilterMode` enum  | `Lp12`  | 41 |

LP12 is the migration default and matches what old patches actually were.

UI: a `SelectorBank` in the FILTER panel header on AMP/FILTER — the slot Mono
step 02 left when it restructured both faces. `LP12` / `LP24` / `BP` / `HP`.
`FilterResponseDisplay` takes a `mode` property that Poly currently hardcodes
to 0 (`crates/mooloop-ui/ui/poly-device.slint:174`); drive it from the mode so
the displayed curve is honest — check whether the component already draws all
four shapes and extend it if not.

## Done when

- Each of the four modes produces its expected response. Assert against a
  rendered sweep: LP24 rolls off roughly twice as steeply as LP12, BP12
  attenuates both extremes, HP12 attenuates the low end.
- LP12 and LP24 at the same Cutoff knob position have comparable corner
  frequencies.
- All four stay finite and bounded at maximum resonance with the cutoff swept
  every sample by envelope and LFO together. Extend
  `resonant_filter_and_drive_stay_bounded` to run per mode.
- Switching mode on a sounding voice produces no step. Reuse the `max_step`
  helper from `parameter_changes_mid_note_do_not_step`.
- `filter_mode = Lp12` with drive unchanged is bit-identical to the pre-step
  build, or the difference is explained by the Drive placement decision and
  recorded here.
