//! What a song says about a hosted plugin: the neutral contract
//! (`docs/plans/plugin-hosting/02-the-neutral-contract.md`).
//!
//! Nothing here loads, runs or scans a plugin. These are the types a saved
//! song carries, so they live beside the other project types, and no plugin
//! format's own types appear in them: `crates/mooloop-plugin-host` is the
//! only crate that names `clack` (a test in `tests/plugin_formats_stay_out.rs`
//! holds that line), and everything above it sees only what is below.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Which plugin standard a plugin speaks. Saved as a lower-case tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    Clap,
    Vst3,
    Au,
}

/// What a song remembers about the plugin a device runs.
///
/// Enough to find it again and to name it when it cannot be found. The file
/// path is deliberately **not** here: a song moved to another machine should
/// find the plugin by `id` through that machine's scan cache, and a path
/// would be wrong there more often than it was right.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRef {
    pub format: PluginFormat,
    /// The plugin's own identifier. For CLAP, its reverse-DNS id.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub vendor: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
}

/// One parameter as the plugin last reported it.
///
/// Not a [`crate::ParamDescriptor`], whose names are `&'static str`: these
/// are owned, because they come from a plugin at runtime. `id` is the
/// plugin's own, sparse and arbitrary -- `4_000_000_000` is as legal as `7`
/// -- and it is what an address's `param` holds for a plugin
/// (`ParamOwner::PluginParam`, Adam 2026-09-23, MOO-74). Nothing may size an
/// array by it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginParamInfo {
    pub id: u32,
    pub name: String,
    /// The plugin's grouping path ("Filter/Envelope"), empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// `Some(n)` for a parameter with `n` discrete positions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stepped: Option<u16>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub automatable: bool,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub modulatable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One named piece of a plugin's saved state.
///
/// The bytes are the plugin's own and mooloop never reads inside them: the
/// plugin versions its own state, which is why a plugin needs no format
/// migration here (`00-status.md`, "The format-migration question is
/// closed").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginStateChunk {
    /// Which piece this is. CLAP writes one chunk, `"clap"`; VST3 writes a
    /// component and a controller chunk.
    pub tag: String,
    pub data: Vec<u8>,
}

/// A plugin's saved state, in memory.
///
/// A list of chunks rather than one blob, because VST3 saves its component
/// and its controller separately. The format is not repeated here: the
/// [`PluginRef`] beside it in [`PluginSlotState`] already says it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginState {
    pub chunks: Vec<PluginStateChunk>,
}

impl PluginState {
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// The bytes this state holds, for the undo history's budget.
    pub fn heap_bytes(&self) -> usize {
        self.chunks.capacity() * std::mem::size_of::<PluginStateChunk>()
            + self
                .chunks
                .iter()
                .map(|chunk| chunk.tag.capacity() + chunk.data.capacity())
                .sum::<usize>()
    }
}

/// A [`PluginState`] as it is written into a song: each chunk a
/// `{ tag, data }` table whose `data` is base64, wrapped at
/// [`STATE_LINE_WIDTH`] columns so a song stays readable and diffable.
///
/// Plugin state is allowed in the TOML where samples are not (Adam,
/// 2026-09-16, answer 1): it is small in the common case, and it belongs to
/// the device the way its parameters do. Reading ignores every whitespace
/// character inside `data`, so a hand-rewrapped file still loads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginStateText(pub PluginState);

/// Columns per line of a chunk's base64. MIME's width.
pub const STATE_LINE_WIDTH: usize = 76;

#[derive(Serialize, Deserialize)]
struct ChunkText {
    tag: String,
    data: String,
}

/// `bytes` as base64, a newline after every [`STATE_LINE_WIDTH`] characters.
pub fn encode_state_bytes(bytes: &[u8]) -> String {
    use base64::Engine as _;
    let flat = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut wrapped = String::with_capacity(flat.len() + flat.len() / STATE_LINE_WIDTH + 1);
    for (index, line) in flat.as_bytes().chunks(STATE_LINE_WIDTH).enumerate() {
        if index > 0 {
            wrapped.push('\n');
        }
        // base64's alphabet is ASCII, so any split is on a char boundary.
        wrapped.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
    }
    wrapped
}

/// The bytes a base64 string holds, with every whitespace character ignored.
pub fn decode_state_bytes(text: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine as _;
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(compact)
}

impl Serialize for PluginStateText {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq as _;
        let mut seq = serializer.serialize_seq(Some(self.0.chunks.len()))?;
        for chunk in &self.0.chunks {
            seq.serialize_element(&ChunkText {
                tag: chunk.tag.clone(),
                data: encode_state_bytes(&chunk.data),
            })?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for PluginStateText {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let texts = Vec::<ChunkText>::deserialize(deserializer)?;
        let chunks = texts
            .into_iter()
            .map(|text| {
                decode_state_bytes(&text.data)
                    .map(|data| PluginStateChunk { tag: text.tag, data })
                    .map_err(serde::de::Error::custom)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self(PluginState { chunks }))
    }
}

/// A hosted plugin's identity within one song.
///
/// Minted from [`crate::Project::next_plugin_slot`] and never reused. **Per
/// project, not per chain**, unlike [`crate::DeviceId`]: a device id is
/// minted from its chain's counter, so moving a device to another channel
/// would renumber it, and the table in [`crate::Project::plugins`] is keyed
/// by this.
///
/// The address of a plugin's *parameter* does not use this: it names the
/// device (`ParamOwner::PluginParam { device }`, MOO-74), and the device's
/// `EffectParams::Plugin` names the slot.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct PluginSlotId(pub u32);

impl PluginSlotId {
    /// Not yet part of a song, for [`crate::DeviceId::UNASSIGNED`]'s reason:
    /// the top of the space, so a value that escapes unminted names nothing.
    pub const UNASSIGNED: Self = Self(u32::MAX);

    pub const fn is_assigned(self) -> bool {
        self.0 != Self::UNASSIGNED.0
    }
}

impl Default for PluginSlotId {
    fn default() -> Self {
        Self::UNASSIGNED
    }
}

/// Everything a song keeps about one hosted plugin.
///
/// `params` is what keeps a *missing* plugin's lanes and routes readable: it
/// is the list as the plugin last reported it, and it is saved back
/// unchanged when the plugin cannot be loaded. A lane naming an id that is
/// not in it is kept, never dropped (Adam, 2026-09-23, MOO-74).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginSlotState {
    pub plugin: PluginRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<PluginParamInfo>,
    /// Parameter ids pinned to the device's face (step 08).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<u32>,
    #[serde(default, skip_serializing_if = "state_is_empty")]
    pub state: PluginStateText,
}

fn state_is_empty(state: &PluginStateText) -> bool {
    state.0.is_empty()
}

impl PluginSlotState {
    /// A slot for `plugin` with nothing reported and nothing saved yet.
    pub fn new(plugin: PluginRef) -> Self {
        Self {
            plugin,
            params: Vec::new(),
            pinned: Vec::new(),
            state: PluginStateText::default(),
        }
    }

    /// The parameter `id`, as last reported, or `None` when the plugin does
    /// not (or no longer) report it.
    pub fn param(&self, id: u32) -> Option<&PluginParamInfo> {
        self.params.iter().find(|param| param.id == id)
    }

    pub fn heap_bytes(&self) -> usize {
        self.plugin.id.capacity()
            + self.plugin.name.capacity()
            + self.plugin.vendor.capacity()
            + self.plugin.version.capacity()
            + self.params.capacity() * std::mem::size_of::<PluginParamInfo>()
            + self
                .params
                .iter()
                .map(|param| param.name.capacity() + param.module.capacity())
                .sum::<usize>()
            + self.pinned.capacity() * std::mem::size_of::<u32>()
            + self.state.0.heap_bytes()
    }
}

/// The hosted plugins of one song, by slot.
pub type PluginSlots = BTreeMap<PluginSlotId, PluginSlotState>;

/// `serde(with)` for a [`PluginSlots`] table: the slot ids written as the
/// table's string keys and parsed back.
///
/// Stated rather than left to the integer key, because a TOML key is always
/// a string and not every path into a `Project` reads it back as a number:
/// the bundle loader goes through `toml::Value` first, which hands serde the
/// key `"0"` as a string, and a `u32` refuses it. Writing and reading the
/// string on purpose makes both paths agree.
pub mod slot_table {
    use super::{PluginSlotId, PluginSlotState, PluginSlots};
    use serde::{Deserialize, Deserializer, Serializer};
    use std::collections::BTreeMap;

    pub fn serialize<S: Serializer>(slots: &PluginSlots, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(slots.iter().map(|(id, slot)| (id.0.to_string(), slot)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<PluginSlots, D::Error> {
        BTreeMap::<String, PluginSlotState>::deserialize(deserializer)?
            .into_iter()
            .map(|(key, slot)| {
                key.parse::<u32>().map(|id| (PluginSlotId(id), slot)).map_err(|_| {
                    serde::de::Error::custom(format!("plugin slot `{key}` is not a slot number"))
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clap_ref() -> PluginRef {
        PluginRef {
            format: PluginFormat::Clap,
            id: "org.mooloop.test-gain".to_owned(),
            name: "Test Gain".to_owned(),
            vendor: "mooloop".to_owned(),
            version: "0.1.0".to_owned(),
        }
    }

    #[test]
    fn state_bytes_wrap_at_the_line_width_and_read_back_through_any_whitespace() {
        let bytes: Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let text = encode_state_bytes(&bytes);
        assert!(text.lines().all(|line| line.len() <= STATE_LINE_WIDTH));
        assert!(text.lines().count() > 1, "{text}");
        assert_eq!(decode_state_bytes(&text).unwrap(), bytes);
        let rewrapped = text.replace('\n', " \r\n\t ");
        assert_eq!(decode_state_bytes(&rewrapped).unwrap(), bytes);
    }

    #[test]
    fn an_empty_state_is_left_out_and_a_bare_slot_reads_back() {
        let slot = PluginSlotState::new(clap_ref());
        let text = toml::to_string(&slot).unwrap();
        assert!(!text.contains("state"), "{text}");
        assert!(!text.contains("params"), "{text}");
        let back: PluginSlotState = toml::from_str(&text).unwrap();
        assert_eq!(back, slot);
    }

    #[test]
    fn a_slot_with_sparse_params_and_state_round_trips() {
        let mut slot = PluginSlotState::new(clap_ref());
        slot.params = [7u32, 1000, 4_000_000_000]
            .into_iter()
            .map(|id| PluginParamInfo {
                id,
                name: format!("P{id}"),
                module: if id == 7 { "Main".to_owned() } else { String::new() },
                min: -1.0,
                max: 1.0,
                default: 0.25,
                stepped: (id == 1000).then_some(4),
                automatable: id != 7,
                modulatable: true,
                hidden: id == 4_000_000_000,
            })
            .collect();
        slot.pinned = vec![1000];
        slot.state = PluginStateText(PluginState {
            chunks: vec![PluginStateChunk { tag: "clap".to_owned(), data: vec![1, 2, 3, 250] }],
        });
        let text = toml::to_string(&slot).unwrap();
        let back: PluginSlotState = toml::from_str(&text).unwrap();
        assert_eq!(back, slot);
        assert_eq!(back.param(4_000_000_000).map(|p| p.hidden), Some(true));
        assert!(back.param(8).is_none());
    }

    #[test]
    fn state_that_is_not_base64_is_refused_rather_than_read_as_empty() {
        let text = r#"
            plugin = { format = "clap", id = "x", name = "X" }
            state = [{ tag = "clap", data = "not base64 !!" }]
        "#;
        assert!(toml::from_str::<PluginSlotState>(text).is_err());
    }

    /// The slot table reads back both straight from text and through
    /// `toml::Value`, which is the path the bundle loader takes and which
    /// hands the key over as a string.
    #[test]
    fn a_slot_table_reads_back_directly_and_through_a_toml_value() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Song {
            #[serde(with = "slot_table")]
            plugins: PluginSlots,
        }
        let mut plugins = PluginSlots::new();
        plugins.insert(PluginSlotId(0), PluginSlotState::new(clap_ref()));
        plugins.insert(PluginSlotId(12), PluginSlotState::new(clap_ref()));
        let song = Song { plugins };
        let text = toml::to_string(&song).unwrap();
        assert_eq!(toml::from_str::<Song>(&text).unwrap(), song);
        let value: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(value.try_into::<Song>().unwrap(), song);
        let bad = "[plugins.x.plugin]\nformat = \"clap\"\nid = \"a\"\nname = \"A\"\n";
        assert!(toml::from_str::<Song>(bad).is_err());
    }

    #[test]
    fn format_tags_are_frozen() {
        for (format, tag) in [
            (PluginFormat::Clap, "clap"),
            (PluginFormat::Vst3, "vst3"),
            (PluginFormat::Au, "au"),
        ] {
            #[derive(Serialize)]
            struct Wrap {
                format: PluginFormat,
            }
            assert_eq!(toml::to_string(&Wrap { format }).unwrap().trim(), format!("format = \"{tag}\""));
        }
    }
}
