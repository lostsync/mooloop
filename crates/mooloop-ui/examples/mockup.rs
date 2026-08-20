slint::include_modules!();

use serde::{Deserialize, Serialize};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::rc::Rc;

// Mirrors MockupItem in ui/mockup.slint - keep the fields in sync.
#[derive(Serialize, Deserialize, Clone)]
struct SavedItem {
    kind: i32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    label: String,
    value: f32,
    text: String,
}

#[derive(Serialize, Deserialize, Default)]
struct SavedLayout {
    items: Vec<SavedItem>,
}

impl From<&MockupItem> for SavedItem {
    fn from(item: &MockupItem) -> Self {
        SavedItem {
            kind: item.kind,
            x: item.x,
            y: item.y,
            width: item.width,
            height: item.height,
            label: item.label.to_string(),
            value: item.value,
            text: item.text.to_string(),
        }
    }
}

impl From<SavedItem> for MockupItem {
    fn from(saved: SavedItem) -> Self {
        MockupItem {
            kind: saved.kind,
            x: saved.x,
            y: saved.y,
            width: saved.width,
            height: saved.height,
            label: saved.label.into(),
            value: saved.value,
            text: saved.text.into(),
        }
    }
}

// Default width/height per palette kind, in the same order as
// MockupKinds.names in ui/mockup.slint.
const PALETTE_SIZES: &[(f32, f32)] = &[
    (220.0, 140.0), // Module frame
    (120.0, 12.0),  // Section label
    (56.0, 80.0),   // Knob
    (22.0, 22.0),   // Mini knob
    (176.0, 22.0),  // Fader (h)
    (32.0, 112.0),  // Fader (v)
    (30.0, 110.0),  // Mixer fader
    (60.0, 24.0),   // Toggle
    (60.0, 24.0),   // Button
    (26.0, 26.0),   // Icon button
    (34.0, 26.0),   // Transport button
    (220.0, 26.0),  // Segmented
    (160.0, 26.0),  // Selector
    (8.0, 8.0),     // LED
    (60.0, 18.0),   // Value readout
    (260.0, 120.0), // Envelope
    (120.0, 10.0),  // Peak meter
    (120.0, 18.0),  // Stereo meter
    (180.0, 120.0), // Sticky note
];

fn main() -> Result<(), slint::PlatformError> {
    let canvas = MockupCanvas::new()?;

    let items: Rc<VecModel<MockupItem>> = Rc::new(VecModel::default());
    canvas.set_items(ModelRc::from(items.clone()));

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_add_item(move |kind| {
            let canvas = canvas_weak.unwrap();
            let (w, h) = PALETTE_SIZES
                .get(kind as usize)
                .copied()
                .unwrap_or((120.0, 60.0));
            let count = items.row_count() as f32;
            let x = 40.0 + (count * 24.0) % 900.0;
            let y = 40.0 + (count * 18.0) % 560.0;
            items.push(MockupItem {
                kind,
                x,
                y,
                width: w,
                height: h,
                label: Default::default(),
                value: 0.5,
                text: "Note".into(),
            });
            canvas.set_selected_index(items.row_count() as i32 - 1);
        });
    }

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_remove_item(move |index| {
            if index >= 0 && (index as usize) < items.row_count() {
                items.remove(index as usize);
            }
            canvas_weak.unwrap().set_selected_index(-1);
        });
    }

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_save_requested(move |path| {
            let layout = SavedLayout {
                items: items.iter().map(|item| SavedItem::from(&item)).collect(),
            };
            let canvas = canvas_weak.unwrap();
            let result = toml::to_string_pretty(&layout)
                .map_err(|e| e.to_string())
                .and_then(|s| std::fs::write(path.as_str(), s).map_err(|e| e.to_string()));
            match result {
                Ok(()) => canvas.set_status_text(format!("saved {path}").into()),
                Err(e) => canvas.set_status_text(format!("save failed: {e}").into()),
            }
        });
    }

    {
        let canvas_weak = canvas.as_weak();
        let items = items.clone();
        canvas.on_load_requested(move || {
            let canvas = canvas_weak.unwrap();
            match std::fs::read_to_string("mockup.toml")
                .map_err(|e| e.to_string())
                .and_then(|s| toml::from_str::<SavedLayout>(&s).map_err(|e| e.to_string()))
            {
                Ok(layout) => {
                    items.set_vec(
                        layout
                            .items
                            .into_iter()
                            .map(MockupItem::from)
                            .collect::<Vec<_>>(),
                    );
                    canvas.set_selected_index(-1);
                    canvas.set_status_text("loaded mockup.toml".into());
                }
                Err(e) => canvas.set_status_text(format!("load failed: {e}").into()),
            }
        });
    }

    canvas.run()
}
