//! Every theme this installation has, in one list.
//!
//! Built-ins, then whatever is in `<config>/mooloop/themes/`, then the
//! wallpaper palette if either generator has cached one. A user theme with a
//! built-in's name replaces it rather than sitting beside it, which is how
//! somebody retunes Nord without losing the ability to call it Nord.
//!
//! **Cached, because resolving a theme is on the path of every live colour
//! preview.** The Appearance page drags a slider and the palette is
//! recomputed forty times a second; reading a directory forty times a second
//! to find out that Dracula is still Dracula is not a thing to do. The cache
//! is dropped whenever a theme is written or removed, and whenever the
//! Appearance page opens -- which is also the moment a hand-edited file or a
//! fresh `wal` run should appear.

use super::wal::{self, WALLPAPER_THEME};
use super::{builtins, file, ThemeDefinition};
use std::sync::{Mutex, OnceLock};

struct Catalog {
    themes: Vec<ThemeDefinition>,
    warnings: Vec<String>,
}

fn cache() -> &'static Mutex<Option<Catalog>> {
    static CACHE: OnceLock<Mutex<Option<Catalog>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Drops the cache. The next lookup reads the themes directory and the
/// wallpaper cache again.
pub(crate) fn refresh() {
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
}

fn build() -> Catalog {
    let mut themes = builtins::all();
    let (user, warnings) = file::load_all();
    for theme in user {
        match themes
            .iter()
            .position(|existing| existing.name == theme.name)
        {
            Some(index) => themes[index] = theme,
            None => themes.push(theme),
        }
    }
    if let Some(palette) = wal::load() {
        let ramp = palette.ramp();
        let dark = ramp.is_dark();
        let colors = super::ThemeColors::Ramp(ramp);
        themes.push(ThemeDefinition {
            name: WALLPAPER_THEME.to_owned(),
            description: format!("From {}.", palette.describe()),
            dark: dark.then_some(colors),
            light: (!dark).then_some(colors),
            ..ThemeDefinition::empty(WALLPAPER_THEME)
        });
    }
    Catalog { themes, warnings }
}

fn with<T>(f: impl FnOnce(&Catalog) -> T) -> T {
    let Ok(mut guard) = cache().lock() else {
        // A poisoned lock means a panic somewhere else; a rebuilt catalog is
        // a better answer than a second panic on top of it.
        return f(&build());
    };
    f(guard.get_or_insert_with(build))
}

pub(crate) fn all() -> Vec<ThemeDefinition> {
    with(|catalog| catalog.themes.clone())
}

pub(crate) fn find(name: &str) -> Option<ThemeDefinition> {
    with(|catalog| {
        catalog
            .themes
            .iter()
            .find(|theme| theme.name == name)
            .cloned()
    })
}

/// One line per theme file that would not load, for the log. Read once at
/// startup rather than shown in the interface: a broken file is a thing the
/// person editing it is already looking at.
pub(crate) fn warnings() -> Vec<String> {
    with(|catalog| catalog.warnings.clone())
}

/// Whether a name belongs to a theme the user cannot delete.
///
/// The wallpaper entry counts: it is not a file mooloop wrote and removing it
/// means running `wal` again, not pressing Remove here.
pub(crate) fn is_protected(name: &str) -> bool {
    builtins::is_builtin(name) || name == WALLPAPER_THEME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_holds_every_built_in() {
        refresh();
        let names: Vec<String> = all().into_iter().map(|theme| theme.name).collect();
        for builtin in builtins::all() {
            assert!(names.contains(&builtin.name), "{} missing", builtin.name);
        }
        assert!(find(builtins::DEFAULT_THEME).is_some());
        assert!(find("no such theme").is_none());
    }

    #[test]
    fn built_ins_cannot_be_deleted() {
        assert!(is_protected(builtins::DEFAULT_THEME));
        assert!(is_protected("Nord"));
        assert!(is_protected(WALLPAPER_THEME));
        assert!(!is_protected("Something Adam Saved"));
    }
}
