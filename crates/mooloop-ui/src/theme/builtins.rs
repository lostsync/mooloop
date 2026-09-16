//! The colourschemes mooloop ships.
//!
//! **This file is content, not code.** Nine of these are somebody else's
//! scheme, written down: the sixteen slots are the published palette, not a
//! derivation, and where a scheme publishes a light variant that variant is
//! here too rather than being derived. A derived light Nord is a guess at
//! Snow Storm; the real Snow Storm is four hex strings.
//!
//! Four are the seed schemes Appearance shipped before a theme could be a
//! ramp, kept byte-identical so that nobody's stored choice moved.
//! `Daylight` is the exception and is not in the list: it *was* light-mode
//! Mooloop, so it is now light-mode Mooloop, and `settings.rs` migrates the
//! name onto the pair.
//!
//! Where a slot is not in the published scheme it is named `derived:` in a
//! comment beside it. That happens twice, always for the same reason: base16
//! has an orange and a brown and most terminal-born schemes do not, so the
//! choice is a derived colour or a swatch palette two colours short.

use super::color::Rgb;
use super::ramp::Ramp;
use super::{ThemeColors, ThemeDefinition};

/// One authored variant: sixteen slots and, optionally, the colour the scheme
/// is *known by*, which is rarely slot 0D. Nord is its frost cyan, Dracula is
/// its purple, Monokai is its green.
struct Authored {
    slots: [&'static str; 16],
    accent: Option<&'static str>,
}

impl Authored {
    fn colors(&self) -> ThemeColors {
        let mut slots = [Rgb::parse_or_black("#000000"); 16];
        for (index, hex) in self.slots.iter().enumerate() {
            slots[index] = Rgb::parse(hex)
                .unwrap_or_else(|| panic!("built-in slot {index:02X} is not a colour: {hex}"));
        }
        ThemeColors::Ramp(Ramp {
            slots,
            accent: self.accent.map(|hex| {
                Rgb::parse(hex).unwrap_or_else(|| panic!("built-in accent is not a colour: {hex}"))
            }),
        })
    }
}

fn ramp_theme(
    name: &str,
    description: &str,
    dark: Authored,
    light: Option<Authored>,
) -> ThemeDefinition {
    ThemeDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        dark: Some(dark.colors()),
        light: light.map(|variant| variant.colors()),
        ..ThemeDefinition::empty(name)
    }
}

fn seed_theme(
    name: &str,
    description: &str,
    dark: (&str, &str, &str),
    light: Option<(&str, &str, &str)>,
) -> ThemeDefinition {
    let seeds = |(base, accent, alert): (&str, &str, &str)| ThemeColors::Seeds {
        base: Rgb::parse(base).expect("built-in base"),
        accent: Rgb::parse(accent).expect("built-in accent"),
        alert: Rgb::parse(alert).expect("built-in alert"),
    };
    ThemeDefinition {
        name: name.to_owned(),
        description: description.to_owned(),
        dark: Some(seeds(dark)),
        light: light.map(seeds),
        ..ThemeDefinition::empty(name)
    }
}

/// The name of the theme a fresh configuration starts on.
pub(crate) const DEFAULT_THEME: &str = "Mooloop";

pub(crate) fn all() -> Vec<ThemeDefinition> {
    vec![
        seed_theme(
            DEFAULT_THEME,
            "The house scheme: near-black, lime, and a warning yellow.",
            ("#18181B", "#84CC16", "#EAB308"),
            // What the Appearance page called Daylight until light and dark
            // became two halves of one theme.
            Some(("#EDEDF0", "#3F7D00", "#B45309")),
        ),
        ramp_theme(
            "Dracula",
            "The dark theme, and Alucard, the light one it ships with.",
            Authored {
                slots: [
                    "#282A36", "#313341", "#44475A", "#6272A4", "#A3ACC7", "#F8F8F2", "#FBFBF7",
                    "#FFFFFF", "#FF5555", "#FFB86C", "#F1FA8C", "#50FA7B", "#8BE9FD", "#BD93F9",
                    "#FF79C6", "#B48759", // derived: Dracula has no brown
                ],
                accent: Some("#BD93F9"),
            },
            Some(Authored {
                slots: [
                    "#FFFBEB", "#F5F1E1", "#CFCFDE", "#6C664B", "#4A4640", "#1F1F1F", "#151515",
                    "#000000", "#CB3A2A", "#A34D14", "#846E15", "#14710A", "#036A96", "#644AC9",
                    "#A3144D", "#7A5A2A", // derived
                ],
                accent: Some("#644AC9"),
            }),
        ),
        ramp_theme(
            "Nord",
            "Polar Night, and Snow Storm turned the other way up.",
            Authored {
                slots: [
                    "#2E3440", "#3B4252", "#434C5E", "#4C566A", "#9BA6BA", "#D8DEE9", "#E5E9F0",
                    "#ECEFF4", "#BF616A", "#D08770", "#EBCB8B", "#A3BE8C", "#88C0D0", "#81A1C1",
                    "#B48EAD", "#976A5F", // derived: Nord has no brown
                ],
                accent: Some("#88C0D0"),
            },
            Some(Authored {
                slots: [
                    "#ECEFF4", "#E5E9F0", "#D8DEE9", "#7B88A1", "#4C566A", "#2E3440", "#242933",
                    "#1A1D24", "#A54A53", "#B06A50", "#9A8340", "#6D8A56", "#4C8EA3", "#5E81AC",
                    "#8A6485", "#7D5A4E", // derived
                ],
                accent: Some("#5E81AC"),
            }),
        ),
        ramp_theme(
            "Gruvbox",
            "Retro groove, medium contrast, in both of its published weights.",
            Authored {
                slots: [
                    "#282828", "#3C3836", "#504945", "#665C54", "#BDAE93", "#D5C4A1", "#EBDBB2",
                    "#FBF1C7", "#FB4934", "#FE8019", "#FABD2F", "#B8BB26", "#8EC07C", "#83A598",
                    "#D3869B", "#D65D0E",
                ],
                accent: Some("#83A598"),
            },
            Some(Authored {
                slots: [
                    "#FBF1C7", "#EBDBB2", "#D5C4A1", "#BDAE93", "#665C54", "#504945", "#3C3836",
                    "#282828", "#9D0006", "#AF3A03", "#B57614", "#79740E", "#427B58", "#076678",
                    "#8F3F71", "#D65D0E",
                ],
                accent: Some("#076678"),
            }),
        ),
        ramp_theme(
            "Everforest",
            "Green-based and low contrast, in medium dark and medium light.",
            Authored {
                slots: [
                    "#2D353B", "#343F44", "#3D484D", "#7A8478", "#9DA9A0", "#D3C6AA", "#E0D8C0",
                    "#F0EAD8", "#E67E80", "#E69875", "#DBBC7F", "#A7C080", "#83C092", "#7FBBB3",
                    "#D699B6", "#B97C68", // derived
                ],
                accent: Some("#A7C080"),
            },
            Some(Authored {
                slots: [
                    "#FDF6E3", "#F4F0D9", "#EFEBD4", "#A6B0A0", "#829181", "#5C6A72", "#4A575D",
                    "#384046", "#F85552", "#F57D26", "#DFA000", "#8DA101", "#35A77C", "#3A94C5",
                    "#DF69BA", "#B26A3A", // derived
                ],
                // Everforest light's published green is #8DA101, which is a
                // 2.5:1 accent against its own surface -- the scheme says
                // "low contrast" and means it. Darkened to clear 3:1, which
                // is the bar `builtins.rs` holds every shipped variant to.
                accent: Some("#798A01"),
            }),
        ),
        ramp_theme(
            "Solarized",
            "Ethan Schoonover's, with the two backgrounds it was designed as.",
            Authored {
                slots: [
                    "#002B36", "#073642", "#586E75", "#657B83", "#839496", "#93A1A1", "#EEE8D5",
                    "#FDF6E3", "#DC322F", "#CB4B16", "#B58900", "#859900", "#2AA198", "#268BD2",
                    "#6C71C4", "#D33682",
                ],
                accent: Some("#268BD2"),
            },
            Some(Authored {
                slots: [
                    // Slot 05 is Solarized's own base01 (#586E75) darkened a
                    // step: as body text on base2 it reads 4.4:1, and 4.5 is
                    // the bar. Solarized's modest contrast is deliberate and
                    // this is the smallest move that clears AA.
                    "#FDF6E3", "#EEE8D5", "#93A1A1", "#839496", "#657B83", "#53676E", "#073642",
                    "#002B36", "#DC322F", "#CB4B16", "#B58900", "#859900", "#2AA198", "#1F7FC4",
                    "#6C71C4", "#D33682",
                ],
                // Solarized's blue against base2 is 3.0036:1 -- over the bar
                // by four thousandths, which is not a margin, it is a
                // coincidence. Darkened a step so the invariant is one the
                // arithmetic can hold rather than one it happens to.
                accent: Some("#1F7FC4"),
            }),
        ),
        ramp_theme(
            "Catppuccin",
            "Mocha and Latte, from the pastel set.",
            Authored {
                slots: [
                    "#1E1E2E", "#313244", "#45475A", "#6C7086", "#A6ADC8", "#CDD6F4", "#DCE0F0",
                    "#EFF1F5", "#F38BA8", "#FAB387", "#F9E2AF", "#A6E3A1", "#94E2D5", "#89B4FA",
                    "#CBA6F7", "#C6A08A", // derived: the flamingo is a pink, not a brown
                ],
                accent: Some("#CBA6F7"),
            },
            Some(Authored {
                slots: [
                    "#EFF1F5", "#E6E9EF", "#CCD0DA", "#9CA0B0", "#6C6F85", "#4C4F69", "#3B3E52",
                    "#292B3A", "#D20F39", "#FE640B", "#DF8E1D", "#40A02B", "#179299", "#1E66F5",
                    "#8839EF", "#A5713A", // derived
                ],
                accent: Some("#8839EF"),
            }),
        ),
        ramp_theme(
            "Tokyo Night",
            "Night, and the Day variant that answers it.",
            Authored {
                slots: [
                    "#1A1B26", "#24283B", "#292E42", "#565F89", "#A9B1D6", "#C0CAF5", "#CFD6F7",
                    "#D5D6DB", "#F7768E", "#FF9E64", "#E0AF68", "#9ECE6A", "#7DCFFF", "#7AA2F7",
                    "#BB9AF7", "#B98B5E", // derived
                ],
                accent: Some("#7AA2F7"),
            },
            Some(Authored {
                slots: [
                    // Day's foreground (#3760BF) and its blue (#2E7DE9) are
                    // 4.1:1 and 2.8:1 against this surface. Both darkened a
                    // step to clear AA and the 3:1 accent bar.
                    "#E1E2E7", "#D6D8DE", "#C4C8DA", "#848CB5", "#6172B0", "#3156AB", "#2E5399",
                    "#1F3D73", "#F52A65", "#B15C00", "#8C6C3E", "#587539", "#007197", "#186EE3",
                    "#9854F1", "#8F6B3E", // derived
                ],
                accent: Some("#186EE3"),
            }),
        ),
        ramp_theme(
            "Rosé Pine",
            "The main variant, and Dawn.",
            Authored {
                slots: [
                    "#191724", "#1F1D2E", "#26233A", "#6E6A86", "#908CAA", "#E0DEF4", "#E8E6F8",
                    "#F0EEFC", "#EB6F92", "#EBBCBA", "#F6C177", "#31748F", "#9CCFD8", "#569FBA",
                    "#C4A7E7", "#B98A72", // derived
                ],
                accent: Some("#C4A7E7"),
            },
            Some(Authored {
                slots: [
                    "#FAF4ED", "#FFFAF3", "#F2E9E1", "#9893A5", "#797593", "#575279", "#46415F",
                    "#332F49", "#B4637A", "#D7827E", "#EA9D34", "#286983", "#56949F", "#3E7C8C",
                    "#907AA9", "#8C6A52", // derived
                ],
                accent: Some("#907AA9"),
            }),
        ),
        ramp_theme(
            "Monokai",
            "The original, with a light variant derived from it.",
            Authored {
                slots: [
                    "#272822", "#383830", "#49483E", "#75715E", "#A59F85", "#F8F8F2", "#F5F4F1",
                    "#F9F8F5", "#F92672", "#FD971F", "#F4BF75", "#A6E22E", "#A1EFE4", "#66D9EF",
                    "#AE81FF", "#CC6633",
                ],
                accent: Some("#A6E22E"),
            },
            None,
        ),
        seed_theme(
            "Graphite",
            "Neutral and cool, with an amber accent.",
            ("#151617", "#F59E0B", "#38BDF8"),
            None,
        ),
        seed_theme(
            "High Contrast",
            "Black, cyan and a hard yellow. The accessible one.",
            ("#000000", "#22D3EE", "#FACC15"),
            Some(("#FFFFFF", "#005F6B", "#7A4A00")),
        ),
        seed_theme(
            "Ember",
            "Warm near-black under an orange.",
            ("#1A1413", "#F97316", "#38BDF8"),
            None,
        ),
        seed_theme(
            "Indigo",
            "Blue-black under a violet.",
            ("#14141F", "#A78BFA", "#F472B6"),
            None,
        ),
    ]
}

pub(crate) fn is_builtin(name: &str) -> bool {
    all().iter().any(|theme| theme.name == name)
}

/// A built-in by name, ignoring anything the user has written.
///
/// Test-only on purpose: what the program looks a theme up in is
/// [`catalog::find`](super::catalog::find), which lets a user's file of the
/// same name replace the built-in. Reaching past that outside a test would be
/// a way to ignore somebody's own Nord.
#[cfg(test)]
pub(crate) fn find(name: &str) -> Option<ThemeDefinition> {
    all().into_iter().find(|theme| theme.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::color::contrast_ratio;

    /// `Authored::colors` unwraps every slot, so a typo here panics at startup
    /// rather than failing a test. This is the test that stops that, and it is
    /// the same guard the channel-colour table had for the same reason.
    #[test]
    fn every_slot_of_every_built_in_is_a_colour() {
        for theme in all() {
            for dark in [true, false] {
                let ramp = theme.variant(dark).ramp();
                assert_eq!(ramp.slots.len(), 16, "{}", theme.name);
            }
        }
    }

    /// The one accessibility claim a shipped scheme has to make: body text on
    /// the surface it sits on clears WCAG AA for small text.
    ///
    /// **Both variants, including the derived ones.** A theme that only
    /// authored a dark ramp still answers Light, and the answer has to be
    /// readable or the mode switch is a trap.
    #[test]
    fn every_variant_of_every_built_in_is_readable() {
        for theme in all() {
            for dark in [true, false] {
                let ramp = theme.variant(dark).ramp();
                let ratio = ramp.text_contrast(1.0);
                assert!(
                    ratio >= 4.5,
                    "{} {} text ratio is {ratio:.2}",
                    theme.name,
                    if dark { "dark" } else { "light" }
                );
            }
        }
    }

    /// An accent nobody can see against the surface is not an accent. This is
    /// the bar `AppearanceSettings::validated` holds a user's own colours to,
    /// so a built-in that failed it would be one the user could not have
    /// written.
    #[test]
    fn every_variant_of_every_built_in_has_a_visible_accent() {
        for theme in all() {
            for dark in [true, false] {
                let ramp = theme.variant(dark).ramp();
                let ratio = ramp.accent_contrast(1.0);
                assert!(
                    ratio >= 3.0,
                    "{} {} accent ratio is {ratio:.2}",
                    theme.name,
                    if dark { "dark" } else { "light" }
                );
            }
        }
    }

    /// A dark variant that is not dark, or a light one that is not light, is
    /// a transposition -- the kind of mistake sixteen hex strings in a row
    /// invite and nothing else here would notice.
    #[test]
    fn dark_variants_are_dark_and_light_variants_are_light() {
        for theme in all() {
            assert!(theme.variant(true).ramp().is_dark(), "{} dark", theme.name);
            assert!(
                !theme.variant(false).ramp().is_dark(),
                "{} light",
                theme.name
            );
        }
    }

    /// The meters have three bands and they have to be three colours. Safe is
    /// the accent, the headroom warning is slot 0A and a clip is slot 08, so a
    /// scheme whose accent *is* its yellow draws a two-colour meter.
    #[test]
    fn every_built_in_draws_a_three_colour_meter() {
        for theme in all() {
            for dark in [true, false] {
                let palette = theme.variant(dark).ramp().palette(1.0);
                for (a, b, pair) in [
                    (palette.meter_safe, palette.meter_warning, "safe/warning"),
                    (palette.meter_warning, palette.meter_clip, "warning/clip"),
                    (palette.meter_safe, palette.meter_clip, "safe/clip"),
                ] {
                    assert!(
                        contrast_ratio(a, b) > 1.15 || a != b,
                        "{} {pair} are the same colour",
                        theme.name
                    );
                    assert_ne!(a.to_hex(), b.to_hex(), "{} {pair}", theme.name);
                }
            }
        }
    }

    #[test]
    fn the_default_theme_exists_and_is_a_built_in() {
        assert!(is_builtin(DEFAULT_THEME));
        assert!(find(DEFAULT_THEME).is_some());
        assert!(find("no such theme").is_none());
    }
}
