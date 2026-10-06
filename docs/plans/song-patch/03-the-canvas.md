# 03 — the canvas

The Modulation pane shows the patch: boxes where the song put them, wires
between them, tags at their ends, and assignment tags for every route. You
can move boxes, draw and remove wires, and assign from any outlet with
today's gesture. Making new kinds of box by typing is step 04; faces in
place and cable bends are step 05. Until step 04, the Add list still adds
the five kinds.

## Sign-off first

Before building, render the canvas with `scripts/slint-sketch --shot` from a
static Slint mock-up of the prototype's first example (the kick gate into an
LFO's `retrigger`, the LFO to Cutoff, `* -0.5` to Res) in the dock and in the
main area, and put the two images to Adam, as song modulation's step 04 did.
The prototype is the design; the mock-up checks it in mooloop's own parts
and at the pane's real size. Build on his word.

## Drawing

In `ui/modulation-shelf.slint`, the module grid (`ModuleGrid`, `:80`, the
tiles at `:826`) goes; the outlet band, the Add list and the selected box's
surface stay until steps 04 to 06 replace them. A new `PatchCanvas`
component takes its place, in a `ScrollView` (the canvas is larger than the
pane):

- **Boxes** are `for box in boxes : Rectangle` placed by `at`, each its
  spelling in the mono face (`lfo`, `* -0.5`), with its jacks as small marks
  on the top edge (inlets) and bottom edge (outlets), note jacks in the note
  colour.
- **Tags** are the prototype's arrow shapes: inlets point right with their
  outlet on the right edge, outlets point left. A bound tag reads its kind
  and its source (`gate  Kick 1`); an unbound one shows `[ ]`.
- **Assignment tags** are drawn from the routes, one per route, wired from
  its outlet, reading the destination and depth (`Cutoff  ML-M1 9  +40%`).
  They are not boxes in the model; their place is the route's `at`
  (step 01), so they stay where they were put.
- **Wires** are `for wire in wires : Path { commands: wire.d }`, the path
  built in Rust as an orthogonal route with rounded corners (the prototype's
  `route` and `rpath`). The automation lane already draws string-built paths
  (`main.slint:7519`); the canvas needs one `Path` per wire, not one per
  segment. Control wires are solid; note wires dashed in the note colour; a
  delayed wire (step 02) carries a small mark at its inlet.
- **Jack names** show above and below a box when it is selected or hovered,
  as in the prototype; the names come from step 01's jack table.

## Gestures

**One `TouchArea` over the whole canvas**, with hit testing in Rust, as the
piano roll does (`ui/piano-grid.slint:335-344`: per-item touch areas move
out from under the cursor). It reports press, move and release with canvas
coordinates; Rust decides what was hit: a jack, a box, a tag, a wire, or
nothing.

- Drag a box or a tag to move it. Drag on empty canvas for a marquee;
  dragging any selected box moves them all. One undo step per drag.
- Drag from an outlet to an inlet to wire it. A wrong-sort inlet does not
  light up and refuses the drop, saying why (a note outlet into a control
  inlet points you to `gate`, as the prototype does). Dropping on an inlet
  that has a wire replaces it.
- Click a wire to select it; Delete removes the selection.
- Click an inlet tag to pick what feeds it, from the song's sources (the
  list `module_input_options` builds today).
- **Assign:** click an outlet's assignment button (or an assignment tag)
  to arm it, then drag any knob in the app: up adds depth, down inverts,
  exactly as Assign works now (`session.rs:1739`). Dragging a knob it
  already drives retunes that route.
- The selected box's surface below the canvas edits its parameters, as the
  shelf does now, until step 05 opens faces in place.

The route-count dots on device faces and their click-to-reveal (owed by
song modulation's *Open*) reveal the pane and select the route's outlet.

## Done when

- Adam has said go on the mock-up.
- The pane shows a converted song's patch, and every gesture above works
  and is one undo step.
- UI tests drive wiring, rewiring, deleting and assigning through the
  canvas `TouchArea`, as `pane_drag` drives the pane tabs.
- A song saved after moving boxes reopens with them where they were.
- `LISTENING.md` has an item to look at the pane: a converted song, the
  kick-gate example, and a wire that closes a loop.
