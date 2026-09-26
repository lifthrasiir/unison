//! Who hands freed memory back to the OS: a thread of its own, not the editor's.
//!
//! mimalloc does not return a freed page at once. It schedules the page to be
//! *purged* — decommitted, a system call per range — once a delay has passed,
//! and carries the purge out in whichever thread next frees or retires a page
//! after that deadline. In the editor that is almost always the UI thread: it
//! allocates and frees every frame, while a rebuild on the background threads
//! frees a gigabyte of expansion at a time. So the UI thread kept picking up the
//! bill for the rebuild's garbage — 15–25 ms of `madvise` inside one frame on a
//! fast Mac, landing on no particular piece of code, which is what made it
//! look like the tessellator or the minimap in turn.
//!
//! [`purge_off_the_ui_thread`] takes the deadline out of reach and makes the
//! purge a chore: the delay is set to an hour, which no thread ever waits out,
//! and a thread whose only job is this forces the purge once a second. Memory
//! still goes back — a second late rather than a second early — and it is
//! never the frame's problem.
//!
//! Only the GUI does this. A headless build frees its memory by exiting.

use std::time::Duration;

/// `mi_option_purge_delay` in mimalloc v3's `mi_option_t`, which
/// `libmimalloc-sys` does not name. Checked against the default below before it
/// is written, so a renumbered enum leaves the allocator alone rather than
/// setting some other option.
const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t = 15;
/// The v3 default for that option, in milliseconds.
const DEFAULT_PURGE_DELAY_MS: std::ffi::c_long = 1000;
const PURGE_DELAY_MS: std::ffi::c_long = 3_600_000;
const PURGE_EVERY: Duration = Duration::from_secs(1);

/// See the module docs. Idempotent; the first call starts the thread.
pub(crate) fn purge_off_the_ui_thread() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        // SAFETY: plain option accessors; the only hazard is the documented
        // race with another thread setting options, and nothing else does.
        unsafe {
            if libmimalloc_sys::mi_option_get(MI_OPTION_PURGE_DELAY) != DEFAULT_PURGE_DELAY_MS {
                return;
            }
            libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, PURGE_DELAY_MS);
        }
        let spawned = std::thread::Builder::new()
            .name("heap-purge".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(PURGE_EVERY);
                    // SAFETY: collects this thread's own heap and purges what
                    // the arenas have scheduled; callable from any thread.
                    unsafe { libmimalloc_sys::mi_collect(true) };
                }
            });
        if spawned.is_err() {
            // Without the thread nothing would ever purge: put the deadline back.
            unsafe {
                libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, DEFAULT_PURGE_DELAY_MS)
            };
        }
    });
}
