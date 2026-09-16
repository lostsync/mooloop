//! What a theme is.
//!
//! A theme is a name, up to two colour variants -- one light, one dark -- and
//! an optional set of style overrides that both variants share. Which variant
//! the interface is wearing is a separate, independently persisted choice
//! ([`Mode`]), so Light/Dark/Auto is one control over every theme rather than
//! two entries per scheme in a list.
//!
//! **A theme that authored only one variant still answers both.** The other is
//! derived -- see [`ThemeColors::inverted`] -- and the derivation is honest
//! rather than good: an authored Snow Storm beats a guess at one every time,
//! which is why `builtins.rs` writes both out wherever the scheme publishes
//! both. What the derivation buys is that the mode control is never a trap.

pub(crate) mod builtins;
pub(crate) mod catalog;
pub(crate) mod color;
pub(crate) mod file;
pub(crate) mod ramp;
pub(crate) mod system;
pub(crate) mod wal;

use color::{contrast_ratio, relative_luminance, Hsl, Rgb};
use ramp::Ramp;

/// The minimum contrast between the accent and the surface it sits on. WCAG's
/// bar for a non-text interface element, and the one
/// `AppearanceSettings::validated` has always held a user's accent to.
pub(crate) const MIN_ACCENT_CONTRAST: f32 = 3.0;

/// Which variant the interface wears.
///
/// `System` is the Linux-first half of this: on a desktop that publishes
/// `org.freedesktop.appearance color-scheme`, mooloop follows it, which is
/// what makes a theme switch at dusk reach the DAW along with everything else.
/// See [`system::prefers_dark`] for what is probed and in what order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Mode {
    #[default]
    Dark,
    Light,
    System,
}

pub(crate) const MODES: [&str; 3] = ["dark", "light", "system"];

impl Mode {
    pub(crate) fn from_name(name: &str) -> Self {
        match name {
            "light" => Self::Light,
            "system" => Self::System,
            _ => Self::Dark,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
            Self::System => "system",
        }
    }

    pub(crate) fn index(self) -> i32 {
        match self {
            Self::Dark => 0,
            Self::Light => 1,
            Self::System => 2,
        }
    }

    pub(crate) fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Light,
            2 => Self::System,
            _ => Self::Dark,
        }
    }

    /// Whether this mode wants the dark variant *right now*. `System` asks
    /// the desktop, so this is the one method here that is not a pure
    /// function of its input.
    pub(crate) fn wants_dark(self) -> bool {
        match self {
            Self::Dark => true,
            Self::Light => false,
            Self::System => system::prefers_dark(),
        }
    }
}

/// One variant's colours, in either of the two forms a theme can state them.
///
/// The seed form is not a legacy path: it is what the Appearance page's three
/// colour pickers write, and it is how a user makes a scheme of their own
/// without typing sixteen hex strings. It synthesizes a ramp, so everything
/// downstream of here reads a ramp and there is one derivation rather than
/// two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ThemeColors {
    Seeds { base: Rgb, accent: Rgb, alert: Rgb },
    Ramp(Ramp),
}

impl ThemeColors {
    pub(crate) fn ramp(&self) -> Ramp {
        match *self {
            Self::Seeds {
                base,
                accent,
                alert,
            } => Ramp::from_seeds(base, accent, alert),
            Self::Ramp(ramp) => ramp,
        }
    }

    /// The same scheme on the other side of the light/dark line.
    ///
    /// A seed variant inverts as three seeds rather than as sixteen slots,
    /// because that is what it is: flipping `base`'s lightness and then making
    /// the accent and the alert legible against the result gives back a scheme
    /// somebody could have typed, which a reversed ramp does not.
    pub(crate) fn inverted(&self) -> Self {
        match *self {
            Self::Seeds {
                base,
                accent,
                alert,
            } => {
                let flipped = flip_lightness(base);
                // Against the *surface*, not the background. On a light ramp
                // the surface is the darker of the two, so an accent made
                // legible against the background alone comes up short exactly
                // where it is drawn -- which is what put the Indigo scheme's
                // violet at 2.7:1 when it flipped.
                let surface = Ramp::from_seeds(flipped, accent, alert)
                    .palette(1.0)
                    .surface;
                Self::Seeds {
                    base: flipped,
                    accent: legible_against(accent, surface, MIN_ACCENT_CONTRAST),
                    alert: legible_against(alert, surface, MIN_ACCENT_CONTRAST),
                }
            }
            Self::Ramp(ramp) => Self::Ramp(ramp.inverted()),
        }
    }
}

/// The same colour on the other side of the ladder: hue and saturation held,
/// lightness reflected about the middle.
fn flip_lightness(color: Rgb) -> Rgb {
    let mut hsl = Hsl::from_rgb(color);
    hsl.l = 1.0 - hsl.l;
    hsl.to_rgb()
}

/// Walks a colour's lightness toward the pole away from `background` until it
/// clears `ratio`, holding its hue and saturation.
///
/// This is what stops a derived variant from being a trap: mooloop's own lime
/// accent has a 2.1 ratio against a near-white background, so a Mooloop that
/// flipped to light without this would ship an accent the program's own
/// validator rejects.
pub(crate) fn legible_against(color: Rgb, background: Rgb, ratio: f32) -> Rgb {
    if contrast_ratio(color, background) >= ratio {
        return color;
    }
    let toward_dark = relative_luminance(background) > 0.18;
    let mut hsl = Hsl::from_rgb(color);
    for _ in 0..24 {
        hsl.l = if toward_dark {
            (hsl.l - 0.04).max(0.0)
        } else {
            (hsl.l + 0.04).min(1.0)
        };
        let candidate = hsl.to_rgb();
        if contrast_ratio(candidate, background) >= ratio {
            return candidate;
        }
    }
    hsl.to_rgb()
}

/// Everything a theme can say that is not a colour.
///
/// All optional, all falling back to whatever the interface is already set to,
/// for the reason `settings.rs` uses `#[serde(default)]` everywhere: a theme
/// that only wants a different font should be three lines, and a theme written
/// against this schema must still load against the next one.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ThemeStyle {
    pub roundness: Option<f32>,
    pub hairline: Option<f32>,
    pub stroke_emphasis: Option<f32>,
    pub font_family: Option<String>,
    pub font_family_mono: Option<String>,
    pub type_scale: Option<f32>,
    pub font_weight: Option<i32>,
    pub density: Option<f32>,
    pub contrast: Option<f32>,
}

impl ThemeStyle {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ThemeDefinition {
    pub name: String,
    pub description: String,
    pub dark: Option<ThemeColors>,
    pub light: Option<ThemeColors>,
    pub style: ThemeStyle,
}

impl ThemeDefinition {
    pub(crate) fn empty(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            description: String::new(),
            dark: None,
            light: None,
            style: ThemeStyle::default(),
        }
    }

    /// The variant for one side of the light/dark line, authored if the theme
    /// wrote one and derived if it did not.
    ///
    /// A theme with neither -- which a hand-written file can be -- falls back
    /// to the default theme's, so that a file missing its `[dark]` table still
    /// produces an interface rather than a panic.
    pub(crate) fn variant(&self, dark: bool) -> ThemeColors {
        let (wanted, other) = if dark {
            (self.dark, self.light)
        } else {
            (self.light, self.dark)
        };
        wanted
            .or_else(|| other.map(|colors| colors.inverted()))
            .unwrap_or(ThemeColors::Seeds {
                base: Rgb::parse_or_black("#18181B"),
                accent: Rgb::parse_or_black("#84CC16"),
                alert: Rgb::parse_or_black("#EAB308"),
            })
    }

    /// Whether the variant on this side was written down rather than worked
    /// out. The Appearance page says so, because a derived Snow Storm is not
    /// Snow Storm and a user comparing it against a screenshot deserves to
    /// know which one they are looking at.
    pub(crate) fn authored(&self, dark: bool) -> bool {
        if dark {
            self.dark.is_some()
        } else {
            self.light.is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_name_round_trips_and_an_unknown_one_is_dark() {
        for name in MODES {
            assert_eq!(Mode::from_name(name).name(), name);
        }
        assert_eq!(Mode::from_name("auto"), Mode::Dark);
        assert_eq!(Mode::from_name(""), Mode::Dark);
        for mode in [Mode::Dark, Mode::Light, Mode::System] {
            assert_eq!(Mode::from_index(mode.index()), mode);
        }
        // Out of range is the default, not a panic: the index comes from a
        // Slint selector, and `FOCUS.md` records that this codebase has two
        // conventions for that and one of them is wrong.
        assert_eq!(Mode::from_index(9), Mode::Dark);
        assert_eq!(Mode::from_index(-1), Mode::Dark);
    }

    #[test]
    fn inverting_seeds_gives_seeds_and_keeps_them_legible() {
        let dark = ThemeColors::Seeds {
            base: Rgb::parse("#18181B").unwrap(),
            accent: Rgb::parse("#84CC16").unwrap(),
            alert: Rgb::parse("#EAB308").unwrap(),
        };
        let light = dark.inverted();
        assert!(matches!(light, ThemeColors::Seeds { .. }));
        let ramp = light.ramp();
        assert!(!ramp.is_dark());
        assert!(
            ramp.accent_contrast(1.0) >= MIN_ACCENT_CONTRAST,
            "accent ratio {:.2}",
            ramp.accent_contrast(1.0)
        );
        assert!(ramp.text_contrast(1.0) >= 4.5);
    }

    /// The lime accent against a near-white background is the case this was
    /// written for: 2.1 before, at or over 3.0 after, and still a lime.
    #[test]
    fn legible_against_darkens_without_changing_the_hue() {
        let white = Rgb::parse("#EDEDF0").unwrap();
        let lime = Rgb::parse("#84CC16").unwrap();
        assert!(contrast_ratio(lime, white) < MIN_ACCENT_CONTRAST);
        let fixed = legible_against(lime, white, MIN_ACCENT_CONTRAST);
        assert!(contrast_ratio(fixed, white) >= MIN_ACCENT_CONTRAST);
        let (a, b) = (Hsl::from_rgb(lime), Hsl::from_rgb(fixed));
        assert!(
            a.hue_delta(b).abs() < 0.02,
            "hue moved {:.3}",
            a.hue_delta(b)
        );
    }

    #[test]
    fn a_colour_that_already_reads_is_left_exactly_alone() {
        let surface = Rgb::parse("#232328").unwrap();
        let lime = Rgb::parse("#84CC16").unwrap();
        assert_eq!(legible_against(lime, surface, MIN_ACCENT_CONTRAST), lime);
    }

    #[test]
    fn a_theme_with_no_variants_still_produces_an_interface() {
        let empty = ThemeDefinition::empty("Nothing");
        assert!(empty.variant(true).ramp().text_contrast(1.0) > 4.5);
        assert!(!empty.authored(true));
        assert!(!empty.authored(false));
    }
}
