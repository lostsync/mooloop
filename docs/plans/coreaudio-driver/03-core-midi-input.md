# 03 Core MIDI input

JACK gives the engine one MIDI input port a patchbay connects. Core Audio has
no patchbay, so connect every Core MIDI source present at startup through
`midir`, pass messages to the callback over a bounded ring, and deliver them at
the start of the next block. Sample-accurate offsets and hot-plugged sources
are out of scope: MIDI input reaches only the buffer device today.
