# 05 — wait (MOO-537)

**`wait 250ms`**, **`wait 1/8`**, **`wait 2b`**: a note lane and a control
lane; delays whatever is wired. Each NoteOff is delayed as its NoteOn. Capped
at 16 beats or 8 seconds, whichever is shorter.

## Plumbing

Notes held across blocks: a fixed-size queue per box, built off the audio
thread with the set. Stop, seek, panic, mute, pattern switch and loop fold
drop the queue and release what sounded, per MODULATION.md's release rules.
The control lane is a ring of control ticks sized for the cap.

## Done when

A delayed note plays at the right offset live and in a bounce, nothing hangs
on stop or seek mid-wait, and the control lane delays an LFO by the set time.
