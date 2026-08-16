//! Applying a [`crate::fix`] plan to the open documents.
//!
//! The plan is made in the background against a *copy* of the whole font
//! ([`super::background::UniformApp::run_clearance_optimizer`]); this is the
//! half that needs the editor, and it is [`super::resize`]'s shape for the same
//! reason: the lines to rewrite may be in any `.unf` of the directory, open or
//! not, so the files are opened first and every rewrite goes through the
//! documents rather than through the disk.
//!
//! **Nothing is written to disk.** A fix run in the editor is an edit like any
//! other: the files are left dirty and the user saves them, or does not. That
//! is also what makes it undoable, which the `fix` subcommand's own writes are
//! not.
//!
//! **One undo entry per file.** Every line one file changes goes into a single
//! [`UndoOp::Compound`], so one undo takes the whole run back — a run that
//! touched forty glyphs is one act, not forty.
//!
//! The plan is applied to what the documents are *now*, which need not be what
//! they were when it was planned: the user can type while it runs. So each fix
//! is re-found by the glyph's name rather than trusted to a line number
//! ([`crate::fix::find_glyph_item`], [`crate::fix::nth_compose_line`]), and one
//! that no longer lands anywhere is dropped rather than applied to whatever is
//! at that line now.

use std::path::PathBuf;

use super::docs::{load_open_document, shadowed_by_open};
use super::*;
use crate::editor::undo::UndoOp;
use crate::fix::clearance::DocumentFixes;

impl UniformApp {
    pub(super) fn apply_clearance_plan(&mut self, plan: Vec<DocumentFixes>) {
        if plan.is_empty() {
            self.set_status(
                "No clearance to optimize: every IDC line is as close to its rule as its \
                 variants allow."
                    .to_string(),
            );
            return;
        }
        self.open_files_for_plan(&plan);

        let (mut lines, mut files) = (0usize, 0usize);
        for doc_fixes in &plan {
            let Some(idx) = self
                .open_documents
                .iter()
                .position(|d| d.document.path == doc_fixes.path)
            else {
                continue;
            };
            let doc = &mut self.open_documents[idx];
            let caret_before = doc.editor_state.cursor;
            let mut ops: Vec<UndoOp> = Vec::new();
            for fix in &doc_fixes.fixes {
                let Some(line) = locate(doc, &fix.glyph, fix.item_idx, fix.compose_idx) else {
                    continue;
                };
                let Some(DocLine::Text(text)) = doc.lines.get_mut(line) else {
                    continue;
                };
                if *text == fix.new_line {
                    continue;
                }
                ops.push(UndoOp::Text {
                    line,
                    col: 0,
                    old: text.clone(),
                    new: fix.new_line.clone(),
                });
                *text = fix.new_line.clone();
                lines += 1;
            }
            if ops.is_empty() {
                continue;
            }
            doc.editor_state.undo.break_coalesce();
            doc.editor_state
                .undo
                .push_compound(ops, caret_before, doc.editor_state.cursor);
            doc.editor_state.undo.break_coalesce();
            // The one path that also rederives the document and marks it
            // dirty; a rewritten IDC line changes what the glyph is.
            doc.flush_pending_changes_forced();
            files += 1;
        }

        self.set_status(match lines {
            0 => {
                "The clearance plan no longer fits the documents; nothing was changed.".to_string()
            }
            _ => format!(
                "Optimized clearance on {lines} line{} in {files} file{}.",
                if lines == 1 { "" } else { "s" },
                if files == 1 { "" } else { "s" },
            ),
        });
    }

    /// Open the files the plan rewrites that are not open yet, in parallel and
    /// appended only, so pane document indices stay valid. Same rule as
    /// [`super::resize`].
    fn open_files_for_plan(&mut self, plan: &[DocumentFixes]) {
        let to_open: Vec<PathBuf> = plan
            .iter()
            .map(|f| &f.path)
            .filter(|path| !shadowed_by_open(&self.open_documents, path))
            .filter(|path| self.font_base_docs.iter().any(|b| &&b.path == path))
            .cloned()
            .collect();
        if to_open.is_empty() {
            return;
        }
        let base_docs = &self.font_base_docs;
        let loaded: Vec<_> = std::thread::scope(|s| {
            let handles: Vec<_> = to_open
                .iter()
                .map(|path| {
                    let path = path.clone();
                    let base_gen = base_docs
                        .iter()
                        .find(|b| b.path == path)
                        .map(|b| (b.edit_gen, b.content_gen));
                    s.spawn(move || load_open_document(path, base_gen).ok())
                })
                .collect();
            handles
                .into_iter()
                .filter_map(|h| h.join().ok().flatten())
                .collect()
        });
        self.open_documents.extend(loaded);
    }
}

/// The DocLine the fix's IDC line is on *now*, or `None` when the document has
/// moved on from the plan.
fn locate(doc: &OpenDocument, glyph: &str, item_idx: usize, compose_idx: usize) -> Option<usize> {
    let item = crate::fix::find_glyph_item(&doc.document, glyph, item_idx)?;
    let starts = &doc.document.item_line_starts;
    let header = starts.get(item).copied()?;
    let end = starts.get(item + 1).copied().unwrap_or(doc.lines.len());
    let text = |i: usize| match doc.lines.get(i) {
        Some(DocLine::Text(t)) => Some(t.as_str()),
        _ => None,
    };
    crate::fix::nth_compose_line(&text, header + 1..end.min(doc.lines.len()), compose_idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::Settings;

    /// Its own directory per test, removed when the test ends. Written inline:
    /// `font/` is downstream data and no test may read it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "uniform-fix-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id(),
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Two parts with a canyon down the middle, in a file the editor has not
    /// opened: exactly the case the Font menu item exists for.
    const SOURCE: &str = "\
meta height 4
meta ascent 3
meta descent 1

audit ideal-clearance test-* 0 1

glyph a:4x4 4 4
@@@@....
@@@@....
@@@@....
@@@@....

glyph b:4x4 4 4
..@@@@@@
..@@@@@@
..@@@@@@
..@@@@@@

glyph test-x 8 4
\u{2FF0} a:4x4 b:4x4
";

    /// Runs the optimizer to completion, as the frame loop would.
    fn run(app: &mut UniformApp, ctx: &egui::Context) {
        app.run_clearance_optimizer(ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.fix_running {
            assert!(
                std::time::Instant::now() < deadline,
                "the optimizer never delivered a plan"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.pump_background_pipeline(ctx);
        }
    }

    fn compose_line(app: &UniformApp) -> String {
        let doc = app
            .open_documents
            .iter()
            .find(|d| d.document.path.file_name().unwrap() == "a.unf")
            .expect("the file the plan touches is opened");
        doc.lines
            .iter()
            .find_map(|l| match l {
                DocLine::Text(t) if t.starts_with('\u{2FF0}') => Some(t.clone()),
                _ => None,
            })
            .expect("the IDC line")
    }

    #[test]
    fn optimizing_rewrites_an_unopened_file_and_leaves_it_dirty() {
        let dir = TempDir::new("apply");
        std::fs::write(dir.0.join("a.unf"), SOURCE).unwrap();
        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        assert!(
            app.open_documents.is_empty(),
            "nothing is open to begin with"
        );

        run(&mut app, &ctx);
        assert_eq!(compose_line(&app), "\u{2FF0} 1 a:4x4 -2 b:4x4");
        let doc = &app.open_documents[0];
        assert!(doc.document.dirty, "an editor fix is an edit, not a write");
        assert_eq!(
            std::fs::read_to_string(dir.0.join("a.unf")).unwrap(),
            SOURCE,
            "and nothing reaches the disk until the user saves",
        );

        // One undo takes the whole run back, however many lines it touched.
        let doc = &mut app.open_documents[0];
        doc.editor_state.undo.undo(&mut doc.lines);
        assert_eq!(compose_line(&app), "\u{2FF0} a:4x4 b:4x4");
    }

    /// A second run has nothing left to do, and says so rather than editing.
    #[test]
    fn a_source_already_at_its_best_is_left_alone() {
        let dir = TempDir::new("noop");
        std::fs::write(dir.0.join("a.unf"), SOURCE).unwrap();
        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        run(&mut app, &ctx);
        let after_first = compose_line(&app);

        run(&mut app, &ctx);
        assert_eq!(compose_line(&app), after_first);
        assert!(
            app.status_message
                .as_ref()
                .is_some_and(|(m, _)| m.contains("No clearance to optimize")),
            "{:?}",
            app.status_message,
        );
    }
}
