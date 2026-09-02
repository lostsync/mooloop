# DS-01 plan status

**Nothing is built.** This directory is the approved design for a second drum
instrument, written before any code. Read `01-what-ds01-is.md`, then work `02`
through `09` in order.

The device is called **DS-01**. It deliberately does not join the `ML-*`
family: ML-M1 and ML-P8 are keyboard instruments that share a voice lineage
and a parameter-id convention, and DS-01 shares neither. Its serialized tag is
`ds01`, chosen explicitly rather than taken from a `rename_all` default — the
ML-M1's frozen `ml1` is the reason to pick an on-disk name on purpose the
first time.

## The four decisions this plan starts from

1. **One universal percussion voice.** Not Kick/Snare/Hat modes, and not a
   set of selectable per-voice engines. A single architecture whose controls
   mean something in every configuration. More drum types come from range and
   factory patches, not from new code paths.
2. **A new generator kind beside the existing `DrumSynth`.** The v1 device
   stays, old projects load unchanged, and no migration is attempted.
3. **One drum per channel stays.** The kit is the set of channels. Choke
   groups already work across channels, and each drum keeps its own insert
   rack, modulation rack, mixer strip, and pattern lane — which is more than
   any multi-part drum plugin gives a single drum.
4. **Name: DS-01.**

## Why v1 cannot be extended in place

The presenting symptom was that mod-source assignment could not be enabled for
the drum synth the way it was for the sampler and the synths. The cause is
structural, and it is worth stating precisely because it decides the whole
design.

`DeviceKind::descriptors()` in `generator.rs` returns `&[]` for `DrumSynth`,
and it is the only generator that does. The comment there calls the missing
table "mechanical work rather than a design question." **That is wrong**, and
this plan supersedes it. `DrumSynthParams` is a mode-union: `mode` selects
Kick, Snare, or Hat, and roughly two thirds of the struct is inert at any
moment. `kick_start_hz` means nothing in Hat mode. A flat descriptor table
over that union produces parameter ids whose meaning depends on a discrete
selector, so:

- a modulation route or automation lane can address a live parameter, and then
  silently stop doing anything when the mode changes;
- `mode` itself is the one control that would make those routes meaningful,
  and it is exactly the kind of structural discrete that must not be a
  modulation destination — switching it mid-hit changes which oscillators and
  envelopes exist;
- the ranges are not shared. `kick_start_hz` is 20-1000 Hz, `snare_tone_hz` is
  40-2000 Hz, `hat_hp_hz` is 500-16000 Hz. There is no honest way to give one
  id one curve.

Giving v1 a descriptor table is therefore not mechanical. It requires deciding
that a parameter's meaning does not depend on a mode, which is a different
instrument. Hence DS-01.

## Two things the investigation turned up that the design has to answer

- **v1 is inconsistent about what a parameter change does to a hit already in
  flight, and nobody decided it.** `render_range` re-copies `self.params` at
  every range boundary, so levels, drive, and mix follow parameter changes
  mid-hit. But envelope coefficients, the pitch tracking factor, and filter
  cutoffs are resolved once inside `trigger()` and never revisited. The split
  is an artifact of where the code happened to read the struct. Under
  modulation that becomes user-visible and arbitrary, so DS-01 must publish
  the split as a rule. Step 02 does.
- **Event order inside a block decides whether a route reaches the hit it was
  aimed at.** Modulation arrives as ordinary timed `Event::ParamValue` at the
  32-frame control rate, and `trigger()` snapshots parameters. A route meant
  to shape *this* hit must land before the note-on at the same offset. That is
  a contract between the renderer and the device, not an implementation
  detail, and it has never been written down because no descriptor-addressed
  generator has had a parameter that latches at note-on before.

## Build order

| Step | What it lands |
| --- | --- |
| `02-the-voice-and-the-descriptor-table.md` | The kind, the params, the ids, the tone and noise sources, and descriptor addressing on day one |
| `03-the-envelopes.md` | Four AHD envelopes with curve and optional gate |
| `04-the-body-resonator.md` | The tuned modal layer — toms, rims, bells, clangs |
| `05-the-burst.md` | Multi-impulse triggering: clap, flam, roll, buzz |
| `06-the-shape-stage.md` | Drive characters, the output stage, the gain contract |
| `07-internal-modulation-and-outlets.md` | DS-01's own matrix and its published outlets |
| `08-the-face.md` | The device face |
| `09-the-kit.md` | Factory patches, range tuning, and the listening pass |

Descriptor addressing lands in **step 02**, not at the end. It is the reason
this instrument exists; it does not get to be the last thing anyone gets to.
