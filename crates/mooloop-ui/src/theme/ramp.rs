//! The sixteen colours a theme is, and everything derived from them.
//!
//! **Why a ramp and not three seeds.** Appearance grew the whole palette from
//! `base`, `accent` and `alert`, which is enough to make a coherent interface
//! and not enough to *be* Nord. `ENHANCEMENTS.md` recorded the open question --
//! whether a named scheme is three seeds or a full ramp -- and said that
//! base16, pywal and wallust would decide it. They all hand over sixteen
//! colours, so the seed model grows a second form rather than being asked to
//! approximate one.
//!
//! **The seed form did not go away and is not a legacy path.** It synthesizes
//! a ramp ([`Ramp::from_seeds`]) and everything downstream reads a ramp, so
//! there is one derivation rather than two. The synthesis is tuned so that the
//! palette it produces matches the one the three seeds produced before the
//! ramp existed -- to within one byte per channel, at every contrast setting,
//! which is the rounding cost of stopping at an 8-bit slot on the way and is
//! all of it. `tests` below is where that is pinned.
//!
//! The slot names are base16's, because that is the interchange format and
//! renaming the thing everyone else publishes helps nobody:
//!
//! | Slot | base16 | What mooloop draws with it |
//! | --- | --- | --- |
//! | 00 | default background | `background`, and `panel` a step below it |
//! | 01 | lighter background | `surface` |
//! | 02 | selection background | `surface-raised`, and `surface-active` just above |
//! | 03 | comments, invisibles | `text-faint`, and `border` just below |
//! | 04 | dark foreground | `text-muted` |
//! | 05 | default foreground | `text` |
//! | 06, 07 | light foreground/background | the light variant's end of the ladder |
//! | 08-0F | red, orange, yellow, green, cyan, blue, magenta, brown | the accent, the alerts, and the swatch palette |

use super::color::{
    blend_hues, contrast_ratio, extend, mix, relative_luminance, rgb, shade, Hsl, Rgb, BLACK, WHITE,
};

/// How far each neutral sits from `base00` when a ramp is synthesized from
/// three seeds, as a fraction of the distance to the contrasting pole.
///
/// **These are the numbers the old `derive_palette` used**, re-expressed as
/// ramp slots. 01, 02, 03, 04 and 05 are its surface, raised, faint, muted and
/// text; 06 and 07 continue the ladder to the pole so that an inverted variant
/// has somewhere to start. Its `active` and `border` are not slots -- they sit
/// between 02 and 03, and [`Ramp::palette`] puts them back.
///
/// The old derivation went from the seed to the pole in one step; this one
/// stops at a slot and carries on, and a slot is eight bits per channel. So
/// the two agree to within one byte per channel rather than exactly, and the
/// test below is written to that -- see its note, which is the whole of what
/// there is to say about it.
const SEED_NEUTRALS: [f32; 8] = [0.0, 0.055, 0.075, 0.25, 0.62, 0.87, 0.94, 1.0];

/// Where `surface-active` and `border` sit between slots 02 and 03.
///
/// Chosen so a seed-synthesized ramp reproduces the old palette exactly:
/// `0.075 + 0.23 * 0.175` is 0.1153 against its 0.115, and
/// `0.075 + 0.54 * 0.175` is 0.1695 against its 0.17. Both land on the same
/// byte. They are also the right places on an authored base16 ramp, where 02
/// is a selection background and 03 is the comment colour: a border belongs
/// nearer the comment, an active surface nearer the selection.
const ACTIVE_BETWEEN_02_AND_03: f32 = 0.23;
const BORDER_BETWEEN_02_AND_03: f32 = 0.54;

/// The panel sits *behind* the work surface, so it moves away from the
/// contrast pole rather than toward it. There is no base16 slot below the
/// background, which is the one place mooloop's palette is wider than the
/// format it imports.
const PANEL_BELOW_BACKGROUND: f32 = -0.12;

/// The eight hues a seed-synthesized ramp carries.
///
/// Slot 0A is overwritten with the `alert` seed and the accent with the
/// `accent` seed, so what is left here is the swatch palette the channel and
/// pattern pickers offered before a theme could supply one -- the same hues,
/// in base16's slot order.
const SEED_HUES: [Rgb; 8] = [
    rgb(0xEF, 0x44, 0x44), // 08 red      -- also `destructive`
    rgb(0xF9, 0x73, 0x16), // 09 orange
    rgb(0xEA, 0xB3, 0x08), // 0A yellow   -- replaced by the alert seed
    rgb(0x22, 0xC5, 0x5E), // 0B green
    rgb(0x14, 0xB8, 0xA6), // 0C cyan
    rgb(0x0E, 0xA5, 0xE9), // 0D blue     -- the accent slot, replaced by the seed
    rgb(0xEC, 0x48, 0x99), // 0E magenta
    rgb(0xA1, 0x62, 0x07), // 0F brown
];

/// How many colours the channel/pattern swatch grid offers. Eleven plus the
/// "no colour" square is twelve, which is the two tidy rows of six
/// `color-picker.slint` draws.
pub(crate) const SWATCH_COUNT: usize = 11;

/// A hue below this saturation has no hue worth sorting by -- a ramp whose
/// "red" slot is a grey should not put that grey between red and orange.
const GREY_SATURATION: f32 = 0.06;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ramp {
    pub slots: [Rgb; 16],
    /// Which colour the interface uses for selection, focus and the safe end
    /// of a meter. `None` means slot 0D, which is base16's blue and is what an
    /// imported scheme will almost always want.
    pub accent: Option<Rgb>,
}

impl Ramp {
    pub(crate) fn slot(&self, index: usize) -> Rgb {
        self.slots[index.min(15)]
    }

    pub(crate) fn accent(&self) -> Rgb {
        self.accent.unwrap_or_else(|| self.slot(0x0D))
    }

    pub(crate) fn is_dark(&self) -> bool {
        self.slot(0).is_dark()
    }

    /// Grows a ramp from the three seeds Appearance has always had.
    ///
    /// The neutrals are the old derivation's own fractions and the hues are
    /// the swatch palette that used to be a separate table in
    /// `channel_colors.rs`. Two seeds land in slots: `alert` in 0A, because
    /// that is what warnings and the meter's headroom band read, and `accent`
    /// as the override.
    pub(crate) fn from_seeds(base: Rgb, accent: Rgb, alert: Rgb) -> Self {
        let dark = base.is_dark();
        let mut slots = [base; 16];
        for (index, fraction) in SEED_NEUTRALS.iter().enumerate() {
            slots[index] = shade(base, dark, *fraction);
        }
        slots[8..16].copy_from_slice(&SEED_HUES);
        slots[0x0A] = alert;
        Self {
            slots,
            accent: Some(accent),
        }
    }

    /// Grows a ramp from a terminal palette: sixteen ANSI colours, optionally
    /// with the background and foreground the generator chose separately.
    ///
    /// This is the shape pywal and wallust write, and it is not base16 --
    /// ANSI's eight extra colours are brightness variants rather than eight
    /// more hues, and it has no slot for "comment" or "selection". So the
    /// neutral ladder is interpolated between the background and the
    /// foreground, with ANSI's bright black taken as the comment colour when
    /// it lands between the two slots it would sit between, and the hues come
    /// from the normal set.
    ///
    /// **Orange and brown are derived.** ANSI has no slot for either, and
    /// leaving them empty would cost the swatch palette two of its eleven, so
    /// they are mixed from red and yellow: orange halfway, brown the same hue
    /// pulled back toward the background.
    pub(crate) fn from_ansi(
        colors: &[Rgb; 16],
        background: Option<Rgb>,
        foreground: Option<Rgb>,
    ) -> Self {
        let background = background.unwrap_or(colors[0]);
        let foreground = foreground.unwrap_or(colors[7]);
        let dark = background.is_dark();
        let step = |fraction: f32| mix(background, foreground, fraction);

        let mut slots = [background; 16];
        slots[0x00] = background;
        slots[0x01] = step(0.06);
        slots[0x02] = step(0.13);
        // ANSI's bright black is the comment colour in every terminal scheme
        // that has one -- but only where it lands in the right place on this
        // ladder, and "somewhere between the background and the foreground" is
        // not the right place. A grey lighter than the *muted* step makes slot
        // 03 lighter than slot 04, which is a hierarchy that runs backwards:
        // faint text drawn brighter than muted text. So it is taken only when
        // it sits strictly between the two slots it would go between, and any
        // scheme whose grey is anywhere else gets the interpolated step.
        slots[0x04] = step(0.66);
        let grey = colors[8];
        let strictly_between = |c: Rgb, low: Rgb, high: Rgb| {
            let (l, a, b) = (
                relative_luminance(c),
                relative_luminance(low),
                relative_luminance(high),
            );
            if a <= b {
                l > a && l < b
            } else {
                l < a && l > b
            }
        };
        slots[0x03] = if strictly_between(grey, slots[0x02], slots[0x04]) {
            grey
        } else {
            step(0.32)
        };
        slots[0x05] = foreground;
        slots[0x06] = shade(foreground, dark, 0.35);
        slots[0x07] = shade(foreground, dark, 0.7);

        let orange = blend_hues(colors[1], colors[3]);
        slots[0x08] = colors[1]; // red
        slots[0x09] = orange;
        slots[0x0A] = colors[3]; // yellow
        slots[0x0B] = colors[2]; // green
        slots[0x0C] = colors[6]; // cyan
        slots[0x0D] = colors[4]; // blue
        slots[0x0E] = colors[5]; // magenta
        slots[0x0F] = mix(orange, background, 0.35);

        Self {
            slots,
            // Bright blue where the generator gave one worth using: a
            // wallpaper scheme's normal blue is often too close to its own
            // background to read as a selection.
            accent: Some(pick_accent(colors, background)),
        }
    }

    /// The other variant of this ramp, for a theme that only authored one.
    ///
    /// The neutral ladder reverses -- what was the darkest background becomes
    /// the lightest, and the ladder keeps its spacing because a reversal is a
    /// reversal at both ends. The hues keep their hue and their saturation and
    /// have their lightness retargeted, because a pastel that reads on black
    /// disappears on white and the fix is never to change the colour.
    ///
    /// An authored variant always beats this one. It exists so that a theme
    /// with one variant still answers Light/Dark/Auto rather than ignoring it.
    pub(crate) fn inverted(&self) -> Self {
        let mut slots = self.slots;
        let mut ladder = [self.slot(0); 8];
        ladder.copy_from_slice(&self.slots[..8]);
        ladder.reverse();
        slots[..8].copy_from_slice(&ladder);
        let now_dark = slots[0].is_dark();
        for slot in slots.iter_mut().skip(8) {
            *slot = retarget_lightness(*slot, now_dark);
        }
        // **The accent is held to the bar, not just to a lightness.**
        // Retargeting alone put Monokai's green at 2.1:1 when it flipped to
        // light -- readable as a word, invisible as a selection. And it is
        // measured against slot 01 rather than slot 00, because that is the
        // surface a selection is actually drawn on and it is the darker of
        // the two on a light ramp.
        let accent = self.accent.map(|accent| {
            super::legible_against(
                retarget_lightness(accent, now_dark),
                slots[1],
                super::MIN_ACCENT_CONTRAST,
            )
        });
        Self { slots, accent }
    }

    /// Every token the interface reads, grown from the ramp.
    ///
    /// `contrast` scales each neutral's distance from the background, which is
    /// one control over the whole hierarchy. Hues are left alone: an accent is
    /// a colour somebody chose, and scaling it toward the background is how a
    /// contrast control ends up *reducing* contrast.
    pub(crate) fn palette(&self, contrast: f32) -> ThemePalette {
        let background = self.slot(0);
        let dark = background.is_dark();
        // Distance from the background scales. `extend` rather than `mix`,
        // because past 1.0 the top of the ramp is meant to leave the authored
        // slot behind and arrive at white -- a clamped weight would make the
        // widen-the-hierarchy control do nothing at its own maximum.
        let open = |slot: usize| extend(background, self.slot(slot), contrast);
        let raised = open(0x02);
        let faint = open(0x03);
        let accent = self.accent();
        let on_accent = if relative_luminance(accent) > 0.45 {
            BLACK
        } else {
            WHITE
        };
        let destructive = self.slot(0x08);
        ThemePalette {
            background,
            panel: shade(background, dark, PANEL_BELOW_BACKGROUND * contrast),
            surface: open(0x01),
            raised,
            active: mix(raised, faint, ACTIVE_BETWEEN_02_AND_03),
            border: mix(raised, faint, BORDER_BETWEEN_02_AND_03),
            text: open(0x05),
            muted: open(0x04),
            faint,
            accent,
            accent_active: mix(accent, background, 0.58),
            focus: mix(accent, on_accent, 0.22),
            warning: self.slot(0x0A),
            destructive,
            destructive_active: mix(destructive, background, 0.62),
            // Meters read as one instrument with the rest of the UI: the safe
            // level is the accent, the headroom warning is the alert colour,
            // and only a true clip falls back to the ramp's red.
            meter_safe: accent,
            meter_warning: self.slot(0x0A),
            meter_clip: destructive,
        }
    }

    /// The eleven colours the channel, track and pattern pickers offer.
    ///
    /// **A channel colour is an identifier, not a shade**, so the rule is
    /// separation rather than prettiness: take the ramp's eight hues in hue
    /// order, then fill to eleven by splitting whichever gap around the wheel
    /// is currently widest. That is adaptive -- a scheme with four blues and
    /// no green gets its extra three colours somewhere useful -- and on the
    /// seed ramp it reproduces the hand-picked eleven this replaced, including
    /// the lime between yellow and green and the indigo between blue and
    /// violet.
    ///
    /// The song stores the colour rather than an index, so this list bounds
    /// nothing: changing themes does not repaint anybody's channels, and the
    /// hex field beside the swatches still reaches every other colour.
    pub(crate) fn swatches(&self) -> Vec<Rgb> {
        let mut hues: Vec<Rgb> = Vec::with_capacity(SWATCH_COUNT);
        for slot in 8..16 {
            let color = self.slot(slot);
            if !hues.contains(&color) {
                hues.push(color);
            }
        }
        if hues.is_empty() {
            return Vec::new();
        }
        hues.sort_by(|a, b| sort_key(*a).total_cmp(&sort_key(*b)));
        while hues.len() < SWATCH_COUNT {
            let widest = widest_gap(&hues);
            let next = (widest + 1) % hues.len();
            let filler = blend_hues(hues[widest], hues[next]);
            if hues.contains(&filler) {
                break;
            }
            hues.insert(widest + 1, filler);
        }
        hues.truncate(SWATCH_COUNT);
        hues
    }

    /// Whether `text` on `surface` clears a WCAG ratio, which is the one
    /// accessibility claim a derived palette can actually make.
    pub(crate) fn text_contrast(&self, contrast: f32) -> f32 {
        let palette = self.palette(contrast);
        contrast_ratio(palette.text, palette.surface)
    }

    pub(crate) fn accent_contrast(&self, contrast: f32) -> f32 {
        let palette = self.palette(contrast);
        contrast_ratio(palette.accent, palette.surface)
    }
}

/// Where a colour sorts on the wheel. A grey has no hue and is parked at the
/// end rather than being called red, which is what `Hsl` reports for it.
fn sort_key(color: Rgb) -> f32 {
    let hsl = Hsl::from_rgb(color);
    if hsl.s < GREY_SATURATION {
        2.0 + hsl.l
    } else {
        hsl.h
    }
}

/// Index of the hue that starts the widest gap to its neighbour, going round.
fn widest_gap(hues: &[Rgb]) -> usize {
    let mut widest = 0;
    let mut widest_size = -1.0f32;
    for index in 0..hues.len() {
        let next = (index + 1) % hues.len();
        let (a, b) = (sort_key(hues[index]), sort_key(hues[next]));
        let size = if b >= a { b - a } else { b + 1.0 - a };
        if size > widest_size {
            widest_size = size;
            widest = index;
        }
    }
    widest
}

/// Moves a hue's lightness onto the side of the ladder the background is not.
const HUE_LIGHTNESS_ON_DARK: f32 = 0.60;
const HUE_LIGHTNESS_ON_LIGHT: f32 = 0.42;

fn retarget_lightness(color: Rgb, background_is_dark: bool) -> Rgb {
    let mut hsl = Hsl::from_rgb(color);
    hsl.l = if background_is_dark {
        hsl.l.max(HUE_LIGHTNESS_ON_DARK)
    } else {
        hsl.l.min(HUE_LIGHTNESS_ON_LIGHT)
    };
    hsl.to_rgb()
}

/// Which of a terminal palette's colours to use for selection and focus.
///
/// Blue is the convention and is tried first in both its normal and bright
/// form; a wallpaper-derived scheme, though, is often eight colours of one
/// hue, and the one that reads as a selection is simply whichever has the most
/// contrast against the background. So: blue if blue is legible, otherwise the
/// most legible colour there is.
fn pick_accent(colors: &[Rgb; 16], background: Rgb) -> Rgb {
    const LEGIBLE: f32 = 3.0;
    for candidate in [colors[12], colors[4]] {
        if contrast_ratio(candidate, background) >= LEGIBLE {
            return candidate;
        }
    }
    colors[1..]
        .iter()
        .copied()
        .max_by(|a, b| contrast_ratio(*a, background).total_cmp(&contrast_ratio(*b, background)))
        .unwrap_or(colors[4])
}

#[derive(Clone, Copy)]
pub(crate) struct ThemePalette {
    pub background: Rgb,
    pub panel: Rgb,
    pub surface: Rgb,
    pub raised: Rgb,
    pub active: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    pub muted: Rgb,
    pub faint: Rgb,
    pub accent: Rgb,
    pub accent_active: Rgb,
    pub focus: Rgb,
    pub warning: Rgb,
    pub destructive: Rgb,
    pub destructive_active: Rgb,
    pub meter_safe: Rgb,
    pub meter_warning: Rgb,
    pub meter_clip: Rgb,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::color::Hsl;

    fn seeds() -> Ramp {
        Ramp::from_seeds(
            Rgb::parse("#18181B").unwrap(),
            Rgb::parse("#84CC16").unwrap(),
            Rgb::parse("#EAB308").unwrap(),
        )
    }

    /// The old three-seed derivation, copied here verbatim from the commit
    /// that replaced it.
    ///
    /// **This is the guard that makes the ramp a widening rather than a
    /// change.** Every existing settings file holds three seeds, and a user
    /// who never opens the Appearance page must see exactly the interface they
    /// saw yesterday. A copy of the old arithmetic is the only thing that can
    /// say so -- comparing the new code against itself would pass whatever it
    /// did.
    fn old_palette(base: Rgb, accent: Rgb, alert: Rgb, contrast: f32) -> Vec<(&'static str, Rgb)> {
        let dark = relative_luminance(base) < 0.4;
        let scale = contrast.clamp(0.6, 1.4);
        let step = |fraction: f32| shade(base, dark, fraction * scale);
        let background = base;
        let on_accent = if relative_luminance(accent) > 0.45 {
            BLACK
        } else {
            WHITE
        };
        let destructive = rgb(0xef, 0x44, 0x44);
        vec![
            ("background", background),
            ("panel", step(-0.12)),
            ("surface", step(0.055)),
            ("raised", step(0.075)),
            ("active", step(0.115)),
            ("border", step(0.17)),
            ("text", step(0.87)),
            ("muted", step(0.62)),
            ("faint", step(0.25)),
            ("accent", accent),
            ("accent_active", mix(accent, background, 0.58)),
            ("focus", mix(accent, on_accent, 0.22)),
            ("warning", alert),
            ("destructive", destructive),
            ("destructive_active", mix(destructive, background, 0.62)),
            ("meter_safe", accent),
            ("meter_warning", alert),
            ("meter_clip", destructive),
        ]
    }

    fn new_palette(ramp: &Ramp, contrast: f32) -> Vec<(&'static str, Rgb)> {
        let p = ramp.palette(contrast);
        vec![
            ("background", p.background),
            ("panel", p.panel),
            ("surface", p.surface),
            ("raised", p.raised),
            ("active", p.active),
            ("border", p.border),
            ("text", p.text),
            ("muted", p.muted),
            ("faint", p.faint),
            ("accent", p.accent),
            ("accent_active", p.accent_active),
            ("focus", p.focus),
            ("warning", p.warning),
            ("destructive", p.destructive),
            ("destructive_active", p.destructive_active),
            ("meter_safe", p.meter_safe),
            ("meter_warning", p.meter_warning),
            ("meter_clip", p.meter_clip),
        ]
    }

    /// **Within one byte per channel, at every contrast setting, on a light
    /// base as well as a dark one.**
    ///
    /// Not exactly equal, and the reason is worth stating rather than
    /// tolerating: the old derivation interpolated from the seed to the
    /// contrast pole in one step, and this one stops at a ramp slot on the
    /// way. A slot is eight bits per channel, so the second interpolation
    /// starts from a rounded number. One byte is what that costs and it is
    /// what this asserts -- a *tolerance*, not a wobble, and anything that
    /// exceeded it would be a real change in what the program looks like.
    ///
    /// The exact figure was measured before the tolerance was chosen: five
    /// schemes, eight contrast settings, twelve tokens, worst case one.
    #[test]
    fn a_seed_ramp_reproduces_the_palette_the_seeds_used_to_produce() {
        let cases = [
            ("#18181B", "#84CC16", "#EAB308"),
            ("#151617", "#F59E0B", "#38BDF8"),
            ("#000000", "#22D3EE", "#FACC15"),
            ("#EDEDF0", "#3F7D00", "#B45309"), // a light base: the poles swap
            ("#1A1413", "#F97316", "#38BDF8"),
        ];
        for (base, accent, alert) in cases {
            let (base, accent, alert) = (
                Rgb::parse(base).unwrap(),
                Rgb::parse(accent).unwrap(),
                Rgb::parse(alert).unwrap(),
            );
            let ramp = Ramp::from_seeds(base, accent, alert);
            for contrast in [0.6, 0.7, 0.85, 1.0, 1.1, 1.2, 1.3, 1.4] {
                let old = old_palette(base, accent, alert, contrast);
                let new = new_palette(&ramp, contrast);
                for ((name, was), (_, now)) in old.into_iter().zip(new) {
                    let drift = [
                        was.r.abs_diff(now.r),
                        was.g.abs_diff(now.g),
                        was.b.abs_diff(now.b),
                    ]
                    .into_iter()
                    .max()
                    .unwrap();
                    assert!(
                        drift <= 1,
                        "{name} moved from {} to {} on {} at contrast {contrast}",
                        was.to_hex(),
                        now.to_hex(),
                        base.to_hex()
                    );
                }
            }
        }
    }

    /// The half of the contrast control that a clamped interpolation silently
    /// removed: at its maximum the top of the ramp reaches white, not the
    /// colour the theme happened to write in slot 05.
    #[test]
    fn contrast_at_its_maximum_still_reaches_the_pole() {
        let ramp = seeds();
        assert_eq!(ramp.palette(1.4).text.to_hex(), "#FFFFFF");
        let light = Ramp::from_seeds(
            Rgb::parse("#EDEDF0").unwrap(),
            Rgb::parse("#3F7D00").unwrap(),
            Rgb::parse("#B45309").unwrap(),
        );
        assert_eq!(light.palette(1.4).text.to_hex(), "#000000");
    }

    /// The eleven the hand-picked table used to hold, to within a hue step.
    /// Not an equality test -- the point of deriving them is that they follow
    /// the scheme -- but the default scheme's swatch row should not have
    /// visibly changed, and a row of eleven wildly different colours is what
    /// a mistake in `widest_gap` would look like.
    #[test]
    fn the_seed_ramps_swatches_are_the_eleven_they_replaced() {
        let swatches = seeds().swatches();
        assert_eq!(swatches.len(), SWATCH_COUNT);
        let previous = [
            "#EF4444", "#F97316", "#EAB308", "#84CC16", "#22C55E", "#14B8A6", "#0EA5E9", "#6366F1",
            "#A855F7", "#EC4899", "#A16207",
        ];
        for hex in previous {
            let want = Hsl::from_rgb(Rgb::parse(hex).unwrap());
            let nearest = swatches
                .iter()
                .map(|s| Hsl::from_rgb(*s).hue_delta(want).abs())
                .fold(f32::MAX, f32::min);
            assert!(
                nearest < 0.06,
                "nothing within a hue step of {hex}; nearest was {nearest:.3} turns away"
            );
        }
    }

    /// Eleven identifiers that are hard to tell apart are one identifier with
    /// extra steps, which is the note the table this replaced was written
    /// under.
    #[test]
    fn swatches_stay_apart() {
        for ramp in [
            seeds(),
            super::super::builtins::all()[1].variant(true).ramp(),
        ] {
            let swatches = ramp.swatches();
            assert_eq!(swatches.len(), SWATCH_COUNT);
            for (index, a) in swatches.iter().enumerate() {
                for b in &swatches[index + 1..] {
                    assert_ne!(a, b, "duplicate swatch {}", a.to_hex());
                }
            }
        }
    }

    /// A ramp with no hues at all -- every accent slot the same grey -- is a
    /// thing a wallpaper importer can produce from a monochrome image, and it
    /// must not spin in `swatches`.
    #[test]
    fn a_ramp_with_one_hue_terminates() {
        let mut ramp = seeds();
        for slot in ramp.slots.iter_mut().skip(8) {
            *slot = rgb(0x80, 0x80, 0x80);
        }
        assert_eq!(ramp.swatches(), vec![rgb(0x80, 0x80, 0x80)]);
    }

    #[test]
    fn inverting_a_dark_ramp_gives_a_light_one_and_back_again() {
        let dark = seeds();
        assert!(dark.is_dark());
        let light = dark.inverted();
        assert!(!light.is_dark());
        // The hues survive the trip: lightness is retargeted, hue is not.
        for slot in 8..16 {
            let (before, after) = (
                Hsl::from_rgb(dark.slot(slot)),
                Hsl::from_rgb(light.slot(slot)),
            );
            if before.s > GREY_SATURATION {
                assert!(
                    before.hue_delta(after).abs() < 0.02,
                    "slot {slot:02X} changed hue"
                );
            }
        }
    }

    #[test]
    fn an_inverted_variant_still_reads() {
        let light = seeds().inverted();
        assert!(
            light.text_contrast(1.0) >= 4.5,
            "derived light variant text ratio {:.2}",
            light.text_contrast(1.0)
        );
    }

    /// pywal writes a background, a foreground, and sixteen ANSI colours. The
    /// interesting half is the neutrals, which ANSI does not have.
    #[test]
    fn an_ansi_palette_grows_a_neutral_ladder() {
        let mut colors = [rgb(0, 0, 0); 16];
        let sample = [
            "#1D1F21", "#CC6666", "#B5BD68", "#F0C674", "#81A2BE", "#B294BB", "#8ABEB7", "#C5C8C6",
            "#969896", "#CC6666", "#B5BD68", "#F0C674", "#81A2BE", "#B294BB", "#8ABEB7", "#FFFFFF",
        ];
        for (slot, hex) in sample.iter().enumerate() {
            colors[slot] = Rgb::parse(hex).unwrap();
        }
        let ramp = Ramp::from_ansi(&colors, None, None);
        assert!(ramp.is_dark());
        // Monotonic away from the background, which is what makes a hierarchy.
        let ladder: Vec<f32> = (0..6).map(|s| relative_luminance(ramp.slot(s))).collect();
        for pair in ladder.windows(2) {
            assert!(pair[1] > pair[0], "ladder went backwards: {ladder:?}");
        }
        // Bright black is *lighter* than the muted step in this scheme, so it
        // is not the comment colour and the interpolated step is used. Taking
        // it would run the hierarchy backwards, which is what the ladder
        // assertion above is here to catch.
        assert_ne!(ramp.slot(3), Rgb::parse("#969896").unwrap());
        // Orange is between red and yellow, and has a hue between theirs.
        let orange = Hsl::from_rgb(ramp.slot(9));
        assert!(
            (0.02..=0.13).contains(&orange.h),
            "orange hue {:.3}",
            orange.h
        );
        assert_eq!(ramp.swatches().len(), SWATCH_COUNT);
    }

    /// The other half of the rule above: a scheme whose bright black *does*
    /// sit between the selection and the muted step keeps it, because that is
    /// the colour the scheme chose for exactly this job.
    #[test]
    fn a_bright_black_in_the_right_place_is_the_comment_colour() {
        let mut colors = [rgb(0, 0, 0); 16];
        let sample = [
            "#191724", "#EB6F92", "#31748F", "#F6C177", "#9CCFD8", "#C4A7E7", "#EBBCBA", "#E0DEF4",
            "#6E6A86", "#EB6F92", "#31748F", "#F6C177", "#9CCFD8", "#C4A7E7", "#EBBCBA", "#E0DEF4",
        ];
        for (slot, hex) in sample.iter().enumerate() {
            colors[slot] = Rgb::parse(hex).unwrap();
        }
        let ramp = Ramp::from_ansi(&colors, None, None);
        assert_eq!(ramp.slot(3), Rgb::parse("#6E6A86").unwrap());
        let ladder: Vec<f32> = (0..6).map(|s| relative_luminance(ramp.slot(s))).collect();
        for pair in ladder.windows(2) {
            assert!(pair[1] > pair[0], "ladder went backwards: {ladder:?}");
        }
    }

    /// A wallpaper scheme whose blue is nearly its background has no usable
    /// blue, and an accent nobody can see is worse than an accent that is the
    /// wrong hue.
    #[test]
    fn an_illegible_blue_is_not_taken_as_the_accent() {
        let mut colors = [rgb(0x10, 0x10, 0x14); 16];
        colors[4] = rgb(0x12, 0x12, 0x1A); // "blue", invisible
        colors[12] = rgb(0x14, 0x14, 0x1C); // bright "blue", also invisible
        colors[3] = rgb(0xF0, 0xD0, 0x60); // the one legible colour
        let ramp = Ramp::from_ansi(&colors, None, Some(rgb(0xE0, 0xE0, 0xE0)));
        assert_eq!(ramp.accent(), colors[3]);
    }
}
