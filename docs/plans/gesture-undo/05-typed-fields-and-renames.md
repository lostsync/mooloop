# 05 · Typed fields and renames

A keystroke is not an edit, and this is the step where saying so is the whole
job.

## The problem

`NameField.edited(string)` (`toolbar.slint:376`) fires on **every
keystroke**. A naively recorded rename is one undo step per character:
renaming a channel "Kick" to "Kick Bus" would be eight entries, and undoing
the rename would take eight Ctrl+Z. `LOOSE_ENDS.md` names this as the reason
renames were left out when the rest of the structural edits went in, and it is
the same fact as the fader's move frames wearing different clothes.

The other typed surfaces have the same shape at lower volume: `StepperField`
(`toolbar.slint:128`), `TempoField` (`:524`), and the typed half of
`KnobField` (`controls.slint:925`).

## The answer

The gesture for a typed field is **the editing session**, not the keystroke
and not the field's lifetime: it opens when the field takes focus with an edit
in it, and closes when the field commits — Enter, or focus leaving. That is
one entry for "renamed Kick to Kick Bus", which is what a person would expect
to undo.

Two details that decide whether it feels right:

- **Escape.** If the field supports abandoning an edit, an abandoned edit
  records nothing — close the gesture by discarding, not by recording. If it
  does not support Escape today, this step does not add it; note it and move
  on.
- **A field that is never committed.** Focus can be lost by the window
  closing or a dialog opening. The install rule from step 02 covers the
  destructive case; for the ordinary one, treat focus loss as a commit,
  because the value has already been written to the project by then.

## Whose problem the caret is

`LOOSE_ENDS.md` carries a related, separate fault: a caret parked in a text
field eats the spacebar, because there is no way to leave a field except
Enter. This step does not fix that, and it should not — but it will make it
more visible, since focus now also decides where an undo entry ends. Say so in
the step's commit rather than leaving the next person to connect them.

## Done when

- [ ] Renaming a channel, a track, a pattern or a device is **one** undo
      entry, whatever its length.
- [ ] A typed tempo, a stepper field and a typed knob value are each one
      entry.
- [ ] An abandoned edit records nothing.
- [ ] A rename interrupted by the window closing does not leave a gesture
      open — the same test shape as step 02's install case.
