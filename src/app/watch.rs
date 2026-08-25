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
//! # Asking for it now (F5)
//!
//! *File ▸ Refresh filesystem* / F5 is the watch's manual override
//! ([`WatchState::request_refresh`]), and it exists because every path above
//! can be silent: a volume with no kernel watch is looked at seconds apart,
//! and a directory the watcher could not be installed on is never looked at at
//! all. So the request goes straight to a scan — no event, no pending set, no
//! settle delay — and it asks for everything: every open document, and the
//! directory, since what changed is exactly what nobody knows.
//!
//! Two things follow from it being *asked for*. What comes back is applied
//! whatever the pointer is over, like a click on the held-changes notice: the
//! user pressed a key in this window, so where the pointer was left says
//! nothing. And the open documents are reported ahead of the directory
//! re-parse rather than with it ([`run_scan`] sends twice), because the file
//! on screen is what the request was about and the re-parse is 34 files of a
//! share behind it.
//!
//! What the re-parse costs is not what it once was, though. It goes through
//! the directory cache the last load left ([`crate::render::ttf_builder::DirCache`]):
//! a file whose size and mtime have not moved is handed back as it was, so a
//! refresh reads only what changed and stats the rest. And a snapshot that
//! turns out to say what is already installed steps no generation
//! ([`super::UniformApp::apply_directory_snapshot`]) — a refresh that finds
//! nothing costs no font build at all, where it used to cost a whole one.
//!
//! Both of those trust the same (size, mtime) pair the poll backend decides
//! on. The open documents do not: their bytes are read and hashed on every
//! scan, because a stamp cannot tell our own save from someone else's write —
//! and the open files are what the request was about.
//!
//! A polled directory also has its interval restarted from the request. The
//! tick that was a second away would only read what the scan thread is reading
//! now; what it must not do is *adopt* what it finds, so the baseline is left
//! alone and a write that lands between the two reads is still reported.
//!
//! # The poll backend
//!
//! A volume the kernel cannot report on is polled by [`spawn_poll_thread`]
//! rather than by `notify`'s `PollWatcher`. Two properties are the reason, and
//! both are about what the watch costs a share rather than about what it
//! detects: a tick is one directory enumeration and no per-file `stat`
//! ([`poll_snapshot`]), and the interval follows what a tick actually costs
//! ([`next_poll_delay`]) instead of being a number guessed in advance.
//!
//! Uniform's own saves are filtered out by content rather than by suppressing
//! the watch around the write: every open document remembers the hash of the
//! bytes it last read from or wrote to disk ([`hash_bytes`]), so a save's own
//! echo is indistinguishable from "the file already says what we think it
//! says" — and a `git checkout` that restores exactly what is in the buffer is
//! correctly a no-op too. That comparison is the first thing the scan does, so
//! a file that did not really change is never even parsed.

use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::document::{DocLine, Document};
use crate::render::ttf_builder::{DirCache, LoadedDir};

/// The shortest interval between two polls of a directory the kernel cannot
/// report on — the floor of the adaptive interval described on
/// [`next_poll_delay`]. Two seconds: a poll tick is one `read_dir` and no
/// per-file `stat` (see [`poll_snapshot`]), which measured 47 ms over SMB, so
/// polling this often costs a fraction of what ten-second polling used to.
/// `UNIFORM_WATCH_POLL_MS` overrides it.
const DEFAULT_POLL_MS: u64 = 2_000;

/// The interval never grows past this, however slow the share: a directory that
/// is only looked at once a minute is still a watch.
const MAX_POLL_MS: u64 = 60_000;

/// How much wall-clock time the poll may spend scanning, as a divisor: the next
/// interval is the tick's own cost times this, so polling occupies at most
/// 1/20th — 5% — of one thread's time no matter what the volume does.
const POLL_DUTY_DIVISOR: u32 = 20;

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
///   was not needed costs one directory enumeration every couple of seconds
///   ([`poll_snapshot`]), which is cheap enough that the caution is affordable.
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

/// What one poll tick knows about a file, and all it needs to know: a change to
/// a `.unf` file changes one of the two.
///
/// Both come out of the directory enumeration itself rather than a `stat` per
/// file — see [`poll_snapshot`] for why that distinction is the whole point.
/// The pair can miss a rewrite that keeps the length and lands inside one tick
/// of the volume's timestamp resolution (2 s on a FAT-ish share); polling has
/// no answer to that, and it is the same blind spot the previous backend had.
#[derive(PartialEq, Eq)]
struct PolledEntry {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

/// One `read_dir` over the font directory's `.unf` files.
///
/// **No per-file `stat`.** On Windows the directory enumeration already carries
/// each entry's size and timestamps (`FindFirstFileW` fills them in), and
/// `DirEntry::metadata` hands those back with no further system call — so a
/// tick is one network round trip rather than one per file. That is not a
/// micro-optimization here: `notify`'s `PollWatcher` walks with
/// `follow_links(true)`, which makes `walkdir` re-`stat` every entry it already
/// had the metadata for, and over SMB that was 44 round trips of ~185 ms every
/// ten seconds — about eight seconds of the share's time out of every ten.
///
/// On Unix `DirEntry::metadata` *is* a `stat`, so this is one call per file
/// there; the volumes that fall back to polling on those platforms are the
/// exception rather than the rule (see [`is_local_volume`]).
fn poll_snapshot(dir: &Path) -> std::collections::BTreeMap<PathBuf, PolledEntry> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !crate::document_io::is_source_file(&path) {
            continue;
        }
        // A symlink's own record says nothing about the file it points at, so
        // that one entry is worth the extra call. There is normally none.
        let is_link = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        let meta = if is_link {
            std::fs::metadata(&path).ok()
        } else {
            entry.metadata().ok()
        };
        let Some(meta) = meta else { continue };
        out.insert(
            path,
            PolledEntry {
                len: meta.len(),
                modified: meta.modified().ok(),
            },
        );
    }
    out
}

/// Turns two consecutive snapshots into the events the watch reports.
///
/// `listing` follows the same rule as the native backends': an entry appearing
/// or disappearing may have changed the file list, a file changing in place has
/// not.
fn diff_snapshots(
    prev: &std::collections::BTreeMap<PathBuf, PolledEntry>,
    next: &std::collections::BTreeMap<PathBuf, PolledEntry>,
) -> Vec<WatchEvent> {
    let mut events = Vec::new();
    for (path, entry) in next {
        match prev.get(path) {
            Some(before) if before == entry => {}
            Some(_) => events.push(WatchEvent {
                path: path.clone(),
                listing: false,
            }),
            None => events.push(WatchEvent {
                path: path.clone(),
                listing: true,
            }),
        }
    }
    for path in prev.keys() {
        if !next.contains_key(path) {
            events.push(WatchEvent {
                path: path.clone(),
                listing: true,
            });
        }
    }
    events
}

/// How long to wait before the next poll, given what the last one cost.
///
/// Proportional to that cost ([`POLL_DUTY_DIVISOR`]) and clamped to
/// `[floor, MAX_POLL_MS]`. A fast volume therefore polls at the floor, and a
/// share slow enough that a tick takes a second backs itself off to twenty —
/// without anyone having to guess a number that suits both. The previous fixed
/// ten seconds had no such property: when a tick grew to eight seconds, it
/// simply ran eight-second scans 80% of the time.
fn next_poll_delay(cost: Duration, floor: Duration) -> Duration {
    let scaled = cost * POLL_DUTY_DIVISOR;
    scaled.clamp(floor, Duration::from_millis(MAX_POLL_MS))
}

/// The OS watch itself: a background watcher and the channel it reports on.
/// Dropping it stops the watch, which is how opening another folder ends the
/// previous one.
struct DirWatcher {
    /// The kernel's own watch, on a volume that has one. `None` when the
    /// directory is polled instead.
    _watcher: Option<Box<dyn Watcher + Send>>,
    /// Tells the poll thread to stop, for the same drop that drops the watcher.
    /// The thread also ends on its own once `rx` is gone, but only at its next
    /// tick, and a dropped watch has to stop touching the share now.
    poll_stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Tells the poll thread that the directory has just been looked at
    /// without it, so its next tick is a whole interval away rather than
    /// whatever was left of the one it was in. See [`WatchState::request_refresh`].
    poll_restart: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    rx: mpsc::Receiver<WatchEvent>,
}

impl Drop for DirWatcher {
    fn drop(&mut self) {
        if let Some(stop) = &self.poll_stop {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Polls `dir` on a thread of its own until the returned flag is set.
///
/// The first tick only records the baseline: every file existing is not a
/// change, and reporting it as one would reload the whole directory a moment
/// after it was read.
fn spawn_poll_thread(
    dir: PathBuf,
    tx: mpsc::Sender<WatchEvent>,
    ctx: egui::Context,
    floor: Duration,
) -> (
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_stop = stop.clone();
    let restart = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let thread_restart = restart.clone();
    std::thread::spawn(move || {
        let baseline = Instant::now();
        let mut previous = poll_snapshot(&dir);
        // The baseline tick is a tick like any other, so its cost is what the
        // first interval is chosen from — a share slow enough to matter says so
        // before the second scan rather than after it.
        let mut cost = baseline.elapsed();
        loop {
            // Slept in slices so that dropping the watch is felt within a frame
            // or two rather than at the end of a minute-long wait.
            let mut deadline = Instant::now() + next_poll_delay(cost, floor);
            loop {
                if thread_stop.load(Ordering::Relaxed) {
                    return;
                }
                if thread_restart.swap(false, Ordering::Relaxed) {
                    // The user asked for the directory to be looked at now, and
                    // the scan thread is doing exactly that. Ticking a second
                    // later because that is where this interval happened to be
                    // would be one more enumeration of the share for nothing,
                    // so the interval starts over from the request. What the
                    // forced scan found is not fed back here: the baseline is
                    // left alone on purpose, so a write that lands between the
                    // two reads is still reported by the next tick rather than
                    // being adopted as "how the directory already was".
                    deadline = Instant::now() + next_poll_delay(cost, floor);
                    continue;
                }
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                std::thread::sleep((deadline - now).min(Duration::from_millis(100)));
            }

            let tick = Instant::now();
            let next = poll_snapshot(&dir);
            cost = tick.elapsed();
            let events = diff_snapshots(&previous, &next);
            previous = next;
            let mut sent = false;
            for event in events {
                if tx.send(event).is_err() {
                    return;
                }
                sent = true;
            }
            if sent {
                ctx.request_repaint();
            }
        }
    });
    (stop, restart)
}

impl DirWatcher {
    fn new(dir: &Path, ctx: &egui::Context) -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();

        // Unknown counts as remote: a poll that was not needed is a wasted
        // directory scan, while a native watch that was not possible is a
        // feature that silently does nothing.
        if !is_local_volume(dir).unwrap_or(false) {
            // Polled by [`spawn_poll_thread`] rather than by `notify`'s
            // `PollWatcher`, which re-`stat`s every file it has already
            // enumerated; [`poll_snapshot`] says what that cost.
            let (poll_stop, poll_restart) =
                spawn_poll_thread(dir.to_path_buf(), tx, ctx, poll_interval());
            return Some(Self {
                _watcher: None,
                poll_stop: Some(poll_stop),
                poll_restart: Some(poll_restart),
                rx,
            });
        }

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

        let mut watcher: Box<dyn Watcher + Send> =
            Box::new(notify::recommended_watcher(handler).ok()?);
        watcher.watch(dir, RecursiveMode::NonRecursive).ok()?;
        Some(Self {
            _watcher: Some(watcher),
            poll_stop: None,
            poll_restart: None,
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
    /// The user asked for this scan (F5 / *File ▸ Refresh filesystem*) rather
    /// than an event having reported something. Two things follow: the open
    /// files are reported before the directory re-parse that would otherwise
    /// make the user wait for them ([`run_scan`]), and what comes back is
    /// applied whatever the pointer is over — the user is looking at the
    /// window they just pressed a key in.
    forced: bool,
    /// The last load of this directory. What has not moved since is served
    /// from it, so the re-parse costs one `stat` per file rather than a read
    /// and a parse. See [`DirCache`].
    cache: Arc<Mutex<DirCache>>,
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
    /// Apply this on arrival whatever the pointer is over: it answers an
    /// explicit request. See [`ScanRequest::forced`].
    forced: bool,
    /// The last result this scan will send. A forced scan sends the open
    /// files first and the directory afterwards, so only the second of the
    /// two frees the scan slot.
    last: bool,
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
    /// `rebuild_inflight` exists for in `background.rs`.
    scanning: bool,
    /// Scanned changes not applied yet, because the pointer is over the pane
    /// showing them. Retried every frame, without touching the filesystem.
    held: Vec<ScannedFile>,
    /// A file list not applied yet, because the pointer is over the sidebar.
    held_listing: Option<Vec<PathBuf>>,
    /// The user clicked the held-changes notice: apply what is held on this
    /// frame whatever the pointer is on. Cleared by the apply it authorizes.
    force_apply: bool,
    /// A refresh the user asked for is waiting to be scanned. It outlives the
    /// settle delay and the pending set — there may be no event at all — and
    /// is cleared by the scan it starts.
    refresh: bool,
    /// What the last load of this directory found, shared with the scan
    /// thread. One scan runs at a time, so the lock is never contended; it is
    /// a lock rather than an owned value because the scan outlives the call
    /// that started it.
    dir_cache: Arc<Mutex<DirCache>>,
}

impl WatchState {
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self::with_cache(DirCache::new())
    }

    /// The watch over a directory that has just been loaded, keeping what that
    /// load found so the first refresh is already cheap.
    pub(super) fn with_cache(cache: DirCache) -> Self {
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
            refresh: false,
            dir_cache: Arc::new(Mutex::new(cache)),
        }
    }

    /// Loads a directory on the calling thread through this watch's cache —
    /// the path Open Folder takes, so that the folder it just read is the one
    /// the next refresh compares against.
    pub(super) fn load_directory(&self, dir: &Path) -> LoadedDir {
        crate::render::ttf_builder::load_docs_from_directory_cached(dir, &mut lock(&self.dir_cache))
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
        self.refresh = false;
        // A scan of the previous directory may still be running; its result is
        // about files that are no longer on screen, so it is dropped on
        // arrival rather than waited for.
        let (tx, rx) = mpsc::channel();
        self.scan_tx = tx;
        self.scan_rx = rx;
        self.scanning = false;
        // The cache describes the directory that is being left; nothing in it
        // says anything about this one.
        *lock(&self.dir_cache) = DirCache::new();
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

    /// F5 / *File ▸ Refresh filesystem*: look at the directory now, without
    /// waiting for an event to say something changed.
    ///
    /// The point of the command is the case where no event will ever arrive —
    /// a share whose kernel reports nothing and whose poll tick is seconds
    /// away, or a watch that could not be installed at all — so it does not go
    /// through the pending set or the settle delay. What it does share with an
    /// event-driven scan is the scan thread: a refresh asked for while one is
    /// running waits for it rather than starting a second.
    pub(super) fn request_refresh(&mut self) {
        self.refresh = true;
        // A polled directory is about to be read by the scan thread, so the
        // tick that was coming would only read it again. The poll interval
        // restarts from here.
        if let Some(watcher) = &self.watcher
            && let Some(restart) = &watcher.poll_restart
        {
            restart.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Whether there is something to scan and no scan is already running. A
    /// refresh the user asked for skips the settle delay: nothing is being
    /// waited out, since the request is not a report that a write is in
    /// progress.
    fn ready_to_scan(&self) -> bool {
        if self.scanning {
            return false;
        }
        if self.refresh {
            return true;
        }
        if self.pending.is_empty() && !self.listing_dirty {
            return false;
        }
        self.settle_at.is_some_and(|at| Instant::now() >= at)
    }

    /// The event paths to resolve into a scan request, and whether the
    /// directory itself has to be looked at — which a refresh always asks for,
    /// since it is asked precisely when nothing is known about what changed.
    fn take_pending(&mut self) -> (Vec<PathBuf>, bool, bool) {
        self.settle_at = None;
        let paths = std::mem::take(&mut self.pending).into_iter().collect();
        let refresh = std::mem::replace(&mut self.refresh, false);
        let listing = std::mem::replace(&mut self.listing_dirty, false) || refresh;
        (paths, listing, refresh)
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
            // `last` is what clears it, so the panic value has to carry it.
            let interim = tx.clone();
            let mut slot = super::background::ResultSlot::new(
                tx,
                ctx.clone(),
                ScanResult {
                    last: true,
                    ..Default::default()
                },
            );
            slot.set(run_scan(request, &interim, &ctx));
        });
    }

    fn take_scan_result(&mut self) -> Option<ScanResult> {
        let result = self.scan_rx.try_recv().ok()?;
        // A forced scan reports twice; the slot is only free once the second
        // of the two has arrived.
        self.scanning &= !result.last;
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

/// A poisoned cache is a cache, not a reason to lose the directory: the load
/// that panicked left it describing fewer files than it might have, and the
/// next load fills those back in.
fn lock(cache: &Mutex<DirCache>) -> std::sync::MutexGuard<'_, DirCache> {
    cache.lock().unwrap_or_else(|e| e.into_inner())
}

/// The scan itself. Runs on its own thread; touches no application state.
///
/// A forced scan reports in two stages, which is why it is handed the sender
/// as well as being one: the open files are what the user is looking at, and
/// re-parsing the directory around them is 34 files of I/O on a share. So the
/// files go back as soon as they are read, and the snapshot follows in the
/// result this returns.
fn run_scan(
    request: ScanRequest,
    interim: &mpsc::Sender<ScanResult>,
    ctx: &egui::Context,
) -> ScanResult {
    let forced = request.forced;
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

    if forced && request.dir.is_some() {
        let staged = ScanResult {
            files: std::mem::take(&mut files),
            snapshot: None,
            listing: None,
            forced: true,
            last: false,
        };
        if interim.send(staged).is_err() {
            // The receiving side went away with the folder it was watching.
            return ScanResult {
                last: true,
                ..Default::default()
            };
        }
        ctx.request_repaint();
    }

    let (snapshot, listing) = match request.dir {
        Some(dir) => {
            let parsed = crate::render::ttf_builder::load_docs_from_directory_cached(
                &dir,
                &mut lock(&request.cache),
            );
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
        forced,
        last: true,
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

    /// *File ▸ Refresh filesystem* / F5, from either frontend.
    ///
    /// Says so in the status line, because the work it starts is otherwise
    /// invisible: over a share the re-parse behind it is seconds long, and a
    /// refresh that finds nothing changes not one pixel — pressing the key
    /// again is then the only way to find out whether the first press did
    /// anything. The two things the user cannot see are what the message
    /// distinguishes: a scan starting now, and a request that has to wait for
    /// the scan already running.
    ///
    /// The repaint is what makes the menu's request as immediate as the key's:
    /// the scan is started by [`Self::pump_file_watch`], which has already run
    /// by the time the menu is dispatched.
    pub(super) fn request_filesystem_refresh(&mut self, ctx: &egui::Context) {
        if self.watch.scanning {
            self.set_status(
                "Refreshing the font directory — the request waits for the scan already running.",
            );
        } else {
            self.set_status("Refreshing the font directory…");
        }
        self.watch.request_refresh();
        ctx.request_repaint();
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
        let mut moved = result.files.len();
        if let Some((docs, errors, sources)) = result.snapshot {
            // The snapshot covers the open files too, so it is the count —
            // adding the staged files to it would report a file the user
            // changed twice.
            moved = self.apply_directory_snapshot(docs, errors, sources);
        }
        // Only the result that frees the scan slot answers the request: the
        // staged one is half of what was asked for.
        if result.forced && result.last {
            self.set_status(match moved {
                0 => "Font directory refreshed: nothing changed on disk.".to_string(),
                1 => "Font directory refreshed: 1 file changed on disk.".to_string(),
                n => format!("Font directory refreshed: {n} files changed on disk."),
            });
        }
        if let Some(listing) = result.listing {
            self.watch.hold_listing(listing);
        }
        for file in result.files {
            self.watch.hold(file);
        }
        // A scan the user asked for is applied on the frame it lands on: the
        // pointer is wherever it was left when the key was pressed, which says
        // nothing about whether the answer is wanted.
        self.watch.force_apply |= result.forced;
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
                unchanged: doc.knows_disk_bytes(file.hash),
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
        let (paths, listing_changed, forced) = self.watch.take_pending();
        let mut files = Vec::new();
        let mut snapshot = listing_changed;
        if forced {
            // Nothing reported anything, so everything is a candidate — and
            // the open documents go in first, since they are what the user is
            // looking at and [`run_scan`] reports them before it re-parses the
            // directory around them.
            files.extend(
                self.open_documents
                    .iter()
                    .map(|doc| (doc.document.path.clone(), doc.disk_hash)),
            );
        }
        for path in paths {
            let name = path.file_name();
            match self
                .open_documents
                .iter()
                .find(|d| d.document.path.file_name() == name)
            {
                Some(doc) => {
                    if !files.iter().any(|(p, _)| p == &doc.document.path) {
                        files.push((doc.document.path.clone(), doc.disk_hash));
                    }
                }
                None => snapshot = true,
            }
        }
        let dir = snapshot.then(|| self.font_dir.clone()).flatten();
        if files.is_empty() && dir.is_none() {
            return;
        }
        let cache = Arc::clone(&self.watch.dir_cache);
        self.watch.start_scan(
            ScanRequest {
                files,
                dir,
                forced,
                cache,
            },
            ctx,
        );
    }

    /// Installs a directory snapshot parsed on the scan thread.
    ///
    /// The generation counters are stepped past the snapshot being replaced —
    /// [`super::UniformApp::current_font_gen`] hashes them, and a freshly
    /// parsed document starts back at zero, which would read as "nothing
    /// changed" and rebuild nothing.
    ///
    /// Stepped only for a document whose *bytes* are not the ones already
    /// installed, though. A refresh re-parses the whole directory because what
    /// changed is exactly what is not known (`run_scan`), and stepping every
    /// document made an F5 that found nothing cost a full resolve and font
    /// build — 2.3 s on `font/` — for a snapshot identical to the one in hand.
    /// A document that did not move keeps its counters verbatim, which is what
    /// "nothing changed" has to look like downstream; anything else here would
    /// only move the wasted rebuild rather than remove it.
    /// Returns how many files moved: parsed differently, appeared, or went
    /// away. That is what a refresh reports having found, and it is counted
    /// here because this is where the comparison already happens.
    fn apply_directory_snapshot(
        &mut self,
        mut docs: Vec<Document>,
        errors: Vec<(PathBuf, String)>,
        sources: Vec<(PathBuf, Vec<u8>)>,
    ) -> usize {
        // Hashed once per file here rather than per document below; the same
        // hash `install_font_snapshot` records for the sources it installs.
        let scanned: HashMap<&Path, u64> = sources
            .iter()
            .map(|(path, bytes)| (path.as_path(), hash_bytes(bytes)))
            .collect();
        // A file the previous snapshot had and this one does not: gone from
        // the directory, and a change of its own.
        let mut moved = self
            .font_base_docs
            .iter()
            .filter(|previous| !docs.iter().any(|doc| doc.path == previous.path))
            .count();
        for doc in &mut docs {
            let previous = self.font_base_docs.iter().find(|d| d.path == doc.path);
            let unchanged = scanned.get(doc.path.as_path()).is_some_and(|hash| {
                self.font_sources
                    .get(&doc.path)
                    .is_some_and(|source| source.hash == *hash)
            });
            if !unchanged {
                moved += 1;
            }
            let Some(previous) = previous else {
                // A file that was not in the snapshot is a change by itself.
                doc.edit_gen = 1;
                doc.content_gen = 1;
                continue;
            };
            if unchanged {
                doc.edit_gen = previous.edit_gen;
                doc.content_gen = previous.content_gen;
                // Not derived from the text, so it cannot be reparsed back:
                // carried, or a document that nothing changed would still
                // hash differently.
                doc.pixel_gen = previous.pixel_gen;
                continue;
            }
            doc.edit_gen = previous.edit_gen.wrapping_add(1);
            doc.content_gen = previous.content_gen.wrapping_add(1);
        }
        self.install_font_snapshot(docs, errors, sources);
        moved
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

        let (tx, rx) = mpsc::channel();
        let result = run_scan(
            ScanRequest {
                files: vec![
                    (same.clone(), Some(hash_bytes(source.as_bytes()))),
                    // The buffer believes something else is on disk.
                    (changed.clone(), Some(0)),
                ],
                dir: Some(dir.clone()),
                forced: false,
                cache: Arc::new(Mutex::new(DirCache::new())),
            },
            &tx,
            &egui::Context::default(),
        );
        assert!(
            rx.try_recv().is_err(),
            "an event-driven scan reports once, at the end"
        );
        assert!(result.last);

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

    /// F5 exists for the case where nothing will ever report a change — a
    /// share the kernel says nothing about, a watch that could not be
    /// installed — so the request has to reach the scan with no event behind
    /// it and no settle delay in front of it, and it has to ask for the
    /// directory as well, since what changed is exactly what is not known.
    #[test]
    fn a_requested_refresh_is_scanned_at_once_and_covers_the_directory() {
        let mut watch = WatchState::new();
        assert!(!watch.ready_to_scan(), "nothing to scan yet");

        watch.request_refresh();
        assert!(watch.ready_to_scan(), "a refresh waits for nothing");

        let (paths, listing, forced) = watch.take_pending();
        assert!(paths.is_empty(), "no event is behind a refresh");
        assert!(listing, "a refresh re-reads the directory");
        assert!(forced);
        assert!(
            !watch.ready_to_scan(),
            "the request is spent by the scan it starts"
        );
    }

    /// A refresh asked for while a scan is running does not start a second
    /// one — it waits, exactly as an event does.
    #[test]
    fn a_refresh_does_not_start_a_second_scan() {
        let mut watch = WatchState::new();
        watch.scanning = true;
        watch.request_refresh();
        assert!(!watch.ready_to_scan());
        watch.scanning = false;
        assert!(watch.ready_to_scan(), "and it is not forgotten either");
    }

    /// The point of asking is the file on screen, and re-parsing the whole
    /// directory around it is 34 files of I/O on a share. So a forced scan
    /// reports the open files first, in a result that does *not* free the scan
    /// slot, and the directory follows in the one that does. Both are marked
    /// forced, so the pointer does not hold either back.
    #[test]
    fn a_forced_scan_reports_the_open_files_before_the_directory() {
        let dir = std::env::temp_dir().join(format!("uniform-refresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let open = dir.join("open.unf");
        let source = "glyph a 2 2\n@@@@\n@@@@\n";
        std::fs::write(&open, source).unwrap();
        std::fs::write(dir.join("other.unf"), source).unwrap();

        let (tx, rx) = mpsc::channel();
        let result = run_scan(
            ScanRequest {
                files: vec![(open.clone(), Some(0))],
                dir: Some(dir.clone()),
                forced: true,
                cache: Arc::new(Mutex::new(DirCache::new())),
            },
            &tx,
            &egui::Context::default(),
        );

        let staged = rx.try_recv().expect("the open files are reported first");
        let scanned: Vec<&Path> = staged.files.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(scanned, [open.as_path()]);
        assert!(staged.forced);
        assert!(!staged.last, "the directory is still coming");
        assert!(staged.snapshot.is_none() && staged.listing.is_none());

        assert!(result.files.is_empty(), "reported already");
        assert!(result.snapshot.is_some() && result.listing.is_some());
        assert!(result.forced);
        assert!(result.last, "the scan slot is freed by the second result");

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

        let (paths, _listing, _forced) = watch.take_pending();
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["a.unf"], "got {names:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The poll backend reports what changed and nothing else — in particular
    /// not the files that were simply *there* when it started, which would
    /// reload the whole directory a moment after it was read.
    #[test]
    fn polling_reports_changes_and_not_the_files_it_started_with() {
        let dir = std::env::temp_dir().join(format!("uniform-poll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.unf"), "glyph a 2 2\n@@@@\n@@@@\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let (stop, _restart) = spawn_poll_thread(dir.clone(), tx, ctx, Duration::from_millis(20));
        // The baseline is taken on that thread — deliberately, so that no
        // `read_dir` of a share ever runs on the UI thread — so the writes below
        // have to come after it, or they are the baseline rather than a change.
        // Ticks in between report nothing, which is half of what this asserts.
        std::thread::sleep(Duration::from_millis(300));

        // A file that changes size and one that appears: an in-place change is
        // not a listing change, a new entry is.
        std::fs::write(
            dir.join("a.unf"),
            "glyph a 2 2\n@@@@\n@@@@\nglyph b 1 1\n@@\n",
        )
        .unwrap();
        std::fs::write(dir.join("b.unf"), "glyph c 1 1\n@@\n").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen: Vec<(String, bool)> = Vec::new();
        while Instant::now() < deadline && seen.len() < 2 {
            while let Ok(event) = rx.try_recv() {
                let name = event
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                seen.push((name, event.listing));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);

        seen.sort();
        assert_eq!(
            seen,
            [("a.unf".to_string(), false), ("b.unf".to_string(), true)],
            "the baseline file must not be reported, and only the new entry touches the listing"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F5 reads the directory itself, so the tick that was one second away is
    /// one more enumeration of the share for nothing. The interval restarts
    /// from the request instead — held here by asking repeatedly, and the
    /// event that was waiting arrives only once the asking stops.
    #[test]
    fn a_requested_refresh_restarts_the_poll_interval() {
        use std::sync::atomic::Ordering;

        let dir = std::env::temp_dir().join(format!("uniform-poll-reset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.unf"), "glyph a 2 2\n@@@@\n@@@@\n").unwrap();

        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let floor = Duration::from_millis(400);
        let (stop, restart) = spawn_poll_thread(dir.clone(), tx, ctx, floor);

        // After the baseline, so it is a change rather than the starting state.
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(dir.join("b.unf"), "glyph c 1 1\n@@\n").unwrap();

        // Asked for more often than one interval is long: every ask pushes the
        // tick a whole interval away, so it never comes round.
        let held_until = Instant::now() + Duration::from_millis(1_200);
        while Instant::now() < held_until {
            restart.store(true, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(100));
            assert!(
                rx.try_recv().is_err(),
                "a poll that keeps being asked to start over must not tick"
            );
        }

        // Nothing asks any more, so the interval runs out and the change that
        // was waiting is reported.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = None;
        while Instant::now() < deadline && seen.is_none() {
            seen = rx.try_recv().ok();
            std::thread::sleep(Duration::from_millis(10));
        }
        stop.store(true, Ordering::Relaxed);
        let event = seen.expect("the poll resumes once nothing restarts it");
        assert_eq!(event.path.file_name().unwrap(), "b.unf");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The interval follows what a tick costs, so the poll can never take more
    /// of the volume than [`POLL_DUTY_DIVISOR`] allows — which is what a
    /// fixed ten seconds could not promise once a tick grew to eight.
    #[test]
    fn the_poll_interval_follows_what_a_tick_costs() {
        let floor = Duration::from_millis(2_000);
        assert_eq!(
            next_poll_delay(Duration::from_millis(1), floor),
            floor,
            "a cheap tick polls at the floor"
        );
        assert_eq!(
            next_poll_delay(Duration::from_millis(500), floor),
            Duration::from_millis(10_000),
            "a slow tick backs off in proportion"
        );
        assert_eq!(
            next_poll_delay(Duration::from_secs(30), floor),
            Duration::from_millis(MAX_POLL_MS),
            "and never past the cap: a directory looked at once a minute is still watched"
        );
    }

    /// A refresh re-parses the whole directory, and installing that snapshot
    /// used to step *every* document's generation — so an F5 that found
    /// nothing still rebuilt the whole font (on `font/` a 1.2 s resolve and a
    /// 1.1 s build) for a snapshot identical to the one in hand. A document
    /// whose bytes are the ones already installed keeps its generations, so
    /// nothing downstream reads it as changed; one whose bytes moved steps
    /// them as before.
    #[test]
    fn a_snapshot_that_matches_what_is_in_hand_rebuilds_nothing() {
        let dir = std::env::temp_dir().join(format!("uniform-snapshot-gen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.unf"), "glyph a 2 2\n@@\n.@\n").unwrap();
        std::fs::write(dir.join("b.unf"), "glyph b 2 2\n@@\n@.\n").unwrap();

        let ctx = egui::Context::default();
        let mut app =
            super::super::UniformApp::with_settings(&ctx, Default::default(), Some(dir.clone()));
        assert_eq!(app.font_base_docs.len(), 2);
        let before = app.current_font_gen();

        let (docs, errors, sources) =
            crate::render::ttf_builder::load_docs_from_directory_with_sources(&dir);
        app.apply_directory_snapshot(docs, errors, sources);
        assert_eq!(
            app.current_font_gen(),
            before,
            "a refresh that found nothing must not look like an edit"
        );

        std::fs::write(dir.join("b.unf"), "glyph b 2 2\n@@\n@@\n").unwrap();
        let (docs, errors, sources) =
            crate::render::ttf_builder::load_docs_from_directory_with_sources(&dir);
        app.apply_directory_snapshot(docs, errors, sources);
        assert_ne!(
            app.current_font_gen(),
            before,
            "and one that found a change still rebuilds"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// F5 over a share is seconds of work with nothing on screen to say so,
    /// and a request that arrives while a scan is running waits without a
    /// word. The status line says which of the two happened, and what the
    /// refresh found once it lands.
    #[test]
    fn a_refresh_says_so_in_the_status_line() {
        let dir =
            std::env::temp_dir().join(format!("uniform-refresh-status-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.unf"), "glyph a 2 2\n@@\n.@\n").unwrap();

        let ctx = egui::Context::default();
        let mut app =
            super::super::UniformApp::with_settings(&ctx, Default::default(), Some(dir.clone()));

        app.request_filesystem_refresh(&ctx);
        let (msg, _) = app.status_message.clone().expect("a refresh says so");
        assert!(msg.contains("Refreshing"), "{msg:?}");

        // A second request while the first is still out waits for it, and says
        // that rather than repeating the first message.
        app.watch.scanning = true;
        app.request_filesystem_refresh(&ctx);
        let (msg, _) = app
            .status_message
            .clone()
            .expect("a queued refresh says so");
        assert!(msg.contains("waits"), "{msg:?}");
        app.watch.scanning = false;

        // What it found: the snapshot is the one already installed.
        let (docs, errors, sources) =
            crate::render::ttf_builder::load_docs_from_directory_with_sources(&dir);
        assert_eq!(app.apply_directory_snapshot(docs, errors, sources), 0);

        std::fs::write(dir.join("b.unf"), "glyph b 2 2\n@@\n@.\n").unwrap();
        let (docs, errors, sources) =
            crate::render::ttf_builder::load_docs_from_directory_with_sources(&dir);
        assert_eq!(
            app.apply_directory_snapshot(docs, errors, sources),
            1,
            "the file that appeared is what moved"
        );

        std::fs::remove_file(dir.join("b.unf")).unwrap();
        let (docs, errors, sources) =
            crate::render::ttf_builder::load_docs_from_directory_with_sources(&dir);
        assert_eq!(
            app.apply_directory_snapshot(docs, errors, sources),
            1,
            "and so is the one that went away"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
