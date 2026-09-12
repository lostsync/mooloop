# EQ v2 status

Nothing has landed. Written 2026-09-12, out of a fix to one bug and a question
Adam asked about what that bug implied.

## What prompted it

`a452bf4` split `EQ_PARAM_CHARACTER`, which had meant a band's Q profile when a
band was selected and a pass filter's slope when a pass filter was -- two
settings of different arity behind one automatable id, so four of the Shape
control's five positions collapsed onto one and an automation lane reported a
value it had not set.

Adam's question was the useful part: **mooloop intends to host CLAP, CLAP
plugins expose arbitrary automatable parameters, and our own devices should
reach the automation system the same way plugins will.** Under that test the EQ
is not a device with a missing feature. It is the one native device whose
parameter model cannot be expressed in the model a plugin host has to have.

## Where this sits against FOCUS.md

Nowhere yet -- Adam asked for a plan, not for it to be built. `FOCUS.md` decides
whether it is next.

Two things about the sequencing are worth saying here rather than in a step.
Step 01 is the only one the CLAP argument forces; 02 to 04 are an EQ that is
merely better, and they are separable. And step 01 is worth doing *before* any
plugin work rather than after: it is a small instance of the same
instance-scoped-parameters problem, on a device whose behaviour is already
understood, which makes it a cheap way to find out whether the model holds.

`README.md` carries the argument and the measurements. The steps are the work.
