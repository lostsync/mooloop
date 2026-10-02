//! Publishing into a model that a `for` repeater draws.
//!
//! Slint rebuilds every instance of a repeater whose model is reset
//! (`VecModel::set_vec`) or replaced by a different `ModelRc`, and a control
//! inside a rebuilt instance loses the press it was being dragged with: the
//! drag moves the value once and stops following the pointer. So a model
//! whose rows a drag inside them can edit is kept and written row by row,
//! and is reset only when its length changes.

use slint::{Model, ModelRc, VecModel};

/// Bring `model` to `rows`, touching only the rows that differ, or resetting
/// it when the length changed. Returns how many rows were written, so zero
/// means no binding moved.
pub(crate) fn write_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: &[T]) -> usize {
    if model.row_count() != rows.len() {
        model.set_vec(rows.to_vec());
        return rows.len().max(1);
    }
    let mut written = 0;
    for (index, row) in rows.iter().enumerate() {
        if model.row_data(index).as_ref() != Some(row) {
            model.set_row_data(index, row.clone());
            written += 1;
        }
    }
    written
}

/// `rows` written into `previous` with [`write_rows`] and that same model
/// returned, when `previous` is a `VecModel`; a new model otherwise.
///
/// For a model that is republished by building a fresh row or setting a
/// window property: handing the repeater back the model it already holds is
/// what keeps its instances.
pub(crate) fn rows_in_place<T: Clone + PartialEq + 'static>(
    previous: Option<ModelRc<T>>,
    rows: Vec<T>,
) -> ModelRc<T> {
    if let Some(model) = previous {
        if let Some(vec) = model.as_any().downcast_ref::<VecModel<T>>() {
            write_rows(vec, &rows);
            return model;
        }
    }
    ModelRc::new(VecModel::from(rows))
}
