//! Colour primitives shared by every part of the theme system.
//!
//! These lived in `settings.rs` when the whole palette was three seeds and one
//! derivation. They moved here when a theme became a sixteen-colour ramp,
//! because the ramp, the wallpaper importers, the light/dark derivation and
//! the swatch palette all need the same arithmetic and none of them is a
//! setting.

use slint::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

pub(crate) const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

impl Rgb {
    /// `#RRGGBB`, or `#RGB` -- the short form because a hand-written theme
    /// file is a thing a person types, and `#fff` is what they will type.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let hex = value.trim().strip_prefix('#')?;
        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let pair = |slice: &str| u8::from_str_radix(slice, 16).ok();
        match hex.len() {
            6 => Some(Self {
                r: pair(&hex[0..2])?,
                g: pair(&hex[2..4])?,
                b: pair(&hex[4..6])?,
            }),
            3 => {
                let double = |c: &str| pair(c).map(|v| v * 17);
                Some(Self {
                    r: double(&hex[0..1])?,
                    g: double(&hex[1..2])?,
                    b: double(&hex[2..3])?,
                })
            }
            _ => None,
        }
    }

    pub(crate) fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Swatch colours come straight from stored hex that has already been
    /// validated once; black is a visible, harmless fallback for the case
    /// where a hand-edited config slipped something else through.
    pub(crate) fn parse_or_black(value: &str) -> Self {
        Self::parse(value).unwrap_or(rgb(0, 0, 0))
    }

    pub(crate) fn color(self) -> Color {
        Color::from_rgb_u8(self.r, self.g, self.b)
    }

    pub(crate) fn is_dark(self) -> bool {
        relative_luminance(self) < 0.4
    }
}

pub(crate) fn mix(a: Rgb, b: Rgb, b_weight: f32) -> Rgb {
    let weight = b_weight.clamp(0.0, 1.0);
    let blend = |x: u8, y: u8| (x as f32 * (1.0 - weight) + y as f32 * weight).round() as u8;
    rgb(blend(a.r, b.r), blend(a.g, b.g), blend(a.b, b.b))
}

/// `mix`, but the factor may go past 1 and the result is clamped per channel
/// rather than the factor being clamped first.
///
/// This is what the contrast control needs and `mix` cannot give it. Contrast
/// scales each neutral's distance from the background, and at 1.4 the top of
/// the ramp is *supposed* to leave the slot behind and arrive at white -- a
/// clamped weight would stop it at the colour the theme authored, which turns
/// the widen-the-hierarchy control into a do-nothing control at its own
/// maximum.
pub(crate) fn extend(from: Rgb, to: Rgb, factor: f32) -> Rgb {
    let channel = |a: u8, b: u8| {
        (a as f32 + (b as f32 - a as f32) * factor)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    rgb(
        channel(from.r, to.r),
        channel(from.g, to.g),
        channel(from.b, to.b),
    )
}

pub(crate) const WHITE: Rgb = rgb(255, 255, 255);
pub(crate) const BLACK: Rgb = rgb(0, 0, 0);

/// Moves `color` toward the foreground pole for positive `amount` and toward
/// the background pole for negative, where the poles swap on a light base.
pub(crate) fn shade(color: Rgb, dark: bool, amount: f32) -> Rgb {
    let (foreground, background) = if dark { (WHITE, BLACK) } else { (BLACK, WHITE) };
    if amount >= 0.0 {
        mix(color, foreground, amount.min(1.0))
    } else {
        mix(color, background, (-amount).min(1.0))
    }
}

pub(crate) fn relative_luminance(color: Rgb) -> f32 {
    let linear = |channel: u8| {
        let c = channel as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

pub(crate) fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let (light, dark) = if relative_luminance(a) >= relative_luminance(b) {
        (a, b)
    } else {
        (b, a)
    };
    (relative_luminance(light) + 0.05) / (relative_luminance(dark) + 0.05)
}

/// Hue, saturation and lightness, all normalized, with hue in turns rather
/// than degrees so that wrapping is `fract()` and nothing has to remember 360.
///
/// This exists for the two jobs a straight RGB lerp does badly: finding a
/// colour *between* two hues (mixing red and green in RGB gives mud, not
/// yellow), and moving a hue's lightness without moving the hue, which is what
/// deriving a light variant from a dark one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

impl Hsl {
    pub(crate) fn from_rgb(color: Rgb) -> Self {
        let (r, g, b) = (
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let delta = max - min;
        if delta <= f32::EPSILON {
            return Self { h: 0.0, s: 0.0, l };
        }
        let s = delta / (1.0 - (2.0 * l - 1.0).abs()).max(f32::EPSILON);
        let h = if max == r {
            ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            (b - r) / delta + 2.0
        } else {
            (r - g) / delta + 4.0
        } / 6.0;
        Self {
            h: h.rem_euclid(1.0),
            s: s.clamp(0.0, 1.0),
            l,
        }
    }

    pub(crate) fn to_rgb(self) -> Rgb {
        let (h, s, l) = (
            self.h.rem_euclid(1.0),
            self.s.clamp(0.0, 1.0),
            self.l.clamp(0.0, 1.0),
        );
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h * 6.0).rem_euclid(2.0) - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (h * 6.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
        rgb(byte(r), byte(g), byte(b))
    }

    /// Signed distance from `self` to `other` around the wheel, taking the
    /// short way. In turns, so the result is in `-0.5..=0.5`.
    pub(crate) fn hue_delta(self, other: Self) -> f32 {
        let raw = (other.h - self.h).rem_euclid(1.0);
        if raw > 0.5 {
            raw - 1.0
        } else {
            raw
        }
    }
}

/// The colour halfway between two hues, going the short way around the wheel.
///
/// Saturation and lightness are averaged. A grey argument keeps the other's
/// hue rather than dragging the result toward an arbitrary one, which is what
/// an unguarded average of `h` would do -- grey has no hue and `from_rgb`
/// reports it as zero, which is red.
pub(crate) fn blend_hues(a: Rgb, b: Rgb) -> Rgb {
    let (x, y) = (Hsl::from_rgb(a), Hsl::from_rgb(b));
    let h = if x.s <= 0.02 {
        y.h
    } else if y.s <= 0.02 {
        x.h
    } else {
        x.h + x.hue_delta(y) / 2.0
    };
    Hsl {
        h,
        s: (x.s + y.s) / 2.0,
        l: (x.l + y.l) / 2.0,
    }
    .to_rgb()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_in_both_lengths() {
        assert_eq!(Rgb::parse("#84CC16").unwrap().to_hex(), "#84CC16");
        assert_eq!(Rgb::parse("#84cc16").unwrap().to_hex(), "#84CC16");
        assert_eq!(Rgb::parse("#fff").unwrap(), WHITE);
        assert_eq!(Rgb::parse("#000").unwrap(), BLACK);
        assert!(Rgb::parse("84CC16").is_none());
        assert!(Rgb::parse("#84CC1").is_none());
        assert!(Rgb::parse("#zzzzzz").is_none());
    }

    /// The whole reason `Hsl` exists is that it is reversible; every derived
    /// light variant is an HSL edit applied to an authored colour, and a
    /// round-trip that drifts would show up as a theme whose hues wandered.
    #[test]
    fn hsl_round_trips_within_one_step() {
        for color in [
            rgb(0x84, 0xCC, 0x16),
            rgb(0xBD, 0x93, 0xF9),
            rgb(0x18, 0x18, 0x1B),
            rgb(0xFF, 0xFF, 0xFF),
            rgb(0, 0, 0),
            rgb(0x7F, 0x7F, 0x7F),
            rgb(0xEF, 0x44, 0x44),
        ] {
            let back = Hsl::from_rgb(color).to_rgb();
            for (a, b) in [(color.r, back.r), (color.g, back.g), (color.b, back.b)] {
                assert!(
                    a.abs_diff(b) <= 1,
                    "{} came back as {}",
                    color.to_hex(),
                    back.to_hex()
                );
            }
        }
    }

    /// Red and green blended in RGB is mud; blended around the wheel it is
    /// the yellow-green a swatch row wants. This is the test that says which
    /// one `blend_hues` is.
    #[test]
    fn blending_two_hues_goes_round_the_wheel_not_through_grey() {
        let blended = blend_hues(rgb(0xEF, 0x44, 0x44), rgb(0x22, 0xC5, 0x5E));
        let hsl = Hsl::from_rgb(blended);
        assert!(
            hsl.s > 0.4,
            "{} desaturated to {:.2}",
            blended.to_hex(),
            hsl.s
        );
        // Between red (0.0 turns) and green (0.36 turns) is a yellow.
        assert!((0.10..=0.26).contains(&hsl.h), "hue {:.3}", hsl.h);
    }

    #[test]
    fn blending_with_grey_keeps_the_other_hue() {
        let grey = rgb(0x80, 0x80, 0x80);
        let lime = rgb(0x84, 0xCC, 0x16);
        let blended = blend_hues(grey, lime);
        let (a, b) = (Hsl::from_rgb(blended), Hsl::from_rgb(lime));
        assert!((a.h - b.h).abs() < 0.02, "{:.3} vs {:.3}", a.h, b.h);
    }

    #[test]
    fn hue_delta_takes_the_short_way() {
        let near_red = Hsl {
            h: 0.02,
            s: 1.0,
            l: 0.5,
        };
        let also_red = Hsl {
            h: 0.98,
            s: 1.0,
            l: 0.5,
        };
        assert!((near_red.hue_delta(also_red) + 0.04).abs() < 1e-5);
        assert!((also_red.hue_delta(near_red) - 0.04).abs() < 1e-5);
    }

    /// The contrast control's own case: past 1.0 the ramp leaves the slot
    /// behind rather than stopping at it.
    #[test]
    fn extend_goes_past_the_target_and_clamps_at_the_end() {
        let bg = rgb(0x18, 0x18, 0x1B);
        let text = rgb(0xE1, 0xE1, 0xE1);
        assert_eq!(extend(bg, text, 0.0), bg);
        assert_eq!(extend(bg, text, 1.0), text);
        assert_eq!(extend(bg, text, 1.4), WHITE);
        assert_eq!(extend(bg, text, 9.0), WHITE);
        // And the other way, on a light ramp.
        let light = rgb(0xED, 0xED, 0xF0);
        let ink = rgb(0x1F, 0x1F, 0x1F);
        assert_eq!(extend(light, ink, 1.4), BLACK);
    }
}
