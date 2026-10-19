# 07 — arp (MOO-539)

**`arp up 1/16`**, **`arp updown 1/8 2`**: notes, `advance`, `reset` → notes.
Holds the chord reaching it and plays it one note at a time. Modes up, down,
updown, played order, random (seeded, restarts with the transport); rate as a
division; octaves; a gate length knob. `advance` lets a counter or a tag
clock it instead. Its own clock reads the transport inside the block, so
notes land on the grid.

## Done when

It follows chord changes without hung notes, lands on the grid live and in a
bounce, and plays the same on every bounce.
