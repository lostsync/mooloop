use mooloop_core::DeviceKind;
use serde::{Deserialize, Serialize};
use slint::Color;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_BASE: &str = "#18181B";
const DEFAULT_ACCENT: &str = "#84CC16";
const DEFAULT_ALERT: &str = "#EAB308";
const MIN_ACCENT_CONTRAST: f32 = 3.0;

pub(crate) const MIN_CONTRAST: f32 = 0.6;
pub(crate) const MAX_CONTRAST: f32 = 1.4;
pub(crate) const MIN_ROUNDNESS: f32 = 0.0;
pub(crate) const MAX_ROUNDNESS: f32 = 3.0;

/// The whole palette is grown from three seeds, so a scheme is just those
/// three hex strings under a name. `base` seeds every neutral (background,
/// panel, surfaces, border, and the three text weights); `accent` is UI state
/// -- selection, focus, meters in their safe range; `alert` is the attention
/// color used for warnings, clipping headroom, and out-of-range readouts.
const BUILTIN_SCHEMES: [(&str, &str, &str, &str); 6] = [
    ("Mooloop", DEFAULT_BASE, DEFAULT_ACCENT, DEFAULT_ALERT),
    ("Graphite", "#151617", "#F59E0B", "#38BDF8"),
    ("High Contrast", "#000000", "#22D3EE", "#FACC15"),
    ("Ember", "#1A1413", "#F97316", "#38BDF8"),
    ("Indigo", "#14141F", "#A78BFA", "#F472B6"),
    ("Daylight", "#EDEDF0", "#3F7D00", "#B45309"),
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ThemeScheme {
    pub name: String,
    pub base: String,
    pub accent: String,
    pub alert: String,
}

impl ThemeScheme {
    fn new(name: &str, base: &str, accent: &str, alert: &str) -> Self {
        Self {
            name: name.to_owned(),
            base: base.to_owned(),
            accent: accent.to_owned(),
            alert: alert.to_owned(),
        }
    }

    pub(crate) fn builtins() -> Vec<Self> {
        BUILTIN_SCHEMES
            .iter()
            .map(|&(name, base, accent, alert)| Self::new(name, base, accent, alert))
            .collect()
    }

    pub(crate) fn is_builtin(name: &str) -> bool {
        BUILTIN_SCHEMES.iter().any(|&(builtin, ..)| builtin == name)
    }
}

/// Everything the Appearance page owns. The palette is not stored: it is
/// derived from these seeds on every apply, so old configs pick up palette
/// changes instead of freezing a stale ramp.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct AppearanceSettings {
    /// Name of the scheme the colors came from, or empty once they have been
    /// edited away from it. Purely a UI affordance -- the colors below are
    /// authoritative.
    #[serde(default)]
    pub scheme: String,
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default = "default_accent")]
    pub accent: String,
    #[serde(default = "default_alert")]
    pub alert: String,
    /// Multiplies every neutral's distance from `base`. 1.0 is the tuned ramp.
    #[serde(default = "default_unit")]
    pub contrast: f32,
    /// Multiplies the shared corner-radius scale. 0 gives square corners.
    #[serde(default = "default_unit")]
    pub roundness: f32,
    #[serde(default = "default_true")]
    pub smooth_curves: bool,
    /// UI-motion speed and easing, by option name as shown on the
    /// Appearance page. Persisted as strings so settings.toml stays
    /// readable; unknown names fall back to the defaults on load.
    #[serde(default = "default_motion_speed")]
    pub motion_speed: String,
    #[serde(default = "default_motion_easing")]
    pub motion_easing: String,
    /// Schemes saved from the Appearance page, listed after the built-ins.
    #[serde(default)]
    pub user_schemes: Vec<ThemeScheme>,
}

pub(crate) const MOTION_SPEEDS: [&str; 4] = ["instant", "fast", "normal", "slow"];
pub(crate) const MOTION_EASINGS: [&str; 4] = ["linear", "ease-out", "ease-in-out", "overshoot"];

fn default_motion_speed() -> String {
    "fast".to_owned()
}

fn default_motion_easing() -> String {
    "ease-in-out".to_owned()
}

/// Maps a persisted speed name onto the Motion global's option index.
/// Unknown names (older or hand-edited configs) fall back to Fast.
pub(crate) fn motion_speed_index(name: &str) -> i32 {
    MOTION_SPEEDS
        .iter()
        .position(|&option| option == name)
        .map(|index| index as i32)
        .unwrap_or(1)
}

/// Maps a persisted easing name onto the Motion global's option index.
pub(crate) fn motion_easing_index(name: &str) -> i32 {
    MOTION_EASINGS
        .iter()
        .position(|&option| option == name)
        .map(|index| index as i32)
        .unwrap_or(2)
}

/// Inverse of [`motion_speed_index`], for persisting the global back.
pub(crate) fn motion_speed_name(index: i32) -> &'static str {
    MOTION_SPEEDS
        .get(index.clamp(0, 3) as usize)
        .unwrap_or(&MOTION_SPEEDS[1])
}

/// Inverse of [`motion_easing_index`], for persisting the global back.
pub(crate) fn motion_easing_name(index: i32) -> &'static str {
    MOTION_EASINGS
        .get(index.clamp(0, 3) as usize)
        .unwrap_or(&MOTION_EASINGS[2])
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct GeneralSettings {
    #[serde(default)]
    pub developer_mode: bool,
    /// Whether the diagnostic log is also written to a file. Off by default:
    /// the console output costs nothing, but a file is state on the user's
    /// disk and they should be the one to ask for it. Survives restarts on
    /// purpose -- a problem worth logging is usually one that has to be caught
    /// on a later run.
    #[serde(default)]
    pub log_to_file: bool,
    /// Whether marker edits resolve onto zero crossings. An editing
    /// preference, not saved sampler state: it changes how an edit lands, not
    /// what any instrument sounds like, so it belongs to the user rather than
    /// to the project.
    #[serde(default)]
    pub snap_markers_to_zero: bool,
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

fn default_unit() -> f32 {
    1.0
}

fn default_base() -> String {
    DEFAULT_BASE.to_owned()
}

fn default_accent() -> String {
    DEFAULT_ACCENT.to_owned()
}

fn default_alert() -> String {
    DEFAULT_ALERT.to_owned()
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
            scheme: BUILTIN_SCHEMES[0].0.to_owned(),
            base: DEFAULT_BASE.to_owned(),
            accent: DEFAULT_ACCENT.to_owned(),
            alert: DEFAULT_ALERT.to_owned(),
            contrast: 1.0,
            roundness: 1.0,
            smooth_curves: true,
            motion_speed: default_motion_speed(),
            motion_easing: default_motion_easing(),
            user_schemes: Vec::new(),
        }
    }
}

impl AppearanceSettings {
    /// Normalizes the seeds (hex casing, clamped scalars) and rejects an
    /// accent that would be unreadable against the surface it derives.
    pub(crate) fn validated(&self) -> Result<Self, ValidationError> {
        let base = Rgb::parse(&self.base).ok_or(ValidationError::InvalidBase)?;
        let accent = Rgb::parse(&self.accent).ok_or(ValidationError::InvalidAccent)?;
        let alert = Rgb::parse(&self.alert).ok_or(ValidationError::InvalidAlert)?;
        let contrast = self.contrast.clamp(MIN_CONTRAST, MAX_CONTRAST);
        let roundness = self.roundness.clamp(MIN_ROUNDNESS, MAX_ROUNDNESS);
        let surface = derive_palette(base, accent, alert, contrast).surface;
        if contrast_ratio(accent, surface) < MIN_ACCENT_CONTRAST {
            return Err(ValidationError::LowContrast);
        }
        Ok(Self {
            scheme: self.scheme.clone(),
            base: base.to_hex(),
            accent: accent.to_hex(),
            alert: alert.to_hex(),
            contrast,
            roundness,
            smooth_curves: self.smooth_curves,
            motion_speed: if MOTION_SPEEDS.contains(&self.motion_speed.as_str()) {
                self.motion_speed.clone()
            } else {
                default_motion_speed()
            },
            motion_easing: if MOTION_EASINGS.contains(&self.motion_easing.as_str()) {
                self.motion_easing.clone()
            } else {
                default_motion_easing()
            },
            user_schemes: self.user_schemes.clone(),
        })
    }

    pub(crate) fn palette(&self) -> ThemePalette {
        let seed = |value: &str, fallback: &str| {
            Rgb::parse(value).unwrap_or_else(|| Rgb::parse(fallback).expect("valid default"))
        };
        derive_palette(
            seed(&self.base, DEFAULT_BASE),
            seed(&self.accent, DEFAULT_ACCENT),
            seed(&self.alert, DEFAULT_ALERT),
            self.contrast,
        )
    }

    /// Built-in schemes first, then the user's own, in save order.
    pub(crate) fn schemes(&self) -> Vec<ThemeScheme> {
        let mut schemes = ThemeScheme::builtins();
        schemes.extend(self.user_schemes.iter().cloned());
        schemes
    }

    /// Name of the scheme whose three seeds match the current colors, or
    /// empty once they have been edited away from every one of them.
    pub(crate) fn matching_scheme_name(&self) -> String {
        let matches = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        self.schemes()
            .into_iter()
            .find(|scheme| {
                matches(&scheme.base, &self.base)
                    && matches(&scheme.accent, &self.accent)
                    && matches(&scheme.alert, &self.alert)
            })
            .map(|scheme| scheme.name)
            .unwrap_or_default()
    }

    pub(crate) fn scheme(&self, name: &str) -> Option<ThemeScheme> {
        self.schemes()
            .into_iter()
            .find(|scheme| scheme.name == name)
    }

    /// Applies a scheme's three seeds, leaving contrast, roundness, and the
    /// graphics preferences alone -- those are independent of color.
    pub(crate) fn apply_scheme(&mut self, scheme: &ThemeScheme) {
        self.scheme = scheme.name.clone();
        self.base = scheme.base.clone();
        self.accent = scheme.accent.clone();
        self.alert = scheme.alert.clone();
    }

    /// Saves the current colors under `name`, replacing a user scheme of the
    /// same name. Built-in names are reserved.
    pub(crate) fn save_user_scheme(&mut self, name: &str) -> Result<(), ValidationError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ValidationError::EmptySchemeName);
        }
        if ThemeScheme::is_builtin(name) {
            return Err(ValidationError::ReservedSchemeName);
        }
        let scheme = ThemeScheme::new(name, &self.base, &self.accent, &self.alert);
        match self
            .user_schemes
            .iter_mut()
            .find(|existing| existing.name == name)
        {
            Some(existing) => *existing = scheme,
            None => self.user_schemes.push(scheme),
        }
        self.scheme = name.to_owned();
        Ok(())
    }

    /// Removes a user scheme. Built-ins are silently left in place, since the
    /// UI only offers Remove on user rows.
    pub(crate) fn remove_user_scheme(&mut self, name: &str) {
        self.user_schemes.retain(|scheme| scheme.name != name);
        if self.scheme == name {
            self.scheme = String::new();
        }
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

/// Everything the sample browser owns: the folders it lists, in display
/// order. Removal and reordering get a Preferences area in a later pass;
/// today the browser's own add affordance is the only writer.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct BrowserSettings {
    #[serde(default)]
    pub locations: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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
    #[serde(default)]
    pub browser: BrowserSettings,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: GeneralSettings::default(),
            appearance: AppearanceSettings::default(),
            audio: AudioSettings::default(),
            shortcuts: ShortcutSettings::default(),
            browser: BrowserSettings::default(),
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
        let appearance = settings
            .appearance
            .validated()
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
pub(crate) fn config_dir() -> PathBuf {
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

/// The diagnostic log, when the preference to write one is on.
///
/// Under the config directory rather than a state or cache directory: mooloop
/// keeps everything of its own in one place already, and someone being asked
/// for their log should find it next to the `settings.toml` they have seen
/// before, not in a second directory they have to be told about.
pub(crate) fn log_path() -> PathBuf {
    config_dir().join("mooloop.log")
}

/// Where a song that could not be saved is parked so it is not lost. Kept out
/// of the user's own folders: these are failures, and they should not turn up
/// mixed in with real songs.
pub(crate) fn quarantine_dir() -> PathBuf {
    config_dir().join("quarantine")
}

fn kind_slug(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Sampler => "sampler",
        DeviceKind::DrumSynth => "drum_synth",
        DeviceKind::MonoSynth => "mono_synth",
        DeviceKind::PolySynth => "poly_synth",
        DeviceKind::Ml1 => "ml1",
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl Rgb {
    fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix('#')?;
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        Some(Self {
            r: u8::from_str_radix(&hex[0..2], 16).ok()?,
            g: u8::from_str_radix(&hex[2..4], 16).ok()?,
            b: u8::from_str_radix(&hex[4..6], 16).ok()?,
        })
    }

    fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
    /// Swatch colors come straight from stored hex that has already been
    /// validated once; black is a visible, harmless fallback for the case
    /// where a hand-edited config slipped something else through.
    pub(crate) fn parse_or_black(value: &str) -> Self {
        Self::parse(value).unwrap_or(rgb(0, 0, 0))
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

/// Grows the full token set from the three seeds.
///
/// Every neutral is `base` moved a fixed fraction toward the contrasting pole
/// (white on a dark base, black on a light one), which is what lets any base
/// color -- including a light one -- produce a coherent ramp instead of only
/// the three ramps the old hardcoded presets shipped. `contrast` scales those
/// fractions, so one control tightens or opens the whole hierarchy at once.
fn derive_palette(base: Rgb, accent: Rgb, alert: Rgb, contrast: f32) -> ThemePalette {
    let dark = relative_luminance(base) < 0.4;
    let scale = contrast.clamp(MIN_CONTRAST, MAX_CONTRAST);
    let step = |fraction: f32| shade(base, dark, fraction * scale);
    let background = base;
    let on_accent = if relative_luminance(accent) > 0.45 {
        rgb(0, 0, 0)
    } else {
        rgb(255, 255, 255)
    };
    let destructive = rgb(0xef, 0x44, 0x44);
    ThemePalette {
        background,
        // The panel sits behind the work surface, so it moves away from the
        // contrast pole rather than toward it.
        panel: step(-0.12),
        surface: step(0.055),
        raised: step(0.075),
        active: step(0.115),
        border: step(0.17),
        text: step(0.87),
        muted: step(0.62),
        faint: step(0.25),
        accent,
        accent_active: mix(accent, background, 0.58),
        focus: mix(accent, on_accent, 0.22),
        warning: alert,
        destructive,
        destructive_active: mix(destructive, background, 0.62),
        // Meters read as one instrument with the rest of the UI: safe level is
        // the accent, the headroom warning is the alert color, and only a true
        // clip falls back to the fixed destructive red.
        meter_safe: accent,
        meter_warning: alert,
        meter_clip: destructive,
    }
}

/// Moves `color` toward the foreground pole for positive `amount` and toward
/// the background pole for negative, where the poles swap on a light base.
fn shade(color: Rgb, dark: bool, amount: f32) -> Rgb {
    let (foreground, background) = if dark {
        (rgb(255, 255, 255), rgb(0, 0, 0))
    } else {
        (rgb(0, 0, 0), rgb(255, 255, 255))
    };
    if amount >= 0.0 {
        mix(color, foreground, amount.min(1.0))
    } else {
        mix(color, background, (-amount).min(1.0))
    }
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
    InvalidBase,
    InvalidAccent,
    InvalidAlert,
    LowContrast,
    EmptySchemeName,
    ReservedSchemeName,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBase => write!(f, "Enter a base color as #RRGGBB"),
            Self::InvalidAccent => write!(f, "Enter an accent as #RRGGBB"),
            Self::InvalidAlert => write!(f, "Enter an alert color as #RRGGBB"),
            Self::LowContrast => write!(f, "Accent needs more contrast against the base color"),
            Self::EmptySchemeName => write!(f, "Name the scheme before saving it"),
            Self::ReservedSchemeName => write!(f, "That name belongs to a built-in scheme"),
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

    fn appearance(base: &str, accent: &str, alert: &str) -> AppearanceSettings {
        AppearanceSettings {
            base: base.to_owned(),
            accent: accent.to_owned(),
            alert: alert.to_owned(),
            ..AppearanceSettings::default()
        }
    }

    #[test]
    fn validates_and_normalizes_seed_colors() {
        let settings = appearance("#18181b", "#84cc16", "#eab308")
            .validated()
            .unwrap();
        assert_eq!(settings.base, "#18181B");
        assert_eq!(settings.accent, "#84CC16");
        assert_eq!(settings.alert, "#EAB308");
        assert!(matches!(
            appearance("#18181B", "lime", "#EAB308").validated(),
            Err(ValidationError::InvalidAccent)
        ));
        assert!(matches!(
            appearance("18181B", "#84CC16", "#EAB308").validated(),
            Err(ValidationError::InvalidBase)
        ));
        assert!(matches!(
            appearance("#18181B", "#232328", "#EAB308").validated(),
            Err(ValidationError::LowContrast)
        ));
    }

    #[test]
    fn clamps_contrast_and_roundness() {
        let settings = AppearanceSettings {
            contrast: 9.0,
            roundness: -4.0,
            ..AppearanceSettings::default()
        }
        .validated()
        .unwrap();
        assert_eq!(settings.contrast, MAX_CONTRAST);
        assert_eq!(settings.roundness, MIN_ROUNDNESS);
    }

    #[test]
    fn every_builtin_scheme_validates() {
        for scheme in ThemeScheme::builtins() {
            let mut settings = AppearanceSettings::default();
            settings.apply_scheme(&scheme);
            assert!(
                settings.validated().is_ok(),
                "built-in scheme {} fails validation",
                scheme.name
            );
        }
    }

    #[test]
    fn derives_a_readable_ramp_from_a_light_base() {
        // A light base has to flip the ramp: text goes dark, surfaces go
        // darker than the background rather than lighter.
        let palette = appearance("#EDEDF0", "#3F7D00", "#B45309").palette();
        assert!(relative_luminance(palette.text) < relative_luminance(palette.background));
        assert!(relative_luminance(palette.surface) < relative_luminance(palette.background));
        assert!(contrast_ratio(palette.text, palette.background) > 7.0);
    }

    #[test]
    fn palette_follows_the_three_seeds() {
        let palette = appearance("#101014", "#22D3EE", "#F97316").palette();
        assert_eq!(palette.accent, Rgb::parse("#22D3EE").unwrap());
        assert_eq!(palette.meter_safe, palette.accent);
        assert_eq!(palette.warning, Rgb::parse("#F97316").unwrap());
        assert_eq!(palette.meter_warning, palette.warning);
        assert_eq!(palette.background, Rgb::parse("#101014").unwrap());
    }

    #[test]
    fn contrast_control_widens_the_neutral_ramp() {
        let tight = AppearanceSettings {
            contrast: MIN_CONTRAST,
            ..AppearanceSettings::default()
        }
        .palette();
        let wide = AppearanceSettings {
            contrast: MAX_CONTRAST,
            ..AppearanceSettings::default()
        }
        .palette();
        assert!(
            contrast_ratio(wide.border, wide.background)
                > contrast_ratio(tight.border, tight.background)
        );
    }

    #[test]
    fn saves_and_removes_user_schemes() {
        let mut settings = appearance("#101014", "#22D3EE", "#F97316");
        settings.save_user_scheme("  Mine  ").unwrap();
        assert_eq!(settings.scheme, "Mine");
        assert_eq!(settings.user_schemes.len(), 1);
        assert_eq!(settings.scheme("Mine").unwrap().accent, "#22D3EE");
        assert_eq!(settings.schemes().len(), ThemeScheme::builtins().len() + 1);

        // Re-saving the same name replaces rather than duplicates.
        settings.accent = "#84CC16".to_owned();
        settings.save_user_scheme("Mine").unwrap();
        assert_eq!(settings.user_schemes.len(), 1);
        assert_eq!(settings.scheme("Mine").unwrap().accent, "#84CC16");

        assert!(matches!(
            settings.save_user_scheme("Mooloop"),
            Err(ValidationError::ReservedSchemeName)
        ));
        assert!(matches!(
            settings.save_user_scheme("   "),
            Err(ValidationError::EmptySchemeName)
        ));

        settings.remove_user_scheme("Mine");
        assert!(settings.user_schemes.is_empty());
        assert_eq!(settings.scheme, "");
    }

    #[test]
    fn upgrades_a_preset_era_config_without_losing_the_accent() {
        // Configs written before schemes carry `preset`/`accent` only; the
        // unknown key is ignored and the new seeds fall back to defaults.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 1\n[appearance]\npreset = 'graphite'\naccent = '#F59E0B'\n",
        )
        .unwrap();
        let appearance = UiSettings::load_from(&path).unwrap().appearance;
        assert_eq!(appearance.accent, "#F59E0B");
        assert_eq!(appearance.base, DEFAULT_BASE);
        assert_eq!(appearance.alert, DEFAULT_ALERT);
        assert_eq!(appearance.contrast, 1.0);
        assert_eq!(appearance.roundness, 1.0);
    }

    #[test]
    fn round_trips_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        let expected = UiSettings {
            schema_version: 1,
            general: GeneralSettings {
                developer_mode: true,
                snap_markers_to_zero: true,
                log_to_file: true,
            },
            appearance: appearance("#151617", "#F59E0B", "#38BDF8")
                .validated()
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
            browser: BrowserSettings {
                locations: vec![PathBuf::from("/sounds/one-shots")],
            },
        };
        expected.save_to(&path).unwrap();
        assert_eq!(UiSettings::load_from(&path).unwrap(), expected);
    }

    #[test]
    fn defaults_missing_browser_settings_for_existing_configs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 1\n[appearance]\npreset = 'mooloop'\naccent = '#84CC16'\n",
        )
        .unwrap();
        assert!(UiSettings::load_from(&path).unwrap().browser.locations.is_empty());
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
        let settings = UiSettings::load_from(&path).unwrap();
        assert!(settings.appearance.smooth_curves);
        let audio = settings.audio;
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
    fn motion_names_round_trip_through_indices() {
        for (index, name) in MOTION_SPEEDS.iter().enumerate() {
            assert_eq!(motion_speed_index(name), index as i32);
            assert_eq!(motion_speed_name(index as i32), *name);
        }
        for (index, name) in MOTION_EASINGS.iter().enumerate() {
            assert_eq!(motion_easing_index(name), index as i32);
            assert_eq!(motion_easing_name(index as i32), *name);
        }
    }

    #[test]
    fn unknown_motion_names_fall_back_to_defaults() {
        assert_eq!(motion_speed_index("snappy"), 1);
        assert_eq!(motion_easing_index("bouncy"), 2);
        let settings = AppearanceSettings {
            motion_speed: "warp".to_owned(),
            motion_easing: "swing".to_owned(),
            ..AppearanceSettings::default()
        }
        .validated()
        .unwrap();
        assert_eq!(settings.motion_speed, "fast");
        assert_eq!(settings.motion_easing, "ease-in-out");
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
