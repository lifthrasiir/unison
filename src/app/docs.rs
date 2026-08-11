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

/// One directory-snapshot file as it was read: the text it was parsed from and
/// the hash of the bytes behind that text.
///
/// Kept so that the two things a Ctrl/Cmd+click does — listing every appearance
/// of a name, and opening the file that declares it — read no files at all. The
/// snapshot is refreshed by the watcher's scan thread, which is where the
/// editor's file I/O belongs (see [`super::watch`]); serving a click from it
/// trades a read on the UI thread for text that can be as stale as the snapshot
/// the rest of the editor already works from. When it *is* stale, the next scan
/// finds the hash it recorded no longer matches disk and reloads, which is the
/// same path an external edit takes anyway.
pub(super) struct FontSource {
    pub(super) text: String,
    pub(super) hash: u64,
}

pub(super) fn font_sources_from(sources: Vec<(PathBuf, Vec<u8>)>) -> HashMap<PathBuf, FontSource> {
    sources
        .into_iter()
        .map(|(path, bytes)| {
            let source = FontSource {
                text: String::from_utf8_lossy(&bytes).into_owned(),
                hash: super::watch::hash_bytes(&bytes),
            };
            (path, source)
        })
        .collect()
}

pub struct OpenDocument {
    pub document: Document,
    pub lines: Vec<DocLine>,
    pub editor_state: EditorState,
    /// Hash of the bytes this document last read from, or wrote to, disk.
    /// The file watcher compares it against what is on disk to tell a real
    /// external change from the echo of our own save; see [`super::watch`].
    pub(super) disk_hash: Option<u64>,
    /// The file changed on disk while this buffer had unsaved edits, and the
    /// edits were kept. Saving over it asks first, and
    /// [`super::UniformApp::reload_from_disk`] is the way to drop them.
    pub(super) external_change: bool,
    /// An external change was noticed while no pane was showing this document,
    /// so its notice is still owed — a toast about a file the user cannot see
    /// explains nothing. Spent the moment a pane shows it.
    pub(super) owed_external_toast: bool,
}

impl OpenDocument {
    /// Flush pending line-level edits into the `Document` model, if any.
    pub(super) fn flush_pending_changes(&mut self) {
        if self.commit_floating_selection() || self.editor_state.has_pending_document_sync() {
            self.flush_pending_changes_forced();
        }
    }

    /// Lands a floating pixel selection in the line buffer, reporting whether
    /// there was one.
    ///
    /// A floating selection is the one edit that lives in [`EditorState`]
    /// rather than in `lines`, so whoever takes the buffer for the document's
    /// real content — a save, a pane closing over it — has to land it first or
    /// write a file without the pixels the user is holding. Losing the keyboard
    /// commits it too (`pixel_selection::reconcile`), which is exactly the
    /// problem: without this, the same edit saved or not depending on where the
    /// focus happened to be.
    fn commit_floating_selection(&mut self) -> bool {
        let Some(sel) = self
            .editor_state
            .pixel_selection
            .clone()
            .filter(|s| s.is_floating())
        else {
            return false;
        };
        crate::editor::pixel_selection::commit_and_clear(
            &self.document,
            &mut self.lines,
            &mut self.editor_state,
            &sel,
        );
        true
    }

    /// Re-derive the `Document` from the lines whether or not the editor asked
    /// for it. Used after the buffer is replaced from outside the editor, which
    /// leaves no pending-sync request behind but every line changed.
    pub(super) fn flush_pending_changes_forced(&mut self) {
        crate::editor::document_view::flush_document_changes(
            &mut self.lines,
            &mut self.document,
            &mut self.editor_state,
        );
    }
}

/// Parses source text into the pair the editor works on: parse, serialize to
/// canonical text, and re-derive from the resulting doclines.
///
/// Pure CPU over a string, which is what lets the file watcher's scan thread
/// run it: see [`super::watch`] for what runs where.
pub(super) fn document_from_source(
    content: &str,
    path: PathBuf,
) -> anyhow::Result<(Document, Vec<DocLine>)> {
    let doc = document_io::parse_document_from_str(content, path.clone())?;
    let mut buf = Vec::new();
    document_io::serialize_document(&doc, &mut buf).expect("writing to a Vec cannot fail");
    let canonical = String::from_utf8(buf).unwrap_or_default();
    let mut lines = document_io::parse_doclines(&canonical);
    if lines.is_empty() {
        lines.push(crate::document::DocLine::Text(String::new()));
    }
    let mut doc = doc;
    if let Ok((fresh_doc, _)) = document_io::derive_document(&lines, path) {
        doc = fresh_doc;
    }
    Ok((doc, lines))
}

/// Loads a file from disk into a fresh `OpenDocument`: parse, serialize to
/// canonical text, re-derive from the resulting doclines, and bump the
/// generation counters past the directory snapshot's (`base_gen`).  Shared by
/// interactive open and the parallel loads `execute_rename` performs.
pub(super) fn load_open_document(
    path: PathBuf,
    base_gen: Option<(u64, u64)>,
) -> anyhow::Result<OpenDocument> {
    // Read once and parse from those bytes, so the hash the watcher compares
    // against is the hash of exactly what was parsed.
    let bytes = std::fs::read(&path)?;
    let content = String::from_utf8_lossy(&bytes).into_owned();
    // What was just read may differ from what the snapshot holds — that is the
    // whole point of reading it — so this *is* a new source revision.
    let gens = base_gen
        .map(|(e, c)| (e.wrapping_add(1), c.wrapping_add(1)))
        .unwrap_or((1, 1));
    open_document_from_text(&content, super::watch::hash_bytes(&bytes), path, gens)
}

/// The half of [`load_open_document`] that does not touch the filesystem, for
/// text already in hand — the directory snapshot's [`FontSource`]. `hash` must
/// be the hash of the bytes `content` was decoded from, since that is what the
/// watcher compares against disk.
///
/// `gens` is the `(edit_gen, content_gen)` the document takes on, and the
/// caller decides whether that is the snapshot's own pair or one past it.
/// Taking the snapshot's pair unchanged says "this is the same source
/// revision": [`super::UniformApp::current_font_gen`] hashes `content_gen`, so
/// stepping it schedules a full font build, and text that came out of the
/// snapshot cannot have changed the font. `pixel_gen` needs no such care — a
/// freshly parsed document and a snapshot document are both at zero, since a
/// snapshot document is never edited in place.
pub(super) fn open_document_from_text(
    content: &str,
    hash: u64,
    path: PathBuf,
    gens: (u64, u64),
) -> anyhow::Result<OpenDocument> {
    let (mut doc, lines) = document_from_source(content, path)?;
    let (edit_gen, content_gen) = gens;
    doc.edit_gen = edit_gen;
    doc.content_gen = content_gen;
    Ok(OpenDocument {
        document: doc,
        lines,
        editor_state: EditorState::new(),
        disk_hash: Some(hash),
        external_change: false,
        owed_external_toast: false,
    })
}

/// Replaces an open document's buffer with what is on disk, reading and
/// parsing on the calling thread.
///
/// That makes it the user-driven reload (the sidebar command), where a click
/// is already waiting on the filesystem. The file watcher must not block a
/// frame on a read, so it has its scan thread produce the lines and calls
/// [`apply_reloaded_lines`] with them; see [`super::watch`].
pub(super) fn reload_open_document(open: &mut OpenDocument) -> anyhow::Result<()> {
    let path = open.document.path.clone();
    let bytes = std::fs::read(&path)?;
    let content = String::from_utf8_lossy(&bytes).into_owned();
    let (_, new_lines) = document_from_source(&content, path)?;
    apply_reloaded_lines(open, new_lines, super::watch::hash_bytes(&bytes));
    Ok(())
}

/// The application half of a reload: swap the buffer, record the undo entry,
/// and take the file's hash as the buffer's own. `new_lines` are the canonical
/// lines of what is on disk, and `hash` the hash of the bytes they came from.
///
/// The replacement is pushed onto the undo stack, so Cmd/Ctrl+Z walks back to
/// what was on screen before the file changed — which is the only way to get
/// it back once the buffer is gone. The stack is then marked saved, since the
/// buffer now *is* the file; undoing past it correctly makes the document
/// dirty again.
///
/// The caret is clamped rather than tracked: the new text has no relation to
/// the old one, so there is no position to follow. Grid editing is left for
/// the same reason — its `item_idx` refers to items that may no longer exist.
pub(super) fn apply_reloaded_lines(open: &mut OpenDocument, new_lines: Vec<DocLine>, hash: u64) {
    let caret_before = open.editor_state.cursor;
    let old_lines = std::mem::replace(&mut open.lines, new_lines.clone());
    if old_lines == open.lines {
        // The canonical form is unchanged (a comment respaced, a trailing
        // newline added). Nothing to record, but the hash still has to move on
        // or the file reports itself changed on every event from now on.
        open.disk_hash = Some(hash);
        return;
    }
    let caret_after = crate::editor::caret::clamp(&open.lines, caret_before);
    open.editor_state.undo.break_coalesce();
    open.editor_state
        .undo
        .push_lines(0, old_lines, new_lines, caret_before, caret_after);
    open.editor_state.undo.mark_saved();

    open.editor_state.reset_for_external_reload(caret_after);
    open.flush_pending_changes_forced();

    open.disk_hash = Some(hash);
    open.external_change = false;
    open.owed_external_toast = false;
}

pub(super) fn file_name_of(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// Asks before writing over files that changed on disk after they were opened.
/// Answering no cancels the save outright, so the version on disk survives.
fn confirm_overwrite(names: &[String]) -> bool {
    let description = if let [one] = names {
        format!(
            "{one} has changed on disk since it was opened here. \
             Saving replaces the file on disk with your version."
        )
    } else {
        format!(
            "These files have changed on disk since they were opened here:\n\n{}\n\n\
             Saving replaces them with your versions.",
            names.join("\n"),
        )
    };
    matches!(
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Overwrite the file on disk?")
            .set_description(description)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show(),
        rfd::MessageDialogResult::Yes,
    )
}

/// Asks before throwing a buffer away for the file on disk.
fn confirm_discard(name: &str) -> bool {
    matches!(
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Discard your changes?")
            .set_description(format!(
                "Reloading {name} from disk discards the unsaved changes in the editor. \
                 They can still be recovered with Undo."
            ))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show(),
        rfd::MessageDialogResult::Yes,
    )
}

impl OpenDocument {
    /// Records that `bytes` are now both the buffer and the file on disk.
    pub(super) fn mark_written(&mut self, bytes: &[u8]) {
        self.document.dirty = false;
        self.editor_state.undo.mark_saved();
        self.disk_hash = Some(super::watch::hash_bytes(bytes));
        self.external_change = false;
        self.owed_external_toast = false;
    }
}

impl UniformApp {
    /// The sidebar's "Reload from disk...": drops the buffer for the file, on
    /// confirmation. The reload is an undo entry like any other external
    /// change, so the discarded work is one Cmd/Ctrl+Z away.
    pub(super) fn reload_from_disk(&mut self, path: &std::path::Path) {
        let Some(idx) = self
            .open_documents
            .iter()
            .position(|d| d.document.path == path)
        else {
            return;
        };
        let name = file_name_of(path);
        if self.open_documents[idx].document.dirty && !confirm_discard(&name) {
            return;
        }
        match reload_open_document(&mut self.open_documents[idx]) {
            Ok(()) => self.set_status(format!("Reloaded {name} from disk")),
            Err(e) => self.set_status(format!("Reload error: {e}")),
        }
    }

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

        // The generations the snapshot holds for this path, if it has it.
        let base_gen = self
            .font_base_docs
            .iter()
            .find(|base| base.path == path)
            .map(|b| (b.edit_gen, b.content_gen));
        // Opening is on the critical path of a Ctrl/Cmd+click, so a file the
        // snapshot already holds is parsed from memory rather than read again;
        // see [`FontSource`] for what that costs.
        let loaded = match self.font_sources.get(&path) {
            // The snapshot's own bytes, re-parsed: the same source revision,
            // and so the same font. Stepping the generations here is what made
            // every Ctrl/Cmd+click into a file cost a full rebuild.
            //
            // "The same font" rests on the round trip `document_from_source`
            // performs — parse, serialize, re-derive — producing the items the
            // snapshot's plain parse did. That is the invariant the parser's
            // round-trip tests exist for, and the one every editor flush
            // already depends on.
            Some(source) => open_document_from_text(
                &source.text,
                source.hash,
                path.clone(),
                base_gen.unwrap_or((0, 0)),
            ),
            None => load_open_document(path.clone(), base_gen),
        };
        match loaded {
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

    /// Installs a directory snapshot — the parsed documents, the files that
    /// failed to parse, and the text the documents were parsed from.
    ///
    /// The only way `font_base_docs` is replaced, so that [`FontSource`] cannot
    /// outlive the documents it belongs to: a search served from a source the
    /// snapshot no longer agrees with would report lines that are not there.
    pub(super) fn install_font_snapshot(
        &mut self,
        docs: Vec<Document>,
        errors: Vec<(PathBuf, String)>,
        sources: Vec<(PathBuf, Vec<u8>)>,
    ) {
        self.font_base_docs = docs;
        self.file_parse_errors = errors;
        self.font_sources = font_sources_from(sources);
    }

    pub(super) fn collect_all_docs(&self) -> Vec<&Document> {
        collect_effective_docs(&self.open_documents, &self.font_base_docs)
    }

    fn has_unsaved_changes(&self) -> bool {
        self.open_documents
            .iter()
            .any(|d| d.document.dirty || d.editor_state.has_pending_document_sync())
    }

    pub(super) fn save_all(&mut self) -> bool {
        for doc in &mut self.open_documents {
            doc.flush_pending_changes();
        }
        // One question for the whole batch rather than one per file: a save-all
        // after a `git checkout` would otherwise be a queue of dialogs.
        let overwritten: Vec<String> = self
            .open_documents
            .iter()
            .filter(|d| d.document.dirty && d.external_change)
            .map(|d| file_name_of(&d.document.path))
            .collect();
        if !overwritten.is_empty() && !confirm_overwrite(&overwritten) {
            return false;
        }

        for doc in &mut self.open_documents {
            if !doc.document.dirty {
                continue;
            }
            let mut buf = Vec::new();
            if let Err(e) = document_io::serialize_doclines(&doc.lines, &mut buf)
                .and_then(|()| document_io::write_and_sync(&doc.document.path, &buf))
            {
                self.status_message = Some((format!("Save error: {e}"), std::time::Instant::now()));
                return false;
            }
            doc.mark_written(&buf);
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
            rfd::MessageDialogResult::Yes => self.save_all(),
            rfd::MessageDialogResult::Custom(s) if s == save => self.save_all(),
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
            && let Some(doc) = self.open_documents.get_mut(idx)
        {
            doc.flush_pending_changes();
            if doc.external_change && !confirm_overwrite(&[file_name_of(&doc.document.path)]) {
                return;
            }
            let mut buf = Vec::new();
            let result = document_io::serialize_doclines(&doc.lines, &mut buf)
                .and_then(|()| document_io::write_and_sync(&doc.document.path, &buf));
            let path_display = doc.document.path.display().to_string();
            match result {
                Ok(()) => {
                    doc.mark_written(&buf);
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

#[cfg(test)]
mod reload_tests {
    use super::*;

    /// A directory of its own per test, removed when the test ends. The
    /// documents here are written inline: `font/` is downstream data and no
    /// test may read it.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "uniform-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id(),
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, name: &str, content: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, content).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn text_of(open: &OpenDocument) -> String {
        let mut buf = Vec::new();
        document_io::serialize_doclines(&open.lines, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    const BEFORE: &str = "glyph a 2 2\n@@@@\n@@@@\n";
    const AFTER: &str = "glyph a 2 2\n....\n@@@@\n";

    /// A floating pixel selection lives in `EditorState`, not in the line
    /// buffer, so anything that takes the buffer for the document's real
    /// content has to land it first. Until it did, whether a save wrote those
    /// pixels came down to where the keyboard focus happened to be — an
    /// unfocused editor commits, a focused one does not — so the same edit
    /// saved differently depending on the mode it was made in.
    #[test]
    fn a_floating_selection_is_landed_before_the_buffer_is_read() {
        use crate::editor::pixel_selection::PixelSelection;

        let dir = TempDir::new("float-flush");
        let path = dir.write("a.unf", "glyph a 2 2\n....\n....\n");
        let mut open = load_open_document(path, None).unwrap();

        let mut float = crate::document::PixelGrid::new(1, 1);
        float.set(
            0,
            0,
            crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
        );
        open.editor_state.mode = crate::editor::EditMode::PixelSelect { item_idx: 0 };
        open.editor_state.pixel_selection = Some(PixelSelection {
            item_idx: 0,
            row: 1,
            col: 0,
            width: 1,
            height: 1,
            float_pixels: Some(float),
        });

        open.flush_pending_changes();

        assert!(
            open.editor_state.pixel_selection.is_none(),
            "the float should have been committed, not left pending"
        );
        assert_eq!(text_of(&open), "glyph a 2 2\n....\n@@..\n");
        assert!(open.document.dirty, "the committed pixels are unsaved");
    }

    /// The whole of requirement 2: the buffer follows the file, the caret
    /// survives, and the previous contents are one undo away — which is the
    /// only place they exist once the buffer is gone.
    #[test]
    fn an_external_change_is_applied_as_an_undo_entry() {
        let dir = TempDir::new("reload");
        let path = dir.write("a.unf", BEFORE);
        let mut open = load_open_document(path.clone(), None).unwrap();
        assert!(!open.document.dirty);
        let before_lines = open.lines.clone();

        std::fs::write(&path, AFTER).unwrap();
        reload_open_document(&mut open).unwrap();

        assert_eq!(text_of(&open), AFTER);
        assert!(!open.document.dirty, "the buffer is what is on disk");
        assert_eq!(
            open.disk_hash,
            Some(super::super::watch::hash_bytes(
                &std::fs::read(&path).unwrap()
            ))
        );

        assert!(open.editor_state.undo.can_undo());
        open.editor_state.perform_undo(&mut open.lines);
        assert_eq!(open.lines, before_lines);
        // Back to text the file no longer holds, so the document is dirty
        // again — and saving it is what would put it back on disk.
        assert!(!open.editor_state.undo.is_at_saved());
    }

    /// A rewrite that canonicalizes to the same lines — here the header
    /// respaced — must not leave an undo entry that undoes nothing. It does
    /// have to move the hash on, or the file reports itself changed on every
    /// event from then on.
    #[test]
    fn an_identical_rewrite_records_nothing_but_still_moves_the_hash() {
        let dir = TempDir::new("reload-noop");
        let path = dir.write("a.unf", BEFORE);
        let mut open = load_open_document(path.clone(), None).unwrap();
        let first_hash = open.disk_hash;

        std::fs::write(&path, "glyph   a   2   2\n@@@@\n@@@@\n").unwrap();
        reload_open_document(&mut open).unwrap();

        assert!(
            !open.editor_state.undo.can_undo(),
            "nothing changed to undo"
        );
        assert_ne!(open.disk_hash, first_hash);
        assert_eq!(
            open.disk_hash,
            Some(super::super::watch::hash_bytes(
                &std::fs::read(&path).unwrap()
            ))
        );
    }

    /// Saving is what makes the buffer and the file agree again, so it clears
    /// the flag that would keep asking about overwriting.
    #[test]
    fn a_save_clears_the_external_change_flag() {
        let dir = TempDir::new("reload-save");
        let path = dir.write("a.unf", BEFORE);
        let mut open = load_open_document(path, None).unwrap();
        open.document.dirty = true;
        open.external_change = true;
        open.owed_external_toast = true;

        open.mark_written(AFTER.as_bytes());

        assert!(!open.document.dirty);
        assert!(!open.external_change);
        assert!(!open.owed_external_toast);
        assert_eq!(
            open.disk_hash,
            Some(super::super::watch::hash_bytes(AFTER.as_bytes()))
        );
    }
}
