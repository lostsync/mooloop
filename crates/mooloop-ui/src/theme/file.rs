//! A theme on disk.
//!
//! One TOML file per theme in `<config>/mooloop/themes/`, so a theme is a
//! thing you can send somebody. Every section and every field is optional and
//! falls back to what the interface is already set to -- a theme that only
//! wants a different font is four lines -- for the reason `settings.rs` uses
//! `#[serde(default)]` throughout: a file written against schema 1 has to load
//! against schema 2.
//!
//! ```toml
//! schema-version = 1
//! name = "Dracula"
//! description = "An homage, not a reproduction."
//!
//! [color]
//! contrast = 1.0
//!
//! [color.dark]
//! accent = "#BD93F9"   # optional: the colour the scheme is known by
//! slots = [            # sixteen, in base16 order: base00 through base0F
//!   "#282A36", "#313341", "#44475A", "#6272A4", "#A3ACC7", "#F8F8F2",
//!   "#FBFBF7", "#FFFFFF", "#FF5555", "#FFB86C", "#F1FA8C", "#50FA7B",
//!   "#8BE9FD", "#BD93F9", "#FF79C6", "#B48759",
//! ]
//!
//! [color.light]
//! base = "#FFFBEB"     # the three-seed form, for a variant you did not
//! accent = "#644AC9"   # want to write sixteen colours for
//! alert = "#846E15"
//!
//! [shape]
//! roundness = 1.0
//! hairline = 1
//! stroke-emphasis = 2
//! relief = "flat"      # or "bevel" (lit blocks) or "inset"; see 02-relief.md
//! relief-depth = 1.0
//!
//! [type]
//! family = "Inter"
//! family-mono = "JetBrains Mono"
//! scale = 1.0
//! weight = 400
//!
//! [metrics]
//! density = 1.0
//! ```
//!
//! **Fonts are named, not shipped.** Slint 1.17.1 has no runtime font
//! registration -- `register_font_from_path` is not in its public API -- so a
//! family is either compiled in or resolved from the system, and a theme can
//! only ask. `family` is therefore a *list*, CSS-style: mooloop hands the
//! whole string to Slint, which walks it. A theme naming a font nobody has
//! still loads and still looks deliberate, which is the property that matters.
//!
//! **A malformed theme is skipped with a message and does not stop startup**,
//! which is the precedent `UiSettings::load_or_default` already set for
//! `settings.toml`. A themes directory somebody has been editing by hand is
//! the ordinary case, not the exceptional one.

use super::color::Rgb;
use super::ramp::Ramp;
use super::{Relief, ThemeColors, ThemeDefinition, ThemeStyle};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 1;

pub(crate) fn themes_dir() -> PathBuf {
    crate::settings::config_dir().join("themes")
}

/// The file a theme of this name is written to. Anything that is not a letter,
/// a digit, a dash or an underscore becomes a dash, so a theme called
/// `Rosé Pine / Dawn` is `rose-pine-dawn.toml` rather than two directories and
/// a surprise.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_dash = true;
    for ch in name.chars() {
        // `is_alphanumeric` rather than the ASCII form: a theme named in
        // Cyrillic should get a filename, not eleven dashes.
        if ch.is_alphanumeric() || ch == '_' {
            out.extend(ch.to_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            out.push('-');
            last_was_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "theme".to_owned()
    } else {
        trimmed
    }
}

#[derive(Debug)]
pub(crate) enum ThemeFileError {
    Io(std::io::Error),
    Parse(String),
    Serialize(String),
    Empty,
}

impl std::fmt::Display for ThemeFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Parse(message) => write!(f, "{message}"),
            Self::Serialize(message) => write!(f, "{message}"),
            Self::Empty => write!(f, "the file names no colours"),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ColorSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    contrast: Option<f32>,
    // The flat three-seed form `03-the-theme-file.md` specified before a theme
    // could be a ramp. Read as the dark variant, never written.
    //
    // **Every scalar in this table is declared before the two sub-tables, and
    // has to be.** TOML cannot write a bare key after a `[table]` header and
    // serde emits fields in declaration order, so a scalar declared below
    // `dark` would produce a file this format could not read back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alert: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dark: Option<VariantSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    light: Option<VariantSection>,
}

/// One variant, in whichever of the two forms it was written.
///
/// `slots` decides which: sixteen of them is a ramp, `base` alone is three
/// seeds. `accent` means the override in the first case and the accent seed in
/// the second, which reads the same way in both -- it is the colour the scheme
/// is known by.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
struct VariantSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alert: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    slots: Vec<String>,
}

impl VariantSection {
    fn from_colors(colors: ThemeColors) -> Self {
        match colors {
            ThemeColors::Seeds {
                base,
                accent,
                alert,
            } => Self {
                base: Some(base.to_hex()),
                accent: Some(accent.to_hex()),
                alert: Some(alert.to_hex()),
                slots: Vec::new(),
            },
            ThemeColors::Ramp(ramp) => Self {
                base: None,
                accent: ramp.accent.map(Rgb::to_hex),
                alert: None,
                slots: ramp.slots.iter().map(|slot| slot.to_hex()).collect(),
            },
        }
    }

    /// `None` when the section names nothing usable, which is different from
    /// an error: a `[color.light]` table that is present and empty is a theme
    /// with no light variant, and the derivation handles that.
    fn colors(&self) -> Option<ThemeColors> {
        if self.slots.len() == 16 {
            let mut slots = [Rgb::parse_or_black("#000000"); 16];
            for (index, hex) in self.slots.iter().enumerate() {
                slots[index] = Rgb::parse(hex)?;
            }
            return Some(ThemeColors::Ramp(Ramp {
                slots,
                accent: self.accent.as_deref().and_then(Rgb::parse),
            }));
        }
        let base = Rgb::parse(self.base.as_deref()?)?;
        Some(ThemeColors::Seeds {
            base,
            // A seed variant that names only a base still has to produce an
            // interface, and the house accent on somebody else's background is
            // a better answer than a refusal to load.
            accent: self
                .accent
                .as_deref()
                .and_then(Rgb::parse)
                .unwrap_or_else(|| Rgb::parse_or_black("#84CC16")),
            alert: self
                .alert
                .as_deref()
                .and_then(Rgb::parse)
                .unwrap_or_else(|| Rgb::parse_or_black("#EAB308")),
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ShapeSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    roundness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hairline: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stroke_emphasis: Option<f32>,
    /// `flat`, `bevel` or `inset`; see [`Relief`].
    #[serde(skip_serializing_if = "Option::is_none")]
    relief: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    relief_depth: Option<f32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
struct TypeSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    family_mono: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weight: Option<i32>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
struct MetricsSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    density: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
struct ThemeFile {
    schema_version: u32,
    #[serde(default)]
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "is_default")]
    color: ColorSection,
    #[serde(default, skip_serializing_if = "is_default")]
    shape: ShapeSection,
    #[serde(default, rename = "type", skip_serializing_if = "is_default")]
    typography: TypeSection,
    #[serde(default, skip_serializing_if = "is_default")]
    metrics: MetricsSection,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

impl ThemeFile {
    fn definition(&self, fallback_name: &str) -> Result<ThemeDefinition, ThemeFileError> {
        let name = if self.name.trim().is_empty() {
            fallback_name.to_owned()
        } else {
            self.name.trim().to_owned()
        };
        let flat = ColorSection {
            base: self.color.base.clone(),
            accent: self.color.accent.clone(),
            alert: self.color.alert.clone(),
            ..ColorSection::default()
        };
        let flat_variant = VariantSection {
            base: flat.base,
            accent: flat.accent,
            alert: flat.alert,
            slots: Vec::new(),
        };
        let dark = self
            .color
            .dark
            .as_ref()
            .and_then(VariantSection::colors)
            .or_else(|| flat_variant.colors());
        let light = self.color.light.as_ref().and_then(VariantSection::colors);
        let style = ThemeStyle {
            roundness: self.shape.roundness,
            hairline: self.shape.hairline,
            stroke_emphasis: self.shape.stroke_emphasis,
            relief: self.shape.relief.as_deref().and_then(Relief::parse),
            relief_depth: self.shape.relief_depth,
            font_family: self.typography.family.clone(),
            font_family_mono: self.typography.family_mono.clone(),
            type_scale: self.typography.scale,
            font_weight: self.typography.weight,
            density: self.metrics.density,
            contrast: self.color.contrast,
        };
        if dark.is_none() && light.is_none() && style.is_empty() {
            return Err(ThemeFileError::Empty);
        }
        Ok(ThemeDefinition {
            name,
            description: self.description.clone(),
            dark,
            light,
            style,
        })
    }

    fn from_definition(theme: &ThemeDefinition) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: theme.name.clone(),
            description: theme.description.clone(),
            color: ColorSection {
                contrast: theme.style.contrast,
                base: None,
                accent: None,
                alert: None,
                dark: theme.dark.map(VariantSection::from_colors),
                light: theme.light.map(VariantSection::from_colors),
            },
            shape: ShapeSection {
                roundness: theme.style.roundness,
                hairline: theme.style.hairline,
                stroke_emphasis: theme.style.stroke_emphasis,
                relief: theme.style.relief.map(|relief| relief.name().to_owned()),
                relief_depth: theme.style.relief_depth,
            },
            typography: TypeSection {
                family: theme.style.font_family.clone(),
                family_mono: theme.style.font_family_mono.clone(),
                scale: theme.style.type_scale,
                weight: theme.style.font_weight,
            },
            metrics: MetricsSection {
                density: theme.style.density,
            },
        }
    }
}

pub(crate) fn read(path: &Path) -> Result<ThemeDefinition, ThemeFileError> {
    let text = std::fs::read_to_string(path).map_err(ThemeFileError::Io)?;
    let file: ThemeFile =
        toml::from_str(&text).map_err(|error| ThemeFileError::Parse(error.to_string()))?;
    let fallback = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Theme".to_owned());
    file.definition(&fallback)
}

pub(crate) fn write(theme: &ThemeDefinition) -> Result<PathBuf, ThemeFileError> {
    write_to(&themes_dir(), theme)
}

/// The directory is a parameter so that a test can point the whole format at
/// a temporary one. `themes_dir()` reads an environment variable, and a test
/// that sets one is a test that races every other test in the binary.
pub(crate) fn write_to(
    directory: &Path,
    theme: &ThemeDefinition,
) -> Result<PathBuf, ThemeFileError> {
    std::fs::create_dir_all(directory).map_err(ThemeFileError::Io)?;
    let path = directory.join(format!("{}.toml", slug(&theme.name)));
    let text = toml::to_string_pretty(&ThemeFile::from_definition(theme))
        .map_err(|error| ThemeFileError::Serialize(error.to_string()))?;
    std::fs::write(&path, text).map_err(ThemeFileError::Io)?;
    Ok(path)
}

pub(crate) fn remove_in(directory: &Path, name: &str) -> Result<(), ThemeFileError> {
    let path = directory.join(format!("{}.toml", slug(name)));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        // A theme the user removed by hand and then removed again in the UI
        // is not an error to report at them.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ThemeFileError::Io(error)),
    }
}

/// Every readable theme in the themes directory, in name order, with one
/// warning line per file that would not load.
///
/// The directory not existing is the ordinary first-run state and is not
/// reported at anybody.
pub(crate) fn load_all() -> (Vec<ThemeDefinition>, Vec<String>) {
    load_all_in(&themes_dir())
}

pub(crate) fn load_all_in(directory: &Path) -> (Vec<ThemeDefinition>, Vec<String>) {
    let mut themes = Vec::new();
    let mut warnings = Vec::new();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return (themes, warnings);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        match read(&path) {
            Ok(theme) => themes.push(theme),
            Err(error) => warnings.push(format!("{}: {error}", path.display())),
        }
    }
    themes.sort_by_key(|theme| theme.name.to_lowercase());
    (themes, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::builtins;

    fn parse(text: &str) -> Result<ThemeDefinition, ThemeFileError> {
        let file: ThemeFile =
            toml::from_str(text).map_err(|error| ThemeFileError::Parse(error.to_string()))?;
        file.definition("Fallback")
    }

    /// The acceptance clause from `03-the-theme-file.md`: a theme file with
    /// only the plan's flat `[color]` table behaves exactly as a scheme did.
    #[test]
    fn a_flat_colour_table_is_the_dark_variant() {
        let theme = parse(
            r##"
            schema-version = 1
            name = "Ember"
            [color]
            base = "#1A1413"
            accent = "#F97316"
            alert = "#38BDF8"
            "##,
        )
        .expect("loads");
        assert_eq!(theme.name, "Ember");
        assert!(theme.authored(true));
        assert!(!theme.authored(false));
        let expected = builtins::find("Ember").unwrap();
        assert_eq!(theme.dark, expected.dark);
    }

    #[test]
    fn a_sixteen_slot_variant_is_a_ramp() {
        let slots: Vec<String> = (0..16)
            .map(|index| format!("\"#{index:02X}{index:02X}{index:02X}\""))
            .collect();
        let theme = parse(&format!(
            r##"
            schema-version = 1
            name = "Ladder"
            [color.dark]
            slots = [{}]
            accent = "#84CC16"
            "##,
            slots.join(", ")
        ))
        .expect("loads");
        let ThemeColors::Ramp(ramp) = theme.dark.unwrap() else {
            panic!("sixteen slots should be a ramp");
        };
        assert_eq!(ramp.slots[15].to_hex(), "#0F0F0F");
        assert_eq!(ramp.accent.unwrap().to_hex(), "#84CC16");
    }

    /// Round trip: every built-in survives being written out and read back,
    /// which is what makes "duplicate this theme and edit it" work.
    #[test]
    fn every_built_in_round_trips_through_the_file_format() {
        for theme in builtins::all() {
            let text =
                toml::to_string_pretty(&ThemeFile::from_definition(&theme)).expect("serializes");
            let back = parse(&text).expect("re-reads");
            assert_eq!(back.name, theme.name, "{}", theme.name);
            assert_eq!(back.dark, theme.dark, "{} dark", theme.name);
            assert_eq!(back.light, theme.light, "{} light", theme.name);
            assert_eq!(back.style, theme.style, "{} style", theme.name);
        }
    }

    /// A theme that changes nothing but the font is four lines, which is the
    /// property `#[serde(default)]` everywhere is for.
    #[test]
    fn a_theme_that_only_names_a_font_is_a_theme() {
        let theme = parse(
            r##"
            schema-version = 1
            name = "Bigger"
            [type]
            family = "Inter, sans-serif"
            scale = 1.4
            "##,
        )
        .expect("loads");
        assert_eq!(theme.style.type_scale, Some(1.4));
        assert_eq!(
            theme.style.font_family.as_deref(),
            Some("Inter, sans-serif")
        );
        assert!(theme.dark.is_none());
        // And it still produces an interface rather than a panic.
        assert!(theme.variant(true).ramp().text_contrast(1.0) > 4.5);
    }

    #[test]
    fn a_file_that_says_nothing_is_rejected_rather_than_listed() {
        assert!(matches!(
            parse("schema-version = 1\nname = \"Nothing\""),
            Err(ThemeFileError::Empty)
        ));
    }

    /// A file written against a later schema still loads: unknown fields are
    /// ignored and the version is recorded rather than enforced. The
    /// alternative is a theme that stops working when mooloop updates, which
    /// is the failure this format is shaped to avoid.
    #[test]
    fn a_newer_schema_and_unknown_fields_still_load() {
        let theme = parse(
            r##"
            schema-version = 9
            name = "From the future"
            relief = "bevel"
            [color]
            base = "#101010"
            [shape]
            roundness = 0.0
            corner-sparkle = 3
            "##,
        )
        .expect("loads");
        assert_eq!(theme.style.roundness, Some(0.0));
        assert!(theme.dark.is_some());
    }

    #[test]
    fn a_malformed_file_is_a_message_rather_than_a_panic() {
        assert!(matches!(
            parse("this is not toml"),
            Err(ThemeFileError::Parse(_))
        ));
        // A colour that is not a colour drops its variant rather than the file.
        let theme = parse(
            r##"
            schema-version = 1
            name = "Half"
            [color]
            base = "not a colour"
            [type]
            scale = 1.2
            "##,
        )
        .expect("loads");
        assert!(theme.dark.is_none());
        assert_eq!(theme.style.type_scale, Some(1.2));
    }

    #[test]
    fn a_name_becomes_a_filename() {
        assert_eq!(slug("Rosé Pine"), "rosé-pine");
        assert_eq!(slug("Tokyo Night"), "tokyo-night");
        assert_eq!(slug("  ../../etc/passwd  "), "etc-passwd");
        assert_eq!(slug("!!!"), "theme");
        assert_eq!(slug("High Contrast"), "high-contrast");
    }

    /// The round trip that matters to a user: save a theme, find the file,
    /// read it back, and get the same interface.
    #[test]
    fn a_theme_written_to_disk_comes_back_the_same() {
        let directory = tempfile::tempdir().unwrap();
        let mut theme = builtins::find("Nord").unwrap();
        theme.name = "My Nord".to_owned();
        theme.style.type_scale = Some(1.25);
        theme.style.font_family = Some("Iosevka, monospace".to_owned());
        let path = write_to(directory.path(), &theme).expect("writes");
        assert_eq!(path.file_name().unwrap(), "my-nord.toml");

        let (found, warnings) = load_all_in(directory.path());
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], theme);

        remove_in(directory.path(), "My Nord").expect("removes");
        assert!(load_all_in(directory.path()).0.is_empty());
        // Removing it twice is not an error to report at anybody.
        remove_in(directory.path(), "My Nord").expect("removes again");
    }

    /// A directory somebody has been editing by hand is the ordinary case.
    /// One bad file must cost one theme, not the list.
    #[test]
    fn a_broken_file_costs_one_theme_and_not_the_directory() {
        let directory = tempfile::tempdir().unwrap();
        write_to(directory.path(), &builtins::find("Nord").unwrap()).unwrap();
        std::fs::write(directory.path().join("broken.toml"), "nope = [").unwrap();
        std::fs::write(directory.path().join("notes.txt"), "not a theme").unwrap();
        let (found, warnings) = load_all_in(directory.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("broken.toml"));
    }

    /// Deleting the themes directory leaves the built-ins and a working app,
    /// which is one of `03-the-theme-file.md`'s acceptance clauses.
    #[test]
    fn a_missing_themes_directory_is_not_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let gone = directory.path().join("never-created");
        assert_eq!(load_all_in(&gone).0.len(), 0);
        assert_eq!(load_all_in(&gone).1.len(), 0);
        assert!(remove_in(&gone, "Anything").is_ok());
    }
}
