//! [`CancelToken`]: the one way a background stage is told its result is no
//! longer wanted.
//!
//! The editor rebuilds the font and re-resolves the sources off the UI thread,
//! and an edit arriving mid-build makes whatever is running obsolete: nobody
//! will ever read its result, because every consumer keys on the generation the
//! result carries and that generation is already stale. Clicking pixels
//! produces a burst of such edits, and without a way to say "stop", each build
//! still ran to completion while the next waited behind it on the shared
//! contour cache — the last edit's font then appeared several full builds later.
//!
//! A token is a single flag, so a check costs one relaxed atomic load and can
//! sit inside the per-glyph loops that dominate a build. What a cancelled stage
//! returns is deliberately *nothing usable*: the collectors that already had a
//! "this input produces no font" case return `None` through it, and the ones
//! that do not — [`crate::ref_composite::resolve_expansion`] — return however
//! far they got, which is a partly-resolved font, i.e. exactly what a source
//! with unresolvable composites produces anyway. Neither is ever read: the only
//! caller that cancels is the one that stopped wanting the result, and it drops
//! it whole. The distinction between "cancelled" and "failed" is drawn one
//! level up, where the worker asks its own token which of the two happened;
//! only the app cares, and only so a cancellation does not surface as an error.
//!
//! Cancellation is one-way: a token is never un-cancelled. The scheduler
//! *replaces* the token when it starts the next stage
//! ([`crate::app::background`]), so a stage cannot inherit the previous one's
//! cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A one-way "stop, nobody wants this" flag shared with a background stage.
///
/// [`CancelToken::never`] is a token that cannot be cancelled and allocates
/// nothing; every caller outside the editor's background pipeline — the `build`
/// and `test` subcommands, the tests — passes that one, so the checks compile
/// down to a null test on a path where there is no one to do the cancelling.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Option<Arc<AtomicBool>>);

impl CancelToken {
    /// A fresh token, not yet cancelled.
    pub fn new() -> Self {
        Self(Some(Arc::new(AtomicBool::new(false))))
    }

    /// A token nothing can cancel.
    pub fn never() -> Self {
        Self(None)
    }

    /// Ask every holder of this token to stop as soon as it notices.
    pub fn cancel(&self) {
        if let Some(flag) = &self.0 {
            flag.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        match &self.0 {
            Some(flag) => flag.load(Ordering::Relaxed),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cancelling one handle cancels every clone: the worker holds a clone, and
    /// the scheduler that flips the flag holds the original.
    #[test]
    fn cancelling_a_clone_cancels_the_original() {
        let token = CancelToken::new();
        let worker = token.clone();
        assert!(!worker.is_cancelled());
        token.cancel();
        assert!(worker.is_cancelled());
    }

    /// A `never` token stays uncancelled even when something tries: the
    /// non-editor build paths hand one to the same collectors the editor does.
    #[test]
    fn a_never_token_cannot_be_cancelled() {
        let token = CancelToken::never();
        token.cancel();
        assert!(!token.is_cancelled());
    }
}
