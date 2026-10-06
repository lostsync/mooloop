# Current System

What the application actually does today, and where each behaviour stops. It
is deliberately blunt about gaps, so decisions are made against the system
that exists. Source and tests settle any disagreement. `SCOPE.md` is the list
of what is missing.

It is split by area. A change that adds, removes or alters user-visible
behaviour updates the area's file in the same commit (`AGENTS.md`,
*Documentation is part of the change*). Read the one area your task touches.

| Area | What it covers |
| --- | --- |
| [Interface](current/interface.md) | The window, panes and views, the channel sidebar, the status bar, the browser, Preferences and the menus, shared widgets, keyboard reach, colours, device selection and the clipboard, the action registry and undo. |
| [Sequencing](current/sequencing.md) | Channels, patterns and the step grid, notes and the piano roll, events and voices, transport modes, the song loop, seeking, and the playlist. |
| [Sampler](current/sampler.md) | The sampler editor and key zones, loop seams, fit to tempo, slices, voice allocation, recording takes into it, and stretch, commit and REBAKE. |
| [Devices](current/devices.md) | The device rack and its host, every effect kind, containers and layers, the Buffer, the generators and Aux In, and instrument DSP. |
| [Mixer](current/mixer.md) | Tracks and their order, the channel strip, sends, solo and mute, analog sum, metering, and latency compensation. |
| [Modulation](current/modulation.md) | Parameter addressing, the song's modulation and its pane, published outlets, and automation lanes. |
| [Plugins](current/plugins.md) | CLAP hosting: scanning, effects and instruments in a chain, faces, GUI windows, presets, lanes and routes. |
| [MIDI](current/midi.md) | A keyboard playing the selected channel, per-channel MIDI input, controller mapping, transport control and MIDI recording. |
| [Files](current/files.md) | Export, and songs, kits and presets on disk: saving, loading, missing samples, autosave and recovery. |
| [Engine](current/engine.md) | The audio path from a UI command to the driver, the command backlog, the DSP foundations, and samples and rendering. |

The release and integration suite is in `docs/OPERATIONS.md`, "All Tests And
Release Verification"; routine work uses the narrowest check `AGENTS.md`
specifies.
