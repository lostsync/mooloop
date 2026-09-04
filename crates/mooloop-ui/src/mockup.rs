//! The UI mockup tool: a drag-and-drop canvas for trying layouts out of real
//! widgets before committing them to a `.slint` file.
//!
//! Everything the tool needs beyond its own `.slint` lives here - the item
//! model, the palette model, TOML persistence, and every callback - so the
//! standalone example and the in-app Developer entry are two callers of one
//! [`wire_mockup`] rather than two copies of the same file.
//!
//! The palette itself is not defined here. `ui/mockup-catalog.slint` owns it,
//! and this module reads sizes, behaviour and grouping back off that global.

use crate::settings::config_dir;
use crate::mockup_ui::{MockupCanvas, MockupCatalog, MockupItem, MockupPaletteRow};
use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use std::path::{Path, PathBuf};
use std::rc::Rc;

include!(concat!(env!("OUT_DIR"), "/mockup_exports.rs"));

/// Exported components the audit never expects the palette to carry, matched
/// by suffix so a new device face or dialog needs no edit here.
const AUDIT_IGNORE_SUFFIXES: &[&str] = &["Dialog", "DeviceFace", "DragHarness"];

/// The rest of the exclusions, by name: whole windows, the preference pages
/// and their rows, and the mockup tool's own parts.
const AUDIT_IGNORE: &[&str] = &[
    "AppearancePage",
    "DeviceRackConcepts",
    "GestureRowView",
    "JackControlSurface",
    "MainWindow",
    "MockupCanvas",
    "MockupSpecimen",
    "ShortcutRowView",
];

/// Role groups in the order the palette shows them. A kind may belong to
/// several; one it names that is not listed here is appended after these.
const GROUP_ORDER: &[&str] = &[
    "Layout",
    "Knobs & faders",
    "Buttons",
    "Meters",
    "Scopes & displays",
    "Device rack",
    "Mixer",
    "Toolbar",
    "Menus",
    "Annotation",
];

/// Pinned last in every axis: exported widgets with no catalog row yet, shown
/// greyed because they cannot be placed until someone writes that row.
const UNCATALOGUED_GROUP: &str = "UNCATALOGUED";

const LAYOUT_VERSION: u32 = 1;

/// One catalog row, read back out of `MockupCatalog.kinds`.
struct CatalogRow {
    /// Save key, and the palette's identity across a catalog reorder.
    component: String,
    /// The exported component behind it: `component` up to the first ':'.
    audit: String,
    name: String,
    module: String,
    groups: Vec<String>,
    width: f32,
    height: f32,
    device_units: bool,
}

fn catalog(canvas: &MockupCanvas) -> Vec<CatalogRow> {
    MockupCatalog::get(canvas)
        .get_kinds()
        .iter()
        .map(|kind| {
            let component = kind.component.to_string();
            let audit = component
                .split_once(':')
                .map_or(component.clone(), |(name, _)| name.to_string());
            CatalogRow {
                component,
                audit,
                name: kind.name.to_string(),
                module: kind.module.to_string(),
                groups: kind.groups.iter().map(|g| g.to_string()).collect(),
                width: kind.default_width,
                height: kind.default_height,
                device_units: kind.device_units,
            }
        })
        .collect()
}

/// Exported components with no catalog row, as `(component, module)`. This is
/// the tool's own to-do list: write a widget, and it shows up here until the
/// palette carries it.
fn uncatalogued(catalog: &[CatalogRow]) -> Vec<(&'static str, &'static str)> {
    EXPORTED_COMPONENTS
        .iter()
        .copied()
        .filter(|(component, _)| {
            !AUDIT_IGNORE.contains(component)
                && !AUDIT_IGNORE_SUFFIXES
                    .iter()
                    .any(|suffix| component.ends_with(suffix))
                && !catalog.iter().any(|row| row.audit == *component)
        })
        .collect()
}

fn header_row(label: &str) -> MockupPaletteRow {
    MockupPaletteRow {
        header: true,
        label: label.into(),
        detail: SharedString::new(),
        kind: -1,
        enabled: false,
    }
}

fn kind_row(index: usize, row: &CatalogRow, detail: &str) -> MockupPaletteRow {
    MockupPaletteRow {
        header: false,
        label: row.name.as_str().into(),
        detail: detail.into(),
        kind: index as i32,
        enabled: true,
    }
}

fn matches(filter: &str, row: &CatalogRow) -> bool {
    filter.is_empty()
        || row.name.to_lowercase().contains(filter)
        || row.component.to_lowercase().contains(filter)
        || row.module.to_lowercase().contains(filter)
}

/// Flattens the catalog into the sidebar's display list. Slint has no `contains`
/// for arrays and a kind's group membership is many-to-many, so the grouping is
/// resolved here and the `.slint` side stays a plain list of rows.
fn palette_rows(catalog: &[CatalogRow], axis: i32, filter: &str) -> Vec<MockupPaletteRow> {
    let filter = filter.trim().to_lowercase();
    let mut rows = Vec::new();

    match axis {
        // Module.
        1 => {
            let mut modules: Vec<&str> = catalog.iter().map(|row| row.module.as_str()).collect();
            modules.sort_unstable();
            modules.dedup();
            for module in modules {
                let members: Vec<usize> = catalog
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.module == module && matches(&filter, row))
                    .map(|(index, _)| index)
                    .collect();
                if members.is_empty() {
                    continue;
                }
                rows.push(header_row(module));
                rows.extend(members.into_iter().map(|i| kind_row(i, &catalog[i], "")));
            }
        }
        // A-Z: one flat list, so it needs no header at all.
        2 => {
            let mut members: Vec<usize> = (0..catalog.len())
                .filter(|&i| matches(&filter, &catalog[i]))
                .collect();
            members.sort_by(|&a, &b| catalog[a].name.cmp(&catalog[b].name));
            rows.extend(
                members
                    .into_iter()
                    .map(|i| kind_row(i, &catalog[i], &catalog[i].module)),
            );
        }
        // Role, the default.
        _ => {
            let mut groups: Vec<String> = GROUP_ORDER.iter().map(|g| g.to_string()).collect();
            let mut extra: Vec<String> = catalog
                .iter()
                .flat_map(|row| row.groups.iter())
                .filter(|group| !GROUP_ORDER.contains(&group.as_str()))
                .cloned()
                .collect();
            extra.sort();
            extra.dedup();
            groups.extend(extra);

            for group in groups {
                let members: Vec<usize> = catalog
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.groups.contains(&group))
                    .filter(|(_, row)| matches(&filter, row))
                    .map(|(index, _)| index)
                    .collect();
                if members.is_empty() {
                    continue;
                }
                rows.push(header_row(&group));
                rows.extend(
                    members
                        .into_iter()
                        .map(|i| kind_row(i, &catalog[i], &catalog[i].module)),
                );
            }
        }
    }

    let missing: Vec<(&str, &str)> = uncatalogued(catalog)
        .into_iter()
        .filter(|(component, module)| {
            filter.is_empty()
                || component.to_lowercase().contains(&filter)
                || module.to_lowercase().contains(&filter)
        })
        .collect();
    if !missing.is_empty() {
        rows.push(header_row(UNCATALOGUED_GROUP));
        rows.extend(
            missing
                .into_iter()
                .map(|(component, module)| MockupPaletteRow {
                    header: false,
                    label: component.into(),
                    detail: module.into(),
                    kind: -1,
                    enabled: false,
                }),
        );
    }
    rows
}

// ---------------------------------------------------------------- persistence

/// One placed item as it is saved. Keyed by the catalog's `component` string
/// rather than its index, so reordering the palette cannot silently turn every
/// knob in a saved layout into something else.
#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct SavedItem {
    component: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(default)]
    label: String,
    #[serde(default)]
    value: f32,
    #[serde(default)]
    text: String,
    #[serde(default)]
    units: i32,
    #[serde(default)]
    half_unit: bool,
    #[serde(default)]
    locked: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SavedLayout {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    items: Vec<SavedItem>,
}

/// Where named layouts live, next to the rest of the UI's settings rather than
/// in whatever directory the tool happened to be launched from.
pub(crate) fn layouts_dir() -> PathBuf {
    config_dir().join("layouts")
}

/// Keeps a typed layout name to one file in [`layouts_dir`].
fn layout_path(name: &str) -> PathBuf {
    let trimmed = name.trim();
    let safe: String = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stem = if safe.is_empty() {
        "layout"
    } else {
        safe.as_str()
    };
    layouts_dir().join(format!("{stem}.toml"))
}

fn to_saved(catalog: &[CatalogRow], item: &MockupItem) -> Option<SavedItem> {
    let row = catalog.get(usize::try_from(item.kind).ok()?)?;
    Some(SavedItem {
        component: row.component.clone(),
        x: item.x,
        y: item.y,
        width: item.width,
        height: item.height,
        label: item.label.to_string(),
        value: item.value,
        text: item.text.to_string(),
        units: item.units,
        half_unit: item.half_unit,
        locked: item.locked,
    })
}

/// Resolves saved items back to kinds. Returns the items that resolved and how
/// many did not: a layout naming a component the catalog has since dropped
/// loads without those, rather than failing whole or placing the wrong widget.
fn from_saved(catalog: &[CatalogRow], saved: Vec<SavedItem>) -> (Vec<MockupItem>, usize) {
    let mut items = Vec::with_capacity(saved.len());
    let mut dropped = 0;
    for entry in saved {
        let Some(kind) = catalog
            .iter()
            .position(|row| row.component == entry.component)
        else {
            dropped += 1;
            continue;
        };
        items.push(MockupItem {
            kind: kind as i32,
            x: entry.x,
            y: entry.y,
            width: entry.width,
            height: entry.height,
            label: entry.label.into(),
            value: entry.value,
            text: entry.text.into(),
            units: entry.units,
            half_unit: entry.half_unit,
            locked: entry.locked,
        });
    }
    (items, dropped)
}

fn read_layout(path: &Path) -> Result<SavedLayout, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    toml::from_str::<SavedLayout>(&text).map_err(|error| error.to_string())
}

/// Replaces the canvas contents with a layout file, by path. The example's
/// snapshot mode uses this to load a fixture that is not in the layouts
/// directory.
pub fn load_mockup_layout(canvas: &MockupCanvas, path: &Path) -> Result<(), String> {
    let model = canvas.get_items();
    let model = model
        .as_any()
        .downcast_ref::<VecModel<MockupItem>>()
        .ok_or("the canvas has not been wired")?;
    let layout = read_layout(path)?;
    let (items, dropped) = from_saved(&catalog(canvas), layout.items);
    let count = items.len();
    model.set_vec(items);
    canvas.set_selected_index(-1);
    canvas.set_status_text(load_status(path, count, dropped).into());
    Ok(())
}

fn load_status(path: &Path, count: usize, dropped: usize) -> String {
    let name = path.file_stem().unwrap_or_default().to_string_lossy();
    if dropped == 0 {
        format!("loaded {count} from {name}")
    } else {
        format!("loaded {count} from {name}, dropped {dropped} unknown")
    }
}

// -------------------------------------------------------------------- wiring

/// Installs the model and every callback on a fresh [`MockupCanvas`]. The
/// canvas is inert until this runs; both the standalone example and the
/// in-app Developer entry call it and nothing else.
pub fn wire_mockup(canvas: &MockupCanvas) {
    let catalog = Rc::new(catalog(canvas));
    let items: Rc<VecModel<MockupItem>> = Rc::new(VecModel::default());
    canvas.set_items(ModelRc::from(items.clone()));
    refresh_palette(canvas, &catalog);

    {
        let weak = canvas.as_weak();
        let catalog = catalog.clone();
        canvas.on_palette_query_changed(move || {
            let Some(canvas) = weak.upgrade() else { return };
            refresh_palette(&canvas, &catalog);
        });
    }

    {
        let weak = canvas.as_weak();
        let catalog = catalog.clone();
        let items = items.clone();
        canvas.on_add_item(move |kind| {
            let Some(canvas) = weak.upgrade() else { return };
            let Some(row) = usize::try_from(kind).ok().and_then(|i| catalog.get(i)) else {
                return;
            };
            let count = items.row_count() as f32;
            items.push(MockupItem {
                kind,
                x: 40.0 + (count * 24.0) % 900.0,
                y: 40.0 + (count * 18.0) % 560.0,
                width: row.width,
                height: row.height,
                label: Default::default(),
                value: 0.5,
                text: "Note".into(),
                units: if row.device_units { 1 } else { 0 },
                half_unit: false,
                locked: false,
            });
            canvas.set_selected_index(items.row_count() as i32 - 1);
        });
    }

    {
        let weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_remove_item(move |index| {
            let Some(canvas) = weak.upgrade() else { return };
            if let Ok(index) = usize::try_from(index) {
                if index < items.row_count() {
                    items.remove(index);
                }
            }
            canvas.set_selected_index(-1);
        });
    }

    {
        let weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_duplicate_item(move |index| {
            let Some(canvas) = weak.upgrade() else { return };
            let Some(mut copy) = usize::try_from(index).ok().and_then(|i| items.row_data(i)) else {
                return;
            };
            // On top and offset, so the copy is the thing under the pointer
            // rather than something hidden exactly behind the original.
            copy.x += 12.0;
            copy.y += 12.0;
            items.push(copy);
            canvas.set_selected_index(items.row_count() as i32 - 1);
        });
    }

    // Array order is paint order, so the four ordering commands are all a
    // remove and an insert; the selection follows the item it was on.
    let reorder = {
        let weak = canvas.as_weak();
        let items = items.clone();
        move |index: i32, target: fn(usize, usize) -> usize| {
            let Some(canvas) = weak.upgrade() else { return };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let count = items.row_count();
            if index >= count {
                return;
            }
            let destination = target(index, count);
            if destination == index {
                return;
            }
            let item = items.remove(index);
            items.insert(destination, item);
            canvas.set_selected_index(destination as i32);
        }
    };
    canvas.on_raise_item({
        let reorder = reorder.clone();
        move |index| reorder(index, |i, count| (i + 1).min(count - 1))
    });
    canvas.on_lower_item({
        let reorder = reorder.clone();
        move |index| reorder(index, |i, _| i.saturating_sub(1))
    });
    canvas.on_front_item({
        let reorder = reorder.clone();
        move |index| reorder(index, |_, count| count - 1)
    });
    canvas.on_back_item(move |index| reorder(index, |_, _| 0));

    {
        let weak = canvas.as_weak();
        let catalog = catalog.clone();
        let items = items.clone();
        canvas.on_save_requested(move |name| {
            let Some(canvas) = weak.upgrade() else { return };
            let path = layout_path(&name);
            let layout = SavedLayout {
                version: LAYOUT_VERSION,
                items: items
                    .iter()
                    .filter_map(|item| to_saved(&catalog, &item))
                    .collect(),
            };
            let result = std::fs::create_dir_all(layouts_dir())
                .map_err(|error| error.to_string())
                .and_then(|()| toml::to_string_pretty(&layout).map_err(|e| e.to_string()))
                .and_then(|text| std::fs::write(&path, text).map_err(|e| e.to_string()));
            canvas.set_status_text(match result {
                Ok(()) => format!("saved {}", path.display()).into(),
                Err(error) => format!("save failed: {error}").into(),
            });
        });
    }

    {
        let weak = canvas.as_weak();
        canvas.on_load_requested(move |name| {
            let Some(canvas) = weak.upgrade() else { return };
            let path = layout_path(&name);
            if let Err(error) = load_mockup_layout(&canvas, &path) {
                canvas.set_status_text(format!("load failed: {error}").into());
            }
        });
    }
}

fn refresh_palette(canvas: &MockupCanvas, catalog: &[CatalogRow]) {
    let rows = palette_rows(
        catalog,
        canvas.get_palette_axis(),
        &canvas.get_palette_filter(),
    );
    canvas.set_palette_rows(ModelRc::new(VecModel::from(rows)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> MockupCanvas {
        i_slint_backend_testing::init_no_event_loop();
        MockupCanvas::new().unwrap()
    }

    /// A typo in a catalog row's `component` used to be invisible until the
    /// item rendered as a blank rectangle. Now it fails here.
    #[test]
    fn every_catalog_component_is_a_real_export() {
        let catalog = catalog(&canvas());
        assert!(!catalog.is_empty());
        for row in &catalog {
            assert!(
                EXPORTED_COMPONENTS
                    .iter()
                    .any(|(component, _)| *component == row.audit),
                "catalog kind {:?} names {:?}, which nothing in ui/ exports",
                row.name,
                row.audit
            );
        }
    }

    /// The module column is what the palette shows under Module, so a wrong
    /// one is a lie in the UI rather than a broken render.
    #[test]
    fn every_catalog_module_matches_the_export() {
        for row in catalog(&canvas()) {
            let module = EXPORTED_COMPONENTS
                .iter()
                .find(|(component, _)| *component == row.audit)
                .map(|(_, module)| *module);
            assert_eq!(module, Some(row.module.as_str()), "kind {:?}", row.name);
        }
    }

    /// An ignore entry that no longer names anything is dead weight that would
    /// quietly hide a future component of the same name.
    #[test]
    fn every_audit_ignore_entry_still_exists() {
        for ignored in AUDIT_IGNORE {
            assert!(
                EXPORTED_COMPONENTS
                    .iter()
                    .any(|(component, _)| component == ignored),
                "AUDIT_IGNORE lists {ignored:?}, which ui/ no longer exports"
            );
        }
        for suffix in AUDIT_IGNORE_SUFFIXES {
            assert!(
                EXPORTED_COMPONENTS
                    .iter()
                    .any(|(component, _)| component.ends_with(suffix)),
                "AUDIT_IGNORE_SUFFIXES lists {suffix:?}, which matches nothing"
            );
        }
    }

    #[test]
    fn uncatalogued_excludes_the_catalogued_and_the_ignored() {
        let catalog = catalog(&canvas());
        let missing = uncatalogued(&catalog);
        for (component, _) in &missing {
            assert!(!AUDIT_IGNORE.contains(component));
            assert!(!catalog.iter().any(|row| row.audit == *component));
        }
        // What is left is the standing to-do, and it is down to the two
        // composite editors nobody has factored: see docs/WIDGET_INVENTORY.md.
        assert!(
            missing.iter().any(|(c, _)| *c == "PianoGrid"),
            "PianoGrid has no catalog row yet, so it belongs in the group"
        );
        assert!(missing.len() <= 4, "the group has grown again: {missing:?}");
    }

    #[test]
    fn role_axis_groups_a_kind_under_every_group_it_names() {
        let catalog = catalog(&canvas());
        let rows = palette_rows(&catalog, 0, "");
        let channel_meter = catalog
            .iter()
            .position(|row| row.component == "ChannelMeter")
            .unwrap() as i32;
        let appearances = rows
            .iter()
            .filter(|row| !row.header && row.kind == channel_meter)
            .count();
        assert_eq!(
            appearances, 2,
            "ChannelMeter is both a meter and a mixer part"
        );
        assert_eq!(rows.last().map(|row| row.header), Some(false));
        assert!(rows.iter().any(|row| row.label == UNCATALOGUED_GROUP));
    }

    #[test]
    fn the_filter_narrows_every_axis() {
        let catalog = catalog(&canvas());
        for axis in 0..3 {
            let rows = palette_rows(&catalog, axis, "knob");
            let kinds: Vec<&str> = rows
                .iter()
                .filter(|row| !row.header && row.enabled)
                .map(|row| row.label.as_str())
                .collect();
            assert!(kinds.contains(&"Mini knob"), "axis {axis}: {kinds:?}");
            assert!(!kinds.contains(&"Peak meter"), "axis {axis}: {kinds:?}");
        }
    }

    fn placed(canvas: &MockupCanvas) -> Vec<String> {
        let catalog = catalog(canvas);
        canvas
            .get_items()
            .iter()
            .map(|item| catalog[item.kind as usize].name.clone())
            .collect()
    }

    /// Array order is paint order, so the ordering commands are the whole
    /// z-order feature; the layers list reads the same array backwards.
    #[test]
    fn the_ordering_commands_move_an_item_through_the_paint_order() {
        let canvas = canvas();
        wire_mockup(&canvas);
        let catalog = catalog(&canvas);
        let frame = catalog
            .iter()
            .position(|r| r.name == "Module frame")
            .unwrap();
        let knob = catalog.iter().position(|r| r.name == "Knob").unwrap();
        canvas.invoke_add_item(frame as i32);
        canvas.invoke_add_item(knob as i32);
        canvas.invoke_add_item(frame as i32);
        assert_eq!(placed(&canvas), ["Module frame", "Knob", "Module frame"]);

        // The knob is in the middle; the selection follows it every step.
        canvas.set_selected_index(1);
        canvas.invoke_front_item(1);
        assert_eq!(placed(&canvas), ["Module frame", "Module frame", "Knob"]);
        assert_eq!(canvas.get_selected_index(), 2);

        canvas.invoke_back_item(2);
        assert_eq!(placed(&canvas), ["Knob", "Module frame", "Module frame"]);
        assert_eq!(canvas.get_selected_index(), 0);

        canvas.invoke_raise_item(0);
        assert_eq!(placed(&canvas), ["Module frame", "Knob", "Module frame"]);
        assert_eq!(canvas.get_selected_index(), 1);

        canvas.invoke_lower_item(1);
        assert_eq!(canvas.get_selected_index(), 0);

        // Already at the bottom, and already at the top: both are no-ops
        // rather than a wrap or an index that walks off the end.
        canvas.invoke_lower_item(0);
        assert_eq!(placed(&canvas), ["Knob", "Module frame", "Module frame"]);
        canvas.invoke_front_item(2);
        assert_eq!(placed(&canvas), ["Knob", "Module frame", "Module frame"]);
    }

    /// A device is sized by the rack-unit picker, which needs a unit count to
    /// start from; everything else stays at 0 units so the panel hides it.
    #[test]
    fn a_placed_device_starts_at_one_rack_unit() {
        let canvas = canvas();
        wire_mockup(&canvas);
        let catalog = catalog(&canvas);
        for (index, row) in catalog.iter().enumerate() {
            canvas.invoke_add_item(index as i32);
            let item = canvas.get_items().row_data(index).unwrap();
            assert_eq!(
                item.units,
                i32::from(row.device_units),
                "kind {:?}",
                row.name
            );
            assert_eq!((item.width, item.height), (row.width, row.height));
        }
    }

    #[test]
    fn duplicating_offsets_the_copy_and_leaves_it_selected() {
        let canvas = canvas();
        wire_mockup(&canvas);
        canvas.invoke_add_item(0);
        let original = canvas.get_items().row_data(0).unwrap();
        canvas.invoke_duplicate_item(0);

        let copy = canvas.get_items().row_data(1).unwrap();
        assert_eq!(canvas.get_selected_index(), 1);
        assert_eq!(copy.kind, original.kind);
        assert_eq!(copy.x - original.x, 12.0);
        assert_eq!(copy.y - original.y, 12.0);
    }

    #[test]
    fn a_layout_round_trips_through_toml() {
        let catalog = catalog(&canvas());
        let knob = catalog.iter().position(|r| r.name == "Knob").unwrap() as i32;
        let frame = catalog
            .iter()
            .position(|r| r.name == "Device frame")
            .unwrap() as i32;
        let original = [
            MockupItem {
                kind: knob,
                x: 12.0,
                y: 34.0,
                width: 56.0,
                height: 80.0,
                label: "CUTOFF".into(),
                value: 0.25,
                text: "".into(),
                units: 0,
                half_unit: false,
                locked: true,
            },
            MockupItem {
                kind: frame,
                x: 200.0,
                y: 8.0,
                width: 444.0,
                height: 268.0,
                label: "".into(),
                value: 0.5,
                text: "".into(),
                units: 2,
                half_unit: false,
                locked: false,
            },
        ];

        let saved: Vec<SavedItem> = original
            .iter()
            .map(|item| to_saved(&catalog, item).unwrap())
            .collect();
        let text = toml::to_string_pretty(&SavedLayout {
            version: LAYOUT_VERSION,
            items: saved,
        })
        .unwrap();
        let reloaded: SavedLayout = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.version, LAYOUT_VERSION);
        let (items, dropped) = from_saved(&catalog, reloaded.items);

        assert_eq!(dropped, 0);
        assert_eq!(items.len(), original.len());
        for (before, after) in original.iter().zip(&items) {
            assert_eq!(before.kind, after.kind);
            assert_eq!((before.x, before.y), (after.x, after.y));
            assert_eq!((before.width, before.height), (after.width, after.height));
            assert_eq!(before.label, after.label);
            assert_eq!(before.units, after.units);
            assert_eq!(before.half_unit, after.half_unit);
            assert_eq!(before.locked, after.locked);
        }
    }

    /// Reordering the catalog must not turn saved knobs into something else,
    /// and a name it no longer knows must not take the whole layout down.
    #[test]
    fn an_unknown_component_is_dropped_rather_than_fatal() {
        let catalog = catalog(&canvas());
        let text = r#"
            version = 1
            [[items]]
            component = "MiniKnob"
            x = 1.0
            y = 2.0
            width = 22.0
            height = 22.0
            [[items]]
            component = "AmbitiousWidgetFromTheFuture"
            x = 3.0
            y = 4.0
            width = 10.0
            height = 10.0
        "#;
        let layout: SavedLayout = toml::from_str(text).unwrap();
        let (items, dropped) = from_saved(&catalog, layout.items);
        assert_eq!(dropped, 1);
        assert_eq!(items.len(), 1);
        assert_eq!(catalog[items[0].kind as usize].component, "MiniKnob");
    }

    #[test]
    fn a_layout_name_cannot_escape_the_layouts_directory() {
        assert_eq!(layout_path("rack row"), layouts_dir().join("rack-row.toml"));
        assert_eq!(
            layout_path("../../etc/passwd"),
            layouts_dir().join("------etc-passwd.toml")
        );
        assert_eq!(layout_path("   "), layouts_dir().join("layout.toml"));
    }
}
