//! Writing an open document to disk.
//!
//! The write is the one piece of ordinary editing that reaches the filesystem,
//! and on the network share this editor is routinely run against it costs
//! whole seconds — the same reason nothing else on the UI thread reads a
//! directory or builds a font ([`super::background`]). So a save is serialized
//! on the UI thread, where the buffer is, and handed to one worker thread that
//! does the writing; the outcome comes back through a channel the frame loop
//! pumps like every other background result.
//!
//! # The revision a write is credited to
//!
//! The buffer moves on while a write is in flight, which is what makes an
//! asynchronous save more than a `thread::spawn`. A finished write says *these
//! bytes* are on disk, not *this buffer* is — so the point it is credited to
//! is taken when the bytes are serialized ([`crate::editor::undo::SavePoint`])
//! and handed back with the outcome. Typing during a save therefore leaves the
//! document dirty, correctly, and undoing back to what was written makes it
//! clean again. Marking the *current* revision saved instead would call edits
//! that never reached disk saved.
//!
//! # One worker, in order
//!
//! Writes run one at a time in the order they were asked for, so two saves of
//! one file cannot land out of order and leave the last write's bytes on disk
//! under the first write's revision. It also keeps a Save All from opening one
//! connection per file on a share that serves them serially anyway.
//!
//! # Quitting
//!
//! Closing the window is the one place that may not simply enqueue and return:
//! the process would exit with the writes still in the worker's queue. There
//! the queue is drained blocking — [`super::UniformApp::finish_pending_saves`]
//! — and a failed write cancels the close, so the error is on screen with the
//! buffer still holding the work.

use std::path::PathBuf;
use std::sync::mpsc;

use super::background::{finish, start};
use super::docs::{confirm_overwrite, file_name_of};
use super::{UniformApp, document_io};
use crate::editor::undo::SavePoint;

/// One file's write, as handed to the worker.
struct SaveJob {
    path: PathBuf,
    bytes: Vec<u8>,
    /// Hash of `bytes`, carried so the outcome can name the write without the
    /// UI thread hashing them a second time.
    hash: u64,
    point: SavePoint,
}

/// What became of one write, on its way back to the document it came from.
pub(super) struct SaveOutcome {
    path: PathBuf,
    hash: u64,
    point: SavePoint,
    error: Option<String>,
}

/// The worker thread and the two channels to it.
pub(super) struct SaveQueue {
    /// `None` once the worker has been found gone: from then on a write is
    /// performed on the calling thread rather than dropped.
    tx: Option<mpsc::Sender<SaveJob>>,
    rx: mpsc::Receiver<SaveOutcome>,
    /// Writes handed over that have not reported back. Counted rather than
    /// derived from the channel: the drain on quit has to know that something
    /// is still owed before it blocks on the receiver.
    in_flight: usize,
    /// A Save All is still landing. Its one status line waits for the last
    /// file rather than being printed per file, which is what the synchronous
    /// version did by finishing before it returned.
    batch: bool,
    batch_failed: bool,
}

impl SaveQueue {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<SaveJob>();
        let (out_tx, out_rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for job in job_rx {
                let error = document_io::write_and_sync(&job.path, &job.bytes)
                    .err()
                    .map(|e| e.to_string());
                let outcome = SaveOutcome {
                    path: job.path,
                    hash: job.hash,
                    point: job.point,
                    error,
                };
                if out_tx.send(outcome).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            tx: Some(job_tx),
            rx: out_rx,
            in_flight: 0,
            batch: false,
            batch_failed: false,
        }
    }

    pub(super) fn is_busy(&self) -> bool {
        self.in_flight > 0
    }

    /// Hands one write over, or — if the worker is gone — performs it here.
    /// A save that cannot be moved off the UI thread is still better than one
    /// that never happens, and the outcome reads the same either way.
    fn submit(&mut self, job: SaveJob) -> Option<SaveOutcome> {
        let job = match self.tx.as_ref().map(|tx| tx.send(job)) {
            Some(Ok(())) => {
                self.in_flight += 1;
                return None;
            }
            Some(Err(mpsc::SendError(job))) => job,
            None => return None,
        };
        self.tx = None;
        let error = document_io::write_and_sync(&job.path, &job.bytes)
            .err()
            .map(|e| e.to_string());
        Some(SaveOutcome {
            path: job.path,
            hash: job.hash,
            point: job.point,
            error,
        })
    }

    fn try_take(&mut self) -> Option<SaveOutcome> {
        let outcome = self.rx.try_recv().ok()?;
        self.in_flight -= 1;
        Some(outcome)
    }

    /// Waits for one owed outcome. `None` means the worker died with writes
    /// still owed, which no further waiting will produce.
    fn recv_blocking(&mut self) -> Option<SaveOutcome> {
        match self.rx.recv() {
            Ok(outcome) => {
                self.in_flight -= 1;
                Some(outcome)
            }
            Err(_) => {
                self.in_flight = 0;
                None
            }
        }
    }
}

impl UniformApp {
    /// Serializes one open document and hands its write over. Returns whether
    /// there is now a write to wait for — a document that does not serialize
    /// says so on the status bar and produces none.
    fn enqueue_save(&mut self, idx: usize) -> bool {
        let Some(doc) = self.open_documents.get_mut(idx) else {
            return false;
        };
        let mut bytes = Vec::new();
        if let Err(e) = document_io::serialize_doclines(&doc.lines, &mut bytes) {
            self.set_status(format!("Save error: {e}"));
            // A batch this file was part of did not go through, so it must not
            // end by saying every file was written.
            self.saves.batch_failed = true;
            return false;
        }
        let hash = crate::app::watch::hash_bytes(&bytes);
        // Taken before the bytes leave: this is the revision they are, and
        // taking it breaks undo coalescing so the next keystroke cannot fold
        // into the entry it names.
        let point = doc.editor_state.undo.save_point();
        // Recorded before the write starts, since the file can be read back —
        // by the watcher — before its outcome is applied here.
        doc.pending_disk_hashes.push(hash);
        let job = SaveJob {
            path: doc.document.path.clone(),
            bytes,
            hash,
            point,
        };
        start(&mut self.bg_tasks.save);
        match self.saves.submit(job) {
            // Written on this thread after all: apply it as if it had come
            // back through the channel, so there is one place that lands one.
            Some(outcome) => {
                self.apply_save_outcome(outcome);
                false
            }
            None => true,
        }
    }

    fn apply_save_outcome(&mut self, outcome: SaveOutcome) {
        let idx = self
            .open_documents
            .iter()
            .position(|d| d.document.path == outcome.path);
        let Some(idx) = idx else {
            // Nothing holds the file any more. The error is still the user's
            // to hear; a success has nothing left to record.
            if let Some(e) = outcome.error {
                self.set_status(format!("Save error ({}): {e}", file_name_of(&outcome.path),));
            }
            return;
        };
        let doc = &mut self.open_documents[idx];
        if let Some(at) = doc
            .pending_disk_hashes
            .iter()
            .position(|h| *h == outcome.hash)
        {
            doc.pending_disk_hashes.remove(at);
        }
        match outcome.error {
            None => {
                doc.mark_written_at(outcome.hash, outcome.point);
                if !self.saves.batch {
                    let path = self.open_documents[idx].document.path.display().to_string();
                    self.set_status(format!("Saved {path}"));
                }
            }
            Some(e) => {
                self.saves.batch_failed = true;
                self.set_status(format!("Save error: {e}"));
            }
        }
    }

    /// Lands whatever the worker has finished. Called once a frame, beside the
    /// other background pumps.
    pub(super) fn pump_saves(&mut self, ctx: &egui::Context) {
        while let Some(outcome) = self.saves.try_take() {
            self.apply_save_outcome(outcome);
            if !self.saves.is_busy() {
                finish(&mut self.bg_tasks.save);
            }
        }
        if self.saves.batch && !self.saves.is_busy() {
            self.saves.batch = false;
            if !std::mem::take(&mut self.saves.batch_failed) {
                self.set_status("Saved all files".to_string());
            }
        }
        if self.saves.is_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Waits for every owed write and lands it, reporting whether all of them
    /// succeeded. The one caller is the close path: see the module docs.
    pub(super) fn finish_pending_saves(&mut self) -> bool {
        let mut all_ok = true;
        while self.saves.is_busy() {
            match self.saves.recv_blocking() {
                Some(outcome) => {
                    all_ok &= outcome.error.is_none();
                    self.apply_save_outcome(outcome);
                }
                None => {
                    self.set_status("Save error: the writer stopped".to_string());
                    return false;
                }
            }
        }
        finish(&mut self.bg_tasks.save);
        if self.saves.batch {
            self.saves.batch = false;
            all_ok &= !std::mem::take(&mut self.saves.batch_failed);
        }
        all_ok
    }

    /// Ctrl/Cmd+S: hands the focused document's write over.
    pub(super) fn save_active(&mut self) {
        let Some(idx) = self.active_doc_idx() else {
            return;
        };
        let Some(doc) = self.open_documents.get_mut(idx) else {
            return;
        };
        doc.flush_pending_changes();
        if doc.external_change && !confirm_overwrite(&[file_name_of(&doc.document.path)]) {
            return;
        }
        let path = doc.document.path.display().to_string();
        if self.enqueue_save(idx) {
            self.set_status(format!("Saving {path}..."));
        }
    }

    /// Ctrl/Cmd+Shift+S. Returns whether the save went ahead — a refused
    /// overwrite cancels the whole batch, which is what the close path reads.
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

        let dirty: Vec<usize> = (0..self.open_documents.len())
            .filter(|&i| self.open_documents[i].document.dirty)
            .collect();
        if dirty.is_empty() {
            return true;
        }
        self.saves.batch = true;
        self.saves.batch_failed = false;
        for idx in dirty {
            self.enqueue_save(idx);
        }
        if self.saves.is_busy() {
            self.set_status("Saving all files...".to_string());
        } else {
            // Everything was written on this thread (no worker): the batch is
            // already over, so its one message is due now.
            self.saves.batch = false;
            if !std::mem::take(&mut self.saves.batch_failed) {
                self.set_status("Saved all files".to_string());
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::Settings;
    use crate::document::DocLine;
    use crate::editor::caret::Caret;

    /// Its own directory per test, removed when the test ends. Written inline:
    /// `font/` is downstream data and no test may read it.
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
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SOURCE: &str = "glyph a 2 2\n@@\n.@\n";

    /// An app with `a.unf` open, and the context its save worker reports to.
    fn app_with_open_file(tag: &str) -> (TempDir, egui::Context, UniformApp) {
        let dir = TempDir::new(tag);
        std::fs::write(dir.0.join("a.unf"), SOURCE).unwrap();
        let ctx = egui::Context::default();
        let mut app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        app.open_file(dir.0.join("a.unf"));
        assert_eq!(app.open_documents.len(), 1);
        (dir, ctx, app)
    }

    /// One edit the undo stack knows about, as the editor would make it.
    fn append_line(app: &mut UniformApp, text: &str) {
        let doc = &mut app.open_documents[0];
        let at = doc.lines.len();
        let caret = Caret::zero();
        doc.editor_state.undo.break_coalesce();
        doc.editor_state.undo.push_lines(
            at,
            Vec::new(),
            vec![DocLine::Text(text.to_string())],
            caret,
            caret,
        );
        doc.lines.push(DocLine::Text(text.to_string()));
        doc.document.dirty = !doc.editor_state.undo.is_at_saved();
    }

    fn pump_until_idle(app: &mut UniformApp, ctx: &egui::Context) {
        for _ in 0..2000 {
            app.pump_saves(ctx);
            if !app.saves.is_busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the save never landed");
    }

    /// The point of doing the write off the UI thread: an edit made while it
    /// is in flight is not on disk, so the finished write may not be credited
    /// to it. The document stays dirty, and undoing back to what was written
    /// makes it clean again.
    #[test]
    fn an_edit_during_a_write_is_not_marked_saved_by_it() {
        let (dir, ctx, mut app) = app_with_open_file("save-revision");
        append_line(&mut app, "# saved");
        assert!(app.open_documents[0].document.dirty);

        assert!(app.enqueue_save(0), "the write is handed to the worker");
        // The buffer moves on while the write is in flight.
        append_line(&mut app, "# typed during the write");
        pump_until_idle(&mut app, &ctx);

        let on_disk = std::fs::read_to_string(dir.0.join("a.unf")).unwrap();
        assert!(on_disk.contains("# saved"));
        assert!(
            !on_disk.contains("# typed during the write"),
            "the write carried the revision it was started from"
        );
        assert!(
            app.open_documents[0].document.dirty,
            "the edit made during the write is not on disk"
        );

        let doc = &mut app.open_documents[0];
        doc.editor_state.undo.undo(&mut doc.lines);
        assert!(
            doc.editor_state.undo.is_at_saved(),
            "undoing back to the written revision is what makes it clean"
        );
    }

    /// The watcher tells our own write from an external change by the bytes'
    /// hash, and the write reaches disk before its outcome reaches the UI
    /// thread. So the hash is recorded when the write is handed over, not when
    /// it comes back.
    #[test]
    fn the_bytes_being_written_are_known_before_the_write_reports_back() {
        let (dir, ctx, mut app) = app_with_open_file("save-pending-hash");
        let opened_hash = app.open_documents[0].disk_hash;
        append_line(&mut app, "# saved");

        let mut bytes = Vec::new();
        document_io::serialize_doclines(&app.open_documents[0].lines, &mut bytes).unwrap();
        let hash = crate::app::watch::hash_bytes(&bytes);

        app.enqueue_save(0);
        let doc = &app.open_documents[0];
        assert_eq!(doc.disk_hash, opened_hash, "nothing has reported back yet");
        assert!(
            doc.knows_disk_bytes(hash),
            "a scan that reads the file mid-write must not call it an external change"
        );

        pump_until_idle(&mut app, &ctx);
        let doc = &app.open_documents[0];
        assert_eq!(doc.disk_hash, Some(hash));
        assert!(
            doc.pending_disk_hashes.is_empty(),
            "the write reported back, so it is no longer pending"
        );
        assert_eq!(
            std::fs::read(dir.0.join("a.unf")).unwrap(),
            bytes,
            "and those are the bytes on disk",
        );
    }

    /// Quitting is the one path that may not leave the writes in the worker's
    /// queue: the process would exit with them unwritten.
    #[test]
    fn the_close_path_waits_for_the_writes_it_started() {
        let (dir, _ctx, mut app) = app_with_open_file("save-drain");
        append_line(&mut app, "# saved");
        assert!(app.save_all());

        assert!(app.finish_pending_saves(), "every write landed");
        assert!(
            std::fs::read_to_string(dir.0.join("a.unf"))
                .unwrap()
                .contains("# saved"),
            "the file is on disk by the time the close goes ahead"
        );
        assert!(!app.open_documents[0].document.dirty);
        assert!(!app.saves.is_busy());
    }

    /// A write that fails leaves the buffer holding the work: the document
    /// stays dirty, and the revision it was started from is not credited.
    #[test]
    fn a_failed_write_leaves_the_document_dirty() {
        let (dir, _ctx, mut app) = app_with_open_file("save-failure");
        append_line(&mut app, "# saved");
        // A directory where the file should be: the rename onto it fails.
        let path = app.open_documents[0].document.path.clone();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        app.enqueue_save(0);
        assert!(!app.finish_pending_saves(), "the write failed");
        assert!(app.open_documents[0].document.dirty);
        assert!(app.open_documents[0].pending_disk_hashes.is_empty());
        drop(dir);
    }
}
