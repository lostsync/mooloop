# Song patch boxes

Adam, after running `main` with the song patch: *"this works but falls well
short of its potential."* The asks: a `wait` box (ms or beats), number boxes
to show and type values, somewhere to store a value or a list, exploding a
chord and packing it back, an arp (from parts and as a box), picking one note
of a chord to delay and transpose (left hand / right hand), switch and gate,
and a Step box that looks like a param sequencer again (Reason Matrix, curve
mode), unipolar or bipolar.

Survey: `main` at `5896570`. Vocabulary is `box_text::VOCABULARY` (20 boxes),
jacks are `ModulatorKind::ports`, notes run in `engine/src/note_patch.rs`,
the contract is `docs/MODULATION.md`.

## The one design call: no list wire

A third wire sort that carries lists would touch the compiler, the save
format, the canvas, and every box. It isn't needed. **A note wire already is a
list**: it carries the set of notes held right now, each NoteOff following its
NoteOn. So "explode, hold in a list, iterate" becomes boxes that work on that
set (`pick`, `unpack`, `arp`), plus a numeric `table` for stored control
values. Everything stays two wire sorts.

## Proposed boxes

| Box | Typed as | Jacks | Does |
| --- | --- | --- | --- |
| **num** | `num`, `num int`, `num 0 1`, `num note` | ctl in → ctl out | Shows the value passing through. With nothing wired in, drag or type it to send a value. `int` rounds, two numbers clamp the range (unsigned = `num 0 1`), `note` shows a pitch as `C3`. |
| **hold** | `hold` | in, trigger → out | Sample and hold: keeps `in` as of the last trigger. |
| **table** | `table 8` | index, value, write → out | n stored values. Reads at `index` (0..1 across n); `write` stores `value` at `index`. Face is drawn bars, same as Step's. |
| **switch** | `switch 3` | in, which → n outs | One input to one of n outlets. Works on control **or** notes (a NoteOff goes where its NoteOn went). `select` stays as n-in-to-1. |
| **gate** | `gate` | in, open → out | Passes `in` while `open` ≥ 0.5. Control or notes; closing releases the notes it let through. |
| **pick** | `pick low`, `pick high`, `pick 2`, `pick -1` | notes → notes | Keeps one note of each chord (notes starting together): lowest, highest, nth from the bottom, nth from the top. |
| **unpack** | `unpack 4` | notes → n note outs | Explodes each chord: outlet k carries its kth note from the bottom. |
| **merge** | `merge 3` | n note ins → notes | Packs streams back into one (an inlet takes one wire, so this is how a left hand and right hand meet at one notes-out). |
| **note** | `note` | pitch, velocity, trigger → notes | Control to notes: plays `pitch` on each trigger, releases on the next or when trigger falls. The way back from `held`/`table`/`num` to sound. |
| **wait** | `wait 250ms`, `wait 1/8`, `wait 2b` | notes in + ctl in → notes out + ctl out | Delays whatever is wired: notes (NoteOffs delayed equally) and/or a control signal. ms, a division, or beats. Capped at 16 beats / 8 s. |
| **arp** | `arp up 1/16`, `arp updown 1/8 2` | notes, advance, reset → notes | Holds the chord and plays it one note at a time: up, down, updown, played order, random; rate as a division; octaves; gate length knob; `advance` lets a counter or tag clock it instead. |

**Rename:** today's `gate` (notes → gate, pitch, velocity) becomes **`held`**,
so `gate` can mean what it means in Max and Pd. Saved songs keep working: a
box is saved by kind, not by its text, so it just reads `held` when opened.

**Step face:** bars you draw with the mouse across the box (drag to paint,
like Matrix curve mode), the playing step lit. Length, rate, glide and
trigger stay knobs. New **polarity** switch: bipolar (`-1..1`, bars grow from
the middle) or unipolar (`0..1`, from the bottom). Typed as `step 8` or
`step 8 uni`. Table uses the same bar face.

### The two patches this has to make

- **Left hand / right hand**, one note at a time in the sequencer:
  notes in → `modal d dorian 7` → notes out (right hand), and the same modal
  outlet → `pick low` → `transpose -12` → `wait 1/8` → into a `merge 2` with
  the right hand, or straight to a second channel.
- **Arp from parts**: notes in → `unpack 4` → four `held` → `select 4`,
  indexed by a `counter 4` on a Beat tag → `note` → notes out. And the
  dedicated `arp up 1/16` that does the same in one box, with empty slots
  and chord changes handled properly.

## New plumbing (what's not just "add a box")

1. **Note boxes with several note jacks** (`switch`, `unpack`, `merge`).
   Today each note box reads one stream and writes one. Each outlet needs its
   own held table. Engine, `note_patch.rs`.
2. **Notes held across blocks** (`wait`, `arp`, `note`): a fixed-size queue
   per box of notes still to play, sized off the audio thread. Stop, seek,
   panic, mute and pattern switch drop the queue and release what sounded,
   per the existing release rules.
3. **Boxes that play on their own clock** (`arp`, `note`, `wait`). The note
   pass runs before the control tick, so a control inlet into a note box is
   read a block late (as `transpose` already is). Arp's own clock reads the
   transport inside the block, so it lands on the grid.
4. **One box, either sort** (`switch`, `gate`, `wait`): `switch` and `gate`
   take the sort of the first wire into `in`; `wait` simply has both lanes.
5. **A box's value shown on the canvas** (`num`, `table`, Step's playing
   step): the canvas already reads outlet activity for cable blinking; this
   reads the value too.
6. **Save format**: new kinds and params (`ModNumParams`, `ModTableParams`
   with its values, Step's `polarity`, …), all `serde(default)` so older
   songs open unchanged. Table values are saved; `hold`'s held value isn't.

## Steps

One PR each, in order; `00-status.md` has the Linear issues.

1. `01-num-and-hold.md`
2. `02-step-drawn-and-table.md`
3. `03-switch-and-gate.md`
4. `04-note-boxes-with-many-jacks.md`
5. `05-wait.md`
6. `06-note.md`
7. `07-arp.md`
8. `08-the-contract.md`

## What Adam agreed (2026-10-19, "sounds good")

1. Today's `gate` box is renamed `held`, so `gate` means what it does in Max and Pd.
2. No list wire: notes are the list, plus `table` for numbers.
3. `wait` is capped at 16 beats or 8 seconds, whichever is shorter.
4. The order above: quick wins first, arp last.
