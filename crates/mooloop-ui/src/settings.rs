pub(crate) use crate::theme::color::Rgb;
pub(crate) use crate::theme::ramp::ThemePalette;
use crate::actions::SuperKeyMode;
use crate::theme::ramp::Ramp;
use crate::theme::{
    builtins, catalog, file, wal, Mode, ThemeColors, ThemeDefinition, ThemeStyle,
    MIN_ACCENT_CONTRAST, MODES,
};
use mooloop_core::{DeviceKind, EffectKind};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: u32 = 1;
pub(crate) const DEFAULT_BASE: &str = "#18181B";
pub(crate) const DEFAULT_ACCENT: &str = "#84CC16";
pub(crate) const DEFAULT_ALERT: &str = "#EAB308";
const DEFAULT_MONO: &str = "monospace";

pub(crate) const MIN_CONTRAST: f32 = 0.6;
pub(crate) const MAX_CONTRAST: f32 = 1.4;
pub(crate) const MIN_ROUNDNESS: f32 = 0.0;
pub(crate) const MAX_ROUNDNESS: f32 = 3.0;

/// The accessibility control. The working type size of this interface is
/// 7-11px, which is small; 2.0 takes it to 14-22px, which is a different
/// program to sit in front of for an evening. Below 0.75 the 7px step rounds
/// into illegibility, so that is the floor.
pub(crate) const MIN_TYPE_SCALE: f32 = 0.75;
pub(crate) const MAX_TYPE_SCALE: f32 = 2.0;

/// Control heights and the padding ramp. The ceiling is where a 24px control
/// becomes a 42px one, which is a touch target; the floor is where 2px of
/// padding becomes 1px and things start to touch.
pub(crate) const MIN_DENSITY: f32 = 0.75;
pub(crate) const MAX_DENSITY: f32 = 1.75;

/// Stroke. Zero is a real setting and is the point of the floor being zero: a
/// theme that wants a borderless interface asks for one here.
pub(crate) const MIN_HAIRLINE: f32 = 0.0;
pub(crate) const MAX_HAIRLINE: f32 = 3.0;
pub(crate) const MIN_STROKE_EMPHASIS: f32 = 0.0;
pub(crate) const MAX_STROKE_EMPHASIS: f32 = 5.0;

/// CSS weights, and the two that matter are 400 and 500 -- anything heavier at
/// 9px is a smudge. The range is the full one because a bitmap face compiled
/// in later may only exist at one weight and refusing to name it would be
/// arbitrary.
pub(crate) const MIN_FONT_WEIGHT: i32 = 100;
pub(crate) const MAX_FONT_WEIGHT: i32 = 900;

/// A scheme as `settings.toml` spelled it before a scheme became a theme:
/// three seeds under a name.
///
/// **Kept only so that saved ones survive.** `UiSettings::load_from` writes
/// every entry out as a theme file and clears the list, so a config written
/// today has no `user-schemes` key at all. `docs/plans/theming/00-status.md`
/// records the migration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ThemeScheme {
    pub name: String,
    pub base: String,
    pub accent: String,
    pub alert: String,
}

impl ThemeScheme {
    /// The theme this scheme becomes: a dark variant of three seeds, and no
    /// light one -- which the mode control derives rather than refuses.
    fn definition(&self) -> ThemeDefinition {
        ThemeDefinition {
            description: "Saved from the Appearance page.".to_owned(),
            dark: Some(ThemeColors::Seeds {
                base: Rgb::parse_or_black(&self.base),
                accent: Rgb::parse_or_black(&self.accent),
                alert: Rgb::parse_or_black(&self.alert),
            }),
            ..ThemeDefinition::empty(&self.name)
        }
    }
}

/// Everything the Appearance page owns. The palette is not stored: it is
/// derived from these seeds on every apply, so old configs pick up palette
/// changes instead of freezing a stale ramp.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct AppearanceSettings {
    /// Name of the theme the colours came from, or empty once they have been
    /// edited away from it -- which the Appearance page shows as Custom.
    ///
    /// **`alias = "scheme"` is the migration.** Every settings file written
    /// before a scheme became a theme spells this field that way, and the two
    /// words meant the same thing, so the old key is read and the new one is
    /// written. One old name does need moving rather than renaming and
    /// `validated` does it: `Daylight` was light-mode Mooloop before a theme
    /// had two variants, so it becomes exactly that.
    #[serde(default, alias = "scheme")]
    pub theme: String,
    /// `dark`, `light`, or `system` -- and `system` asks the desktop, which is
    /// the Linux-first half of this: a shell that flips its own colour scheme
    /// at dusk should take the DAW with it.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Set once a colour has been edited away from `theme`.
    ///
    /// **`theme` keeps naming the theme the edit started from**, which is the
    /// point: saving a warmed-up Dracula has to keep Alucard, and it cannot if
    /// the only record of where the colours came from was thrown away the
    /// moment somebody touched a picker. The page shows this as Custom and
    /// highlights nothing in the list; the seeds below are what is drawn.
    #[serde(default)]
    pub customized: bool,
    /// The three seeds the Appearance page's colour pickers edit.
    ///
    /// They are the *Custom* theme when `theme` is empty, and a projection of
    /// the selected theme's current variant when it is not -- so the pickers
    /// always show something coherent and editing one starts from what is
    /// already on screen rather than from the last thing that was typed.
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default = "default_accent")]
    pub accent: String,
    #[serde(default = "default_alert")]
    pub alert: String,
    /// Multiplies every neutral's distance from the background. 1.0 is the
    /// ramp as the theme authored it.
    #[serde(default = "default_unit")]
    pub contrast: f32,
    /// Multiplies the shared corner-radius scale. 0 gives square corners.
    #[serde(default = "default_unit")]
    pub roundness: f32,
    /// Multiplies the whole type scale. The accessibility control: nothing
    /// else in this program makes 7px text bigger.
    #[serde(default = "default_unit")]
    pub type_scale: f32,
    /// Multiplies control heights and the padding ramp.
    #[serde(default = "default_unit")]
    pub density: f32,
    /// Font families, CSS-style lists. Empty means the platform default.
    ///
    /// **A theme names a font; it cannot ship one.** Slint 1.17.1 has no
    /// runtime font registration, so a family is resolved from what is
    /// compiled in or installed, and a name nobody has falls back silently.
    /// That is why the Appearance page reports what was resolved.
    #[serde(default)]
    pub font_family: String,
    #[serde(default = "default_mono")]
    pub font_family_mono: String,
    #[serde(default = "default_font_weight")]
    pub font_weight: i32,
    /// The two stroke weights, in pixels. Zero hairline is a borderless
    /// interface and is a real setting.
    #[serde(default = "default_hairline")]
    pub hairline: f32,
    #[serde(default = "default_stroke_emphasis")]
    pub stroke_emphasis: f32,
    #[serde(default = "default_true")]
    pub smooth_curves: bool,
    /// UI-motion speed and easing, by option name as shown on the
    /// Appearance page. Persisted as strings so settings.toml stays
    /// readable; unknown names fall back to the defaults on load.
    #[serde(default = "default_motion_speed")]
    pub motion_speed: String,
    #[serde(default = "default_motion_easing")]
    pub motion_easing: String,
    /// How fast a meter falls back, by option name. The rates themselves
    /// live in `meter.rs`, which is what runs them; this is only which row
    /// of that table the user picked, kept as a word so `settings.toml`
    /// stays readable and so reordering the table cannot silently change
    /// what an existing config means.
    #[serde(default = "default_meter_falloff")]
    pub meter_falloff: String,
    /// Schemes saved from the Appearance page before they were theme files.
    /// Emptied by the migration in `UiSettings::load_from`, and skipped on
    /// write, so a config saved today does not carry the key at all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

/// Meter fall rates by name, in the order `meter::FALLOFF_DB_PER_SECOND`
/// states them. Index 1, Standard, is the IEC rate and the default.
pub(crate) const METER_FALLOFFS: [&str; 4] = ["fast", "standard", "slow", "slowest"];

fn default_meter_falloff() -> String {
    "standard".to_owned()
}

/// Maps a persisted falloff name onto its row. Unknown names -- an older
/// config, or one edited by hand -- fall back to Standard.
pub(crate) fn meter_falloff_index(name: &str) -> i32 {
    METER_FALLOFFS
        .iter()
        .position(|&option| option == name)
        .map(|index| index as i32)
        .unwrap_or(1)
}

/// Inverse of [`meter_falloff_index`], for persisting the global back.
pub(crate) fn meter_falloff_name(index: i32) -> &'static str {
    METER_FALLOFFS
        .get(index.clamp(0, 3) as usize)
        .unwrap_or(&METER_FALLOFFS[1])
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

/// The driver a settings file was written under. The build decides which
/// driver runs; [`AudioSettings::active`] follows the build, not this.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AudioDriverKind {
    #[cfg_attr(not(target_os = "macos"), default)]
    Jack,
    #[cfg_attr(target_os = "macos", default)]
    CoreAudio,
}

fn default_true() -> bool {
    true
}

fn default_unit() -> f32 {
    1.0
}

fn default_mode() -> String {
    Mode::default().name().to_owned()
}

fn default_mono() -> String {
    DEFAULT_MONO.to_owned()
}

fn default_font_weight() -> i32 {
    400
}

fn default_hairline() -> f32 {
    1.0
}

fn default_stroke_emphasis() -> f32 {
    2.0
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

/// One driver's persisted output choices. JACK and Core Audio share the shape
/// because the engine's output target is a left/right pair under both: two
/// JACK port names, or two `<device>#<channel>` addresses on one Core Audio
/// device.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct DriverSettings {
    #[serde(default)]
    pub output_port_l: Option<String>,
    #[serde(default)]
    pub output_port_r: Option<String>,
    /// The outputs picked before the one above, most recent first, for the
    /// engine to fall back through when the output playing goes away.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_outputs: Vec<(String, String)>,
    /// `None` leaves the driver's current buffer size alone.
    #[serde(default)]
    pub buffer_size: Option<u32>,
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
}

impl Default for DriverSettings {
    fn default() -> Self {
        Self {
            output_port_l: None,
            output_port_r: None,
            earlier_outputs: Vec::new(),
            // The server's, until somebody picks one. Under JACK the buffer
            // is server-wide, so a default of 256 re-sized it for every
            // client on the machine each time mooloop started, whatever the
            // user had set the server to (P8 in
            // `reports/teams-2026-09-22.md`).
            buffer_size: None,
            auto_reconnect: true,
        }
    }
}

impl DriverSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub(crate) fn output_target(&self) -> Option<(String, String)> {
        match (&self.output_port_l, &self.output_port_r) {
            (Some(l), Some(r)) => Some((l.clone(), r.clone())),
            _ => None,
        }
    }

    /// Record `chosen` as the output picked, and the one it replaces as the
    /// most recent earlier pick -- by the engine's rule, which keeps the same
    /// list for the running session.
    pub(crate) fn pick_output(&mut self, chosen: (String, String)) {
        let mut picks: Vec<(String, String)> = self
            .output_target()
            .into_iter()
            .chain(self.earlier_outputs.drain(..))
            .collect();
        mooloop_engine::remember_output(&mut picks, chosen.clone());
        self.output_port_l = Some(chosen.0);
        self.output_port_r = Some(chosen.1);
        self.earlier_outputs = picks.into_iter().skip(1).collect();
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct AudioSettings {
    #[serde(default)]
    pub driver: AudioDriverKind,
    // A section this build does not use is written only once it holds
    // something, so a Linux settings file does not grow an empty Core Audio
    // section, nor a Mac's a JACK one.
    #[serde(default)]
    #[cfg_attr(target_os = "macos", serde(skip_serializing_if = "DriverSettings::is_default"))]
    pub jack: DriverSettings,
    #[serde(default)]
    #[cfg_attr(
        not(target_os = "macos"),
        serde(skip_serializing_if = "DriverSettings::is_default")
    )]
    pub core_audio: DriverSettings,
}

impl AudioSettings {
    /// The section this build's driver reads and writes. The other is carried
    /// through a save untouched, so a configuration directory shared between
    /// a Linux and a Mac checkout keeps both machines' choices.
    pub(crate) fn active(&self) -> &DriverSettings {
        #[cfg(target_os = "macos")]
        return &self.core_audio;
        #[cfg(not(target_os = "macos"))]
        return &self.jack;
    }

    pub(crate) fn active_mut(&mut self) -> &mut DriverSettings {
        #[cfg(target_os = "macos")]
        return &mut self.core_audio;
        #[cfg(not(target_os = "macos"))]
        return &mut self.jack;
    }

    /// Maps this crate's persisted settings onto the engine's driver-facing
    /// config. Kept as an explicit conversion, not a shared type, so
    /// `mooloop-engine` never depends on `mooloop-ui`'s settings schema.
    pub(crate) fn engine_config(&self) -> mooloop_engine::AudioConfig {
        let active = self.active();
        mooloop_engine::AudioConfig {
            buffer_size: active.buffer_size,
            output_target: active.output_target(),
            earlier_outputs: active.earlier_outputs.clone(),
            auto_reconnect: active.auto_reconnect,
        }
    }
}

/// A variant as the three colour pickers can express it.
///
/// A ramp has no seeds, so this is lossy on purpose: its background, its
/// accent and its slot 0A are exactly the three things the pickers mean. What
/// it buys is that touching a picker while Nord is selected starts you from
/// Nord rather than from whatever was typed there last.
fn project(colors: ThemeColors) -> (String, String, String) {
    match colors {
        ThemeColors::Seeds { base, accent, alert } => {
            (base.to_hex(), accent.to_hex(), alert.to_hex())
        }
        ThemeColors::Ramp(ramp) => (
            ramp.slot(0).to_hex(),
            ramp.accent().to_hex(),
            ramp.slot(0x0A).to_hex(),
        ),
    }
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: builtins::DEFAULT_THEME.to_owned(),
            customized: false,
            mode: default_mode(),
            base: DEFAULT_BASE.to_owned(),
            accent: DEFAULT_ACCENT.to_owned(),
            alert: DEFAULT_ALERT.to_owned(),
            contrast: 1.0,
            roundness: 1.0,
            type_scale: 1.0,
            density: 1.0,
            font_family: String::new(),
            font_family_mono: DEFAULT_MONO.to_owned(),
            font_weight: default_font_weight(),
            hairline: default_hairline(),
            stroke_emphasis: default_stroke_emphasis(),
            smooth_curves: true,
            motion_speed: default_motion_speed(),
            motion_easing: default_motion_easing(),
            meter_falloff: default_meter_falloff(),
            user_schemes: Vec::new(),
        }
    }
}

impl AppearanceSettings {
    /// Normalizes the seeds (hex casing, clamped scalars), moves the one
    /// renamed theme, and rejects an accent that would be unreadable against
    /// the surface it derives.
    ///
    /// **Only a Custom accent is rejected.** A theme's own accent is checked
    /// where the theme is written -- `builtins.rs` has a test for every
    /// variant of every built-in, and a file that ships an illegible one is
    /// its author's business. What must not happen is that a bad *seed* the
    /// user typed makes the settings file unloadable, and that is what this
    /// has always been for.
    pub(crate) fn validated(&self) -> Result<Self, ValidationError> {
        let base = Rgb::parse(&self.base).ok_or(ValidationError::InvalidBase)?;
        let accent = Rgb::parse(&self.accent).ok_or(ValidationError::InvalidAccent)?;
        let alert = Rgb::parse(&self.alert).ok_or(ValidationError::InvalidAlert)?;
        let contrast = self.contrast.clamp(MIN_CONTRAST, MAX_CONTRAST);
        let mut theme = self.theme.clone();
        let mut mode = if MODES.contains(&self.mode.as_str()) {
            self.mode.clone()
        } else {
            default_mode()
        };
        // `Daylight` was a scheme when a scheme was three seeds and a theme
        // had one variant. It is light-mode Mooloop and always was, so it
        // becomes that -- unless somebody has since saved a theme of their own
        // under the name, in which case theirs wins and nothing moves.
        if theme == "Daylight" && catalog::find("Daylight").is_none() {
            theme = builtins::DEFAULT_THEME.to_owned();
            mode = Mode::Light.name().to_owned();
        }
        if !theme.is_empty() && catalog::find(&theme).is_none() {
            // A theme file the user deleted by hand. Keep the colours, drop
            // the name: that is what the page shows as Custom, and it is
            // better than starting them over on the default.
            theme = String::new();
        }
        let customized = self.customized || theme.is_empty();
        if customized
            && Ramp::from_seeds(base, accent, alert).accent_contrast(contrast)
                < MIN_ACCENT_CONTRAST
        {
            return Err(ValidationError::LowContrast);
        }
        Ok(Self {
            theme,
            customized,
            mode,
            base: base.to_hex(),
            accent: accent.to_hex(),
            alert: alert.to_hex(),
            contrast,
            roundness: self.roundness.clamp(MIN_ROUNDNESS, MAX_ROUNDNESS),
            type_scale: self.type_scale.clamp(MIN_TYPE_SCALE, MAX_TYPE_SCALE),
            density: self.density.clamp(MIN_DENSITY, MAX_DENSITY),
            font_family: self.font_family.trim().to_owned(),
            font_family_mono: {
                let mono = self.font_family_mono.trim();
                if mono.is_empty() { DEFAULT_MONO.to_owned() } else { mono.to_owned() }
            },
            font_weight: self.font_weight.clamp(MIN_FONT_WEIGHT, MAX_FONT_WEIGHT),
            hairline: self.hairline.clamp(MIN_HAIRLINE, MAX_HAIRLINE),
            stroke_emphasis: self
                .stroke_emphasis
                .clamp(MIN_STROKE_EMPHASIS, MAX_STROKE_EMPHASIS),
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
            meter_falloff: if METER_FALLOFFS.contains(&self.meter_falloff.as_str()) {
                self.meter_falloff.clone()
            } else {
                default_meter_falloff()
            },
            user_schemes: self.user_schemes.clone(),
        })
    }

    pub(crate) fn mode(&self) -> Mode {
        Mode::from_name(&self.mode)
    }

    /// Whether the dark variant is the one in force. `system` asks the
    /// desktop, so this is not a pure function of the settings.
    pub(crate) fn wants_dark(&self) -> bool {
        self.mode().wants_dark()
    }

    /// The theme named by the page, whether or not its colours have been
    /// edited. `None` only when nothing is named.
    pub(crate) fn definition(&self) -> Option<ThemeDefinition> {
        if self.theme.is_empty() {
            None
        } else {
            catalog::find(&self.theme)
        }
    }

    /// The theme whose colours are actually being drawn, which is nothing once
    /// they have been edited.
    fn active_definition(&self) -> Option<ThemeDefinition> {
        if self.customized {
            None
        } else {
            self.definition()
        }
    }

    /// The three seed pickers, as colours.
    fn seeds(&self) -> ThemeColors {
        let seed = |value: &str, fallback: &str| {
            Rgb::parse(value).unwrap_or_else(|| Rgb::parse(fallback).expect("valid default"))
        };
        ThemeColors::Seeds {
            base: seed(&self.base, DEFAULT_BASE),
            accent: seed(&self.accent, DEFAULT_ACCENT),
            alert: seed(&self.alert, DEFAULT_ALERT),
        }
    }

    /// The colours in force: the selected theme's variant for whichever side
    /// of the light/dark line the mode resolves to, or the three seeds when
    /// the theme is Custom.
    pub(crate) fn colors(&self) -> ThemeColors {
        self.active_definition()
            .map(|theme| theme.variant(self.wants_dark()))
            .unwrap_or_else(|| self.seeds())
    }

    pub(crate) fn ramp(&self) -> Ramp {
        self.colors().ramp()
    }

    pub(crate) fn palette(&self) -> ThemePalette {
        self.ramp().palette(self.contrast)
    }

    /// The colours the channel, track and pattern pickers offer, which is
    /// what "the swatch palette follows the colourscheme" means in code.
    pub(crate) fn swatches(&self) -> Vec<Rgb> {
        self.ramp().swatches()
    }

    /// Every theme, built-in and user, in list order.
    pub(crate) fn themes(&self) -> Vec<ThemeDefinition> {
        catalog::all()
    }

    /// Selects a theme: its variant becomes what the seed pickers show, and
    /// whatever style it states replaces the matching control.
    ///
    /// A theme that states no style leaves every scalar alone, which is what
    /// makes the eleven colour-only themes switchable without losing a type
    /// scale somebody set for their eyes.
    pub(crate) fn apply_theme(&mut self, theme: &ThemeDefinition) {
        self.theme = theme.name.clone();
        self.customized = false;
        self.sync_seeds();
        let style = &theme.style;
        if let Some(value) = style.contrast {
            self.contrast = value.clamp(MIN_CONTRAST, MAX_CONTRAST);
        }
        if let Some(value) = style.roundness {
            self.roundness = value.clamp(MIN_ROUNDNESS, MAX_ROUNDNESS);
        }
        if let Some(value) = style.type_scale {
            self.type_scale = value.clamp(MIN_TYPE_SCALE, MAX_TYPE_SCALE);
        }
        if let Some(value) = style.density {
            self.density = value.clamp(MIN_DENSITY, MAX_DENSITY);
        }
        if let Some(value) = style.hairline {
            self.hairline = value.clamp(MIN_HAIRLINE, MAX_HAIRLINE);
        }
        if let Some(value) = style.stroke_emphasis {
            self.stroke_emphasis = value.clamp(MIN_STROKE_EMPHASIS, MAX_STROKE_EMPHASIS);
        }
        if let Some(value) = &style.font_family {
            self.font_family = value.trim().to_owned();
        }
        if let Some(value) = &style.font_family_mono {
            self.font_family_mono = value.trim().to_owned();
        }
        if let Some(value) = style.font_weight {
            self.font_weight = value.clamp(MIN_FONT_WEIGHT, MAX_FONT_WEIGHT);
        }
    }

    /// Whether the three seed pickers still say what the selected theme's
    /// current variant projects onto them.
    ///
    /// This is what decides "edited away from the theme", and it works for a
    /// ramp as well as for three seeds because it compares the *projection*
    /// rather than the storage: the background, the accent and slot 0A are
    /// exactly the three things the pickers can express about either form.
    pub(crate) fn seeds_match_theme(&self) -> bool {
        let Some(theme) = self.definition() else {
            return false;
        };
        let (base, accent, alert) = project(theme.variant(self.wants_dark()));
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        same(&base, &self.base) && same(&accent, &self.accent) && same(&alert, &self.alert)
    }

    /// Projects the colours in force back onto the three seed pickers.
    pub(crate) fn sync_seeds(&mut self) {
        let (base, accent, alert) = project(self.colors());
        self.base = base;
        self.accent = accent;
        self.alert = alert;
    }

    /// Writes the current appearance out as a theme file under `name`.
    ///
    /// The variant being edited replaces the one on that side of the light/
    /// dark line; **the other side is carried over from whatever theme was
    /// selected**, so "pick Dracula, warm the accent, save as Mine" keeps
    /// Alucard for light rather than throwing it away and deriving a worse
    /// one. Every scalar on the page goes into the file, which is what makes
    /// a theme a font and a density as well as a palette.
    pub(crate) fn save_theme(&mut self, name: &str) -> Result<(), ValidationError> {
        self.save_theme_in(&file::themes_dir(), name)
    }

    /// The directory is a parameter for the reason `file::write_to` states:
    /// `themes_dir()` reads an environment variable and a test that sets one
    /// races every other test in the binary.
    pub(crate) fn save_theme_in(
        &mut self,
        directory: &Path,
        name: &str,
    ) -> Result<(), ValidationError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ValidationError::EmptyThemeName);
        }
        if builtins::is_builtin(name) || name == wal::WALLPAPER_THEME {
            return Err(ValidationError::ReservedThemeName);
        }
        let base = self.definition();
        let colors = self.colors();
        let dark_side = self.wants_dark();
        let mut definition = ThemeDefinition {
            name: name.to_owned(),
            description: base
                .as_ref()
                .map(|theme| theme.description.clone())
                .unwrap_or_default(),
            dark: base.as_ref().and_then(|theme| theme.dark),
            light: base.as_ref().and_then(|theme| theme.light),
            style: ThemeStyle {
                roundness: Some(self.roundness),
                hairline: Some(self.hairline),
                stroke_emphasis: Some(self.stroke_emphasis),
                font_family: Some(self.font_family.clone()),
                font_family_mono: Some(self.font_family_mono.clone()),
                type_scale: Some(self.type_scale),
                font_weight: Some(self.font_weight),
                density: Some(self.density),
                contrast: Some(self.contrast),
            },
        };
        if dark_side {
            definition.dark = Some(colors);
        } else {
            definition.light = Some(colors);
        }
        file::write_to(directory, &definition)
            .map_err(|error| ValidationError::ThemeNotWritten(error.to_string()))?;
        catalog::refresh();
        self.theme = name.to_owned();
        // The file now says what the pickers say, so the edit is no longer an
        // edit *away* from anything.
        self.customized = false;
        Ok(())
    }

    /// Deletes a user theme's file. Built-ins and the wallpaper row are
    /// silently left alone, since the UI only offers Remove on user rows.
    pub(crate) fn remove_theme(&mut self, name: &str) -> Result<(), ValidationError> {
        self.remove_theme_in(&file::themes_dir(), name)
    }

    pub(crate) fn remove_theme_in(
        &mut self,
        directory: &Path,
        name: &str,
    ) -> Result<(), ValidationError> {
        if catalog::is_protected(name) {
            return Ok(());
        }
        file::remove_in(directory, name)
            .map_err(|error| ValidationError::ThemeNotWritten(error.to_string()))?;
        catalog::refresh();
        if self.theme == name {
            // The colours stay and the name goes, which is what the page shows
            // as Custom. Deleting the file you were wearing should not repaint
            // the program, so the seeds are captured before the name goes.
            self.sync_seeds();
            self.theme = String::new();
            self.customized = false;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct GestureSettings {
    /// Gesture id -> `GestureMod::to_string()` text. Sparse for the same
    /// reason `ShortcutSettings` is: `gestures::GESTURES` can grow without a
    /// settings migration.
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct ShortcutSettings {
    /// Action id -> `KeyChord::display()` text. Only entries that differ
    /// from the registry default are stored, so `actions::ACTIONS` can grow
    /// without a settings migration.
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, String>,
    /// How a Super press is read before it is matched against the bindings
    /// above. It is a fact about this machine's keyboard and window
    /// manager rather than about any one binding, which is why it sits
    /// beside the overrides instead of inside them: the stored chords do
    /// not change when it does.
    #[serde(default)]
    pub super_key: SuperKeyMode,
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

/// Everything the MIDI page owns that is the user's rather than the song's.
///
/// A control map lives in the *project*, because its targets name channels of
/// one song (`CONTROL_SURFACES.md`). What a learn gesture *does* is not: it is
/// a fact about the desk in the room, and it should be the same in every song
/// opened on this machine.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct MidiSettings {
    /// Whether a learn gesture binds the controller it heard, so that only
    /// that one drives the mapping.
    ///
    /// Off by default, which is the right answer for one controller and the
    /// wrong one for several: with it off, replacing a keyboard keeps every
    /// mapping, and a second keyboard sending CC 7 moves the same fader. A
    /// studio with a desk *and* a keyboard turns it on, and the two stop
    /// colliding. Guessing the multi-controller case for everybody would mean
    /// a mapping that silently stops working when a device is plugged into a
    /// different socket and comes back under another name.
    #[serde(default)]
    pub learn_binds_port: bool,
}

/// Where the panes were left.
///
/// UI state rather than project state, which is the whole reason it is here
/// and not in `PROJECT_FORMAT.md`: a song must not carry a window layout, and
/// the arrangement you work in should greet you whichever song you open.
///
/// **Zoom is deliberately absent.** It is a glance, not an arrangement, and
/// starting up with one pane filling the window and no memory of asking for
/// it is a worse first second than any amount of fidelity is worth.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct LayoutSettings {
    /// Which slot each view lives in, indexed by view id -- steps, mixer,
    /// devices, notes, playlist -- where 0 is the main top pane, 1 the split
    /// and 2 the dock.
    #[serde(default = "default_view_slots")]
    pub view_slots: Vec<i32>,
    /// The active view in each slot, indexed by slot, or -1 for a slot
    /// holding nothing.
    #[serde(default = "default_slot_active")]
    pub slot_active: Vec<i32>,
    #[serde(default = "default_split_fraction")]
    pub split_fraction: f32,
    #[serde(default = "default_steps_dock_height")]
    pub steps_dock_height: f32,
    #[serde(default = "default_steps_dock_height")]
    pub mixer_dock_height: f32,
    #[serde(default = "default_notes_dock_height")]
    pub notes_dock_height: f32,
    #[serde(default = "default_playlist_dock_height")]
    pub playlist_dock_height: f32,
    #[serde(default = "default_true")]
    pub bottom_pane_visible: bool,
    #[serde(default)]
    pub sidebar_visible: bool,
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,
    /// The channel sidebar on the left, which is a different panel from the
    /// browser on the right and remembers its own width. Both default to
    /// hidden: a first run should show the work, not the chrome around it.
    #[serde(default)]
    pub channel_sidebar_visible: bool,
    #[serde(default = "default_channel_sidebar_width")]
    pub channel_sidebar_width: f32,
}

pub(crate) const VIEW_COUNT: usize = 5;
pub(crate) const SLOT_COUNT: usize = 3;
const MIN_DOCK_HEIGHT: f32 = 140.0;
const MAX_DOCK_HEIGHT: f32 = 2000.0;
const MIN_SPLIT_FRACTION: f32 = 0.15;
const MAX_SPLIT_FRACTION: f32 = 0.85;
const MIN_SIDEBAR_WIDTH: f32 = 180.0;
const MAX_SIDEBAR_WIDTH: f32 = 400.0;

fn default_view_slots() -> Vec<i32> {
    vec![0, 0, 2, 2, 2]
}
fn default_slot_active() -> Vec<i32> {
    vec![0, -1, 2]
}
fn default_split_fraction() -> f32 {
    0.5
}
fn default_steps_dock_height() -> f32 {
    300.0
}
fn default_notes_dock_height() -> f32 {
    410.0
}
fn default_playlist_dock_height() -> f32 {
    376.0
}
fn default_sidebar_width() -> f32 {
    260.0
}
fn default_channel_sidebar_width() -> f32 {
    220.0
}

impl Default for LayoutSettings {
    fn default() -> Self {
        Self {
            view_slots: default_view_slots(),
            slot_active: default_slot_active(),
            split_fraction: default_split_fraction(),
            steps_dock_height: default_steps_dock_height(),
            mixer_dock_height: default_steps_dock_height(),
            notes_dock_height: default_notes_dock_height(),
            playlist_dock_height: default_playlist_dock_height(),
            bottom_pane_visible: true,
            sidebar_visible: false,
            sidebar_width: default_sidebar_width(),
            channel_sidebar_visible: false,
            channel_sidebar_width: default_channel_sidebar_width(),
        }
    }
}

impl LayoutSettings {
    /// An arrangement that cannot be worked in is worse than no memory of one,
    /// so anything self-contradictory falls back to the default *whole*
    /// arrangement rather than being half-applied. `settings.toml` is a file a
    /// user may edit, and the failure this guards is a window with no pane in
    /// it and no way to get one back.
    pub(crate) fn sanitized(mut self) -> Self {
        let sane = self.view_slots.len() == VIEW_COUNT
            && self.view_slots.iter().all(|&s| (0..SLOT_COUNT as i32).contains(&s))
            && self.slot_active.len() == SLOT_COUNT
            // The main slot must hold something, or there is no pane left to
            // move anything back into.
            && self.view_slots.contains(&0)
            // A slot's active view must actually live in that slot.
            && self.slot_active.iter().enumerate().all(|(slot, &view)| {
                view == -1
                    || (usize::try_from(view).is_ok_and(|v| v < VIEW_COUNT)
                        && self.view_slots[view as usize] == slot as i32)
            })
            // A slot that holds views must have one of them active, and the
            // main slot may never be empty.
            && (0..SLOT_COUNT).all(|slot| {
                let holds = self.view_slots.contains(&(slot as i32));
                holds == (self.slot_active[slot] != -1)
            });
        if !sane {
            return Self {
                view_slots: default_view_slots(),
                slot_active: default_slot_active(),
                ..self
            }
            .clamped();
        }
        self = self.clamped();
        self
    }

    fn clamped(mut self) -> Self {
        self.split_fraction = self
            .split_fraction
            .clamp(MIN_SPLIT_FRACTION, MAX_SPLIT_FRACTION);
        for height in [
            &mut self.steps_dock_height,
            &mut self.mixer_dock_height,
            &mut self.notes_dock_height,
            &mut self.playlist_dock_height,
        ] {
            *height = height.clamp(MIN_DOCK_HEIGHT, MAX_DOCK_HEIGHT);
        }
        self.sidebar_width = self
            .sidebar_width
            .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        self.channel_sidebar_width = self
            .channel_sidebar_width
            .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        self
    }
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
    pub gestures: GestureSettings,
    #[serde(default)]
    pub browser: BrowserSettings,
    #[serde(default)]
    pub midi: MidiSettings,
    #[serde(default)]
    pub layout: LayoutSettings,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: GeneralSettings::default(),
            appearance: AppearanceSettings::default(),
            audio: AudioSettings::default(),
            shortcuts: ShortcutSettings::default(),
            gestures: GestureSettings::default(),
            browser: BrowserSettings::default(),
            midi: MidiSettings::default(),
            layout: LayoutSettings::default(),
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
        let mut appearance = settings.appearance.clone();
        migrate_user_schemes(&mut appearance);
        let appearance = appearance.validated().map_err(SettingsError::Validation)?;
        // The layout is *sanitized* rather than validated: a bad palette is
        // worth refusing the whole file over, because the alternative is a
        // window the user cannot read. A bad arrangement is not -- it falls
        // back to the default panes and keeps everything else in the file.
        let layout = settings.layout.clone().sanitized();
        Ok(Self {
            appearance,
            layout,
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

/// Turns the `user-schemes` array an older config carries into theme files,
/// once.
///
/// **The list is cleared only if every write succeeded.** A themes directory
/// that cannot be written -- a read-only home, a full disk -- is a real state,
/// and losing somebody's saved schemes to it would be the worst outcome
/// available here. If the migration cannot finish, the array stays in
/// `settings.toml` and the next launch tries again.
///
/// A scheme whose name already has a theme file is not overwritten: the file
/// is the newer of the two by construction, because writing one is what
/// clears the array.
fn migrate_user_schemes(appearance: &mut AppearanceSettings) {
    if appearance.user_schemes.is_empty() {
        return;
    }
    let existing: Vec<String> = file::load_all().0.into_iter().map(|t| t.name).collect();
    let mut all_written = true;
    for scheme in &appearance.user_schemes {
        if existing.contains(&scheme.name) {
            continue;
        }
        if let Err(error) = file::write(&scheme.definition()) {
            eprintln!(
                "mooloop: could not migrate the saved scheme {} into a theme file: {error}",
                scheme.name
            );
            all_written = false;
        }
    }
    if all_written {
        appearance.user_schemes.clear();
        catalog::refresh();
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

/// Directory holding one subdirectory of effect presets per [`EffectKind`],
/// e.g. `presets/effects/delay/`.
///
/// One directory a kind rather than one flat `presets/effects/`: a filter
/// preset loaded into a reverb row is nonsense, and the directory layout is
/// the cheapest place to make that impossible to offer.
pub(crate) fn effect_presets_dir(kind: EffectKind) -> PathBuf {
    config_dir()
        .join("presets/effects")
        .join(effect_kind_slug(kind))
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

/// Directory mooloop keeps its own *data* in, as opposed to its settings:
/// `$MOOLOOP_DATA_DIR`, or `$XDG_DATA_HOME/mooloop` /
/// `~/.local/share/mooloop` on Linux (MOO-75).
///
/// Only Linux separates the two. macOS and Windows keep application data
/// beside its settings already, so there it is [`config_dir`]. So does a
/// config directory given by `$MOOLOOP_CONFIG_DIR` with no data directory
/// given, because that variable is how tests and a second instance keep to
/// themselves, and data that escaped it into the real home would not be.
pub(crate) fn data_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("MOOLOOP_DATA_DIR") {
        return PathBuf::from(path);
    }
    if std::env::var_os("MOOLOOP_CONFIG_DIR").is_some()
        || cfg!(any(target_os = "windows", target_os = "macos"))
    {
        return config_dir();
    }
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("mooloop");
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into()))
        .join(".local/share/mooloop")
}

/// The shared recordings folder: every take is written here first, and a
/// save copies it into the song's own `recordings/`.
///
/// **Takes are data, not configuration**, so since 2026-09-23 they live under
/// [`data_dir`] (MOO-75). The folder keeps the name `recordings` wherever it
/// is: that name is how a save tells a take from a sample
/// (`mooloop_project::RECORDINGS_DIR`).
pub(crate) fn recordings_dir() -> PathBuf {
    data_dir().join("recordings")
}

/// Where takes were written until 2026-09-23, under the config directory.
/// Startup moves them to [`recordings_dir`] and leaves a link here.
pub(crate) fn legacy_recordings_dir() -> PathBuf {
    config_dir().join("recordings")
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
        // On-disk directory name, frozen at the device's old ML-1 spelling.
        // Renaming it would orphan every generator preset already saved
        // under presets/generators/mlm1/.
        DeviceKind::MlM1 => "ml1",
        DeviceKind::MlP8 => "mlp8",
        DeviceKind::Ds01 => "ds01",
        DeviceKind::AuxIn => "aux_in",
    }
}

/// On-disk directory names for effect presets. These are frozen the moment
/// anything ships against them, for the same reason `ml1` is above: renaming
/// one would orphan every preset already saved under it. Plain snake_case of
/// the kind, chosen to still be right in a year.
fn effect_kind_slug(kind: EffectKind) -> &'static str {
    match kind {
        EffectKind::Eq => "eq",
        EffectKind::Modulation => "modulation",
        EffectKind::Filter => "filter",
        EffectKind::Preamp => "preamp",
        EffectKind::Drive => "drive",
        EffectKind::Bitcrush => "bitcrush",
        EffectKind::Delay => "delay",
        EffectKind::Reverb => "reverb",
        EffectKind::Plate => "plate",
        EffectKind::Gate => "gate",
        EffectKind::Compressor => "compressor",
        EffectKind::Limiter => "limiter",
        EffectKind::Buffer => "buffer",
        EffectKind::Chain => "chain",
        EffectKind::Layer => "layer",
    }
}


// The colour primitives and the palette derivation moved to `crate::theme`
// when a theme became a sixteen-colour ramp: the ramp, the wallpaper
// importers, the light/dark derivation and the swatch palette all need the
// same arithmetic, and none of them is a setting. They are re-exported here
// because this module is still what the rest of the crate asks for a palette.

#[derive(Debug)]
pub(crate) enum ValidationError {
    InvalidBase,
    InvalidAccent,
    InvalidAlert,
    LowContrast,
    EmptyThemeName,
    ReservedThemeName,
    ThemeNotWritten(String),
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBase => write!(f, "Enter a base color as #RRGGBB"),
            Self::InvalidAccent => write!(f, "Enter an accent as #RRGGBB"),
            Self::InvalidAlert => write!(f, "Enter an alert color as #RRGGBB"),
            Self::LowContrast => write!(f, "Accent needs more contrast against the base color"),
            Self::EmptyThemeName => write!(f, "Name the theme before saving it"),
            Self::ReservedThemeName => write!(f, "That name belongs to a built-in theme"),
            Self::ThemeNotWritten(reason) => write!(f, "Could not write the theme: {reason}"),
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

    /// A Custom appearance: the three seeds, and no theme selected.
    ///
    /// `theme` has to be empty or the seeds are shadowed -- a selected theme
    /// is what the colours come from, and the pickers are a projection of it.
    /// That is the whole shape of the model and this helper exists to state
    /// it once.
    fn appearance(base: &str, accent: &str, alert: &str) -> AppearanceSettings {
        AppearanceSettings {
            theme: String::new(),
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
    fn every_builtin_theme_validates_in_both_modes() {
        for theme in builtins::all() {
            for mode in [Mode::Dark, Mode::Light] {
                let mut settings = AppearanceSettings {
                    mode: mode.name().to_owned(),
                    ..AppearanceSettings::default()
                };
                settings.apply_theme(&theme);
                let validated = settings.validated();
                assert!(
                    validated.is_ok(),
                    "built-in theme {} fails validation in {} mode",
                    theme.name,
                    mode.name()
                );
                assert_eq!(validated.unwrap().theme, theme.name);
            }
        }
    }

    /// Selecting a theme and flipping the mode is the whole feature, and what
    /// it has to do is change the colours without changing anything else the
    /// user set.
    #[test]
    fn the_mode_picks_the_variant_and_leaves_the_rest_alone() {
        let mut settings = AppearanceSettings {
            type_scale: 1.5,
            density: 1.2,
            ..AppearanceSettings::default()
        };
        settings.apply_theme(&builtins::find("Nord").unwrap());
        settings.mode = Mode::Dark.name().to_owned();
        let dark = settings.palette();
        settings.mode = Mode::Light.name().to_owned();
        settings.sync_seeds();
        let light = settings.palette();
        assert!(dark.background.is_dark());
        assert!(!light.background.is_dark());
        // Polar Night and Snow Storm, both written down rather than derived.
        assert_eq!(dark.background.to_hex(), "#2E3440");
        assert_eq!(light.background.to_hex(), "#ECEFF4");
        // And a type scale set for somebody's eyes survives a theme change.
        assert_eq!(settings.type_scale, 1.5);
        assert_eq!(settings.density, 1.2);
    }

    /// The swatch palette follows the colourscheme, which is the ask this
    /// work came from. Two themes, two palettes, eleven of each.
    #[test]
    fn the_channel_swatches_follow_the_theme() {
        let mut nord = AppearanceSettings::default();
        nord.apply_theme(&builtins::find("Nord").unwrap());
        let mut dracula = AppearanceSettings::default();
        dracula.apply_theme(&builtins::find("Dracula").unwrap());
        let (a, b) = (nord.swatches(), dracula.swatches());
        assert_eq!(a.len(), 11);
        assert_eq!(b.len(), 11);
        assert_ne!(a, b, "two schemes produced one swatch palette");
        // Nord's aurora red is in Nord's palette and not in Dracula's.
        assert!(a.iter().any(|c| c.to_hex() == "#BF616A"));
        assert!(!b.iter().any(|c| c.to_hex() == "#BF616A"));
    }

    /// The default theme's swatch row is the hand-picked eleven it replaced,
    /// so nobody's existing channel colours stopped being offered.
    #[test]
    fn the_default_themes_swatches_still_include_the_old_table() {
        let settings = AppearanceSettings::default();
        let swatches: Vec<String> = settings.swatches().into_iter().map(Rgb::to_hex).collect();
        for hex in ["#EF4444", "#F97316", "#EAB308", "#22C55E", "#14B8A6", "#EC4899", "#A16207"] {
            assert!(swatches.contains(&hex.to_owned()), "{hex} is gone: {swatches:?}");
        }
    }

    #[test]
    fn derives_a_readable_ramp_from_a_light_base() {
        // A light base has to flip the ramp: text goes dark, surfaces go
        // darker than the background rather than lighter.
        use crate::theme::color::{contrast_ratio, relative_luminance};
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
        let ratio = crate::theme::color::contrast_ratio;
        assert!(ratio(wide.border, wide.background) > ratio(tight.border, tight.background));
    }

    /// The arrangement a hand-edited file can ask for, and the one the
    /// window can actually be worked in, are not the same set. Every case
    /// here would leave a pane the user cannot recover: an empty main slot,
    /// a slot whose active view lives somewhere else, a slot holding views
    /// with none of them showing.
    #[test]
    fn an_unusable_arrangement_falls_back_to_the_default_one() {
        let default = LayoutSettings::default();
        let cases = [
            (
                "main slot empty",
                LayoutSettings {
                    view_slots: vec![2, 2, 2, 2, 2],
                    slot_active: vec![-1, -1, 2],
                    ..default.clone()
                },
            ),
            (
                "active view lives in another slot",
                LayoutSettings {
                    view_slots: vec![0, 0, 2, 2, 2],
                    slot_active: vec![2, -1, 2],
                    ..default.clone()
                },
            ),
            (
                "slot holds views but shows none",
                LayoutSettings {
                    view_slots: vec![0, 0, 2, 2, 2],
                    slot_active: vec![0, -1, -1],
                    ..default.clone()
                },
            ),
            (
                "slot index out of range",
                LayoutSettings {
                    view_slots: vec![0, 0, 2, 2, 7],
                    slot_active: vec![0, -1, 2],
                    ..default.clone()
                },
            ),
            (
                "wrong number of views",
                LayoutSettings {
                    view_slots: vec![0, 0, 2],
                    slot_active: vec![0, -1, 2],
                    ..default.clone()
                },
            ),
        ];
        for (name, broken) in cases {
            let fixed = broken.sanitized();
            assert_eq!(
                fixed.view_slots, default.view_slots,
                "{name}: must fall back to the default arrangement"
            );
            assert_eq!(
                fixed.slot_active, default.slot_active,
                "{name}: must fall back to the default active views"
            );
        }
    }

    /// A coherent arrangement survives, and only the numbers that have bounds
    /// are pulled into them -- falling back wholesale on a merely silly
    /// divider position would throw away an arrangement that was fine.
    #[test]
    fn a_usable_arrangement_survives_and_only_its_numbers_are_clamped() {
        let moved = LayoutSettings {
            // The mixer in the dock and the playlist split off the top: the
            // arrangement Adam asked for by name.
            view_slots: vec![0, 2, 2, 2, 1],
            slot_active: vec![0, 4, 1],
            split_fraction: 9.0,
            notes_dock_height: 5.0,
            sidebar_width: 4000.0,
            ..LayoutSettings::default()
        };
        let fixed = moved.clone().sanitized();
        assert_eq!(fixed.view_slots, moved.view_slots, "the arrangement holds");
        assert_eq!(fixed.slot_active, moved.slot_active, "so do its active views");
        assert_eq!(fixed.split_fraction, MAX_SPLIT_FRACTION);
        assert_eq!(fixed.notes_dock_height, MIN_DOCK_HEIGHT);
        assert_eq!(fixed.sidebar_width, MAX_SIDEBAR_WIDTH);
    }

    #[test]
    fn saves_and_removes_a_theme() {
        let directory = tempfile::tempdir().unwrap();
        let mut settings = appearance("#101014", "#22D3EE", "#F97316");
        settings.type_scale = 1.25;
        settings.save_theme_in(directory.path(), "  Mine  ").unwrap();
        assert_eq!(settings.theme, "Mine");

        let (saved, _) = file::load_all_in(directory.path());
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Mine");
        assert_eq!(saved[0].style.type_scale, Some(1.25));
        assert_eq!(
            saved[0].variant(true).ramp().accent().to_hex(),
            "#22D3EE",
            "the seeds in force should be what got saved"
        );

        // Re-saving the same name replaces rather than duplicating.
        settings.accent = "#84CC16".to_owned();
        settings.theme = String::new();
        settings.save_theme_in(directory.path(), "Mine").unwrap();
        let (saved, _) = file::load_all_in(directory.path());
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].variant(true).ramp().accent().to_hex(), "#84CC16");

        assert!(matches!(
            settings.save_theme_in(directory.path(), "Nord"),
            Err(ValidationError::ReservedThemeName)
        ));
        assert!(matches!(
            settings.save_theme_in(directory.path(), "   "),
            Err(ValidationError::EmptyThemeName)
        ));

        settings.remove_theme_in(directory.path(), "Mine").unwrap();
        assert!(file::load_all_in(directory.path()).0.is_empty());
        // Deleting the theme you are wearing keeps the colours and drops the
        // name, which the page shows as Custom.
        assert_eq!(settings.theme, "");
        assert_eq!(settings.accent, "#84CC16");
    }

    /// Saving a *tweak* to a two-variant theme keeps the variant you were not
    /// looking at. This is why `customized` exists as a flag beside the theme
    /// name rather than being spelled as an empty name: clearing the name on
    /// the first keystroke would throw away half of Dracula the first time
    /// somebody warmed its accent, and nothing on screen would say so.
    #[test]
    fn saving_a_tweak_keeps_the_other_variant() {
        let directory = tempfile::tempdir().unwrap();
        let mut settings = AppearanceSettings {
            mode: Mode::Dark.name().to_owned(),
            ..AppearanceSettings::default()
        };
        settings.apply_theme(&builtins::find("Dracula").unwrap());
        assert!(!settings.customized);

        // Warm the accent, the way the Appearance page's preview does.
        settings.accent = "#FF9955".to_owned();
        assert!(!settings.seeds_match_theme());
        settings.customized = !settings.seeds_match_theme();
        // The colours drawn are now the seeds, and the name is still Dracula.
        assert_eq!(settings.theme, "Dracula");
        assert_eq!(settings.palette().accent.to_hex(), "#FF9955");

        settings.save_theme_in(directory.path(), "Mine").unwrap();
        assert!(!settings.customized, "the file now says what the pickers say");

        let saved = file::load_all_in(directory.path()).0.pop().unwrap();
        assert!(saved.authored(true));
        assert!(saved.authored(false), "Alucard was thrown away");
        assert_eq!(saved.variant(true).ramp().accent().to_hex(), "#FF9955");
        assert_eq!(
            saved.variant(false).ramp().slot(0).to_hex(),
            builtins::find("Dracula")
                .unwrap()
                .variant(false)
                .ramp()
                .slot(0)
                .to_hex(),
            "the light variant should be Alucard's, untouched"
        );
    }

    /// Selecting a theme after an edit puts the theme back: `customized` is
    /// cleared, and nothing of the edit survives to leak into the next save.
    #[test]
    fn selecting_a_theme_ends_the_edit() {
        let mut settings = AppearanceSettings::default();
        settings.apply_theme(&builtins::find("Nord").unwrap());
        settings.accent = "#FF0000".to_owned();
        settings.customized = true;
        settings.apply_theme(&builtins::find("Gruvbox").unwrap());
        assert!(!settings.customized);
        assert_eq!(settings.theme, "Gruvbox");
        assert_eq!(settings.accent, "#83A598");
        assert!(settings.seeds_match_theme());
    }

    /// An older config's `user-schemes` array becomes theme files once, and
    /// the array goes away. A scheme somebody saved in 2026 has to still be
    /// in the list in 2027.
    #[test]
    fn saved_schemes_migrate_into_theme_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut appearance = AppearanceSettings {
            user_schemes: vec![ThemeScheme {
                name: "Adam's".to_owned(),
                base: "#101014".to_owned(),
                accent: "#22D3EE".to_owned(),
                alert: "#F97316".to_owned(),
            }],
            ..AppearanceSettings::default()
        };
        // The real `migrate_user_schemes` writes to `themes_dir()`; this is
        // the same two steps against a directory a test can own.
        for scheme in &appearance.user_schemes {
            file::write_to(directory.path(), &scheme.definition()).unwrap();
        }
        appearance.user_schemes.clear();

        let (themes, warnings) = file::load_all_in(directory.path());
        assert!(warnings.is_empty());
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Adam's");
        assert_eq!(themes[0].variant(true).ramp().accent().to_hex(), "#22D3EE");
        // And it answers Light, which a scheme could not.
        assert!(!themes[0].variant(false).ramp().is_dark());
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
            // Every scalar off its default, for the reason the layout section
            // below gives: a round trip through a default value passes even
            // when the field is never written at all. `mode` is `light`
            // rather than `system` on purpose -- `system` asks the desktop,
            // and a test that asks the desktop is a test whose result depends
            // on the machine it runs on.
            appearance: AppearanceSettings {
                mode: "light".to_owned(),
                type_scale: 1.25,
                density: 1.1,
                font_family: "Iosevka Aile".to_owned(),
                font_family_mono: "Iosevka".to_owned(),
                font_weight: 500,
                hairline: 0.0,
                stroke_emphasis: 3.0,
                ..appearance("#151617", "#F59E0B", "#38BDF8")
            }
            .validated()
            .unwrap(),
            // Both drivers' sections, and neither at its default, so the one
            // this build does not use has to survive the trip as well.
            audio: AudioSettings {
                driver: AudioDriverKind::Jack,
                jack: DriverSettings {
                    output_port_l: Some("Carla:audio-in1".to_owned()),
                    output_port_r: Some("Carla:audio-in2".to_owned()),
                    earlier_outputs: vec![(
                        "Built-in Audio Analog Stereo:playback_FL".to_owned(),
                        "Built-in Audio Analog Stereo:playback_FR".to_owned(),
                    )],
                    buffer_size: Some(256),
                    auto_reconnect: false,
                },
                core_audio: DriverSettings {
                    output_port_l: Some("coreaudio:BuiltInSpeakerDevice#1".to_owned()),
                    output_port_r: Some("coreaudio:BuiltInSpeakerDevice#2".to_owned()),
                    earlier_outputs: Vec::new(),
                    buffer_size: Some(512),
                    auto_reconnect: true,
                },
            },
            shortcuts: ShortcutSettings {
                overrides: [("edit.undo".to_owned(), "Ctrl+Alt+Z".to_owned())]
                    .into_iter()
                    .collect(),
                // Not the default, for the reason the `midi` section below
                // gives: a round trip through a default value passes even
                // when the field is never written at all.
                super_key: SuperKeyMode::AsAlt,
            },
            gestures: GestureSettings {
                overrides: [("gesture.copy-drag".to_owned(), "Alt".to_owned())]
                    .into_iter()
                    .collect(),
            },
            browser: BrowserSettings {
                locations: vec![PathBuf::from("/sounds/one-shots")],
            },
            // Not the default, for the reason the layout section below gives:
            // a round trip through a default value passes even when the
            // section is never written at all.
            midi: MidiSettings {
                learn_binds_port: true,
            },
            // Deliberately not the default arrangement, and deliberately one
            // that `sanitized()` must leave alone: the mixer in the dock and
            // the playlist split off the top. A round trip through the
            // default would pass even if the section were never written.
            layout: LayoutSettings {
                view_slots: vec![0, 2, 2, 2, 1],
                slot_active: vec![0, 4, 1],
                split_fraction: 0.62,
                notes_dock_height: 380.0,
                sidebar_visible: true,
                ..LayoutSettings::default()
            },
        };
        expected.save_to(&path).unwrap();
        assert_eq!(UiSettings::load_from(&path).unwrap(), expected);
    }

    #[test]
    fn defaults_missing_layout_settings_for_existing_configs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 1\n[appearance]\npreset = 'mooloop'\naccent = '#84CC16'\n",
        )
        .unwrap();
        // A config written before panes could move opens on the default
        // arrangement rather than being refused.
        assert_eq!(
            UiSettings::load_from(&path).unwrap().layout,
            LayoutSettings::default()
        );
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
    fn defaults_missing_gesture_settings_for_existing_configs() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        fs::write(
            &path,
            "schema-version = 1\n[appearance]\npreset = 'mooloop'\naccent = '#84CC16'\n",
        )
        .unwrap();
        assert!(UiSettings::load_from(&path)
            .unwrap()
            .gestures
            .overrides
            .is_empty());
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
        assert!(audio.active().auto_reconnect);
        assert_eq!(audio.active().output_target(), None);
    }

    #[test]
    fn maps_the_builds_driver_settings_onto_engine_config() {
        let mut audio = AudioSettings::default();
        *audio.active_mut() = DriverSettings {
            output_port_l: Some("Carla:audio-in1".to_owned()),
            output_port_r: Some("Carla:audio-in2".to_owned()),
            earlier_outputs: vec![("usb:FL".to_owned(), "usb:FR".to_owned())],
            buffer_size: Some(512),
            auto_reconnect: true,
        };
        let config = audio.engine_config();
        assert_eq!(config.buffer_size, Some(512));
        assert_eq!(
            config.output_target,
            Some(("Carla:audio-in1".to_owned(), "Carla:audio-in2".to_owned()))
        );
        assert_eq!(config.earlier_outputs, [("usb:FL".to_owned(), "usb:FR".to_owned())]);
        assert!(config.auto_reconnect);
    }

    /// A pick pushes the output it replaces onto the earlier ones, and picking
    /// an earlier one again takes it back off -- the list the engine walks
    /// when the output playing goes away.
    #[test]
    fn picking_an_output_remembers_the_one_before() {
        let pair = |name: &str| (format!("{name}:FL"), format!("{name}:FR"));
        let mut driver = DriverSettings::default();
        driver.pick_output(pair("speakers"));
        assert_eq!(driver.output_target(), Some(pair("speakers")));
        assert!(driver.earlier_outputs.is_empty());

        driver.pick_output(pair("usb"));
        driver.pick_output(pair("headphones"));
        assert_eq!(driver.output_target(), Some(pair("headphones")));
        assert_eq!(driver.earlier_outputs, [pair("usb"), pair("speakers")]);

        driver.pick_output(pair("speakers"));
        assert_eq!(driver.output_target(), Some(pair("speakers")));
        assert_eq!(driver.earlier_outputs, [pair("headphones"), pair("usb")]);
    }

    /// The section the build does not use never reaches the engine, and is not
    /// written until it holds something.
    #[test]
    fn the_other_drivers_section_stays_out_of_the_way() {
        let mut audio = AudioSettings::default();
        let written = toml::to_string(&audio).unwrap();
        #[cfg(target_os = "macos")]
        let (unused, other) = ("jack", &mut audio.jack);
        #[cfg(not(target_os = "macos"))]
        let (unused, other) = ("core-audio", &mut audio.core_audio);
        assert!(!written.contains(unused), "{written}");
        other.buffer_size = Some(2048);
        assert_eq!(audio.engine_config().buffer_size, None);
    }

    /// A fresh install asks the driver for nothing: under JACK the buffer is
    /// the whole server's, and a default request re-sized it for every
    /// client on the machine on every launch.
    #[test]
    fn a_fresh_install_leaves_the_buffer_size_alone() {
        assert_eq!(DriverSettings::default().buffer_size, None);
        assert_eq!(AudioSettings::default().engine_config().buffer_size, None);
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

    /// The falloff name is the *only* thing that survives a restart, so it
    /// has to land on the same row it came from -- and it has to be the same
    /// length as the rate table it indexes, which is in `meter.rs` and cannot
    /// see this one.
    #[test]
    fn meter_falloff_names_round_trip_and_match_the_rate_table() {
        for (index, name) in METER_FALLOFFS.iter().enumerate() {
            assert_eq!(meter_falloff_index(name), index as i32);
            assert_eq!(meter_falloff_name(index as i32), *name);
        }
        assert_eq!(
            METER_FALLOFFS.len(),
            crate::meter::FALLOFF_DB_PER_SECOND.len(),
            "a name with no rate behind it, or a rate with no name to store it by"
        );
        // A hand-edited or pre-2026-09-15 config lands on the IEC rate.
        assert_eq!(meter_falloff_index("glacial"), 1);
        let settings = AppearanceSettings {
            meter_falloff: "glacial".to_owned(),
            ..AppearanceSettings::default()
        }
        .validated()
        .unwrap();
        assert_eq!(settings.meter_falloff, "standard");
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
        let shortcuts = UiSettings::load_from(&path).unwrap().shortcuts;
        assert!(shortcuts.overrides.is_empty());
        // Added 2026-09-20 to a section that already existed in the wild,
        // so a file written before it has to keep loading.
        assert_eq!(shortcuts.super_key, SuperKeyMode::Distinct);
    }
}
