use mooloop_core::DeviceKind;
use serde::{Deserialize, Serialize};
use slint::Color;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_ACCENT: &str = "#84CC16";
const MIN_ACCENT_CONTRAST: f32 = 3.0;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AppearancePreset {
    #[default]
    Mooloop,
    Graphite,
    HighContrast,
}

impl AppearancePreset {
    pub(crate) fn from_index(index: i32) -> Self {
        match index {
            1 => Self::Graphite,
            2 => Self::HighContrast,
            _ => Self::Mooloop,
        }
    }

    pub(crate) fn index(self) -> i32 {
        match self {
            Self::Mooloop => 0,
            Self::Graphite => 1,
            Self::HighContrast => 2,
        }
    }

    pub(crate) fn palette(self, accent: Rgb) -> ThemePalette {
        let (background, panel, surface, raised, active, border, text, muted, faint) = match self {
            Self::Mooloop => (
                rgb(0x18, 0x18, 0x1b),
                rgb(0x13, 0x13, 0x16),
                rgb(0x23, 0x23, 0x28),
                rgb(0x2a, 0x2a, 0x2e),
                rgb(0x33, 0x33, 0x3a),
                rgb(0x3f, 0x3f, 0x46),
                rgb(0xe4, 0xe4, 0xe7),
                rgb(0xa1, 0xa1, 0xaa),
                rgb(0x52, 0x52, 0x5b),
            ),
            Self::Graphite => (
                rgb(0x15, 0x16, 0x17),
                rgb(0x11, 0x12, 0x13),
                rgb(0x22, 0x24, 0x26),
                rgb(0x2c, 0x2f, 0x31),
                rgb(0x37, 0x3a, 0x3d),
                rgb(0x45, 0x49, 0x4d),
                rgb(0xf1, 0xf3, 0xf5),
                rgb(0xa6, 0xad, 0xb4),
                rgb(0x61, 0x68, 0x6f),
            ),
            Self::HighContrast => (
                rgb(0x00, 0x00, 0x00),
                rgb(0x08, 0x08, 0x08),
                rgb(0x15, 0x15, 0x15),
                rgb(0x26, 0x26, 0x26),
                rgb(0x36, 0x36, 0x36),
                rgb(0x70, 0x70, 0x70),
                rgb(0xff, 0xff, 0xff),
                rgb(0xc7, 0xc7, 0xc7),
                rgb(0x88, 0x88, 0x88),
            ),
        };
        let on_accent = if relative_luminance(accent) > 0.45 {
            rgb(0, 0, 0)
        } else {
            rgb(255, 255, 255)
        };
        ThemePalette {
            background,
            panel,
            surface,
            raised,
            active,
            border,
            text,
            muted,
            faint,
            accent,
            accent_active: mix(accent, background, 0.58),
            focus: mix(accent, on_accent, 0.22),
            warning: rgb(0xea, 0xb3, 0x08),
            destructive: rgb(0xef, 0x44, 0x44),
            destructive_active: rgb(0x7f, 0x1d, 0x1d),
            meter_safe: rgb(0x22, 0xc5, 0x5e),
            meter_warning: rgb(0xea, 0xb3, 0x08),
            meter_clip: rgb(0xef, 0x44, 0x44),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct AppearanceSettings {
    pub preset: AppearancePreset,
    pub accent: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct GeneralSettings {
    #[serde(default)]
    pub developer_mode: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AudioDriverKind {
    #[default]
    Jack,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct JackSettings {
    #[serde(default)]
    pub output_port_l: Option<String>,
    #[serde(default)]
    pub output_port_r: Option<String>,
    /// `None` leaves the JACK server's current buffer size alone.
    #[serde(default)]
    pub buffer_size: Option<u32>,
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
}

impl Default for JackSettings {
    fn default() -> Self {
        Self {
            output_port_l: None,
            output_port_r: None,
            buffer_size: Some(256),
            auto_reconnect: true,
        }
    }
}

impl JackSettings {
    pub(crate) fn output_target(&self) -> Option<(String, String)> {
        match (&self.output_port_l, &self.output_port_r) {
            (Some(l), Some(r)) => Some((l.clone(), r.clone())),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct AudioSettings {
    #[serde(default)]
    pub driver: AudioDriverKind,
    #[serde(default)]
    pub jack: JackSettings,
}

impl AudioSettings {
    /// Maps this crate's persisted settings onto the engine's driver-facing
    /// config. Kept as an explicit conversion, not a shared type, so
    /// `mooloop-engine` never depends on `mooloop-ui`'s settings schema.
    pub(crate) fn engine_config(&self) -> mooloop_engine::AudioConfig {
        mooloop_engine::AudioConfig {
            buffer_size: self.jack.buffer_size,
            output_target: self.jack.output_target(),
            auto_reconnect: self.jack.auto_reconnect,
        }
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            preset: AppearancePreset::Mooloop,
            accent: DEFAULT_ACCENT.to_owned(),
        }
    }
}

impl AppearanceSettings {
    pub(crate) fn validated(
        preset: AppearancePreset,
        accent: &str,
    ) -> Result<Self, ValidationError> {
        let accent_rgb = Rgb::parse(accent)?;
        let surface = preset.palette(accent_rgb).surface;
        if contrast_ratio(accent_rgb, surface) < MIN_ACCENT_CONTRAST {
            return Err(ValidationError::LowContrast);
        }
        Ok(Self {
            preset,
            accent: accent_rgb.to_hex(),
        })
    }

    pub(crate) fn palette(&self) -> ThemePalette {
        let accent =
            Rgb::parse(&self.accent).unwrap_or_else(|_| Rgb::parse(DEFAULT_ACCENT).unwrap());
        self.preset.palette(accent)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ShortcutSettings {
    /// Action id -> `KeyChord::display()` text. Only entries that differ
    /// from the registry default are stored, so `actions::ACTIONS` can grow
    /// without a settings migration.
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct UiSettings {
    schema_version: u32,
    #[serde(default)]
    pub general: GeneralSettings,
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub shortcuts: ShortcutSettings,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: GeneralSettings::default(),
            appearance: AppearanceSettings::default(),
            audio: AudioSettings::default(),
            shortcuts: ShortcutSettings::default(),
        }
    }
}

impl UiSettings {
    pub(crate) fn load_or_default() -> Self {
        let path = settings_path();
        match Self::load_from(&path) {
            Ok(settings) => settings,
            Err(SettingsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Self::default()
            }
            Err(error) => {
                eprintln!(
                    "mooloop: ignoring invalid settings at {}: {error}",
                    path.display()
                );
                Self::default()
            }
        }
    }

    fn load_from(path: &Path) -> Result<Self, SettingsError> {
        let text = fs::read_to_string(path).map_err(SettingsError::Io)?;
        let settings: Self = toml::from_str(&text).map_err(SettingsError::Parse)?;
        if settings.schema_version != SCHEMA_VERSION {
            return Err(SettingsError::UnsupportedVersion(settings.schema_version));
        }
        let appearance =
            AppearanceSettings::validated(settings.appearance.preset, &settings.appearance.accent)
                .map_err(SettingsError::Validation)?;
        Ok(Self {
            appearance,
            ..settings
        })
    }

    pub(crate) fn save(&self) -> Result<(), SettingsError> {
        self.save_to(&settings_path())
    }

    fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(SettingsError::Io)?;
        }
        let text = toml::to_string_pretty(self).map_err(SettingsError::Serialize)?;
        let temporary = path.with_extension("toml.tmp");
        fs::write(&temporary, text).map_err(SettingsError::Io)?;
        if let Err(error) = fs::rename(&temporary, path) {
            if path.exists() {
                fs::remove_file(path).map_err(SettingsError::Io)?;
                fs::rename(&temporary, path).map_err(SettingsError::Io)?;
            } else {
                return Err(SettingsError::Io(error));
            }
        }
        Ok(())
    }
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// Directory presets and prefs both live under: `$MOOLOOP_CONFIG_DIR`, or
/// the platform config directory (`%APPDATA%\mooloop`,
/// `~/Library/Application Support/mooloop`, or
/// `$XDG_CONFIG_HOME/mooloop`/`~/.config/mooloop`).
fn config_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("MOOLOOP_CONFIG_DIR") {
        return PathBuf::from(path);
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = std::env::var_os("APPDATA") {
        return PathBuf::from(path).join("mooloop");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("Library/Application Support/mooloop");
    }
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("mooloop");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".config/mooloop")
}

/// Directory holding one subdirectory of generator presets per
/// [`DeviceKind`], e.g. `presets/generators/mono_synth/`.
pub(crate) fn generator_presets_dir(kind: DeviceKind) -> PathBuf {
    config_dir()
        .join("presets/generators")
        .join(kind_slug(kind))
}

/// Directory holding whole-channel presets (`presets/channels/`).
pub(crate) fn channel_presets_dir() -> PathBuf {
    config_dir().join("presets/channels")
}

fn kind_slug(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Sampler => "sampler",
        DeviceKind::DrumSynth => "drum_synth",
        DeviceKind::MonoSynth => "mono_synth",
        DeviceKind::PolySynth => "poly_synth",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    fn parse(value: &str) -> Result<Self, ValidationError> {
        let hex = value.strip_prefix('#').ok_or(ValidationError::InvalidHex)?;
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ValidationError::InvalidHex);
        }
        Ok(Self {
            r: u8::from_str_radix(&hex[0..2], 16).map_err(|_| ValidationError::InvalidHex)?,
            g: u8::from_str_radix(&hex[2..4], 16).map_err(|_| ValidationError::InvalidHex)?,
            b: u8::from_str_radix(&hex[4..6], 16).map_err(|_| ValidationError::InvalidHex)?,
        })
    }

    fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
    pub(crate) fn color(self) -> Color {
        Color::from_rgb_u8(self.r, self.g, self.b)
    }
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

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

fn mix(a: Rgb, b: Rgb, b_weight: f32) -> Rgb {
    let blend = |x: u8, y: u8| (x as f32 * (1.0 - b_weight) + y as f32 * b_weight).round() as u8;
    rgb(blend(a.r, b.r), blend(a.g, b.g), blend(a.b, b.b))
}

fn relative_luminance(color: Rgb) -> f32 {
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

fn contrast_ratio(a: Rgb, b: Rgb) -> f32 {
    let (light, dark) = if relative_luminance(a) >= relative_luminance(b) {
        (a, b)
    } else {
        (b, a)
    };
    (relative_luminance(light) + 0.05) / (relative_luminance(dark) + 0.05)
}

#[derive(Debug)]
pub(crate) enum ValidationError {
    InvalidHex,
    LowContrast,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHex => write!(f, "Enter an accent as #RRGGBB"),
            Self::LowContrast => write!(f, "Accent needs more contrast against the selected theme"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum SettingsError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    UnsupportedVersion(u32),
    Validation(ValidationError),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Parse(error) => error.fmt(f),
            Self::Serialize(error) => error.fmt(f),
            Self::UnsupportedVersion(version) => write!(f, "unsupported schema version {version}"),
            Self::Validation(error) => error.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_accent() {
        let settings = AppearanceSettings::validated(AppearancePreset::Mooloop, "#84cc16").unwrap();
        assert_eq!(settings.accent, "#84CC16");
        assert!(matches!(
            AppearanceSettings::validated(AppearancePreset::Mooloop, "lime"),
            Err(ValidationError::InvalidHex)
        ));
        assert!(matches!(
            AppearanceSettings::validated(AppearancePreset::Mooloop, "#232328"),
            Err(ValidationError::LowContrast)
        ));
    }

    #[test]
    fn round_trips_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        let expected = UiSettings {
            schema_version: 1,
            general: GeneralSettings {
                developer_mode: true,
            },
            appearance: AppearanceSettings::validated(AppearancePreset::Graphite, "#F59E0B")
                .unwrap(),
            audio: AudioSettings {
                driver: AudioDriverKind::Jack,
                jack: JackSettings {
                    output_port_l: Some("Carla:audio-in1".to_owned()),
                    output_port_r: Some("Carla:audio-in2".to_owned()),
                    buffer_size: Some(256),
                    auto_reconnect: false,
                },
            },
            shortcuts: ShortcutSettings {
                overrides: [("edit.undo".to_owned(), "Ctrl+Alt+Z".to_owned())]
                    .into_iter()
                    .collect(),
            },
        };
        expected.save_to(&path).unwrap();
        assert_eq!(UiSettings::load_from(&path).unwrap(), expected);
    }

    #[test]
    fn defaults_missing_audio_settings_for_existing_configs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 1\n[appearance]\npreset = 'mooloop'\naccent = '#84CC16'\n",
        )
        .unwrap();
        let audio = UiSettings::load_from(&path).unwrap().audio;
        assert_eq!(audio, AudioSettings::default());
        assert!(audio.jack.auto_reconnect);
        assert_eq!(audio.jack.output_target(), None);
    }

    #[test]
    fn maps_jack_settings_onto_engine_config() {
        let jack = JackSettings {
            output_port_l: Some("Carla:audio-in1".to_owned()),
            output_port_r: Some("Carla:audio-in2".to_owned()),
            buffer_size: Some(512),
            auto_reconnect: true,
        };
        let audio = AudioSettings {
            driver: AudioDriverKind::Jack,
            jack,
        };
        let config = audio.engine_config();
        assert_eq!(config.buffer_size, Some(512));
        assert_eq!(
            config.output_target,
            Some(("Carla:audio-in1".to_owned(), "Carla:audio-in2".to_owned()))
        );
        assert!(config.auto_reconnect);
    }

    #[test]
    fn rejects_unknown_schema_and_corrupt_toml() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 2\n[appearance]\npreset = 'mooloop'\naccent = '#84CC16'\n",
        )
        .unwrap();
        assert!(matches!(
            UiSettings::load_from(&path),
            Err(SettingsError::UnsupportedVersion(2))
        ));
        fs::write(&path, "not toml = [").unwrap();
        assert!(matches!(
            UiSettings::load_from(&path),
            Err(SettingsError::Parse(_))
        ));
    }

    #[test]
    fn defaults_missing_general_settings_for_existing_configs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 1\n[appearance]\npreset = 'mooloop'\naccent = '#84CC16'\n",
        )
        .unwrap();
        assert!(!UiSettings::load_from(&path).unwrap().general.developer_mode);
    }

    #[test]
    fn defaults_missing_shortcut_settings_for_existing_configs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 1\n[appearance]\npreset = 'mooloop'\naccent = '#84CC16'\n",
        )
        .unwrap();
        assert!(UiSettings::load_from(&path)
            .unwrap()
            .shortcuts
            .overrides
            .is_empty());
    }
}
