//! Where the seconds before the first frame go.
//!
//! Launching the editor off a network share takes ten to twenty seconds, and
//! from the outside there is nothing to tell apart the three candidates: the
//! loader paging the executable in over SMB, the directory load reading a
//! hundred `.unf` files one round trip at a time, or the initial font build
//! that [`crate::app::UniformApp::new`] runs *before* eframe ever paints. This
//! module makes each one a number.
//!
//! The timeline is a flat list of marks taken on the startup path plus, for the
//! directory load, one row per file. Collection stops at the first painted
//! frame — the same loader runs on every later rebuild, and a growing list of
//! rows would be a leak for no gain.
//!
//! Three ways to read it out:
//!
//! - `UNIFORM_PERF` set: the report goes to stderr once, after the first frame.
//! - In the GUI: *View → Startup timing…*, which is the only route when the
//!   binary was started from a shell that is gone or a shortcut that has no
//!   console.
//! - `uniform probe -i DIR`: the same measurements with no window at all, so a
//!   share can be timed without the GUI in the way. It also re-runs the
//!   directory load, which is what tells a cold cache from a warm one.
//!
//! `before_main` — process creation to the first line of `main` — is the one
//! number that answers "is it the executable itself?". It is not the whole
//! story on Windows, where the image is demand-paged for the life of the
//! process (see `run-local.cmd`), so a slow share keeps charging for code pages
//! long after the loader is done; a *large* `before_main` is conclusive, a
//! small one is not an acquittal.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One point on the startup path.
struct Mark {
    label: String,
    /// Since [`Timeline::origin`].
    at: Duration,
    /// Since the previous mark — the cost of the stage that just ended.
    delta: Duration,
}

/// One `.unf` file the directory load went through.
struct FileStat {
    path: PathBuf,
    bytes: usize,
    read: Duration,
    parse: Duration,
}

struct Timeline {
    origin: Instant,
    before_main: Option<Duration>,
    marks: Vec<Mark>,
    /// Set by [`init`], cleared by [`first_frame_done`]: only the startup pass
    /// records files.
    collecting: bool,
    dir_scan: Option<(PathBuf, Duration)>,
    files: Vec<FileStat>,
}

impl Timeline {
    fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }
}

fn timeline() -> &'static Mutex<Timeline> {
    static TIMELINE: OnceLock<Mutex<Timeline>> = OnceLock::new();
    TIMELINE.get_or_init(|| {
        Mutex::new(Timeline {
            origin: Instant::now(),
            before_main: None,
            marks: Vec::new(),
            collecting: false,
            dir_scan: None,
            files: Vec::new(),
        })
    })
}

/// Starts the timeline. Call it as the first statement of `main`, so that
/// everything after it is measured and `before_main` covers exactly the part
/// that is not ours.
pub fn init() {
    let before_main = time_before_main();
    let mut t = timeline().lock().unwrap();
    t.origin = Instant::now();
    t.before_main = before_main;
    t.collecting = true;
}

/// Records a point on the startup path. Cheap enough to leave in unconditionally
/// (one lock, one push), and a no-op once the first frame is up.
pub fn mark(label: impl Into<String>) {
    let mut t = timeline().lock().unwrap();
    if !t.collecting {
        return;
    }
    let at = t.elapsed();
    let delta = at - t.marks.last().map(|m| m.at).unwrap_or_default();
    t.marks.push(Mark {
        label: label.into(),
        at,
        delta,
    });
}

/// Records how long it took just to enumerate the font directory — on a share
/// this is one round trip that can outweigh the reads that follow.
pub fn record_dir_scan(dir: &Path, elapsed: Duration) {
    let mut t = timeline().lock().unwrap();
    if !t.collecting {
        return;
    }
    t.dir_scan = Some((dir.to_path_buf(), elapsed));
}

/// Records one file of the directory load. Called from
/// [`crate::render::ttf_builder::load_docs_from_directory_with_sources`], which
/// also runs on every later rebuild — hence the `collecting` guard.
pub fn record_file(path: &Path, bytes: usize, read: Duration, parse: Duration) {
    let mut t = timeline().lock().unwrap();
    if !t.collecting {
        return;
    }
    t.files.push(FileStat {
        path: path.to_path_buf(),
        bytes,
        read,
        parse,
    });
}

/// Ends collection. The report stays available for the GUI window; only new
/// rows stop being added.
pub fn first_frame_done() {
    let mut t = timeline().lock().unwrap();
    t.collecting = false;
}

/// Whether `[perf]`-style logging was asked for. The editor's own stage timings
/// read this too (`app::perf_log_enabled`): one variable turns on all of it.
pub fn perf_logging() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("UNIFORM_PERF").is_some())
}

/// A `[perf]` stage timer: prints `label` and its wall time when dropped, and
/// costs nothing (not even the clock read) when `UNIFORM_PERF` is unset.
///
/// The build pipeline is a chain of stages inside one function call, so a timer
/// that ends at a scope boundary is what fits it; the editor's stages, which end
/// at a thread's exit, keep their explicit `Instant`s.
pub struct PerfStage {
    label: &'static str,
    start: Option<Instant>,
}

impl PerfStage {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            start: perf_logging().then(Instant::now),
        }
    }
}

impl Drop for PerfStage {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            eprintln!("[perf] {}: {:?}", self.label, start.elapsed());
        }
    }
}

/// Clears everything but the origin, so a second measurement (the `probe`
/// subcommand's repeat run) does not read as a continuation of the first.
pub fn restart_collection() {
    let mut t = timeline().lock().unwrap();
    t.origin = Instant::now();
    t.marks.clear();
    t.dir_scan = None;
    t.files.clear();
    t.collecting = true;
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// The human-readable timeline. Both the stderr dump and the GUI window print
/// exactly this.
pub fn report() -> String {
    let t = timeline().lock().unwrap();
    let mut out = String::new();
    out.push_str("Startup timing\n");
    out.push_str("==============\n\n");
    if cfg!(debug_assertions) {
        // A debug build spends tens of seconds in resolve and the font build,
        // which would read as an I/O problem it is not.
        out.push_str(
            "(debug build \u{2014} compute stages are 10-30x slower than a release build)\n\n",
        );
    }

    match t.before_main {
        Some(d) => out.push_str(&format!(
            "before main()          {:>12}   (process creation \u{2192} first line of main:\n\
             \x20                                    loader, image paging, runtime init)\n",
            ms(d)
        )),
        None => out.push_str("before main()                     n/a   (not measurable here)\n"),
    }
    out.push('\n');

    out.push_str("stage                           elapsed         since previous\n");
    for m in &t.marks {
        out.push_str(&format!(
            "  {:<30}{:>10}   {:>14}\n",
            m.label,
            ms(m.at),
            ms(m.delta)
        ));
    }

    if let Some((dir, d)) = &t.dir_scan {
        out.push_str(&format!(
            "\ndirectory scan (read_dir) of {}: {}\n",
            dir.display(),
            ms(*d)
        ));
    }

    if !t.files.is_empty() {
        let total_read: Duration = t.files.iter().map(|f| f.read).sum();
        let total_parse: Duration = t.files.iter().map(|f| f.parse).sum();
        let total_bytes: usize = t.files.iter().map(|f| f.bytes).sum();
        let n = t.files.len();
        out.push_str(&format!(
            "\n{n} file(s), {:.1} KiB: read {} ({} avg), parse {}\n",
            total_bytes as f64 / 1024.0,
            ms(total_read),
            ms(total_read / n as u32),
            ms(total_parse),
        ));
        // Sorted by read time: on a share the interesting thing is the tail,
        // and the file order is alphabetical noise.
        let mut by_read: Vec<&FileStat> = t.files.iter().collect();
        by_read.sort_by_key(|f| std::cmp::Reverse(f.read));
        out.push_str("  slowest reads:\n");
        for f in by_read.iter().take(10) {
            let name = f
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            out.push_str(&format!(
                "    {:<28}{:>10} read {:>10} parse  {:>8} B\n",
                name,
                ms(f.read),
                ms(f.parse),
                f.bytes
            ));
        }
    }

    out
}

/// Prints the report to stderr, once, when `UNIFORM_PERF` is set.
#[cfg(feature = "editor")]
pub fn log_report_once() {
    static DONE: OnceLock<()> = OnceLock::new();
    if !perf_logging() || DONE.set(()).is_err() {
        return;
    }
    eprint!("{}", report());
}

/// Process creation to now, for the one question the in-process timeline cannot
/// answer: how much of the wait happened before any of our code ran.
#[cfg(target_os = "windows")]
fn time_before_main() -> Option<Duration> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }
    let now = unsafe { GetSystemTimeAsFileTime() };
    let as_u64 = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    // FILETIME ticks are 100 ns.
    let ticks = as_u64(now).checked_sub(as_u64(creation))?;
    Some(Duration::from_nanos(ticks.saturating_mul(100)))
}

/// macOS has no `kinfo_proc` in `libc`, so this reads the one field it wants
/// straight out of the `KERN_PROC_PID` blob. `struct kinfo_proc` opens with
/// `struct extern_proc`, whose first member is the union that puts
/// `p_un.__p_starttime` — a `timeval` — at offset 0; that placement is part of
/// the published `<sys/proc.h>` layout, and the size check below refuses the
/// read if a kernel ever returns something smaller.
#[cfg(target_os = "macos")]
fn time_before_main() -> Option<Duration> {
    const TIMEVAL_LEN: usize = 16; // i64 tv_sec + i32 tv_usec + padding

    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_PID,
        std::process::id() as i32,
    ];
    let mut size: libc::size_t = 0;
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size < TIMEVAL_LEN {
        return None;
    }
    let mut buf = vec![0u8; size];
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buf.as_mut_ptr().cast::<libc::c_void>(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || size < TIMEVAL_LEN {
        return None;
    }
    let sec = i64::from_ne_bytes(buf[0..8].try_into().ok()?);
    let usec = i32::from_ne_bytes(buf[8..12].try_into().ok()?);

    let mut now = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe { libc::gettimeofday(&mut now, std::ptr::null_mut()) };
    let start_us = sec * 1_000_000 + i64::from(usec);
    let now_us = now.tv_sec * 1_000_000 + i64::from(now.tv_usec);
    u64::try_from(now_us - start_us)
        .ok()
        .map(Duration::from_micros)
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn time_before_main() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The timeline is a process-wide singleton, so the tests that reset it
    /// cannot overlap.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn marks_accumulate_and_report() {
        let _guard = test_lock();
        restart_collection();
        mark("a");
        mark("b");
        record_file(
            Path::new("/x/one.unf"),
            12,
            Duration::from_millis(3),
            Duration::from_millis(1),
        );
        let text = report();
        assert!(text.contains("  a "), "{text}");
        assert!(text.contains("one.unf"), "{text}");
        assert!(text.contains("1 file(s)"), "{text}");
    }

    #[test]
    fn recording_stops_after_the_first_frame() {
        let _guard = test_lock();
        restart_collection();
        first_frame_done();
        mark("late");
        assert!(!report().contains("late"));
    }
}
