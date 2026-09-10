# 02 — relief

The step that makes an homage look like one. Everything else in this plan is
sweeping; this is a design decision with a wrong answer available.

## The problem

macOS Classic and Impulse Tracker are the same UI idea: **every surface is a
lit block.** A button is a slab with light falling on it from the top-left,
and a pressed button is the same slab pushed in, so the light comes from the
bottom-right instead. Nothing in either interface is a flat rectangle with a
uniform border.

A Slint `Rectangle` has `border-color`, singular. There is no per-edge stroke.
So the difference between mooloop's chrome and a bevelled chrome is not a
token value -- it is a different number of elements, and no property on
`Theme` can produce it.

## The shape of the answer

One token that names a *drawing*, and one component that does the drawing.

```slint
// 0 = flat (today: fill plus a uniform hairline border)
// 1 = bevel (a lit block: two light edges, two dark edges)
// 2 = inset (the same block pushed in: the light edges swap)
in-out property <int> relief: 0;
// How far the bevel's edges depart from the surface they sit on.
in-out property <float> relief-depth: 1.0;
```

and in `controls.slint`:

```slint
export component Surface inherits Rectangle {
    in property <color> fill: Theme.surface-raised;
    in property <bool> pressed: false;
    in property <bool> outline: true;   // false for a surface with no edge

    background: root.fill;
    border-radius: Theme.radius-md;
    border-width: Theme.relief == 0 && root.outline ? Theme.hairline : 0px;
    border-color: Theme.border;

    if Theme.relief != 0 && root.outline : Bevel {
        // two Rectangles, or a Path, per lit pair -- see below
        depth: Theme.relief-depth;
        inset: (Theme.relief == 2) != root.pressed;
        base: root.fill;
    }
}
```

The controls then inherit `Surface` rather than `Rectangle`, and every one of
them gets bevels for free the day `relief` is set. `ToolButton`
(`controls.slint:63`) is the first and the template: it already computes its
own `background` from `pressed` / `active` / `enabled`, which is exactly the
`fill` a `Surface` wants, and its `border-width` conditional is exactly what
moves into the component.

## Two things to get right

**A bevel is derived from the fill, not authored.** The light and dark edges
are the surface lightened and darkened by `relief-depth`, computed the way
`derive_palette` already computes the neutral ramp -- so a bevel works on any
palette, including a light one, without a theme listing eight more colors.
Authoring them would be four more fields per theme and a guarantee that some
theme gets them wrong.

**Radius and relief are exclusive in practice.** A rounded bevel is a mitre
problem, and both target interfaces are square. So a theme that sets
`relief != 0` sets `roundness: 0`, and the Appearance page should say so
rather than let the two fight. Whether to *enforce* it is a call to make when
the drawing exists and it can be looked at; do not enforce it in advance.

## The long tail, and why it is acceptable to leave

The 28 exported controls are the interface a user touches, and converting them
is bounded work. The ~190 anonymous `Rectangle`s in `main.slint`,
`modulation-shelf.slint` and `device-displays.slint` are panels, rails,
separators and backgrounds -- and most of them **should stay flat even in a
bevelled theme**, because a bevel on every container is what makes a
bevelled UI look like a parody of itself rather than like the thing.

So the conversion target is: the controls, the device header, the rack row,
the panel and pane edges, and the dock. Not everything that is a rectangle.
Pick them by asking whether the element is a *thing you press or a plate
something sits on*; if it is neither, it is a divider and it stays flat.

## Order

Do `ToolButton` alone, sketch it with `scripts/slint-sketch`, and look at it
before touching the other 27. The whole step's risk is that the derived bevel
looks wrong at mooloop's sizes -- a 1px light edge on a 24px control at a 9px
type size is a different proposition to a 2px one on a 32px control -- and one
button answers that for 0.05s a try.
