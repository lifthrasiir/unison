//! Carrying out a glyph resize across every file that refers to the glyph.
//!
//! The editor works out *what* the resize is (see
//! [`crate::editor::glyph_resize`], which also holds the rules); this is the
//! half that needs the whole font: a `ref` naming the resized glyph may sit in
//! any `.unf` of the directory, open or not, and its offset has to move with
//! the glyph. So the shape of this file is [`super::rename`]'s — find the
//! unopened files that mention the name, open them, then rewrite every open
//! document — and for the same reason: those two are the only editor actions
//! that reach outside the document they were invoked in.
//!
//! **One undo entry per file.** Every line a file changes — the glyph's own
//! block included — goes into a single [`UndoOp::Compound`], so one undo takes
//! the resize back whole. A resize split across entries would leave a header
//! describing a grid that no longer matches it, which is exactly the state the
//! parser cannot read.

use std::collections::HashSet;
use std::path::PathBuf;

use super::docs::{load_open_document, shadowed_by_open};
use super::*;
use crate::editor::glyph_resize::{self, ResizeAction, ResolveEnv};

/// Whether an unopened file can possibly hold a `ref` to one of `names`.
/// Read from the parsed items of the directory snapshot, so no file is opened
/// to find out.
fn doc_may_reference(items: &[crate::document::DocumentItem], names: &HashSet<String>) -> bool {
    items.iter().any(|item| match item {
        crate::document::DocumentItem::Glyph { body, .. } => {
            body.refs.iter().any(|r| names.contains(&r.name))
        }
        _ => false,
    })
}

impl UniformApp {
    pub(super) fn execute_resize(&mut self, action: &ResizeAction) {
        let all_docs =
            super::docs::collect_effective_docs(&self.open_documents, &self.font_base_docs);
        let names = glyph_resize::target_names(&all_docs, &self.name_parts, &action.glyph_name);
        drop(all_docs);

        // Open the files that name the glyph, so their `ref` lines can move
        // with it. Loaded in parallel, appended only, so pane document
        // indices stay valid.
        let to_open: Vec<PathBuf> = self
            .font_base_docs
            .iter()
            .filter(|base| {
                !shadowed_by_open(&self.open_documents, &base.path)
                    && doc_may_reference(&base.items, &names)
            })
            .map(|base| base.path.clone())
            .collect();
        if !to_open.is_empty() {
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
            for open_doc in loaded {
                self.open_documents.push(open_doc);
            }
        }

        let mut changed_count = 0usize;
        for doc in &mut self.open_documents {
            let env = ResolveEnv {
                named_glyphs: &self.named_glyphs,
                name_parts: &self.name_parts,
                alt_index: &self.alt_index,
            };
            // The glyph is defined in exactly one file; everywhere else only
            // the `ref` lines move.
            let define_item = (doc.document.path == action.path)
                .then(|| defining_item(&doc.document, action))
                .flatten();
            if define_item.is_none() && !doc_may_reference(&doc.document.items, &names) {
                continue;
            }
            let plan = glyph_resize::plan_document_resize(
                &doc.document,
                &doc.lines,
                &names,
                action.deltas,
                define_item,
                env,
                action.kind,
                self.font_meta,
            );
            if plan.is_empty() {
                continue;
            }
            let caret_before = doc.editor_state.cursor;
            let ops = glyph_resize::apply_plan(&mut doc.lines, plan);
            if ops.is_empty() {
                continue;
            }
            doc.editor_state.undo.break_coalesce();
            doc.editor_state
                .undo
                .push_compound(ops, caret_before, doc.editor_state.cursor);
            doc.editor_state.undo.break_coalesce();
            // The plan already leaves header and grid agreeing, so the
            // reconcile pass this goes through has nothing to fix; it is the
            // one path that also rederives and marks the file dirty.
            doc.flush_pending_changes_forced();
            changed_count += 1;
        }

        if changed_count > 0 {
            let d = action.deltas;
            self.set_status(format!(
                "Resized glyph '{}' ({:+} {:+} {:+} {:+} l/r/t/b, {} file{})",
                action.glyph_name,
                d.left,
                d.right,
                d.top,
                d.bottom,
                changed_count,
                if changed_count == 1 { "" } else { "s" },
            ));
        }
    }
}

/// The item the action names, if it is still the glyph it named. The document
/// may have been rederived between the frame that produced the action and
/// this, which renumbers items; the name is what identifies the glyph, and the
/// index is only a hint.
fn defining_item(doc: &Document, action: &ResizeAction) -> Option<usize> {
    let is_target = |idx: usize| {
        matches!(
            doc.items.get(idx),
            Some(crate::document::DocumentItem::Glyph { name, .. })
                if name.0 == action.glyph_name
        )
    };
    if is_target(action.item_idx) {
        return Some(action.item_idx);
    }
    (0..doc.items.len()).find(|&idx| is_target(idx))
}
