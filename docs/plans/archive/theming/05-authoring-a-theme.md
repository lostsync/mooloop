# 05 — authoring a theme

Part (b) of Adam's question: once the tokens exist, what does writing a theme
actually involve? This is the guide, and the two worked examples are the
acceptance test for steps 01-03 -- if an homage cannot be reached from the
token surface, the token surface is short an axis.

## The loop

1. Copy a built-in from `<config>/mooloop/themes/` and rename it.
2. Open Preferences > Appearance and select it. **Colors preview live as you
   drag**, because `apply_appearance` is the one funnel for preview and Apply
   both (`lib.rs:176`).
3. Shape, type and metrics need a reselect rather than a live drag, for the
   same reason motion does -- see the carve-out in `03-the-theme-file.md`.
4. Read the contrast report (step 04) before deciding you are done.

There is no build in that loop. That is the point of the whole plan.

## The five decisions a theme makes

**1. The base seed, and how wide the ramp is.** `base` is the background, and
every neutral -- panel, three surfaces, border, three text weights -- is
derived from it by `derive_palette`. `contrast` multiplies every neutral's
distance from the base, and it is the most underrated field in the file.

The ramp is narrow by default: `surface` is 5.5% off the base, `border` 17%.
That suits mooloop's own look, where the chrome sits *close* to its
background. **An interface whose chrome sits far from its background needs a
high contrast value, not a different base.** This is exactly the Impulse
Tracker case below -- black background, mid-grey boxes -- and getting it wrong
by picking a grey base instead produces a washed-out approximation with no
black in it anywhere.

**2. Accent, which is state.** Selection, focus, meters in their safe range.
Pick the target interface's *selection* color, not its brand color.

**3. Alert, which is attention.** Warnings and meter headroom. A true clip
ignores this and stays the fixed red, deliberately -- a clip must never blend
into a chosen palette.

**4. Shape.** `roundness` and `relief` together decide the era more than the
colors do. `roundness: 0` + `relief: bevel` is 1984-1997. `roundness: 1` +
`relief: flat` is what mooloop is now.

**5. Type.** The family list, and the scale. Remember a theme cannot ship a
font -- see `03-the-theme-file.md` -- so list several and expect the fallback.

## Worked example: Platinum

An homage to Mac OS 8, not a reproduction of it.

```toml
schema-version = 1
name = "Platinum"
description = "Grey slabs, square corners, light from the top-left."

[color]
base = "#dddddd"      # a light base flips the ramp; no second code path
accent = "#4a6fa5"    # the selection blue, not the Apple logo
alert = "#c07000"
contrast = 1.20       # Platinum's chrome is close to its background

[shape]
roundness = 0.0
relief = "bevel"
relief-depth = 1.0
hairline = 1
stroke-emphasis = 2

[type]
family = "Charcoal, Geneva, Helvetica, sans-serif"
family-mono = "Monaco, monospace"
scale = 1.0

[metrics]
density = 1.0

[motion]
speed = "Fast"
easing = "Linear"     # nothing in 1997 eased
```

What it will get right: the slabs, the square corners, the light direction,
the near-flat grey ramp. What it will not: the striped title bar, the
scroll-bar thumb's texture, and the fact that Platinum's window frame is not
mooloop's window frame. That is the scope boundary from the README doing its
job.

## Worked example: Impulse

An homage to Impulse Tracker and Schism, which is the harder of the two and
the one that tests the ramp.

```toml
schema-version = 1
name = "Impulse"
description = "Black field, grey boxes, one bright green."

[color]
base = "#0a0a0e"      # near-black, because the field really is black
accent = "#33dd33"    # the volume-column green
alert = "#dddd33"
contrast = 2.20       # the whole trick: mid-grey boxes on a black field

[shape]
roundness = 0.0
relief = "bevel"
relief-depth = 1.4    # IT's bevels are heavier than Platinum's
hairline = 1

[type]
family = "MoolooPixel, Terminus, monospace"
family-mono = "MoolooPixel, Terminus, monospace"
scale = 1.0
weight = 400

[metrics]
density = 0.9         # tracker chrome is tighter than mooloop's

[motion]
speed = "Instant"
easing = "Linear"     # a tracker does not animate. at all.
```

Three notes this example exists to make:

- **`contrast` at 2.2 is doing the work `base` cannot.** The default 1.0 ramp
  on a near-black base gives surfaces at `#131317`, which is mooloop-dark, not
  tracker. Pushing contrast is what separates the boxes from the field.
- **`MoolooPixel` is the font step 03 says to compile in.** It is named first
  so the theme works on a machine with nothing installed, with Terminus as the
  courtesy fallback for anyone who has it.
- **`speed = "Instant"` is not a performance setting.** It is the theme
  making a statement, and it is available today.

## What to check before calling a theme done

- The contrast report's worst pair. A homage that fails 4.5:1 on body text is
  a homage nobody can use for eight hours.
- A meter under signal. `meter_safe` is the accent, so an accent chosen purely
  for chrome can make a meter unreadable.
- A device face, not just the dialogs. The DS-01 and the EQ keep their own
  graphics and a theme has to sit next to them.
- The interface at `type-scale: 1.25`, if the theme sets a family with
  different metrics to the default. A font swap is a layout change.
