//! Open documents: loading, flushing, saving and exporting.

use super::*;

/// Whether a directory-snapshot document at `path` is shadowed by an open
/// document editing the same file.
pub(super) fn shadowed_by_open(open_documents: &[OpenDocument], path: &std::path::Path) -> bool {
    open_documents
        .iter()
        .any(|open_doc| open_doc.document.path == path)
}

pub(super) fn collect_effective_docs<'a>(
    open_documents: &'a [OpenDocument],
    font_base_docs: &'a [Document],
) -> Vec<&'a Document> {
    let mut all_docs: Vec<&Document> = open_documents
        .iter()
        .map(|open_doc| &open_doc.document)
        .collect();
    for base_doc in font_base_docs {
        if !shadowed_by_open(open_documents, &base_doc.path) {
            all_docs.push(base_doc);
        }
    }
    all_docs.sort_by(|a, b| a.path.cmp(&b.path));
    all_docs
}

pub struct OpenDocument {
    pub document: Document,
    pub lines: Vec<DocLine>,
    pub editor_state: EditorState,
}

impl OpenDocument {
    /// Flush pending line-level edits into the `Document` model, if any.
    pub(super) fn flush_pending_changes(&mut self) {
        if self.editor_state.has_pending_document_sync() {
            crate::editor::document_view::flush_document_changes(
                &mut self.lines,
                &mut self.document,
                &mut self.editor_state,
            );
        }
    }
}

/// Loads a file from disk into a fresh `OpenDocument`: parse, serialize to
/// canonical text, re-derive from the resulting doclines, and bump the
/// generation counters past the directory snapshot's (`base_gen`).  Shared by
/// interactive open and the parallel loads `execute_rename` performs.
pub(super) fn load_open_document(
    path: PathBuf,
    base_gen: Option<(u64, u64)>,
) -> anyhow::Result<OpenDocument> {
    let doc = document_io::parse_document(&path)?;
    let mut buf = Vec::new();
    document_io::serialize_document(&doc, &mut buf).ok();
    let canonical = String::from_utf8(buf).unwrap_or_default();
    let mut lines = document_io::parse_doclines(&canonical);
    if lines.is_empty() {
        lines.push(crate::document::DocLine::Text(String::new()));
    }
    let mut doc = doc;
    if let Ok((fresh_doc, _)) = document_io::derive_document(&lines, path.clone()) {
        doc = fresh_doc;
    }
    let (edit_gen, content_gen) = base_gen
        .map(|(e, c)| (e.wrapping_add(1), c.wrapping_add(1)))
        .unwrap_or((1, 1));
    doc.edit_gen = edit_gen;
    doc.content_gen = content_gen;
    Ok(OpenDocument {
        document: doc,
        lines,
        editor_state: EditorState::new(),
    })
}

impl UniformApp {
    fn flush_open_document(&mut self, idx: usize) {
        let Some(doc) = self.open_documents.get_mut(idx) else {
            return;
        };
        doc.flush_pending_changes();
    }

    fn flush_all_open_documents(&mut self) {
        for doc in &mut self.open_documents {
            doc.flush_pending_changes();
        }
    }

    /// Opens `path` into a pane and focuses it. Which pane is
    /// [`Panes::show_document`]'s call: the placeholder if one is up,
    /// otherwise the pane that last had the focus — and a file that is
    /// already on screen only moves the focus to the pane showing it, since
    /// no document is ever shown by two panes at once.
    pub(super) fn open_file(&mut self, path: PathBuf) {
        if let Some(idx) = self.active_doc_idx() {
            self.flush_open_document(idx);
        }
        if let Some(idx) = self
            .open_documents
            .iter()
            .position(|d| d.document.path == path)
        {
            self.panes.show_document(idx);
            return;
        }

        // Replacing the directory snapshot with an opened file is a new
        // source revision even when the path is unchanged. The file may have
        // changed on disk since the folder was loaded.
        let base_gen = self
            .font_base_docs
            .iter()
            .find(|base| base.path == path)
            .map(|b| (b.edit_gen, b.content_gen));
        match load_open_document(path.clone(), base_gen) {
            Ok(open_doc) => {
                self.open_documents.push(open_doc);
                self.panes.show_document(self.open_documents.len() - 1);
                self.set_status(format!("Opened {}", path.display()));
            }
            Err(e) => {
                self.set_status(format!("Error: {e}"));
            }
        }
    }

    pub(super) fn collect_all_docs(&self) -> Vec<&Document> {
        collect_effective_docs(&self.open_documents, &self.font_base_docs)
    }

    fn has_unsaved_changes(&self) -> bool {
        self.open_documents.iter().any(|d| {
            d.document.dirty || d.editor_state.has_pending_document_sync()
        })
    }

    pub(super) fn save_all(&mut self) -> bool {
        for doc in &mut self.open_documents {
            doc.flush_pending_changes();
            if !doc.document.dirty {
                continue;
            }
            let mut buf = Vec::new();
            if let Err(e) = document_io::serialize_doclines(&doc.lines, &mut buf)
                .and_then(|()| {
                    document_io::write_and_sync(&doc.document.path, &buf)
                })
            {
                self.status_message =
                    Some((format!("Save error: {e}"), std::time::Instant::now()));
                return false;
            }
            doc.document.dirty = false;
            doc.editor_state.undo.mark_saved();
        }
        true
    }

    pub(super) fn confirm_close_and_maybe_save(&mut self) -> bool {
        if !self.has_unsaved_changes() {
            return true;
        }

        let save = "Save";
        let dont_save = "Don't Save";
        let cancel = "Cancel";

        let result = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Do you want to save changes before closing?")
            .set_description("Your unsaved changes will be lost if you close without saving.")
            .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                save.into(),
                dont_save.into(),
                cancel.into(),
            ))
            .show();

        match &result {
            rfd::MessageDialogResult::Yes => {
                self.save_all()
            }
            rfd::MessageDialogResult::Custom(s) if s == save => {
                self.save_all()
            }
            rfd::MessageDialogResult::No => true,
            rfd::MessageDialogResult::Custom(s) if s == dont_save => true,
            _ => false,
        }
    }

    pub(super) fn export_to_path(&mut self, path: PathBuf) {
        self.flush_all_open_documents();
        let all_docs = self.collect_all_docs();
        let Some(font_bytes) = crate::render::build_font_from_documents(&all_docs) else {
            self.set_status("Export failed: could not build font".to_string());
            return;
        };
        let is_woff2 = path.extension().and_then(|e| e.to_str()) == Some("woff2");
        let output_bytes = if is_woff2 {
            match crate::render::ttf_to_woff2(&font_bytes) {
                Ok(b) => b,
                Err(e) => {
                    self.set_status(format!("Export error: {e}"));
                    return;
                }
            }
        } else {
            font_bytes
        };
        match std::fs::write(&path, &output_bytes) {
            Ok(()) => {
                self.last_export_path = Some(path.clone());
                self.set_status(format!(
                    "Exported {} ({} bytes)",
                    path.display(),
                    output_bytes.len(),
                ));
            }
            Err(e) => {
                self.set_status(format!("Export error: {e}"));
            }
        }
    }

    pub(super) fn export_with_dialog(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export Font")
            .add_filter("TrueType Font", &["ttf"])
            .add_filter("WOFF2 Font", &["woff2"]);
        if let Some(ref last) = self.last_export_path {
            if let Some(dir) = last.parent() {
                dialog = dialog.set_directory(dir);
            }
            if let Some(name) = last.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy().to_string());
            }
        } else if let Some(ref dir) = self.font_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.export_to_path(path);
        }
    }

    pub(super) fn save_active(&mut self) {
        if let Some(idx) = self.active_doc_idx()
            && let Some(doc) = self.open_documents.get_mut(idx) {
                doc.flush_pending_changes();
                let mut buf = Vec::new();
                let result = document_io::serialize_doclines(&doc.lines, &mut buf)
                    .and_then(|()| {
                        document_io::write_and_sync(&doc.document.path, &buf)
                    });
                let path_display = doc.document.path.display().to_string();
                match result {
                    Ok(()) => {
                        doc.document.dirty = false;
                        doc.editor_state.undo.mark_saved();
                        self.status_message =
                            Some((format!("Saved {path_display}"), std::time::Instant::now()));
                    }
                    Err(e) => {
                        self.status_message =
                            Some((format!("Save error: {e}"), std::time::Instant::now()));
                    }
                }
            }
    }
}
