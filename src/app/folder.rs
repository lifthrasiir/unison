//! *File ▸ Open Folder*: replacing the font the editor works on.

use super::background::finish;
use super::*;

impl UniformApp {
    /// Makes `dir` the font directory, dropping everything the old one left.
    ///
    /// The caller has already asked about unsaved changes.
    pub(super) fn switch_folder(&mut self, ctx: &egui::Context, dir: PathBuf) {
        self.font_dir = Some(dir.clone());
        // A run still out reports on the old folder: its result is dropped
        // when it lands, and its flag released now so one can start here.
        self.folder_gen = self.folder_gen.wrapping_add(1);
        if std::mem::take(&mut self.assert_running) {
            finish(&mut self.bg_tasks.test);
        }
        if std::mem::take(&mut self.fix_running) {
            finish(&mut self.bg_tasks.optimize);
        }
        self.open_documents.clear();
        // The pane layout is not carried across folders: its documents
        // are gone, and pane indices would dangle. The navigation
        // history indexes the same list, so it goes with them. The zoom
        // level is a view preference rather than part of that layout, so
        // it does carry.
        self.panes = Panes::new_with_zoom(self.panes.focused().zoom_level);
        self.nav_history.clear();
        // Its hits name files that are no longer the ones on screen.
        self.search = SearchState::default();
        self.sidebar.set_directory(&dir);
        self.watch.set_directory(&dir, ctx);
        // Through the watch's (just cleared) cache, so the first refresh
        // in this folder compares against what was read here.
        let (base_docs, parse_errors, sources) = self.watch.load_directory(&dir);
        self.install_font_snapshot(base_docs, parse_errors, sources);
        // The faces of the old folder mean nothing in the new one. This
        // folder's own last face is applied straight away, from a scan of
        // its `face` lines rather than from a resolve — exactly as at
        // startup, and for the same reason: a face applied later is a
        // second full build.
        self.face_ids = {
            let refs: Vec<&Document> = self.font_base_docs.iter().map(|d| &**d).collect();
            crate::faces::FaceSet::collect(&refs)
                .faces
                .iter()
                .map(|f| f.id.clone())
                .collect()
        };
        self.selected_face = self
            .settings
            .face_for(&dir)
            .filter(|f| self.face_ids.iter().any(|id| id == f))
            .unwrap_or_default()
            .to_string();
        // Both background stages are still working on the folder that just
        // went away. Nothing they produce is wanted, and the font build in
        // particular holds the contour cache this thread is about to clear
        // — so it would be waited on rather than merely wasted.
        self.rebuild_cancel.cancel();
        crate::parallel::lock_memo(&self.contour_cache).clear();
        crate::parallel::lock_memo(&self.composite_grid_cache).clear();
        self.font_build_gen = self.font_build_gen.wrapping_add(1);
        // Neither the font nor the derived data is built here: a folder on
        // a share takes tens of seconds to build and resolve, and doing it
        // on this thread is the freeze that startup no longer has. The
        // pipeline picks both up on the next frame, and until it does this
        // folder looks like a directory whose first build has not landed —
        // which is exactly what it is.
        self.arm_initial_font_build();
        self.shaped_preview.invalidate_font(self.font_data_gen);
        // The old folder's derived data is *wrong* here rather than merely
        // stale, so it is dropped rather than left to be replaced.
        self.named_glyphs = Arc::default();
        self.resolved_gen = self.resolved_gen.wrapping_add(1);
        self.composite_seeds = Arc::default();
        self.alt_index = Default::default();
        self.name_parts = NamePartsMap::default();
        self.char_props = Default::default();
        self.color_aliases = Default::default();
        self.anchor_aligns = Default::default();
        self.font_meta = Default::default();
        self.scoped_name_parts = Default::default();
        self.exists_matches = Default::default();
        // Whatever keys a cache on the derived data (the palette's glyph list)
        // has to see it replaced.
        self.derived_gen = self.derived_gen.wrapping_add(1);
        self.issues.clear();
        self.issues_line_ids = Default::default();
        self.glyph_flags = Default::default();
        self.assert_issues.clear();
        self.assert_line_ids = Default::default();
        self.assert_gen = self.assert_gen.wrapping_add(1);
        self.issue_marks = Default::default();
        self.issue_marks_key = None;
        // No resolve has run for this generation: what arms the derived-data
        // rebuild on the next pump.
        self.named_glyphs_gen = u64::MAX;
        self.issues_gen = u64::MAX;
        self.set_status(format!("Opened folder {}", dir.display()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::background::startup_tests::TempDir;
    use crate::app::settings::Settings;
    use crate::issues::{Issue, Severity};

    fn finding(file: &std::path::Path) -> Issue {
        Issue {
            severity: Severity::Error,
            glyph: None,
            message: "assert same a b".to_string(),
            file: file.to_path_buf(),
            line: 0,
            file_line: 1,
        }
    }

    /// Nothing the old folder produced survives into the new one: not its
    /// assertion findings, not the derived data the palette caches on, and
    /// not a background result that lands after the switch.
    #[test]
    fn a_new_folder_starts_with_nothing_of_the_old_one() {
        let old = TempDir::new("folder-old");
        let new = TempDir::new("folder-new");
        std::fs::write(old.0.join("a.unf"), "glyph a 1 1\n@@\n").unwrap();
        std::fs::write(new.0.join("b.unf"), "glyph b 1 1\n@@\n").unwrap();
        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(old.0.clone()));

        app.assert_issues = vec![finding(&old.0.join("a.unf"))];
        let derived_gen = app.derived_gen;
        // A run started in the old folder, still out when the folder changes.
        app.assert_running = true;
        let late = app.assert_tx.clone();
        let late_gen = app.folder_gen;

        app.switch_folder(&ctx, new.0.clone());
        assert!(app.assert_issues.is_empty(), "the old folder's findings");
        assert_ne!(
            app.derived_gen, derived_gen,
            "the palette's cached glyph list"
        );
        assert!(!app.assert_running, "a new run can start in the new folder");

        late.send((late_gen, vec![finding(&old.0.join("a.unf"))]))
            .unwrap();
        app.pump_background_pipeline(&ctx);
        assert!(app.assert_issues.is_empty(), "the late result is dropped");
    }
}
