//! Running one pure closure over an index range on every core.
//!
//! The build pipeline has three stages shaped exactly alike — trace this wave of
//! composites, lay out that wave, flatten those grids — and each of them is a
//! pure function of a shared, immutable input. What they need is not a thread
//! pool but the one loop that hands such a range out; this is that loop, so the
//! three do not each grow their own.
//!
//! # Why the work is stolen rather than sliced
//!
//! A wave holds everything from a two-ref accent to a fully nested ideograph,
//! and the ratio between them is a couple of orders of magnitude. A static
//! split leaves every thread but one idle behind whoever drew the slowest slice,
//! so items are handed out one at a time from a shared counter instead.
//!
//! Cancellation is read per item, not per stride: an item here is a whole glyph
//! composed, which dwarfs a relaxed load, and a stride would let *every* thread
//! run that many past a cancel.

use crate::cancel::CancelToken;

/// Below this many items a thread costs more than it saves — spawning and
/// joining is tens of microseconds, and the rounds after the first are usually
/// a handful of glyphs.
const MIN_PARALLEL: usize = 32;

/// `f(i)` for every `i` in `0..count`, in parallel, as a vector indexed the same
/// way. `None` where the run was cancelled before that index was reached.
pub(crate) fn map_indexed<R: Send>(
    count: usize,
    cancel: &CancelToken,
    f: impl Fn(usize) -> R + Sync,
) -> Vec<Option<R>> {
    let mut out: Vec<Option<R>> = (0..count).map(|_| None).collect();
    if count == 0 {
        return out;
    }

    let threads = if count < MIN_PARALLEL {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(count)
    };
    if threads <= 1 {
        for (i, slot) in out.iter_mut().enumerate() {
            if cancel.is_cancelled() {
                break;
            }
            *slot = Some(f(i));
        }
        return out;
    }

    let next = std::sync::atomic::AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, R)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                let (next, f) = (&next, &f);
                scope.spawn(move || {
                    let mut done: Vec<(usize, R)> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= count || cancel.is_cancelled() {
                            break;
                        }
                        done.push((i, f(i)));
                    }
                    done
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (i, value) in parts.into_iter().flatten() {
        out[i] = Some(value);
    }
    out
}
