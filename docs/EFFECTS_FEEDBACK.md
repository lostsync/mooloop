EFFECTS

ALL: Adding more than 8 effects seems to still be disabled. I thought this restriction had been dealt with. An internal limit is fine but it should be high enough that your cpu would choke from DSP before you hit it. Might just be UI lagging behind internals.

Move knob labels above the actual knob and put the knob's value display where those labels currently are, right beneath the knobs. Put the values in a bright color, inside a small dark box appropriately sized for the value to emulate a small readout display. monospace ui font could work well here, maybe something like Lilex: https://github.com/mishamyrt/Lilex

Can we antialias the splines/lines used to draw filter shapes, etc? If yes, lets put that in and tie it to an appearance option in prefs. 

In every device, it has its own name up at the top. The device's name is already shown in its frame. Maybe we drop the in-device name and left-align the header row buttons above the scopes if they exist. Related issue: in devices with no buttons up there like bitcrush and the dynamics ones, this header area is taller than in devices like drive and filter that do have buttons above the scope.

Looking at drum synth and reverb next to each other, i see two similar but different styles of header-placed type toggle buttons. which is our 'real' one? i probably prefer the smaller style from effects devices. it looks like in the smaller all caps selector buttons from effects devices, longer labels like 'CHAMBER' stress the left/right padding.

in general points on splines should be draggable if it makes sense. for instance, a filter curve - the freq point should be draggable x,y for freq,gain and maybe scrollwheel would adjust Q. 

we should have a kind of knob that has a toggle for ms and beat divisions. we'd use that e.g. in a delay but maybe not for compressor attack - just raw ms there (although that could be cool)

Screenshots of the devices are available here:

```
mooloop/reference/img/devices git:main*
> ls -1
 bitcrush.png
 delay.png
 drive.png
 dynamics.png
 eq.png
 filter.png
 mod.png
 plate.png
 reverb.png
```

Filter: Need BP. Other filter modes would be cool, e.g. Moog, etc. Needs poles or db/oct, a way to set slope. Note is shown in freq knob value but that should only be visible in the tooltip. Filter should have a sat/drive control.

Drive: Pretty ok with this one. 

Delay: Tempo sync option

Bitcrush: Few diff ways to do the math - maybe set as a row of toggles to swap between bitcrusher styles. 

Comp/Gate/Limiter: All 3 need to show the input signal on their scopes such that it is possible to have a visual idea of where to set things. we could also show the threshold point on the curve in the scope. it should probably be draggable. 

EQ: Has a lot of UI quirks, mostly around point selection. Clicking a point doesn't reliably select it, especially if there are multiple on top of each other. There is a row of buttons from Low -> LP under the scope. They toggle on and off, but i dont think that does anything? There's a separate ON toggle that does seem to be the actual on/off control for the selected band. When you do select a band, the params in the UI do not update to show this. For example that on/off button wouldn't change from ON to OFF if i clicked from an enabled band to a disabled one. We don't currently have enough vertical space to stack the scope, buttons under it, and then knobs too.

Reverb: I dont actually know what that green square in the scope is for. I can't move it. Don't know what it means/does. The scope is not as wide as the 4 buttons below it. Not a huge deal but looks weird. The rendering in the scope might make more sense as a cube? idk how we position the point in 3d within that space if we do that - maybe it needs x,y,z inputs? we could accept them as %. we would maybe need to rotate the cube, so then controls for that are necessary. it might be more trouble than it's worth, but i do think we could find something more visually appealing and meaningful to show the space and simulated capture position. we have a lot of blank space on the right side of the plugin. all it's really showing is info we can already see on the left. maybe it should show the IR, kinda like the drum synth does, or some kind of visual representation of what the reverb will sound like. need to be able to darken it. needs a low cut on the input. it's supposed to also allow the actual loading of IRs but i dont see that anywhere. im ok with dropping that and just making an IR loader later - i'd rather have a dedicated one anyway. 

Mod: visually similar to Reverb in many ways and suffering many of the same layout issues. It is very large compared to what it does. The scope does not need to be that big. doesnt need to say 'stereo variable delay' or have the rate and depth shown independently of the knobs. i think using something other than full size knobs would be good here. we have a lot of flexibility in laying out these devices. shared surfaces are cool but not when they fight usability. 

Plate: Knobs extend outside of frame. Unclear what should be in scope - doesnt really show anything. Scope should probably show the shape of the reverb. Maybe we could try to make this look cool, maybe a spectral display that kinda lets you 'see' the tone, eg low end might ring out longer and be shown at the bottom. NI does something like this in RC24 and RC48.
