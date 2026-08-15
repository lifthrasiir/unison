//! What the application remembers between runs.
//!
//! # Two stores, not one
//!
//! Enabling eframe's `persistence` feature gives the application *two* sets of
//! saved state, and only the second one is written here.
//!
//! The first is egui's own: every `SidePanel`/`TopBottomPanel` width, every
//! `ScrollArea` offset, the theme preference and the global zoom factor go
//! into `egui::Memory` and are saved under eframe's `"egui"` key without this
//! module touching them. **Do not add a field here for anything on that
//! list** — two owners of one value means the loser silently wins every other
//! frame. The window geometry is a third such case, saved under `"window"` and
//! controlled by `NativeOptions::persist_window`.
//!
//! [`Settings`] is the rest: the state that is [`super::UniformApp`]'s own
//! field rather than a widget's.
//!
//! # Deliberately not saved
//!
//! No session restore. Which files were open, the pane split, the caret
//! positions and the navigation history are *not* remembered: restoring them
//! faithfully means answering what a reopened file that changed on disk should
//! do, and that question is out of scope. Only [`Settings::font_dir`] — the
//! directory itself — comes back, so that launching with no argument lands
//! where the last run did.
//!
//! Nothing derived is saved either (built fonts, resolved glyphs, issues), nor
//! anything that only describes this run (toasts, search hits, undo stacks).
//!
//! # Evolving the format
//!
//! [`eframe::get_value`] parses the whole blob or returns `None`, and a `None`
//! resets *every* setting at once. So: `serde(default)` on the container, so a
//! field added later reads as its default out of an older file; and
//! [`Settings::clamp`] on the way in, because a value that was in range when
//! it was written need not be in range now (a smaller screen, a font-size
//! bound that moved, a tab that no longer exists).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::panels::{IssueFilter, SEARCH_TAB};
use super::zoom::{
    DEFAULT_PREVIEW_FONT_SIZE, MAX_PREVIEW_FONT_SIZE, MAX_ZOOM_LEVEL, MIN_PREVIEW_FONT_SIZE,
    MIN_ZOOM_LEVEL,
};
use crate::specimen::SpecimenOptions;

/// How many directories keep a remembered face. The list is there so that
/// alternating between two font directories does not make each forget the
/// other's face; it is not a history the user ever sees, so it is bounded at a
/// size no one reaches by hand.
const MAX_REMEMBERED_FACES: usize = 16;

/// The application's own persisted state. See the module docs for what is
/// deliberately *not* in here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Settings {
    /// The font directory of the last run. A command-line argument overrides
    /// it, and a path that is no longer a directory is dropped on load.
    pub(super) font_dir: Option<PathBuf>,
    /// The face last selected in each directory, most-recently-set first.
    /// Per directory because a face id means nothing outside the source that
    /// declares it — see [`Settings::face_for`].
    pub(super) faces: Vec<(PathBuf, String)>,
    /// View menu: the metric box over every glyph grid.
    pub(super) show_metrics: bool,
    /// F12: draw the UI in the stock font rather than the one being edited.
    pub(super) escape_mode: bool,
    /// Which bottom-panel tab was open, `None` for the collapsed panel. The
    /// panel's *height* is egui's to remember, not ours.
    pub(super) bottom_panel_tab: Option<usize>,
    /// Which severities the Issues tab lists. Saved for the same reason the
    /// specimen's options are: it is a way of looking at the source that
    /// belongs to the person, not to the run.
    pub(super) issue_filter: IssueFilter,
    pub(super) specimen: SpecimenOptions,
    /// The preview's text. Saved because it is the one thing in the window the
    /// user types from scratch every run otherwise.
    pub(super) preview_text: String,
    pub(super) preview_font_size: f32,
    /// The shaper backend's *name*, not its index: which backends exist is
    /// platform-dependent, so an index saved on one machine points at a
    /// different engine on another.
    pub(super) preview_backend: String,
    pub(super) preview_color_font: bool,
    /// The editor zoom level. Zoom is per pane, but a pane is not restored, so
    /// what is saved is one level for the panes of the next run to start at.
    pub(super) zoom_level: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_dir: None,
            faces: Vec::new(),
            show_metrics: true,
            escape_mode: false,
            bottom_panel_tab: None,
            issue_filter: IssueFilter::default(),
            specimen: SpecimenOptions::default(),
            preview_text: String::new(),
            preview_font_size: DEFAULT_PREVIEW_FONT_SIZE,
            preview_backend: String::new(),
            preview_color_font: true,
            zoom_level: MIN_ZOOM_LEVEL,
        }
    }
}

impl Settings {
    /// Forces every field back into the range this build accepts. Runs on the
    /// way in, once, so nothing downstream has to repeat the check.
    pub(super) fn clamp(&mut self) {
        let d = Self::default();
        if !self.preview_font_size.is_finite() {
            self.preview_font_size = d.preview_font_size;
        }
        self.preview_font_size = self
            .preview_font_size
            .clamp(MIN_PREVIEW_FONT_SIZE, MAX_PREVIEW_FONT_SIZE);
        self.zoom_level = self.zoom_level.clamp(MIN_ZOOM_LEVEL, MAX_ZOOM_LEVEL);
        // A tab index from a build with more tabs than this one has would
        // otherwise open a panel that draws nothing.
        self.bottom_panel_tab = self.bottom_panel_tab.filter(|t| *t <= SEARCH_TAB);
        self.faces.truncate(MAX_REMEMBERED_FACES);
    }

    /// Records the directory the next run should open with.
    pub(super) fn set_font_dir(&mut self, dir: &Path) {
        self.font_dir = Some(absolute(dir));
    }

    /// The face last selected in `dir`. The caller still has to check the id
    /// against the faces the directory actually declares — a source is edited
    /// between runs, and the face that was picked may be gone.
    pub(super) fn face_for(&self, dir: &Path) -> Option<&str> {
        let dir = absolute(dir);
        self.faces
            .iter()
            .find(|(d, _)| *d == dir)
            .map(|(_, f)| f.as_str())
    }

    /// Records `face` as `dir`'s, moving it to the front of the list so the
    /// entry dropped by the bound is always the least recently chosen one.
    pub(super) fn remember_face(&mut self, dir: &Path, face: &str) {
        let dir = absolute(dir);
        self.faces.retain(|(d, _)| *d != dir);
        self.faces.insert(0, (dir, face.to_string()));
        self.faces.truncate(MAX_REMEMBERED_FACES);
    }
}

/// The form a directory is saved and looked up in.
///
/// Everything here outlives the process, so a path relative to *this* run's
/// working directory is not a path at all: `uniform font/` names one directory
/// today and another one from a different shell tomorrow. Absolute is also
/// what makes the face memory a map — the same directory reached by two names
/// must be one entry, not two.
///
/// Lexical (`std::path::absolute`), not [`std::fs::canonicalize`]: a symlink
/// the user opened through is part of how they name the directory and should
/// survive into the next run, and canonicalizing would also fail outright for
/// a directory that has since been removed — which is a path this still has to
/// be able to write down and discard on the way back in.
fn absolute(dir: &Path) -> PathBuf {
    std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf())
}

impl super::UniformApp {
    /// Brings `self.settings` up to date with the window as it is now, and
    /// writes it out. Called by [`eframe::App::save`], which runs on the main
    /// thread every `auto_save_interval` and once more on exit — so this must
    /// stay cheap. Nothing here reads a document or a build.
    pub(super) fn save_settings(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(dir) = self.font_dir.clone() {
            // Recorded here as well as at the moment of choosing, so that a
            // face picked before this build learned to remember it — or one
            // restored and never touched — is not forgotten again.
            let face = self.selected_face.clone();
            if !face.is_empty() {
                self.settings.remember_face(&dir, &face);
            }
            self.settings.set_font_dir(&dir);
        }
        self.settings.show_metrics = self.show_metrics;
        self.settings.escape_mode = self.escape_mode;
        self.settings.bottom_panel_tab = self.bottom_panel_tab;
        self.settings.issue_filter = self.issue_filter;
        self.settings.specimen = self.specimen.options;
        self.settings.preview_text = self.shaped_preview.text();
        self.settings.preview_font_size = self.preview_font_size;
        self.settings.preview_backend = self.shaped_preview.selected_backend_name().to_string();
        self.settings.preview_color_font = self.shaped_preview.color_font;
        self.settings.zoom_level = self.panes.focused().zoom_level;
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}

#[cfg(test)]
mod tests {
    use eframe::Storage as _;

    use super::*;

    /// The `eframe::Storage` the application is handed, minus the file. Tests
    /// go through it rather than serializing directly, so what they exercise
    /// is the real `set_value`/`get_value` pair — the same RON encoder, and
    /// the same "a blob that will not parse reads as `None`" behavior — that
    /// [`super::super::UniformApp::save_settings`] writes through.
    #[derive(Default)]
    struct TestStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for TestStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn flush(&mut self) {}
    }

    /// Saves and loads back the way the application does.
    fn round_trip(settings: &Settings) -> Option<Settings> {
        let mut storage = TestStorage::default();
        eframe::set_value(&mut storage, eframe::APP_KEY, settings);
        eframe::get_value(&storage, eframe::APP_KEY)
    }

    /// Loads a blob written by some other build of the application.
    fn load_raw(blob: &str) -> Option<Settings> {
        let mut storage = TestStorage::default();
        storage.set_string(eframe::APP_KEY, blob.to_owned());
        eframe::get_value(&storage, eframe::APP_KEY)
    }

    #[test]
    fn defaults_match_the_applications_own() {
        let d = Settings::default();
        assert!(d.show_metrics);
        assert!(d.preview_color_font);
        assert_eq!(d.preview_font_size, DEFAULT_PREVIEW_FONT_SIZE);
        assert_eq!(d.zoom_level, MIN_ZOOM_LEVEL);
        assert_eq!(d.specimen, SpecimenOptions::default());
        // The one severity that starts out hidden; see `IssueFilter`.
        assert!(!d.issue_filter.notes);
        assert!(d.issue_filter.errors && d.issue_filter.warnings && d.issue_filter.todos);
    }

    #[test]
    fn every_field_survives_a_save_and_load() {
        let mut before = Settings {
            font_dir: Some(PathBuf::from("/tmp/font")),
            preview_text: "가나다\nabc".to_string(),
            preview_backend: "rustybuzz".to_string(),
            preview_font_size: 48.0,
            preview_color_font: false,
            bottom_panel_tab: Some(SEARCH_TAB),
            issue_filter: IssueFilter {
                errors: false,
                warnings: true,
                todos: false,
                notes: true,
            },
            show_metrics: false,
            escape_mode: true,
            zoom_level: 4,
            specimen: SpecimenOptions {
                show_undeclared: true,
                group_by_block: false,
                ..SpecimenOptions::default()
            },
            ..Settings::default()
        };
        before.remember_face(Path::new("/tmp/font"), "wide");

        assert_eq!(round_trip(&before), Some(before));
    }

    /// A `None` from `get_value` resets everything at once, so the two ways a
    /// blob can be out of step with this build must not produce one. First: a
    /// build that did not yet have some of these fields.
    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s = load_raw("(zoom_level: 5, escape_mode: true)").unwrap();
        assert_eq!(s.zoom_level, 5);
        assert!(s.escape_mode);
        assert!(s.show_metrics);
        assert_eq!(s.preview_font_size, DEFAULT_PREVIEW_FONT_SIZE);
        assert_eq!(s.font_dir, None);
    }

    /// Second: a build that had a field this one has since dropped.
    #[test]
    fn unknown_fields_are_ignored() {
        let s = load_raw("(zoom_level: 3, gone_in_this_build: 7)").unwrap();
        assert_eq!(s.zoom_level, 3);
    }

    /// A blob that will not parse at all is the case `clamp` cannot help with;
    /// it degrades to the defaults rather than to a panic.
    #[test]
    fn an_unreadable_blob_loads_as_nothing() {
        assert_eq!(load_raw("not ron at all {{{"), None);
    }

    #[test]
    fn clamp_forces_every_field_into_range() {
        let mut s = Settings {
            preview_font_size: 1e9,
            zoom_level: 99,
            bottom_panel_tab: Some(SEARCH_TAB + 1),
            ..Settings::default()
        };
        s.clamp();
        assert_eq!(s.preview_font_size, MAX_PREVIEW_FONT_SIZE);
        assert_eq!(s.zoom_level, MAX_ZOOM_LEVEL);
        assert_eq!(s.bottom_panel_tab, None);

        let mut s = Settings {
            preview_font_size: f32::NAN,
            zoom_level: 0,
            bottom_panel_tab: Some(SEARCH_TAB),
            ..Settings::default()
        };
        s.clamp();
        assert_eq!(s.preview_font_size, DEFAULT_PREVIEW_FONT_SIZE);
        assert_eq!(s.zoom_level, MIN_ZOOM_LEVEL);
        assert_eq!(s.bottom_panel_tab, Some(SEARCH_TAB));
    }

    /// The directory is remembered for the *next* run, which need not start in
    /// the working directory this one did: `uniform font/` and
    /// `uniform /path/to/font` are the same directory and must not be saved as
    /// two, nor saved in a form that resolves elsewhere tomorrow.
    #[test]
    fn a_relative_directory_is_saved_as_an_absolute_one() {
        let cwd = std::env::current_dir().unwrap();
        let relative = Path::new("font");
        let absolute = cwd.join("font");

        let mut s = Settings::default();
        s.set_font_dir(relative);
        assert_eq!(s.font_dir, Some(absolute.clone()));

        s.remember_face(relative, "wide");
        assert_eq!(s.face_for(&absolute), Some("wide"));
        // And the other way around: the same directory named absolutely finds
        // what the relative name stored, rather than adding a second entry.
        s.remember_face(&absolute, "narrow");
        assert_eq!(s.faces.len(), 1);
        assert_eq!(s.face_for(relative), Some("narrow"));
    }

    #[test]
    fn face_memory_is_per_directory_and_most_recent_first() {
        let mut s = Settings::default();
        s.remember_face(Path::new("/a"), "wide");
        s.remember_face(Path::new("/b"), "narrow");
        assert_eq!(s.face_for(Path::new("/a")), Some("wide"));
        assert_eq!(s.face_for(Path::new("/b")), Some("narrow"));
        assert_eq!(s.face_for(Path::new("/c")), None);

        // Re-choosing moves the directory to the front rather than duplicating it.
        s.remember_face(Path::new("/a"), "mono");
        assert_eq!(s.faces.len(), 2);
        assert_eq!(s.face_for(Path::new("/a")), Some("mono"));
        assert_eq!(s.faces[0].0, PathBuf::from("/a"));
    }

    #[test]
    fn face_memory_drops_the_least_recent_past_the_bound() {
        let mut s = Settings::default();
        for i in 0..MAX_REMEMBERED_FACES + 4 {
            s.remember_face(Path::new(&format!("/dir{i}")), "face");
        }
        assert_eq!(s.faces.len(), MAX_REMEMBERED_FACES);
        assert_eq!(s.face_for(Path::new("/dir0")), None);
        assert_eq!(
            s.face_for(Path::new(&format!("/dir{}", MAX_REMEMBERED_FACES + 3))),
            Some("face")
        );
    }

    /// `clamp` also has to bound a list that grew in a file rather than
    /// through `remember_face`.
    #[test]
    fn clamp_bounds_the_face_list() {
        let mut s = Settings {
            faces: (0..100)
                .map(|i| (PathBuf::from(format!("/dir{i}")), "face".to_string()))
                .collect(),
            ..Settings::default()
        };
        s.clamp();
        assert_eq!(s.faces.len(), MAX_REMEMBERED_FACES);
    }
}
