//! Running one pure closure over an index range on every core.
//!
//! The build pipeline has several stages shaped exactly alike — trace this wave
//! of composites, lay out that wave, flatten those grids, settle which
//! alternative every character of a wide `map` line takes — and each of them is
//! a pure function of a shared, immutable input. What they need is not a thread
//! pool but the one loop that hands such a range out; this is that loop, so
//! they do not each grow their own.
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

/// Locks a memo shared between the stages above, whether or not an earlier
/// holder panicked.
///
/// Every such mutex guards a cache keyed by content: an entry is either there
/// and right or absent, and a holder that panicked mid-insert left at worst an
/// entry missing. Poisoning would instead turn one panicked build — which the
/// editor survives, see `ResultSlot` — into every later build panicking on the
/// lock, until a restart.
pub(crate) fn lock_memo<T: ?Sized>(memo: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    memo.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_memo_outlives_a_panic_while_it_was_held() {
        let memo = std::sync::Mutex::new(vec![1]);
        let _ = std::thread::scope(|s| {
            s.spawn(|| {
                let _held = memo.lock().unwrap();
                panic!("a build that failed mid-stage");
            })
            .join()
        });
        assert!(memo.is_poisoned());
        lock_memo(&memo).push(2);
        assert_eq!(*lock_memo(&memo), vec![1, 2]);
    }
}
