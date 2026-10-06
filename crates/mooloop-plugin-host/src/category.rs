//! What kind of plugin a plugin says it is, beyond effect or instrument: the
//! browser's **Type** filter (`docs/plans/plugin-browser/04-filter-by-category.md`).
//!
//! A category is only ever what the plugin declares. CLAP plugins declare
//! standard feature strings (`clap/plugin-features.h`), which the scan keeps
//! in [`ScannedPlugin::features`](crate::scan::ScannedPlugin::features);
//! [`categories`] reads them through one table, so nothing above this crate
//! reads a feature string. VST3's subcategories (`Fx|EQ`) map to the same
//! enum when VST3 is hosted. AU reports no category. A plugin that declares
//! none has none, and is never guessed at from its name.

/// One kind of plugin, as the browser offers it. The order of the variants
/// is the order the browser lists them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PluginCategory {
    /// Equalizers.
    Eq,
    /// Compressors, limiters, gates, expanders, transient shapers, de-essers.
    Dynamics,
    /// Filters.
    Filter,
    /// Distortion, saturation and glitch.
    Distortion,
    /// Chorus, flanger, phaser, tremolo, frequency shifters.
    Modulation,
    /// Delays.
    Delay,
    /// Reverbs.
    Reverb,
    /// Pitch shifting and correction, phase vocoders.
    Pitch,
    /// Meters and analyzers.
    Analyzer,
    /// Utilities and restoration.
    Utility,
    /// Synthesizers.
    Synth,
    /// Samplers.
    Sampler,
    /// Drums and drum machines.
    Drums,
}

impl PluginCategory {
    /// Every category, in the order the browser lists them.
    pub const ALL: [Self; 13] = [
        Self::Eq,
        Self::Dynamics,
        Self::Filter,
        Self::Distortion,
        Self::Modulation,
        Self::Delay,
        Self::Reverb,
        Self::Pitch,
        Self::Analyzer,
        Self::Utility,
        Self::Synth,
        Self::Sampler,
        Self::Drums,
    ];

    /// The word the browser shows for it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Eq => "EQ",
            Self::Dynamics => "Dynamics",
            Self::Filter => "Filter",
            Self::Distortion => "Distortion",
            Self::Modulation => "Modulation",
            Self::Delay => "Delay",
            Self::Reverb => "Reverb",
            Self::Pitch => "Pitch",
            Self::Analyzer => "Analyzer",
            Self::Utility => "Utility",
            Self::Synth => "Synth",
            Self::Sampler => "Sampler",
            Self::Drums => "Drums",
        }
    }
}

/// CLAP's standard feature strings that name a category, and the category
/// each names. A feature string not listed here (`stereo`, `mono`,
/// `mixing`, `mastering`, `granular`, ...) says nothing about the category.
const CLAP_FEATURES: [(&str, PluginCategory); 27] = [
    ("equalizer", PluginCategory::Eq),
    ("compressor", PluginCategory::Dynamics),
    ("expander", PluginCategory::Dynamics),
    ("gate", PluginCategory::Dynamics),
    ("limiter", PluginCategory::Dynamics),
    ("transient-shaper", PluginCategory::Dynamics),
    ("deesser", PluginCategory::Dynamics),
    ("filter", PluginCategory::Filter),
    ("distortion", PluginCategory::Distortion),
    ("chorus", PluginCategory::Modulation),
    ("flanger", PluginCategory::Modulation),
    ("phaser", PluginCategory::Modulation),
    ("tremolo", PluginCategory::Modulation),
    ("frequency-shifter", PluginCategory::Modulation),
    ("delay", PluginCategory::Delay),
    ("reverb", PluginCategory::Reverb),
    ("pitch-shifter", PluginCategory::Pitch),
    ("pitch-correction", PluginCategory::Pitch),
    ("analyzer", PluginCategory::Analyzer),
    ("utility", PluginCategory::Utility),
    ("restoration", PluginCategory::Utility),
    ("synthesizer", PluginCategory::Synth),
    ("sampler", PluginCategory::Sampler),
    ("drum", PluginCategory::Drums),
    ("drum-machine", PluginCategory::Drums),
    ("glitch", PluginCategory::Distortion),
    ("phase-vocoder", PluginCategory::Pitch),
];

/// The categories a CLAP plugin's feature strings declare, in
/// [`PluginCategory::ALL`]'s order and each once. Empty when it declares
/// none.
pub fn categories(features: &[String]) -> Vec<PluginCategory> {
    let mut found: Vec<PluginCategory> = CLAP_FEATURES
        .iter()
        .filter(|(feature, _)| features.iter().any(|declared| declared == feature))
        .map(|(_, category)| *category)
        .collect();
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn standard_clap_features_name_their_category() {
        assert_eq!(categories(&features(&["audio-effect", "equalizer", "stereo"])), [PluginCategory::Eq]);
        assert_eq!(categories(&features(&["audio-effect", "limiter"])), [PluginCategory::Dynamics]);
        assert_eq!(categories(&features(&["instrument", "synthesizer"])), [PluginCategory::Synth]);
        assert_eq!(categories(&features(&["instrument", "drum-machine"])), [PluginCategory::Drums]);
    }

    #[test]
    fn a_plugin_declaring_two_categories_is_in_both_once_each() {
        assert_eq!(
            categories(&features(&["reverb", "delay", "compressor", "gate"])),
            [PluginCategory::Dynamics, PluginCategory::Delay, PluginCategory::Reverb]
        );
    }

    #[test]
    fn a_plugin_declaring_no_category_has_none() {
        assert!(categories(&features(&["audio-effect", "stereo", "mixing"])).is_empty());
        assert!(categories(&[]).is_empty());
    }

    #[test]
    fn every_category_has_a_feature_that_names_it() {
        for category in PluginCategory::ALL {
            assert!(
                CLAP_FEATURES.iter().any(|(_, named)| *named == category),
                "{category:?} can never be declared"
            );
        }
    }
}
