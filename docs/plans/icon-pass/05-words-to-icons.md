# 05 — words that wanted to be icons

The original ask was *"a text label -> icon pass"*. Steps 01-04 deal with
symbols that are already symbols. This step deals with **words standing in for
a shape**, which `UI_DESIGN.md` has asked to be icons since before the survey:
*"draw the thing when the thing is a shape"* (`:737-747`), and waveform and
filter selectors as an *"icon or short-label selector bank"* (`:95`).

## Candidates

| Where | Today | As icons |
| --- | --- | --- |
| Oscillator waveforms | "S T W P" (`device-oscillator.slint:57`), "SIN TRI SAW PLS" (`mlp8-device.slint:463`), "Sin Tri Saw Sq S&H" (`mono-device.slint:182`, `poly-device.slint:193`) | one waveform set: sine, triangle, saw, pulse or square, S&H |
| LFO shapes | "SIN … SQR RND" (`modulation-shelf.slint:1202`) | the same waveform set |
| Filter types | "LP BP HP" (`filter-device.slint:31`) | the EQ's pass shapes from step 03 |
| Sampler loop modes | "Off / Loop / Pong" (`sampler-device.slint:631`); these were Path icons in `8ef32a22` | once, loop, ping-pong |
| Modulation polarity | "BI" / "UNI" (`mlp8-device.slint:80`), `±` / `↑` (the shelf) | one bipolar/unipolar pair |
| Worded add buttons | "+ ZONE" (`sampler-device.slint:106`), "+ ADD ROUTE" (`mlp8-device.slint:1957`) | `add` + a word, or `add` alone with a tooltip |
| Signal flow in a label | "→M" (`mixer.slint:490`) | decide per site |
| Add menus | the rack's `+` rows and the joins' effect menu are words alone | the kind's icon beside its name, from step 02 |

**A waveform icon is a small display.** It doesn't have to be a new drawing
per face: one waveform set in the registry serves the oscillators, the LFOs
and the modulation shelf, which today spell the same five shapes five ways.

## Adam sees a sheet first

These change what people read on a face, so none converts before Adam looks.
Draw every candidate, with the words beside the icons, in the mockup catalog.
Post the sheet on this step's issue with the `Question` label. He picks, per
row: icon, icon + word, or keep the words.

Keep words where the word is the information: a quantity, a name, a unit,
M/S on the mixer.

## Who does it

Interface draws. Each face's owner converts its selector:

- **Instruments**: oscillators, the sampler, ML-P8.
- **Control**: the modulation shelf.
- **Effects**: the filter.
- **Mixer**: `→M`.
- **Interface**: the menus.

## Done when

- Adam has picked per row, and each picked row draws from the registry.
- The five waveform spellings are one set.
- Rung 4 on antibox, and the changed snapshots have been looked at.
- `UI_DESIGN.md:95` no longer describes a wish.

0.1.7.
