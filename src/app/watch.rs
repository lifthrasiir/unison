//! Watching the open font directory for changes made outside the editor.
//!
//! The watch is an OS-level one (`notify`'s `RecommendedWatcher`: FSEvents on
//! macOS, `ReadDirectoryChangesW` on Windows, inotify on Linux) over the font
//! directory alone, non-recursively — polling a directory the size of `font/`
//! every frame is not affordable, and the editor has to stay responsive while
//! a `git checkout` rewrites forty files at once.
//!
//! # What a change is allowed to do
//!
//! An external change never overwrites work in progress, and never moves a
//! surface the pointer is on:
//!
//! - A file the user has *not* edited is reloaded in place, as an implicit undo
//!   entry ([`super::docs::apply_reloaded_lines`]) so Cmd/Ctrl+Z still walks
//!   back to what was on screen before the change.
//! - A file with unsaved edits keeps them. The buffer is flagged
//!   ([`super::docs::OpenDocument::external_change`]), a toast says so, and the
//!   next save of that file asks before overwriting.
//! - Either case is postponed while the pointer is over the surface the change
//!   would disturb — the sidebar for the file list, the pane showing the file
//!   for its contents — so a list never reorders under a click and a document
//!   never scrolls out from under a drag.
//!
//! A postponement is not silent. As long as something is held back, a sticky
//! toast ([`HELD_CHANGES_TOAST`]) says what is waiting; without it the editor
//! shows a document that is no longer what is on disk and gives no sign of it,
//! for as long as the pointer happens to rest there. Clicking that toast is the
//! user saying "now" — [`WatchState::force_apply`], which is exactly the
//! pointer check waived for one apply, since the pointer is demonstrably on the
//! toast rather than on anything the change would disturb.
//!
//! [`classify`] is that policy, as one pure function; everything around it is
//! plumbing.
//!
//! # Why the events are not acted on directly
//!
//! An event is a claim that something *may* have changed; finding out what did
//! takes a read. So an event goes through two delays before anything happens.
//!
//! First the settle delay ([`SETTLE_MS`]), for two reasons:
//!
//! - A single logical write arrives as several events. Uniform's own
//!   [`crate::document_io::write_and_sync`] writes `.~name.unf` and renames it
//!   over the target; other editors do the same or worse.
//! - The file may be *mid-write* when the first event arrives, so reading it
//!   then yields a truncated document.
//!
//! Then the scan, on a thread of its own — [`WatchState`] has the whole of
//! that rule. Nothing that touches the filesystem may run between two frames:
//! over SMB (where the poll backend is the one running, and where this editor
//! is routinely used) one read is tens of milliseconds and a directory
//! re-parse is 34 files of them.
//!
//! Uniform's own saves are filtered out by content rather than by suppressing
//! the watch around the write: every open document remembers the hash of the
//! bytes it last read from or wrote to disk ([`hash_bytes`]), so a save's own
//! echo is indistinguishable from "the file already says what we think it
//! says" — and a `git checkout` that restores exactly what is in the buffer is
//! correctly a no-op too. That comparison is the first thing the scan does, so
//! a file that did not really change is never even parsed.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::document::{DocLine, Document};

/// How often a directory the kernel cannot report on is re-scanned. Ten
/// seconds: the scan is one `read_dir` plus a `stat` per file, which is cheap
/// locally and a handful of round trips over SMB, and a change made in another
/// window is not something the editor has to see within the second.
/// `UNIFORM_WATCH_POLL_MS` overrides it.
const DEFAULT_POLL_MS: u64 = 10_000;

/// How long events are left to settle before the directory is looked at. Long
/// enough to swallow the write/rename pair of one save, short enough that an
/// external edit shows up as soon as the user looks back at the window.
pub(super) const SETTLE_MS: u64 = 150;

/// How often a change held back by the pointer is retried. The retry itself is
/// free — the scanned result is in hand, so it only re-reads a pointer
/// position — but it keeps frames coming, and what it waits for (the pointer
/// leaving) is something the user takes seconds to do.
const RETRY_MS: u64 = 500;

/// One filesystem event, reduced to what the application acts on.
struct WatchEvent {
    path: PathBuf,
    /// The event could have added or removed a directory entry, so the file
    /// list has to be re-read as well as the file itself.
    listing: bool,
}

/// Hash of a file's bytes: what tells a real external change from the echo of
/// our own save. Taken wherever the bytes are already in hand — the scan
/// thread, `load_open_document`, a save — so nothing re-reads a file to
/// compute it.
pub(super) fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Whether `dir` sits on a volume whose changes this machine's kernel
/// notifications can actually see, or `None` if that cannot be determined.
///
/// The native backends are all notification mechanisms of the kernel that
/// *owns* the filesystem, so a network mount is a blind spot — and a silent
/// one, since `Watcher::watch` succeeds and simply no event ever arrives.
/// `notify`'s own documentation says as much ("Network mounted filesystems
/// like NFS may not emit any events for notify to listen to"), and this
/// repository is itself on an SMB share, so the fallback is not a corner case
/// here. Specifically:
///
/// - **macOS**: FSEvents reads a journal that `fseventsd` keeps per *local*
///   volume. A network mount has no journal, so not even this machine's own
///   writes through the mount are reported. `MNT_LOCAL` is the flag for it.
/// - **Windows**: `ReadDirectoryChangesW` is the exception — SMB2 carries a
///   `CHANGE_NOTIFY` request, so a share whose server implements it (Samba,
///   Windows Server) does deliver events. Server support varies enough that
///   `DRIVE_REMOTE` and UNC paths are treated as remote anyway; a poll that
///   was not needed costs one directory scan every ten seconds.
/// - **Elsewhere**: assumed local. Linux and the BSDs are not targets here,
///   and inotify/kqueue on a local directory is the behaviour to keep.
fn is_local_volume(dir: &Path) -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c_path.as_ptr(), &mut buf) } != 0 {
            return None;
        }
        Some(buf.f_flags & libc::MNT_LOCAL as u32 != 0)
    }
    #[cfg(target_os = "windows")]
    {
        use std::path::{Component, Prefix};
        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
        use windows::Win32::System::WindowsProgramming::DRIVE_REMOTE;
        use windows::core::PCWSTR;

        // Canonicalized, so a relative path or a junction resolves to the
        // volume it actually lives on.
        let full = dir.canonicalize().ok()?;
        let Component::Prefix(prefix) = full.components().next()? else {
            return None;
        };
        match prefix.kind() {
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => Some(false),
            Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => {
                let root: Vec<u16> = format!("{}:\\", drive as char)
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
                Some(kind != DRIVE_REMOTE)
            }
            _ => None,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = dir;
        Some(true)
    }
}

fn poll_interval() -> Duration {
    let ms = std::env::var("UNIFORM_WATCH_POLL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(DEFAULT_POLL_MS);
    Duration::from_millis(ms)
}

/// The OS watch itself: a background watcher and the channel it reports on.
/// Dropping it stops the watch, which is how opening another folder ends the
/// previous one.
struct DirWatcher {
    _watcher: Box<dyn Watcher + Send>,
    rx: mpsc::Receiver<WatchEvent>,
}

impl DirWatcher {
    fn new(dir: &Path, ctx: &egui::Context) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let handler = move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            // A rename reports both ends, a removal only the old path; either
            // way the entry set may have changed. Modifications of an existing
            // file do not touch it.
            let listing = !matches!(
                event.kind,
                EventKind::Modify(notify::event::ModifyKind::Data(_)) | EventKind::Access(_)
            );
            let mut sent = false;
            for path in event.paths {
                if !crate::document_io::is_source_file(&path) {
                    continue;
                }
                if tx.send(WatchEvent { path, listing }).is_ok() {
                    sent = true;
                }
            }
            if sent {
                ctx.request_repaint();
            }
        };

        // Unknown counts as remote: a poll that was not needed is a wasted
        // directory scan, while a native watch that was not possible is a
        // feature that silently does nothing.
        let mut watcher: Box<dyn Watcher + Send> = if is_local_volume(dir).unwrap_or(false) {
            Box::new(notify::recommended_watcher(handler).ok()?)
        } else {
            let config = notify::Config::default().with_poll_interval(poll_interval());
            Box::new(notify::PollWatcher::new(handler, config).ok()?)
        };
        watcher.watch(dir, RecursiveMode::NonRecursive).ok()?;
        Some(Self {
            _watcher: watcher,
            rx,
        })
    }
}

/// What one changed file asks the application to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ChangeAction {
    /// The bytes on disk are the ones we last read or wrote: nothing happened.
    Ignore,
    /// No unsaved edits — replace the buffer with the file.
    Reload,
    /// Unsaved edits, and the file is on screen: keep them and say so now.
    WarnNow,
    /// Unsaved edits in a file no pane shows: keep them and say so once it is
    /// shown, since a toast about a file the user cannot see explains nothing.
    WarnLater,
    /// The pointer is over the surface this would disturb; try again later.
    Defer,
}

/// What is known about a changed file at the moment it is looked at.
#[derive(Clone, Copy, Debug)]
pub(super) struct FileStatus {
    /// The file's bytes hash to what this document last read or wrote.
    pub(super) unchanged: bool,
    /// The buffer has edits that are not on disk.
    pub(super) dirty: bool,
    /// Some pane is showing the document.
    pub(super) displayed: bool,
    /// The pointer is over the pane showing it.
    pub(super) pointer_over: bool,
    /// The user asked for the held changes now, by clicking the notice. The
    /// pointer is on that notice, so what it is nominally over means nothing.
    pub(super) forced: bool,
}

/// The whole external-change policy. See the module docs.
pub(super) fn classify(status: FileStatus) -> ChangeAction {
    if status.unchanged {
        return ChangeAction::Ignore;
    }
    if status.dirty {
        // Warning about it moves nothing on screen, so the pointer is
        // irrelevant here — only reloading has to wait.
        return if status.displayed {
            ChangeAction::WarnNow
        } else {
            ChangeAction::WarnLater
        };
    }
    if status.displayed && status.pointer_over && !status.forced {
        ChangeAction::Defer
    } else {
        ChangeAction::Reload
    }
}

/// What the scan thread was asked to look at.
struct ScanRequest {
    /// Files to read, with the hash the buffer believes is on disk. A file
    /// that still hashes to it is not parsed at all.
    files: Vec<(PathBuf, Option<u64>)>,
    /// Set when the directory's entry set may have changed, or a file no
    /// buffer holds did: the whole snapshot is re-parsed.
    dir: Option<PathBuf>,
}

/// What the scan thread found for one file.
pub(super) struct ScannedFile {
    pub(super) path: PathBuf,
    pub(super) hash: u64,
    pub(super) outcome: ScanOutcome,
}

pub(super) enum ScanOutcome {
    /// Canonical lines, parsed off the UI thread; ready to be swapped in.
    Parsed(Vec<DocLine>),
    /// The bytes changed but do not parse. The buffer is kept either way.
    Failed(String),
}

/// One scan's worth of work, all of it done off the UI thread.
#[derive(Default)]
pub(super) struct ScanResult {
    files: Vec<ScannedFile>,
    /// The re-parsed directory snapshot, its parse errors, and the bytes it was
    /// parsed from — the UI thread keeps those, so nothing reads these files
    /// again to search or open them.
    snapshot: Option<crate::render::ttf_builder::LoadedDir>,
    /// The `.unf` files the directory holds, so refreshing the sidebar costs
    /// the UI thread no `read_dir` of its own.
    listing: Option<Vec<PathBuf>>,
}

/// The watch, the scan thread behind it, and the changes waiting to be applied.
///
/// # What runs where
///
/// Everything that touches the filesystem runs on the scan thread: reading a
/// changed file, hashing it to find out whether it *really* changed, parsing
/// it, and re-parsing the directory snapshot. Over SMB — where this editor is
/// routinely used, and where the poll backend is the one running — a single
/// one of those reads is tens of milliseconds, and the directory re-parse is
/// 34 files of it. None of that may happen between two frames.
///
/// The UI thread is left with decisions and moves: classify each scanned file
/// ([`classify`]), swap in lines that are already parsed, push the undo entry.
/// A change held back because the pointer is over its pane keeps its scanned
/// result, so the retries that follow cost no I/O at all — they only re-read a
/// pointer position.
pub(super) struct WatchState {
    watcher: Option<DirWatcher>,
    /// Event paths reported, not yet handed to a scan.
    pending: BTreeSet<PathBuf>,
    /// The directory's entry set may have changed.
    listing_dirty: bool,
    /// When the pending set may be scanned. Pushed forward by every new event,
    /// so a burst of writes is scanned once.
    settle_at: Option<Instant>,
    scan_tx: mpsc::Sender<ScanResult>,
    scan_rx: mpsc::Receiver<ScanResult>,
    /// A scan thread is running. Without this, a directory being rewritten
    /// file by file would start a re-parse per event — the same pile-up
    /// `derived_inflight` exists for in `background.rs`.
    scanning: bool,
    /// Scanned changes not applied yet, because the pointer is over the pane
    /// showing them. Retried every frame, without touching the filesystem.
    held: Vec<ScannedFile>,
    /// A file list not applied yet, because the pointer is over the sidebar.
    held_listing: Option<Vec<PathBuf>>,
    /// The user clicked the held-changes notice: apply what is held on this
    /// frame whatever the pointer is on. Cleared by the apply it authorizes.
    force_apply: bool,
}

impl WatchState {
    pub(super) fn new() -> Self {
        let (scan_tx, scan_rx) = mpsc::channel();
        Self {
            watcher: None,
            pending: BTreeSet::new(),
            listing_dirty: false,
            settle_at: None,
            scan_tx,
            scan_rx,
            scanning: false,
            held: Vec::new(),
            held_listing: None,
            force_apply: false,
        }
    }

    /// Starts watching `dir`, dropping any previous watch. A directory that
    /// cannot be watched (a platform limit, a vanished path) leaves the editor
    /// working exactly as it did before there was a watch at all.
    pub(super) fn set_directory(&mut self, dir: &Path, ctx: &egui::Context) {
        self.watcher = None;
        self.pending.clear();
        self.listing_dirty = false;
        self.settle_at = None;
        self.held.clear();
        self.held_listing = None;
        self.force_apply = false;
        // A scan of the previous directory may still be running; its result is
        // about files that are no longer on screen, so it is dropped on
        // arrival rather than waited for.
        let (tx, rx) = mpsc::channel();
        self.scan_tx = tx;
        self.scan_rx = rx;
        self.scanning = false;
        self.watcher = DirWatcher::new(dir, ctx);
    }

    /// Drains the watcher's channel into the pending set.
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        let mut arrived = false;
        if let Some(watcher) = &self.watcher {
            while let Ok(event) = watcher.rx.try_recv() {
                self.listing_dirty |= event.listing;
                self.pending.insert(event.path);
                arrived = true;
            }
        }
        if arrived {
            self.settle_at = Some(Instant::now() + Duration::from_millis(SETTLE_MS));
            ctx.request_repaint_after(Duration::from_millis(SETTLE_MS));
        }
    }

    /// Whether the events have settled and no scan is already running.
    fn ready_to_scan(&self) -> bool {
        if self.scanning || (self.pending.is_empty() && !self.listing_dirty) {
            return false;
        }
        self.settle_at.is_some_and(|at| Instant::now() >= at)
    }

    /// The event paths to resolve into a scan request.
    fn take_pending(&mut self) -> (Vec<PathBuf>, bool) {
        self.settle_at = None;
        let paths = std::mem::take(&mut self.pending).into_iter().collect();
        let listing = std::mem::replace(&mut self.listing_dirty, false);
        (paths, listing)
    }

    /// Reads the files and re-parses the directory on a thread of its own.
    fn start_scan(&mut self, request: ScanRequest, ctx: &egui::Context) {
        self.scanning = true;
        let tx = self.scan_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // An empty result is "nothing changed", which is the safe reading
            // if the scan dies: `scanning` clears and the next event scans
            // again, instead of the watch going silent for the whole session.
            let mut slot = super::background::ResultSlot::new(tx, ctx, ScanResult::default());
            slot.set(run_scan(request));
        });
    }

    fn take_scan_result(&mut self) -> Option<ScanResult> {
        let result = self.scan_rx.try_recv().ok()?;
        self.scanning = false;
        Some(result)
    }

    /// Holds a scanned change until the pointer allows it to be applied.
    fn hold(&mut self, file: ScannedFile) {
        // A newer scan of the same file supersedes the held one.
        self.held.retain(|f| f.path != file.path);
        self.held.push(file);
    }

    fn hold_listing(&mut self, listing: Vec<PathBuf>) {
        self.held_listing = Some(listing);
    }

    /// Whether anything is still waiting, so the caller keeps the frames
    /// coming: what the deferrals wait on is a pointer position, and a frame
    /// is what re-reads it.
    fn has_held(&self) -> bool {
        !self.held.is_empty() || self.held_listing.is_some()
    }

    /// What the sticky notice should say, or `None` when nothing is waiting.
    /// Names the files when it can: "something changed" is not actionable, and
    /// the user is usually holding the pointer over one of them.
    fn held_notice(&self) -> Option<String> {
        let names: Vec<&str> = self
            .held
            .iter()
            .map(|f| {
                f.path
                    .file_name()
                    .map(|n| n.to_str().unwrap_or("a file"))
                    .unwrap_or("a file")
            })
            .collect();
        let what = match (names.as_slice(), self.held_listing.is_some()) {
            ([], false) => return None,
            ([], true) => "The file list changed on disk".to_string(),
            ([one], _) => format!("{one} changed on disk"),
            (many, _) => format!("{} files changed on disk", many.len()),
        };
        Some(format!(
            "{what}. Click here to reload now — otherwise it happens on its own \
             once the pointer leaves."
        ))
    }
}

/// The sticky toast that says a scanned change is waiting for the pointer.
pub(super) const HELD_CHANGES_TOAST: &str = "watch_held_changes";

/// The scan itself. Runs on its own thread; touches no application state.
fn run_scan(request: ScanRequest) -> ScanResult {
    let mut files = Vec::new();
    for (path, known) in request.files {
        let Ok(bytes) = std::fs::read(&path) else {
            // Deleted or renamed away between the event and the read. The
            // buffer is all that is left of it, so it stays as it is.
            continue;
        };
        let hash = hash_bytes(&bytes);
        if Some(hash) == known {
            // The echo of our own save, or a rewrite with identical bytes.
            // Not parsed, and not reported: there is nothing to decide.
            continue;
        }
        let content = String::from_utf8_lossy(&bytes).into_owned();
        let outcome = match super::docs::document_from_source(&content, path.clone()) {
            Ok((_, lines)) => ScanOutcome::Parsed(lines),
            Err(e) => ScanOutcome::Failed(e.to_string()),
        };
        files.push(ScannedFile {
            path,
            hash,
            outcome,
        });
    }

    let (snapshot, listing) = match request.dir {
        Some(dir) => {
            let parsed = crate::render::ttf_builder::load_docs_from_directory_with_sources(&dir);
            let mut listing: Vec<PathBuf> = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| crate::document_io::is_source_file(path))
                .collect();
            listing.sort();
            (Some(parsed), Some(listing))
        }
        None => (None, None),
    };

    ScanResult {
        files,
        snapshot,
        listing,
    }
}

impl super::UniformApp {
    /// Applies whatever the watch has reported, once it has settled.
    ///
    /// Runs before the panels, so a reload lands in the same frame's editors
    /// rather than one frame late — but the pointer positions it consults are
    /// last frame's rects, which is what they have to be: this frame's are not
    /// laid out yet, and a rect moves only when a panel is dragged.
    pub(super) fn pump_file_watch(&mut self, ctx: &egui::Context) {
        // Owed notices are about panes, not about the filesystem, so they are
        // paid whether or not anything changed on disk this frame.
        self.pay_owed_external_toasts();

        self.watch.poll(ctx);
        self.collect_scan_result();
        self.apply_watch_changes(ctx);
        self.maybe_start_scan(ctx);

        // Put up (or take down) the notice after the apply, so a change that
        // went through this frame never flashes one.
        let notice = self.watch.held_notice();
        self.toasts.set_sticky(HELD_CHANGES_TOAST, notice);

        if self.watch.has_held() {
            ctx.request_repaint_after(std::time::Duration::from_millis(RETRY_MS));
        }
    }

    /// The held-changes notice was clicked: apply what is waiting on the next
    /// frame, pointer or no pointer.
    ///
    /// Not applied on the spot because the click is found where the toasts are
    /// drawn — after this frame's panes have already been laid out and painted
    /// from the buffers a reload would replace.
    pub(super) fn apply_held_watch_changes(&mut self, ctx: &egui::Context) {
        self.watch.force_apply = true;
        ctx.request_repaint();
    }

    /// Takes a finished scan: the snapshot lands right away, the per-file
    /// results go to the queue [`Self::apply_watch_changes`] drains.
    fn collect_scan_result(&mut self) {
        let Some(result) = self.watch.take_scan_result() else {
            return;
        };
        if let Some((docs, errors, sources)) = result.snapshot {
            self.apply_directory_snapshot(docs, errors, sources);
        }
        if let Some(listing) = result.listing {
            self.watch.hold_listing(listing);
        }
        for file in result.files {
            self.watch.hold(file);
        }
    }

    /// Applies whatever the pointer now allows. No filesystem access: every
    /// held change carries the bytes' hash and its parsed lines already.
    fn apply_watch_changes(&mut self, ctx: &egui::Context) {
        let forced = std::mem::take(&mut self.watch.force_apply);

        if let Some(listing) = self.watch.held_listing.take() {
            // A rename field addresses rows by index, and a list refreshed
            // under the pointer moves rows out from under a click. A rename in
            // progress holds the list back even when the user asked for the
            // refresh now: the row it is editing would change under it.
            if self.sidebar.is_editing() || (!forced && self.pointer_over_sidebar(ctx)) {
                self.watch.held_listing = Some(listing);
            } else {
                self.sidebar.set_files(listing);
            }
        }

        let held = std::mem::take(&mut self.watch.held);
        for file in held {
            // Matched by file name, not by path: the watch reports absolute,
            // symlink-resolved paths (`/private/var/…`), while a document's
            // path is whatever the folder was opened as — often relative. Both
            // name the same directory, so the file name is the whole key.
            let name = file.path.file_name();
            let Some(idx) = self
                .open_documents
                .iter()
                .position(|d| d.document.path.file_name() == name)
            else {
                continue;
            };
            let doc = &self.open_documents[idx];
            let status = FileStatus {
                // Re-checked here, not just in the scan: the file may have
                // been saved from this editor while the scan was in flight,
                // which makes those bytes ours after all.
                unchanged: doc.disk_hash == Some(file.hash),
                dirty: doc.document.dirty || doc.editor_state.has_pending_document_sync(),
                displayed: self.panes.pane_showing(idx).is_some(),
                pointer_over: self.pointer_over_document(ctx, idx),
                forced,
            };
            match classify(status) {
                ChangeAction::Ignore => {}
                ChangeAction::Defer => self.watch.hold(file),
                ChangeAction::Reload => match file.outcome {
                    ScanOutcome::Parsed(lines) => {
                        super::docs::apply_reloaded_lines(
                            &mut self.open_documents[idx],
                            lines,
                            file.hash,
                        );
                    }
                    ScanOutcome::Failed(e) => {
                        self.set_status(format!(
                            "{} changed on disk but does not parse: {e}",
                            super::docs::file_name_of(&file.path),
                        ));
                    }
                },
                ChangeAction::WarnNow | ChangeAction::WarnLater => {
                    let displayed = self.panes.pane_showing(idx).is_some();
                    let doc = &mut self.open_documents[idx];
                    doc.external_change = true;
                    doc.owed_external_toast = !displayed;
                    if displayed {
                        let name = super::docs::file_name_of(&doc.document.path);
                        self.toasts.push(external_change_notice(&name));
                    }
                }
            }
        }
    }

    /// Turns the settled events into a scan request and hands it to a thread.
    ///
    /// This is where an event path becomes something to read: an open
    /// document's own path (with the hash its buffer believes is on disk, so
    /// the scan can skip parsing what has not really changed), or — for a file
    /// no buffer holds — a reason to re-parse the directory snapshot.
    fn maybe_start_scan(&mut self, ctx: &egui::Context) {
        if !self.watch.ready_to_scan() {
            return;
        }
        let (paths, listing_changed) = self.watch.take_pending();
        let mut files = Vec::new();
        let mut snapshot = listing_changed;
        for path in paths {
            let name = path.file_name();
            match self
                .open_documents
                .iter()
                .find(|d| d.document.path.file_name() == name)
            {
                Some(doc) => files.push((doc.document.path.clone(), doc.disk_hash)),
                None => snapshot = true,
            }
        }
        let dir = snapshot.then(|| self.font_dir.clone()).flatten();
        if files.is_empty() && dir.is_none() {
            return;
        }
        self.watch.start_scan(ScanRequest { files, dir }, ctx);
    }

    /// Installs a directory snapshot parsed on the scan thread.
    ///
    /// The generation counters are stepped past the snapshot being replaced —
    /// [`super::UniformApp::current_font_gen`] hashes them, and a freshly
    /// parsed document starts back at zero, which would read as "nothing
    /// changed" and rebuild nothing.
    fn apply_directory_snapshot(
        &mut self,
        mut docs: Vec<Document>,
        errors: Vec<(PathBuf, String)>,
        sources: Vec<(PathBuf, Vec<u8>)>,
    ) {
        for doc in &mut docs {
            let (edit_gen, content_gen) = self
                .font_base_docs
                .iter()
                .find(|d| d.path == doc.path)
                .map(|d| (d.edit_gen, d.content_gen))
                .unwrap_or((0, 0));
            doc.edit_gen = edit_gen.wrapping_add(1);
            doc.content_gen = content_gen.wrapping_add(1);
        }
        self.install_font_snapshot(docs, errors, sources);
    }

    /// Says what a change to a file no pane was showing did, now that a pane
    /// is showing it.
    fn pay_owed_external_toasts(&mut self) {
        let showing: Vec<usize> = (0..self.open_documents.len())
            .filter(|&idx| self.panes.pane_showing(idx).is_some())
            .collect();
        let mut owed = Vec::new();
        for idx in showing {
            let doc = &mut self.open_documents[idx];
            if std::mem::take(&mut doc.owed_external_toast) {
                owed.push(super::docs::file_name_of(&doc.document.path));
            }
        }
        for name in owed {
            self.toasts.push(external_change_notice(&name));
        }
    }

    fn pointer_over_sidebar(&self, ctx: &egui::Context) -> bool {
        ctx.input(|i| i.pointer.latest_pos())
            .is_some_and(|p| self.sidebar_rect.contains(p))
    }

    fn pointer_over_document(&self, ctx: &egui::Context, doc_idx: usize) -> bool {
        let Some(rect) = self
            .panes
            .pane_showing(doc_idx)
            .and_then(|pane| self.panes.get(pane))
            .and_then(|pane| pane.view_rect)
        else {
            return false;
        };
        ctx.input(|i| i.pointer.latest_pos())
            .is_some_and(|p| rect.contains(p))
    }
}

fn external_change_notice(name: &str) -> String {
    format!(
        "{name} changed on disk. Your unsaved edits were kept — saving will \
         overwrite the file, or right-click the file in the sidebar to reload it."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(unchanged: bool, dirty: bool, displayed: bool, pointer_over: bool) -> FileStatus {
        FileStatus {
            unchanged,
            dirty,
            displayed,
            pointer_over,
            forced: false,
        }
    }

    /// Our own save comes back as an event; it must not be read as a change,
    /// or every save would flag the file it just wrote.
    #[test]
    fn identical_bytes_are_not_a_change() {
        for dirty in [false, true] {
            for displayed in [false, true] {
                assert_eq!(
                    classify(status(true, dirty, displayed, true)),
                    ChangeAction::Ignore,
                );
            }
        }
    }

    #[test]
    fn a_clean_document_is_reloaded_unless_the_pointer_is_on_it() {
        assert_eq!(
            classify(status(false, false, true, false)),
            ChangeAction::Reload
        );
        assert_eq!(
            classify(status(false, false, false, false)),
            ChangeAction::Reload
        );
        assert_eq!(
            classify(status(false, false, true, true)),
            ChangeAction::Defer
        );
        // Not displayed: there is no surface the pointer could be over, so a
        // stale `pointer_over` must not hold the reload back forever.
        assert_eq!(
            classify(status(false, false, false, true)),
            ChangeAction::Reload
        );
    }

    /// Clicking the notice is the user asking for the reload now, so the
    /// pointer check that held it back is waived — the pointer is on the
    /// notice, not on anything the reload disturbs.
    #[test]
    fn a_forced_change_is_applied_under_the_pointer() {
        let forced = FileStatus {
            forced: true,
            ..status(false, false, true, true)
        };
        assert_eq!(classify(forced), ChangeAction::Reload);
        // It waives the pointer, nothing else: unchanged bytes are still
        // nothing, and unsaved edits are still kept.
        assert_eq!(
            classify(FileStatus {
                forced: true,
                ..status(true, false, true, true)
            }),
            ChangeAction::Ignore,
        );
        assert_eq!(
            classify(FileStatus {
                forced: true,
                ..status(false, true, true, true)
            }),
            ChangeAction::WarnNow,
        );
    }

    /// A held change must say so, by name, and the notice must go away by
    /// itself once nothing is held — it is the only sign the user gets that
    /// the document on screen is not what is on disk.
    #[test]
    fn what_is_held_back_is_named_in_the_notice() {
        let mut watch = WatchState::new();
        assert_eq!(watch.held_notice(), None);

        watch.hold(ScannedFile {
            path: PathBuf::from("/font/num.unf"),
            hash: 0,
            outcome: ScanOutcome::Parsed(Vec::new()),
        });
        let one = watch.held_notice().expect("a held change says so");
        assert!(one.starts_with("num.unf changed on disk."), "{one}");
        assert!(one.contains("Click here"), "{one}");

        watch.hold(ScannedFile {
            path: PathBuf::from("/font/latin.unf"),
            hash: 0,
            outcome: ScanOutcome::Parsed(Vec::new()),
        });
        assert!(
            watch.held_notice().unwrap().starts_with("2 files changed"),
            "{:?}",
            watch.held_notice(),
        );

        watch.held.clear();
        watch.hold_listing(Vec::new());
        assert!(
            watch.held_notice().unwrap().starts_with("The file list"),
            "{:?}",
            watch.held_notice(),
        );

        watch.held_listing = None;
        assert_eq!(watch.held_notice(), None);
    }

    /// Unsaved edits are never dropped, and the warning is not deferred: it
    /// moves nothing on screen.
    #[test]
    fn a_dirty_document_is_kept_and_warned_about() {
        assert_eq!(
            classify(status(false, true, true, true)),
            ChangeAction::WarnNow
        );
        assert_eq!(
            classify(status(false, true, true, false)),
            ChangeAction::WarnNow
        );
        assert_eq!(
            classify(status(false, true, false, false)),
            ChangeAction::WarnLater
        );
    }

    /// The scan is the whole point of the background thread: it must skip a
    /// file whose bytes still hash to what the buffer knows (that is the
    /// "did it *really* change" check, and skipping it means no parse), parse
    /// the one that did change, and hand back the directory in one go so the
    /// UI thread never reads anything itself.
    #[test]
    fn the_scan_skips_unchanged_files_and_parses_the_rest() {
        let dir = std::env::temp_dir().join(format!("uniform-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let same = dir.join("same.unf");
        let changed = dir.join("changed.unf");
        let source = "glyph a 2 2\n@@@@\n@@@@\n";
        std::fs::write(&same, source).unwrap();
        std::fs::write(&changed, source).unwrap();
        std::fs::write(dir.join(".~staging.unf"), "not a document").unwrap();

        let result = run_scan(ScanRequest {
            files: vec![
                (same.clone(), Some(hash_bytes(source.as_bytes()))),
                // The buffer believes something else is on disk.
                (changed.clone(), Some(0)),
            ],
            dir: Some(dir.clone()),
        });

        let scanned: Vec<&Path> = result.files.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(
            scanned,
            [changed.as_path()],
            "unchanged files must not be parsed"
        );
        assert!(matches!(result.files[0].outcome, ScanOutcome::Parsed(_)));
        assert_eq!(result.files[0].hash, hash_bytes(source.as_bytes()));

        // The snapshot and the file list come from the same scan, so applying
        // them costs the UI thread no filesystem access of its own.
        let (docs, _errors, sources) = result.snapshot.expect("snapshot");
        assert_eq!(docs.len(), 2);
        // Every snapshot document's source comes back with it, so a later
        // search or open of one reads nothing.
        let mut sourced: Vec<&Path> = sources.iter().map(|(p, _)| p.as_path()).collect();
        sourced.sort();
        assert_eq!(sourced, [changed.as_path(), same.as_path()]);
        let listing: Vec<&Path> = result
            .listing
            .as_deref()
            .expect("listing")
            .iter()
            .map(PathBuf::as_path)
            .collect();
        assert_eq!(listing, [changed.as_path(), same.as_path()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The volume probe has to answer for a plain local directory, or every
    /// watch quietly degrades to a ten-second poll. A botched `statfs` layout
    /// or the wrong flag shows up here and nowhere else.
    #[test]
    fn a_local_directory_is_recognized_as_local() {
        let dir = std::env::temp_dir();
        assert_eq!(is_local_volume(&dir), Some(true), "{}", dir.display());
    }

    /// A path that cannot be probed must not be reported as local: unknown
    /// falls back to polling, which works everywhere.
    #[test]
    fn an_unprobeable_path_is_not_claimed_to_be_local() {
        let missing = std::env::temp_dir().join("uniform-no-such-dir-9d3f1a");
        assert_ne!(is_local_volume(&missing), Some(true));
    }

    /// The OS watch itself, end to end: a write to a watched directory has to
    /// come back through [`WatchState`] as that file, and the settle delay has
    /// to have passed before it does. Nothing else here exercises the
    /// platform backend, and a watch that silently fails to arm looks exactly
    /// like a quiet filesystem.
    #[test]
    fn a_write_to_the_directory_is_reported() {
        let dir = std::env::temp_dir().join(format!("uniform-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();

        let mut watch = WatchState::new();
        watch.set_directory(&dir, &ctx);
        std::fs::write(dir.join("a.unf"), "glyph a 2 2\n@@@@\n@@@@\n").unwrap();
        // Written and ignored: it must not come back as a change.
        std::fs::write(dir.join(".~a.unf"), "staging").unwrap();

        // The backend delivers asynchronously, so this waits rather than
        // assuming; the cap is a failure bound, not the expected duration.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while Instant::now() < deadline {
            watch.poll(&ctx);
            if watch.ready_to_scan() {
                ready = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready, "the watch reported nothing in 5s");

        let (paths, _listing) = watch.take_pending();
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["a.unf"], "got {names:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
