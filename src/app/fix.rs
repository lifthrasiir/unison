//! Applying a [`crate::fix`] plan to the open documents.
//!
//! The plan is made in the background against a *copy* of the whole font
//! ([`crate::app::UniformApp::run_clearance_optimizer`]); this is the
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
        let paths: Vec<PathBuf> = plan
            .iter()
            .map(|f| f.path.clone())
            .filter(|path| self.font_base_docs.iter().any(|b| &b.path == path))
            .collect();
        self.open_for_edit(&paths);

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
            // `locate` reads the derived document; a line the caret is still
            // on has to be in it first.
            if doc.editor_state.has_pending_document_sync() {
                doc.flush_pending_changes_forced();
            }
            let caret_before = doc.editor_state.cursor;
            let mut ops: Vec<UndoOp> = Vec::new();
            for fix in &doc_fixes.fixes {
                let Some(line) = locate(doc, fix) else {
                    continue;
                };
                let Some(DocLine::Text(text)) = doc.lines.get_mut(line) else {
                    continue;
                };
                if **text == fix.new_line {
                    continue;
                }
                ops.push(UndoOp::Text {
                    line,
                    col: 0,
                    old: String::clone(text),
                    new: fix.new_line.clone(),
                });
                **text = fix.new_line.clone();
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
}

/// The DocLine the fix's IDC line is on *now*, or `None` when the document has
/// moved on from the plan — the glyph is gone, or its line no longer says what
/// the plan read (the user edited it while the plan was being made).
fn locate(doc: &OpenDocument, fix: &crate::fix::clearance::ClearanceFix) -> Option<usize> {
    let item = crate::fix::find_glyph_item(&doc.document, &fix.glyph, fix.item_idx)?;
    let crate::document::DocumentItem::Glyph { body, .. } = &doc.document.items[item] else {
        return None;
    };
    if body.compose.get(fix.compose_idx)?.format_line() != fix.old_line {
        return None;
    }
    let compose_idx = fix.compose_idx;
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

    /// Two parts with a canyon down the middle and a cell left at the right
    /// edge, in a file the editor has not opened: exactly the case the Font
    /// menu item exists for.
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

glyph test-x 9 4
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
                DocLine::Text(t) if t.starts_with('\u{2FF0}') => Some(String::clone(t)),
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
        assert_eq!(compose_line(&app), "\u{2FF0} a:4x4 1 b:4x4");
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

    /// The plan is made from what the buffer says, including a line the
    /// editor has not re-derived yet because the caret is still on it: a plan
    /// from the stale derive would write the old line back over the edit.
    #[test]
    fn optimizing_reads_an_edit_not_yet_rederived() {
        let dir = TempDir::new("pending");
        std::fs::write(dir.0.join("a.unf"), SOURCE).unwrap();
        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        app.open_file(dir.0.join("a.unf"));
        let doc = &mut app.open_documents[0];
        let at = doc
            .lines
            .iter()
            .position(|l| l.as_text().is_some_and(|t| t.starts_with('\u{2FF0}')))
            .unwrap();
        doc.lines[at] = DocLine::text("\u{2FF0} b:4x4 a:4x4".to_string());
        doc.editor_state.pending_reparse_line = Some(at);

        run(&mut app, &ctx);
        assert!(
            compose_line(&app).starts_with("\u{2FF0} b:4x4 "),
            "the edit survives: {}",
            compose_line(&app)
        );
    }

    /// And a line edited *while* the plan is being made is not overwritten
    /// when it lands: the fix is for the line the plan read, not for whatever
    /// the same glyph's IDC line says by then.
    #[test]
    fn a_line_edited_during_the_run_is_left_alone() {
        let dir = TempDir::new("during");
        std::fs::write(dir.0.join("a.unf"), SOURCE).unwrap();
        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        app.open_file(dir.0.join("a.unf"));

        app.run_clearance_optimizer(&ctx);
        let doc = &mut app.open_documents[0];
        let at = doc
            .lines
            .iter()
            .position(|l| l.as_text().is_some_and(|t| t.starts_with('\u{2FF0}')))
            .unwrap();
        doc.lines[at] = DocLine::text("\u{2FF0} b:4x4 a:4x4".to_string());
        doc.flush_pending_changes_forced();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.fix_running {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.pump_background_pipeline(&ctx);
        }
        assert_eq!(compose_line(&app), "\u{2FF0} b:4x4 a:4x4");
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
