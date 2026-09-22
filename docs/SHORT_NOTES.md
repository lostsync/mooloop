# Short notes

Adam's quick notes: things worth doing that have no plan yet and no obvious
home. Delete a line when it is done or has moved somewhere better.

✓ add option to swap alt/super or recognize super as alt (5311de4)
✓ should be able to assign mixer track from channel sidebar (49c7a6a)
✓ in the sequencer, let's split the mute button (figuratively) and have a yellow half-height solo button above it. we can remove the M and S from these. (79c1158)
- stuff doesnt resize properly when you bump font size or spacing in theme prefs. if its a slint limitation we need to remove those controls. toolbars etc also dont resize to accomodate changed button sizes
- if a toolbar is cut off becuase it is too short, e.g. if the pane is split, we should put the cut off items in an overflow menu
- clicking a preset in the sidebar insta-loads a new instance of that device
✓ add new channel menu in seq is stale (3fff067)
- midi recording doesnt loop properly - after 1 playthrough notes just stack at the last tick. should just loop around and start over on the selected pattern. should probably have a toggle for overdub/replace
- need a device like chain but for layers
- need one for mid/side that lets you put devices on mid or side and set levels
- need a 'tool' or 'utility' device that has gain, pan, width, maybe polarity invert and sweepable phase offset?
- more shaping options to the oscs
- build sidechains into dynamics devices
- add audio input modulator == level and/or env follower
- math modulator should accept 2 inputs so one can modify another
- note generators for mod rack
✓ in prefs, there's this line on the section tabs - it looks weird; it's right in the center of the tab. just highlight the whole tab or something (f5c8b24)
✓ allow keyboard browsing in sample sidebar. up/down should play if it selects a playable file and preview is selected. l/r should expand/collapse. (a70d025)
