//! Stack usage monitor and stack-overflow post-mortem.
//!
//! The editor has been observed to die with a stack overflow on the main thread
//! at unpredictable moments, with no obvious unbounded recursion in our own
//! code. This module provides the runtime evidence needed to pin it down:
//!
//! * A watchdog thread samples the main thread's stack pointer on an interval
//!   and logs the current/peak usage, so a gradual creep can be told apart from
//!   a sudden dive.
//! * On Windows the sampler suspends the main thread, so the reading covers
//!   *every* frame -- including code inside egui/wgpu/DirectWrite -- without
//!   any instrumentation. When usage crosses a threshold it walks the suspended
//!   thread's stack and logs a symbolized backtrace, which is the whole point:
//!   we get the offending call chain *before* the process dies.
//! * A vectored exception handler catches `EXCEPTION_STACK_OVERFLOW` itself and
//!   dumps a backtrace as a last resort. `SetThreadStackGuarantee` reserves room
//!   so the handler can actually run.
//! * The same handler *records* every first-chance exception -- code, faulting
//!   address, thread, phase, one line per distinct site with a running count.
//!   A backtrace answers "where is the stack", not "what put it there", and an
//!   overflow whose frames are all exception-dispatch machinery (as observed on
//!   2026-07-29) is caused by whatever keeps raising, which no walk can see.
//!   Recording happens at the fault point, so it is atomics only; the watchdog
//!   does the logging.
//!
//! Two rules keep the monitor from wedging the process it is meant to diagnose,
//! both learned the hard way from a hang mid-report:
//!
//! * Nothing that allocates or takes a lock may run while the main thread is
//!   suspended -- the walker collects bare addresses and symbolizes afterwards.
//!   The exception handler logs from the main thread, so logging under
//!   suspension deadlocks against it.
//! * The handler reports at most once and tells the watchdog to leave it alone
//!   while it does, since reporting itself costs stack.
//!
//! On non-Windows targets only the cheap parts are active (bounds + `probe`
//! high-water marking + periodic logging); this keeps `cargo test` and native
//! macOS runs working without pulling in signal handling.
//!
//! Everything is off unless `UNIFORM_STACKMON` is set. Configuration:
//!
//! | variable | default | meaning |
//! |---|---|---|
//! | `UNIFORM_STACKMON` | unset (off) | `1`/`on` to enable |
//! | `UNIFORM_STACKMON_INTERVAL_MS` | `250` | sampling interval |
//! | `UNIFORM_STACKMON_DUMP_PCT` | `40` | dump a backtrace past this % of the stack |
//! | `UNIFORM_STACKMON_LOG` | `uniform-stackmon.log` | log file path |
//! | `UNIFORM_STACKMON_QUIET` | unset | only log when the peak grows |

// `collapse` and friends are only rendered by the Windows backend, but they
// are pure logic and live outside it so `cargo test` reaches them on any host.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering};

/// Highest address of the main thread's stack (stacks grow downwards).
static STACK_HIGH: AtomicUsize = AtomicUsize::new(0);
/// Lowest usable address of the main thread's stack.
static STACK_LOW: AtomicUsize = AtomicUsize::new(0);
/// High-water mark of `STACK_HIGH - sp`, in bytes.
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Most recent sample, in bytes.
static CURRENT: AtomicUsize = AtomicUsize::new(0);
/// Usage past which we dump a backtrace, in bytes.
static DUMP_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Next usage that triggers a dump; raised after each dump so one deep episode
/// does not produce hundreds of backtraces.
static NEXT_DUMP_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Coarse "what is the main thread doing" marker, set via [`phase`].
static PHASE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());

const DUMP_STEP: usize = 512 * 1024;

// ---------------------------------------------------------------- public API

/// Enables the monitor. Must be called from the main thread, once, before the
/// GUI starts. A no-op unless `UNIFORM_STACKMON` is set.
pub fn init() {
    let on = std::env::var("UNIFORM_STACKMON")
        .map(|v| matches!(v.as_str(), "1" | "on" | "yes" | "true"))
        .unwrap_or(false);
    if !on || ENABLED.swap(true, Ordering::SeqCst) {
        return;
    }

    let (low, high) = stack_bounds();
    STACK_HIGH.store(high, Ordering::SeqCst);
    STACK_LOW.store(low, Ordering::SeqCst);

    let size = high.saturating_sub(low);
    let pct = env_usize("UNIFORM_STACKMON_DUMP_PCT", 40).clamp(1, 99);
    let dump_at = size / 100 * pct;
    DUMP_AT.store(dump_at, Ordering::SeqCst);
    NEXT_DUMP_AT.store(dump_at, Ordering::SeqCst);

    log(&format!(
        "--- stackmon start: main stack {:#x}..{:#x} ({}), dump at {} ({}%), mode={}",
        low,
        high,
        human(size),
        human(dump_at),
        pct,
        if platform::SAMPLING {
            "sampling"
        } else {
            "probe-only"
        },
    ));

    platform::init_main_thread();
    platform::self_test();
    spawn_watchdog();
}

/// Records the caller's stack depth as a high-water sample.
///
/// Only needed on platforms without sampling support; on Windows the watchdog
/// already sees every depth. Cheap enough (one load, one compare) to leave in.
#[inline]
#[cfg(feature = "editor")]
pub fn probe() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let marker = 0u8;
    let sp = &marker as *const u8 as usize;
    let high = STACK_HIGH.load(Ordering::Relaxed);
    if high == 0 || sp > high {
        return;
    }
    PEAK.fetch_max(high - sp, Ordering::Relaxed);
}

/// Marks what the main thread is currently doing, so a deep-stack report can
/// name the phase. `tag` must be a `'static` NUL-free string.
#[inline]
#[cfg(feature = "editor")]
pub fn phase(tag: &'static str) {
    if ENABLED.load(Ordering::Relaxed) {
        PHASE.store(tag.as_ptr() as *mut u8, Ordering::Relaxed);
        PHASE_LEN.store(tag.len(), Ordering::Relaxed);
    }
}

static PHASE_LEN: AtomicUsize = AtomicUsize::new(0);

fn current_phase() -> &'static str {
    let p = PHASE.load(Ordering::Relaxed);
    let len = PHASE_LEN.load(Ordering::Relaxed);
    if p.is_null() || len == 0 {
        return "-";
    }
    // Safety: only ever set from `phase` with a `'static str`'s pointer/length.
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, len)) }
}

// ----------------------------------------------------------------- watchdog

fn spawn_watchdog() {
    let interval =
        std::time::Duration::from_millis(env_usize("UNIFORM_STACKMON_INTERVAL_MS", 250) as u64);
    let quiet = std::env::var("UNIFORM_STACKMON_QUIET").is_ok();

    std::thread::Builder::new()
        .name("stackmon".into())
        .spawn(move || {
            let heartbeat = std::time::Duration::from_millis(
                env_usize("UNIFORM_STACKMON_HEARTBEAT_MS", 4000) as u64,
            );
            let mut last_heartbeat = std::time::Instant::now();
            let mut last_reported_peak = 0usize;
            loop {
                std::thread::sleep(interval);
                // Before anything else: the exception handler only *records*
                // (it must not allocate or take the log lock at the point of a
                // fault), so the watchdog is what turns that table into log
                // lines. Doing it first means a report below is preceded by
                // the exceptions that led to it.
                platform::report_exceptions();

                let used = platform::sample_main_stack().unwrap_or(0);
                if used > 0 {
                    CURRENT.store(used, Ordering::Relaxed);
                    PEAK.fetch_max(used, Ordering::Relaxed);
                }
                let peak = PEAK.load(Ordering::Relaxed);
                let grew = peak > last_reported_peak;

                let due = !quiet && last_heartbeat.elapsed() >= heartbeat;
                if grew || due {
                    if due {
                        last_heartbeat = std::time::Instant::now();
                    }
                    log(&format!(
                        "used {:>9} peak {:>9} / {:>9} ({:>2}%) phase={}",
                        human(CURRENT.load(Ordering::Relaxed)),
                        human(peak),
                        human(stack_size()),
                        percent(peak),
                        current_phase(),
                    ));
                }
                if grew {
                    last_reported_peak = peak;
                }

                if peak >= NEXT_DUMP_AT.load(Ordering::Relaxed) {
                    NEXT_DUMP_AT.store(peak + DUMP_STEP, Ordering::Relaxed);
                    log(&format!(
                        "!!! deep stack: {} ({}%) in phase {} -- backtrace of the main thread:",
                        human(peak),
                        percent(peak),
                        current_phase(),
                    ));
                    platform::dump_main_thread_backtrace();
                }
            }
        })
        .ok();
}

fn stack_size() -> usize {
    STACK_HIGH
        .load(Ordering::Relaxed)
        .saturating_sub(STACK_LOW.load(Ordering::Relaxed))
}

fn percent(used: usize) -> usize {
    let size = stack_size();
    if size == 0 { 0 } else { used * 100 / size }
}

// ------------------------------------------------------------------ logging

static LOG: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
static LOG_OPENED: AtomicBool = AtomicBool::new(false);
thread_local! {
    /// Set while [`log`] is running on this thread; see the guard there.
    /// `Cell<bool>` has no destructor, so this is a plain TLS slot with no
    /// lazy-init machinery -- safe to touch from an exception handler.
    static IN_LOG: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Writes one line to stderr and to the log file.
///
/// Takes a lock and allocates, so it must never be called while the main
/// thread is suspended. The Windows exception handler *does* call it, from the
/// main thread; if the watchdog were to log with that thread frozen mid-`log`,
/// both threads would wedge forever (the watchdog waiting on the lock, the
/// main thread unable to release it). That is why the stack walker collects
/// bare addresses while suspended and only symbolizes and logs afterwards.
pub(crate) fn log(msg: &str) {
    use std::io::Write;
    // `LOG` is a plain (non-reentrant) mutex, so a second `log` on a thread
    // that is already inside one would deadlock against itself. That is not
    // hypothetical: the exception handler logs, and an exception can be raised
    // from anywhere -- including from inside this function's own file I/O.
    // Dropping the nested line is the only safe answer.
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            IN_LOG.set(false);
        }
    }
    if IN_LOG.replace(true) {
        return;
    }
    let _reentry = Guard;

    eprintln!("[stackmon] {msg}");
    let mut guard = match LOG.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if !LOG_OPENED.swap(true, Ordering::SeqCst) {
        let path = std::env::var("UNIFORM_STACKMON_LOG")
            .unwrap_or_else(|_| "uniform-stackmon.log".to_string());
        *guard = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
    }
    if let Some(f) = guard.as_mut() {
        let _ = writeln!(f, "{msg}");
        let _ = f.flush();
    }
}

fn human(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn stack_bounds() -> (usize, usize) {
    platform::stack_bounds().unwrap_or_else(|| {
        // Last-resort guess: assume we are near the top of an 8 MiB stack.
        let marker = 0u8;
        let sp = &marker as *const u8 as usize;
        let high = (sp + 0xffff) & !0xffff;
        (high.saturating_sub(8 * 1024 * 1024), high)
    })
}

// Only the Windows backend renders backtraces, but the folding below is pure
// logic, so it lives out here where `cargo test` can reach it on any host.
/// Collapsed entries printed from each end when a backtrace report is long.
const HEAD_ENTRIES: usize = 40;
const TAIL_ENTRIES: usize = 60;
/// Longest repeating frame cycle [`collapse`] will recognize.
const MAX_CYCLE: usize = 16;

/// One line of a backtrace report: either a single frame, or a frame cycle
/// that repeats `reps` times (runaway recursion).
struct Entry {
    /// 1-based depth of the first frame this entry covers.
    depth: usize,
    /// Length of the repeating cycle; 1 for an ordinary frame.
    period: usize,
    reps: usize,
}

impl Entry {
    fn frames(&self) -> usize {
        self.period * self.reps
    }
}

/// Folds repeating frame cycles into single entries.
///
/// Collapsing only adjacent *identical* frames is not enough: a runaway
/// recursion usually cycles through several distinct frames (a Rust function
/// and its callee, or `RtlRaiseException` / `KiUserExceptionDispatcher` /
/// `RtlVirtualUnwind`), so a one-frame window folds nothing and the report
/// drowns in copies of the cycle.
fn collapse(pcs: &[u64]) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pcs.len() {
        // Prefer the period covering the most frames; ties go to the shortest
        // period, which is the one actually being recursed on.
        let mut best: Option<(usize, usize)> = None;
        for k in 1..=MAX_CYCLE {
            if i + 2 * k > pcs.len() {
                break;
            }
            let mut reps = 1;
            while i + (reps + 1) * k <= pcs.len()
                && pcs[i..i + k] == pcs[i + reps * k..i + (reps + 1) * k]
            {
                reps += 1;
            }
            if reps >= 2 && best.is_none_or(|(bk, br)| reps * k > bk * br) {
                best = Some((k, reps));
            }
        }
        let (period, reps) = best.unwrap_or((1, 1));
        out.push(Entry {
            depth: i + 1,
            period,
            reps,
        });
        i += period * reps;
    }
    out
}

// ------------------------------------------------------------ windows backend

#[cfg(target_os = "windows")]
mod platform {
    use super::{
        ENABLED, Entry, HEAD_ENTRIES, STACK_HIGH, TAIL_ENTRIES, collapse, current_phase, human,
        log,
    };
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use windows::Win32::Foundation::{EXCEPTION_STACK_OVERFLOW, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_POINTERS, EXCEPTION_RECORD,
    };

    // The `windows` crate exposes these as Rust-ABI wrappers, but `StackWalkEx`
    // wants raw `system`-ABI callbacks, so import them directly.
    #[link(name = "dbghelp")]
    unsafe extern "system" {
        fn SymFunctionTableAccess64(process: HANDLE, addr_base: u64) -> *mut c_void;
        fn SymGetModuleBase64(process: HANDLE, addr: u64) -> u64;
    }
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetCurrentThreadId, GetCurrentThreadStackLimits,
        OpenThread, ResumeThread, SetThreadStackGuarantee, SuspendThread, THREAD_GET_CONTEXT,
        THREAD_QUERY_INFORMATION, THREAD_SUSPEND_RESUME,
    };

    pub(super) const SAMPLING: bool = true;

    /// Handle to the main thread, opened once at init.
    static MAIN_THREAD: AtomicUsize = AtomicUsize::new(0);
    static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

    /// Set while [`veh`] is running on the main thread. The watchdog skips its
    /// suspend/dump for that tick: the handler is already producing the better
    /// (self-captured, at the point of overflow) backtrace, and freezing it
    /// mid-report buys nothing.
    static IN_VEH: AtomicBool = AtomicBool::new(false);
    /// Set once [`veh`] has produced its one dump, so a second overflow inside
    /// the handler's own reporting does not recurse.
    static VEH_DUMPED: AtomicBool = AtomicBool::new(false);

    /// How many frames a single walk may collect. Runaway recursion routinely
    /// blows past any "reasonable" depth, and the interesting frames -- the
    /// call that started the recursion -- are the *outermost* ones, i.e. the
    /// last ones collected. A small cap hides exactly what we are after.
    const MAX_FRAMES: usize = 32768;
    /// `SymFromAddr` displacements beyond this mean the "name" is almost
    /// certainly just the nearest export, not the actual function.
    const NEAREST_EXPORT_DISP: u64 = 4096;

    pub(super) fn stack_bounds() -> Option<(usize, usize)> {
        let mut low = 0usize;
        let mut high = 0usize;
        unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
        if high > low { Some((low, high)) } else { None }
    }

    pub(super) fn init_main_thread() {
        unsafe {
            // Reserve stack for the exception handler, so that the vectored
            // handler below can run (and allocate) after the guard page is hit.
            let mut guarantee: u32 = 128 * 1024;
            let _ = SetThreadStackGuarantee(&mut guarantee);

            MAIN_THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);
            let access = THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME | THREAD_QUERY_INFORMATION;
            if let Ok(h) = OpenThread(access, false, GetCurrentThreadId()) {
                MAIN_THREAD.store(h.0 as usize, Ordering::SeqCst);
            } else {
                log("warning: OpenThread failed; sampling disabled");
            }

            AddVectoredExceptionHandler(1, Some(veh));
        }
        let _ = GetCurrentThread;
    }

    fn main_thread() -> Option<HANDLE> {
        let raw = MAIN_THREAD.load(Ordering::SeqCst);
        if raw == 0 {
            None
        } else {
            Some(HANDLE(raw as *mut c_void))
        }
    }

    /// `CONTEXT` must be 16-byte aligned for `GetThreadContext`.
    #[repr(C, align(16))]
    struct AlignedContext(windows::Win32::System::Diagnostics::Debug::CONTEXT);

    const CONTEXT_CONTROL_FULL: u32 = 0x0010_0007; // AMD64 CONTEXT_FULL

    fn on_main_thread() -> bool {
        unsafe { GetCurrentThreadId() == MAIN_THREAD_ID.load(Ordering::Relaxed) }
    }

    /// Runs `f` with the main thread's register context, keeping that thread
    /// suspended for the duration so a stack walk sees a consistent stack.
    ///
    /// Suspending yourself deadlocks, so calls from the main thread (the
    /// startup self-test, the exception handler) capture their own context
    /// in place instead.
    ///
    /// IMPORTANT: `f` runs with the main thread frozen, so it must not
    /// allocate, log, or otherwise take any lock the main thread might be
    /// holding -- the heap lock, the loader lock, or [`super::LOG`]. Doing so
    /// hangs the process outright. `f` is only ever [`collect_frames`], which
    /// fills caller-provided buffers and touches nothing else.
    fn with_main_context<R>(f: impl FnOnce(&mut AlignedContext) -> R) -> Option<R> {
        use windows::Win32::System::Diagnostics::Debug::{
            CONTEXT_FLAGS, GetThreadContext, RtlCaptureContext,
        };
        unsafe {
            let mut ctx = AlignedContext(std::mem::zeroed());
            ctx.0.ContextFlags = CONTEXT_FLAGS(CONTEXT_CONTROL_FULL);

            if on_main_thread() {
                RtlCaptureContext(&mut ctx.0);
                return Some(f(&mut ctx));
            }

            // The handler is mid-report on a stack we would only be freezing;
            // let it finish and sample again next tick.
            if IN_VEH.load(Ordering::Acquire) {
                return None;
            }

            let h = main_thread()?;
            if SuspendThread(h) == u32::MAX {
                return None;
            }
            let ok = GetThreadContext(h, &mut ctx.0).is_ok();
            let result = if ok { Some(f(&mut ctx)) } else { None };
            ResumeThread(h);
            result
        }
    }

    pub(super) fn sample_main_stack() -> Option<usize> {
        // Only reads a register: no allocation while the thread is suspended.
        let sp = with_main_context(|ctx| ctx.0.Rsp as usize)?;
        let high = STACK_HIGH.load(Ordering::Relaxed);
        if sp == 0 || sp > high {
            None
        } else {
            Some(high - sp)
        }
    }

    pub(super) fn dump_main_thread_backtrace() {
        // Everything that allocates or takes a lock happens outside the
        // suspend window: symbol init and the frame buffers up front,
        // symbolization and logging after the thread is running again. Only
        // `collect_frames` sees the frozen thread. See `with_main_context`.
        ensure_symbols();
        log_new_modules();
        let mut pcs = vec![0u64; MAX_FRAMES];
        let mut sps = vec![0u64; MAX_FRAMES];
        match with_main_context(|ctx| collect_frames(ctx, &mut pcs, &mut sps)) {
            Some(n) => report_frames(&pcs[..n], &sps[..n]),
            None => log("  (could not capture the main thread context)"),
        }
    }

    /// Collects a backtrace from `ctx` and reports it, all on the current
    /// thread. Used by the exception handler, which already owns a context and
    /// must not suspend itself.
    fn dump_own_context(ctx: &mut AlignedContext) {
        ensure_symbols();
        let mut pcs = vec![0u64; MAX_FRAMES];
        let mut sps = vec![0u64; MAX_FRAMES];
        let n = collect_frames(ctx, &mut pcs, &mut sps);
        report_frames(&pcs[..n], &sps[..n]);
    }

    /// Logs one backtrace at startup so it is immediately obvious whether
    /// symbolization works at all, rather than finding out during a crash.
    pub(super) fn self_test() {
        log_new_modules();
        log("self-test: backtrace of the main thread at init --");
        dump_main_thread_backtrace();
    }

    /// Logs every module loaded since the last call, as `base+size name`.
    ///
    /// A bare address in a report is only actionable against a module list:
    /// ASLR moves every base, so the mapping has to come from the same run.
    /// Modules keep loading well after startup (wgpu, the D3D and DirectWrite
    /// stacks), hence "since the last call" rather than a one-shot dump.
    fn log_new_modules() {
        use windows::Win32::System::Diagnostics::Debug::EnumerateLoadedModules64;

        static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
        static PENDING: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

        unsafe extern "system" fn cb(
            name: windows::core::PCSTR,
            base: u64,
            size: u32,
            _ctx: *const c_void,
        ) -> windows::Win32::Foundation::BOOL {
            let mut seen = match SEEN.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if !seen.contains(&base) {
                seen.push(base);
                let name = unsafe { name.to_string() }.unwrap_or_default();
                let line = format!("  {base:#018x}..{:#018x}  {name}", base + size as u64);
                if let Ok(mut pending) = PENDING.lock() {
                    pending.push(line);
                }
            }
            true.into()
        }

        // The callback only collects: `log` takes a lock dbghelp has no reason
        // to expect us to hold inside its enumeration.
        unsafe {
            let _ = EnumerateLoadedModules64(GetCurrentProcess(), Some(cb), None);
        }
        let lines = match PENDING.lock() {
            Ok(mut g) => std::mem::take(&mut *g),
            Err(p) => std::mem::take(&mut *p.into_inner()),
        };
        if lines.is_empty() {
            return;
        }
        log(&format!("modules loaded ({}):", lines.len()));
        for line in lines {
            log(&line);
        }
    }

    /// Initializes dbghelp. Called before any walk, never while the main
    /// thread is suspended: `SymInitialize` takes the loader lock.
    ///
    /// `SYMOPT_DEFERRED_LOADS` is deliberately *not* set, so every module's
    /// symbols are loaded here rather than lazily from inside a walk -- a lazy
    /// load would take the loader lock with the main thread frozen.
    fn ensure_symbols() {
        use windows::Win32::System::Diagnostics::Debug::{
            SYMOPT_LOAD_LINES, SYMOPT_UNDNAME, SymInitialize, SymSetOptions,
        };
        static SYM_READY: std::sync::Once = std::sync::Once::new();
        SYM_READY.call_once(|| unsafe {
            SymSetOptions(SYMOPT_LOAD_LINES | SYMOPT_UNDNAME);
            let _ = SymInitialize(GetCurrentProcess(), None, true);
        });
    }

    /// Walks the stack described by `ctx`, filling `pcs` with return addresses
    /// and `sps` with the matching stack pointers. Returns the frame count.
    ///
    /// Runs with the main thread suspended, so it does nothing but call
    /// `StackWalkEx` into caller-provided buffers: no allocation, no logging,
    /// no symbolization. See `with_main_context`.
    fn collect_frames(ctx: &mut AlignedContext, pcs: &mut [u64], sps: &mut [u64]) -> usize {
        use windows::Win32::System::Diagnostics::Debug::{
            ADDRESS_MODE, STACKFRAME_EX, StackWalkEx,
        };

        let Some(hthread) = main_thread() else {
            return 0;
        };
        let cap = pcs.len().min(sps.len());
        unsafe {
            let hproc = GetCurrentProcess();
            let mut frame = STACKFRAME_EX::default();
            frame.StackFrameSize = std::mem::size_of::<STACKFRAME_EX>() as u32;
            frame.AddrPC.Offset = ctx.0.Rip;
            frame.AddrPC.Mode = ADDRESS_MODE(3); // AddrModeFlat
            frame.AddrFrame.Offset = ctx.0.Rbp;
            frame.AddrFrame.Mode = ADDRESS_MODE(3);
            frame.AddrStack.Offset = ctx.0.Rsp;
            frame.AddrStack.Mode = ADDRESS_MODE(3);

            let mut depth = 0usize;
            while depth < cap {
                let ok = StackWalkEx(
                    0x8664, // IMAGE_FILE_MACHINE_AMD64
                    hproc,
                    hthread,
                    &mut frame,
                    &mut ctx.0 as *mut _ as *mut c_void,
                    None,
                    Some(SymFunctionTableAccess64),
                    Some(SymGetModuleBase64),
                    None,
                    0,
                );
                if !ok.as_bool() || frame.AddrPC.Offset == 0 {
                    break;
                }
                pcs[depth] = frame.AddrPC.Offset;
                sps[depth] = frame.AddrStack.Offset;
                depth += 1;
            }
            depth
        }
    }

    fn report_frames(pcs: &[u64], sps: &[u64]) {
        if pcs.is_empty() {
            log("  (StackWalkEx produced no frames)");
            return;
        }

        let entries = collapse(pcs);
        log(&format!(
            "  {} frames, {} entries after folding repeats",
            pcs.len(),
            entries.len(),
        ));

        // A frame's size is the distance to the next frame's stack pointer;
        // the outermost frame has nothing to measure against.
        let bytes = |from: usize, to: usize| -> Option<usize> {
            if to < sps.len() {
                sps[to].checked_sub(sps[from]).map(|d| d as usize)
            } else {
                None
            }
        };

        let show = |e: &Entry| {
            let first = e.depth - 1;
            if e.reps == 1 && e.period == 1 {
                let size = bytes(first, first + 1)
                    .map(|b| format!("  [{}]", human(b)))
                    .unwrap_or_default();
                log(&format!("  #{:<6} {}{}", e.depth, describe(pcs[first]), size));
                return;
            }
            let each = bytes(first, first + e.period);
            let total = bytes(first, first + e.frames());
            log(&format!(
                "  #{:<6} cycle of {} frame(s) x{}{}{}:",
                e.depth,
                e.period,
                e.reps,
                each.map(|b| format!(", {} each", human(b)))
                    .unwrap_or_default(),
                total
                    .map(|b| format!(", {} total", human(b)))
                    .unwrap_or_default(),
            ));
            for off in 0..e.period {
                log(&format!("           | {}", describe(pcs[first + off])));
            }
        };

        // The recursion's entry point is at the *outermost* end, so both ends
        // matter; elide the middle rather than truncating the tail.
        if entries.len() <= HEAD_ENTRIES + TAIL_ENTRIES {
            for e in &entries {
                show(e);
            }
        } else {
            for e in &entries[..HEAD_ENTRIES] {
                show(e);
            }
            log(&format!(
                "       ... {} entries elided ...",
                entries.len() - HEAD_ENTRIES - TAIL_ENTRIES,
            ));
            for e in &entries[entries.len() - TAIL_ENTRIES..] {
                show(e);
            }
        }
    }

    /// Renders one address as `module!symbol+0xdisp (file:line)`.
    ///
    /// Without a PDB, dbghelp falls back to the module's export table and
    /// happily returns the nearest export -- which for ntdll can be a wholly
    /// unrelated function tens of kilobytes away. Printing the displacement
    /// and flagging export-only modules keeps such names from being read as
    /// fact.
    fn describe(addr: u64) -> String {
        use windows::Win32::System::Diagnostics::Debug::{
            IMAGEHLP_LINE64, IMAGEHLP_MODULE64, SYMBOL_INFO, SymExport, SymFromAddr,
            SymGetLineFromAddr64, SymGetModuleInfo64,
        };
        unsafe {
            let hproc = GetCurrentProcess();

            let mut module = IMAGEHLP_MODULE64 {
                SizeOfStruct: std::mem::size_of::<IMAGEHLP_MODULE64>() as u32,
                ..Default::default()
            };
            let (module_name, export_only, base) =
                if SymGetModuleInfo64(hproc, addr, &mut module).is_ok() {
                    let name = std::ffi::CStr::from_ptr(module.ModuleName.as_ptr())
                        .to_string_lossy()
                        .into_owned();
                    (name, module.SymType == SymExport, module.BaseOfImage)
                } else {
                    (String::new(), false, 0)
                };

            let mut buf = [0u8; std::mem::size_of::<SYMBOL_INFO>() + 512];
            let sym = buf.as_mut_ptr() as *mut SYMBOL_INFO;
            (*sym).SizeOfStruct = std::mem::size_of::<SYMBOL_INFO>() as u32;
            (*sym).MaxNameLen = 511;
            let mut disp = 0u64;
            let name = if SymFromAddr(hproc, addr, Some(&mut disp), sym).is_ok() {
                let n = (*sym).NameLen as usize;
                let p = std::ptr::addr_of!((*sym).Name) as *const u8;
                let name = String::from_utf8_lossy(std::slice::from_raw_parts(p, n)).into_owned();
                if disp == 0 {
                    name
                } else {
                    format!("{name}+{disp:#x}")
                }
            } else {
                String::new()
            };

            let mut line_disp = 0u32;
            let mut line = IMAGEHLP_LINE64 {
                SizeOfStruct: std::mem::size_of::<IMAGEHLP_LINE64>() as u32,
                ..Default::default()
            };
            let where_ = if SymGetLineFromAddr64(hproc, addr, &mut line_disp, &mut line).is_ok() {
                let file = std::ffi::CStr::from_ptr(line.FileName.0 as *const i8)
                    .to_string_lossy()
                    .into_owned();
                format!(" ({}:{})", file, line.LineNumber)
            } else {
                String::new()
            };

            let named = !name.is_empty();
            let mut text = match (module_name.is_empty(), name.is_empty()) {
                // No symbol at all. The *module* is still known from the load
                // address alone, and `module+RVA` is what a later
                // `llvm-symbolizer`/`windbg` run needs -- printing the raw
                // address instead throws that away, which is exactly what left
                // the one interesting frame of the 2026-07-29 report unnamed.
                (true, true) => format!("{addr:#018x}"),
                (false, true) => format!("{module_name}+{:#x} ({addr:#018x})", addr - base),
                (true, false) => name,
                (false, false) => format!("{module_name}!{name}"),
            };
            text.push_str(&where_);
            if where_.is_empty() && named && (export_only || disp > NEAREST_EXPORT_DISP) {
                text.push_str(&format!(
                    "  <- no symbols; nearest export, name unreliable (rva {:#x})",
                    addr.saturating_sub(base),
                ));
            }
            text
        }
    }

    // ------------------------------------------------- first-chance exceptions

    /// One distinct `(code, faulting address)` pair seen by [`veh`].
    ///
    /// All-atomic and fixed-size on purpose: [`veh`] runs at the point of the
    /// fault, on any thread, possibly while that thread holds the heap lock or
    /// the log lock -- so recording must not allocate, log, or block. The
    /// watchdog turns these into log lines later, from a thread that is free
    /// to do both.
    struct ExcSlot {
        /// Written last, with `Release`: nonzero means the slot is readable.
        count: AtomicU64,
        code: AtomicU32,
        thread: AtomicU32,
        addr: AtomicU64,
        /// `ExceptionInformation[0..3]`: for an access violation, the access
        /// type and the address that could not be accessed; for an in-page
        /// error, those two plus the `NTSTATUS` the paging I/O failed with --
        /// which is the whole answer to *why* a page could not be read, and is
        /// unrecoverable from the address alone.
        info0: AtomicU64,
        info1: AtomicU64,
        info2: AtomicU64,
        /// Phase at the *first* sighting, as a `'static` str ptr/len.
        phase: AtomicUsize,
        phase_len: AtomicUsize,
        /// Count as of the last line the watchdog logged for this slot.
        reported: AtomicU64,
    }

    /// Distinct exceptions tracked. A storm repeats one pair, so this only has
    /// to be wide enough for the ordinary background noise plus the culprit.
    const MAX_EXC: usize = 32;

    static EXC: [ExcSlot; MAX_EXC] = [const { ExcSlot::new() }; MAX_EXC];
    static EXC_USED: AtomicUsize = AtomicUsize::new(0);
    static EXC_DROPPED: AtomicU64 = AtomicU64::new(0);

    impl ExcSlot {
        const fn new() -> Self {
            Self {
                count: AtomicU64::new(0),
                code: AtomicU32::new(0),
                thread: AtomicU32::new(0),
                addr: AtomicU64::new(0),
                info0: AtomicU64::new(0),
                info1: AtomicU64::new(0),
                info2: AtomicU64::new(0),
                phase: AtomicUsize::new(0),
                phase_len: AtomicUsize::new(0),
                reported: AtomicU64::new(0),
            }
        }
    }

    /// Records one exception. Allocation- and lock-free; see [`ExcSlot`].
    ///
    /// Racing threads can end up with two slots for one pair. That is fine --
    /// a duplicate line is far cheaper than the synchronization needed to
    /// avoid it at a fault point.
    unsafe fn record_exception(rec: *const EXCEPTION_RECORD) {
        unsafe {
            let code = (*rec).ExceptionCode.0 as u32;
            let addr = (*rec).ExceptionAddress as u64;
            let used = EXC_USED.load(Ordering::Acquire).min(MAX_EXC);
            for slot in &EXC[..used] {
                if slot.count.load(Ordering::Acquire) != 0
                    && slot.code.load(Ordering::Relaxed) == code
                    && slot.addr.load(Ordering::Relaxed) == addr
                {
                    slot.count.fetch_add(1, Ordering::AcqRel);
                    return;
                }
            }

            let idx = EXC_USED.fetch_add(1, Ordering::AcqRel);
            if idx >= MAX_EXC {
                EXC_USED.store(MAX_EXC, Ordering::Release);
                EXC_DROPPED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let slot = &EXC[idx];
            slot.code.store(code, Ordering::Relaxed);
            slot.addr.store(addr, Ordering::Relaxed);
            slot.thread.store(GetCurrentThreadId(), Ordering::Relaxed);
            let n = (*rec).NumberParameters as usize;
            let params = &(*rec).ExceptionInformation;
            slot.info0
                .store(if n > 0 { params[0] as u64 } else { 0 }, Ordering::Relaxed);
            slot.info1
                .store(if n > 1 { params[1] as u64 } else { 0 }, Ordering::Relaxed);
            slot.info2
                .store(if n > 2 { params[2] as u64 } else { 0 }, Ordering::Relaxed);
            let phase = current_phase();
            slot.phase.store(phase.as_ptr() as usize, Ordering::Relaxed);
            slot.phase_len.store(phase.len(), Ordering::Relaxed);
            // Release: publishes every field above to the readers' `Acquire`.
            slot.count.store(1, Ordering::Release);
        }
    }

    /// Logs every exception seen since the last call, with its running count.
    ///
    /// Called from the watchdog (and from the overflow handler, which is
    /// already logging anyway), never with the main thread suspended: it
    /// symbolizes and logs.
    pub(super) fn report_exceptions() {
        let used = EXC_USED.load(Ordering::Acquire).min(MAX_EXC);
        for slot in &EXC[..used] {
            let count = slot.count.load(Ordering::Acquire);
            if count == 0 {
                continue;
            }
            let reported = slot.reported.swap(count, Ordering::AcqRel);
            if reported == count {
                continue;
            }
            let code = slot.code.load(Ordering::Relaxed);
            let addr = slot.addr.load(Ordering::Relaxed);
            let phase = {
                let p = slot.phase.load(Ordering::Relaxed) as *const u8;
                let len = slot.phase_len.load(Ordering::Relaxed);
                if p.is_null() || len == 0 {
                    "-"
                } else {
                    // Safety: set from `current_phase`, which yields `'static`.
                    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, len)) }
                }
            };
            log(&format!(
                "exception {:#010x} ({}) x{}{} at {} thread {} phase={}{}",
                code,
                exception_name(code),
                count,
                if reported == 0 {
                    String::new()
                } else {
                    format!(" (+{})", count - reported)
                },
                describe(addr),
                slot.thread.load(Ordering::Relaxed),
                phase,
                fault_detail(code, slot),
            ));
        }
        let dropped = EXC_DROPPED.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            log(&format!(
                "  ({dropped} exception(s) not recorded: more than {MAX_EXC} distinct sites)"
            ));
        }
    }

    /// The handful of codes worth naming; anything else prints as bare hex.
    fn exception_name(code: u32) -> &'static str {
        match code {
            0xC000_0005 => "access violation",
            0xC000_0006 => "in-page error",
            0xC000_001D => "illegal instruction",
            0xC000_008C => "array bounds exceeded",
            0xC000_008E => "float divide by zero",
            0xC000_0094 => "integer divide by zero",
            0xC000_00FD => "stack overflow",
            0xC000_0409 => "security check failure",
            0xC000_0374 => "heap corruption",
            0x4000_001E | 0x4000_001F => "debug print",
            0x4064_2472 => "thread name",
            0x8000_0003 => "breakpoint",
            0xE06D_7363 => "C++ exception",
            0xE24C_4A02 => "Rust panic",
            _ => "?",
        }
    }

    /// The `[read of 0x...]` tail of an access-violation / in-page-error line.
    ///
    /// The two codes carry the same first two parameters; an in-page error adds
    /// the `NTSTATUS` the paging read failed with, and *that* is the diagnosis.
    /// An in-page error on a code address means Windows could not fault in a
    /// page of an image that is already mapped, which is a property of where
    /// the file lives (a share that hiccuped, a volume that went away, a file
    /// rewritten under the running mapping), not of the code that touched it.
    fn fault_detail(code: u32, slot: &ExcSlot) -> String {
        if code != 0xC000_0005 && code != 0xC000_0006 {
            return String::new();
        }
        let kind = match slot.info0.load(Ordering::Relaxed) {
            0 => "read",
            1 => "write",
            8 => "execute (DEP)",
            _ => "?",
        };
        let addr = slot.info1.load(Ordering::Relaxed);
        if code == 0xC000_0005 {
            return format!("  [{kind} of {addr:#018x}]");
        }
        let status = slot.info2.load(Ordering::Relaxed) as u32;
        format!(
            "  [{kind} of {addr:#018x} failed with {:#010x} ({})]",
            status,
            ntstatus_name(status),
        )
    }

    /// The paging-failure statuses worth naming, chosen for what they say about
    /// the *storage* the image came from.
    fn ntstatus_name(status: u32) -> &'static str {
        match status {
            0xC000_0009 => "bad initial PTE",
            0xC000_0011 => "end of file (image truncated or rewritten)",
            0xC000_0022 => "access denied",
            0xC000_0056 => "delete pending",
            0xC000_009C => "device data error",
            0xC000_00C4 => "unexpected network error",
            0xC000_00C9 => "network name deleted",
            0xC000_0185 => "I/O device error",
            0xC000_01E5 => "file invalid (backing file changed)",
            0xC000_026E => "volume dismounted",
            _ => "?",
        }
    }

    unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
        const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
        unsafe {
            if !ENABLED.load(Ordering::Relaxed) || info.is_null() {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            let rec = (*info).ExceptionRecord;
            if rec.is_null() {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            // Every first-chance exception is recorded, not just the fatal
            // one: an overflow whose stack is *all* exception-dispatch frames
            // is caused by whatever keeps raising, and that raise is invisible
            // by the time the stack is walked.
            record_exception(rec);
            if (*rec).ExceptionCode != EXCEPTION_STACK_OVERFLOW {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            if GetCurrentThreadId() != MAIN_THREAD_ID.load(Ordering::Relaxed) {
                log("!!! STACK OVERFLOW on a non-main thread");
                return EXCEPTION_CONTINUE_SEARCH;
            }
            // Reporting is not free of stack: symbolization runs inside the
            // 128 KiB `SetThreadStackGuarantee` reserve, and overflowing that
            // re-enters this handler. Report once, then get out of the way and
            // let the process die normally.
            if VEH_DUMPED.swap(true, Ordering::SeqCst) {
                return EXCEPTION_CONTINUE_SEARCH;
            }
            // Tells the watchdog not to suspend us mid-report.
            IN_VEH.store(true, Ordering::Release);

            log("!!! STACK OVERFLOW on the main thread -- backtrace follows");
            // Flush here too: the watchdog may never get another tick, and the
            // exceptions recorded so far are the best evidence of *why* the
            // stack grew.
            report_exceptions();
            let ctxp = (*info).ContextRecord;
            if !ctxp.is_null() {
                let mut ctx = AlignedContext(*ctxp);
                dump_own_context(&mut ctx);
            }

            IN_VEH.store(false, Ordering::Release);
            EXCEPTION_CONTINUE_SEARCH
        }
    }
}

// -------------------------------------------------------- non-windows backend

#[cfg(not(target_os = "windows"))]
mod platform {
    pub(super) fn stack_bounds() -> Option<(usize, usize)> {
        #[cfg(target_os = "macos")]
        unsafe {
            let t = libc::pthread_self();
            let high = libc::pthread_get_stackaddr_np(t) as usize;
            let size = libc::pthread_get_stacksize_np(t);
            if high > size {
                return Some((high - size, high));
            }
        }
        None
    }

    pub(super) fn init_main_thread() {}

    pub(super) fn self_test() {}

    /// No exception handler here, so there is never anything to report.
    pub(super) fn report_exceptions() {}

    pub(super) const SAMPLING: bool = false;

    /// No cross-thread sampling here; [`super::probe`] feeds the peak instead.
    pub(super) fn sample_main_stack() -> Option<usize> {
        None
    }

    pub(super) fn dump_main_thread_backtrace() {
        super::log("  (remote backtrace is only implemented on Windows)");
    }
}

#[cfg(test)]
mod tests {
    use super::{HEAD_ENTRIES, MAX_CYCLE, TAIL_ENTRIES, collapse};

    /// `(depth, period, reps)` for each folded entry.
    fn fold(pcs: &[u64]) -> Vec<(usize, usize, usize)> {
        collapse(pcs)
            .iter()
            .map(|e| (e.depth, e.period, e.reps))
            .collect()
    }

    #[test]
    fn distinct_frames_are_not_folded() {
        assert_eq!(fold(&[1, 2, 3]), [(1, 1, 1), (2, 1, 1), (3, 1, 1)]);
    }

    #[test]
    fn empty_stack_folds_to_nothing() {
        assert!(fold(&[]).is_empty());
    }

    #[test]
    fn identical_adjacent_frames_fold() {
        assert_eq!(fold(&[7, 7, 7, 7]), [(1, 1, 4)]);
    }

    /// The case the old adjacent-only folding missed entirely: the runaway
    /// cycles through several *distinct* frames, as an exception-dispatch loop
    /// or any mutual recursion does.
    #[test]
    fn multi_frame_cycle_folds() {
        let mut pcs = vec![10, 11];
        for _ in 0..100 {
            pcs.extend([20, 21, 22]);
        }
        pcs.extend([30, 31]);
        assert_eq!(
            fold(&pcs),
            [
                (1, 1, 1),
                (2, 1, 1),
                (3, 3, 100),
                (303, 1, 1),
                (304, 1, 1),
            ]
        );
    }

    /// A cycle whose period exceeds the search window stays unfolded rather
    /// than being mis-folded at some shorter period.
    #[test]
    fn cycle_longer_than_the_window_is_left_alone() {
        let period: Vec<u64> = (0..MAX_CYCLE as u64 + 1).collect();
        let pcs: Vec<u64> = period.iter().chain(period.iter()).copied().collect();
        assert!(fold(&pcs).iter().all(|&(_, p, r)| p == 1 && r == 1));
    }

    /// Ties on covered frames go to the shorter period: `[5, 5, 5, 5]` is one
    /// frame four times, not two frames twice.
    #[test]
    fn shortest_period_wins_ties() {
        assert_eq!(fold(&[5, 5, 5, 5]), [(1, 1, 4)]);
    }

    /// Depths are 1-based frame numbers, so they must keep counting the frames
    /// a fold swallowed -- otherwise the elided report mislabels every frame
    /// after the first cycle.
    #[test]
    fn depths_count_folded_frames() {
        let entries = collapse(&[1, 2, 2, 2, 3]);
        assert_eq!(fold(&[1, 2, 2, 2, 3]), [(1, 1, 1), (2, 1, 3), (5, 1, 1)]);
        assert_eq!(entries.iter().map(|e| e.frames()).sum::<usize>(), 5);
    }

    /// The whole point of folding: a stack too deep to print raw becomes short
    /// enough that both ends -- including the frames that started the
    /// recursion -- survive without eliding anything.
    #[test]
    fn runaway_recursion_fits_in_the_report() {
        let mut pcs: Vec<u64> = (0..20).collect();
        for _ in 0..5000 {
            pcs.extend([90, 91, 92, 93]);
        }
        pcs.extend(100..130);
        assert!(collapse(&pcs).len() <= HEAD_ENTRIES + TAIL_ENTRIES);
    }
}
