//! The identity a buffer line keeps while the lines around it move.
//!
//! Anything reported against a document names its line by index, and an index
//! is only good for the buffer it was computed from. The build runs on a
//! snapshot and reports seconds later, by which time the reader may have
//! inserted or deleted lines above the one it is about; the editor then has to
//! say where that line went, or whether it is gone.
//!
//! It asks the line. Every [`DocLine`](super::DocLine) carries a [`LineId`]
//! inside its value, so whatever moves the value moves the identity with it:
//! a splice, a `Vec::insert`, the undo stack's clones going back in. No edit
//! has to report what it did, which is the point — an edit path that forgot to
//! would have shifted every finding below it, where a line that loses its id
//! only loses its own.
//!
//! What the identity deliberately does *not* survive is a line being built
//! anew from text. Rewriting a line in place keeps it (the value is mutated
//! through [`Tracked`]'s `DerefMut`); an edit that replaces lines with fresh
//! ones decides for itself which of them are the old ones continued, through
//! [`DocLine::inherit_ids`](super::DocLine::inherit_ids).
//!
//! # Allocation
//!
//! Ids are unique for the process, not per document, so a line pasted from one
//! buffer into another can never collide with a line already there. Documents
//! are parsed on every core at startup and on each rebuild, and a shared
//! counter bumped per line would bounce one cache line between all of them —
//! so each thread takes a block of ids at a time and hands them out locally.
//!
//! Only the editor reads an id. The headless build keeps the wrapper, so that
//! `DocLine` has one shape, but not the field.

use std::ops::{Deref, DerefMut};

#[cfg(feature = "editor")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LineId(u64);

#[cfg(feature = "editor")]
impl LineId {
    fn fresh() -> Self {
        use std::cell::Cell;
        use std::sync::atomic::{AtomicU64, Ordering};

        const BLOCK: u64 = 4096;
        static NEXT_BLOCK: AtomicU64 = AtomicU64::new(0);
        thread_local! {
            /// `(next, end)` of this thread's current block.
            static LOCAL: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
        }
        LOCAL.with(|local| {
            let (mut next, mut end) = local.get();
            if next == end {
                next = NEXT_BLOCK.fetch_add(BLOCK, Ordering::Relaxed);
                end = next + BLOCK;
            }
            local.set((next + 1, end));
            LineId(next)
        })
    }
}

/// A line's value together with its [`LineId`]. Reads and writes go through
/// to the value; equality and `Debug` see only the value, so two buffers with
/// the same text compare equal whatever their history.
#[derive(Clone)]
pub struct Tracked<T> {
    #[cfg(feature = "editor")]
    id: LineId,
    value: T,
}

impl<T> Tracked<T> {
    /// A value that has not been a line before, under a fresh id.
    pub fn new(value: T) -> Self {
        Self {
            #[cfg(feature = "editor")]
            id: LineId::fresh(),
            value,
        }
    }
}

#[cfg(feature = "editor")]
impl<T> Tracked<T> {
    pub fn line_id(&self) -> LineId {
        self.id
    }

    pub(super) fn set_line_id(&mut self, id: LineId) {
        self.id = id;
    }
}

impl<T> Deref for Tracked<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Tracked<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: PartialEq> PartialEq for Tracked<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

/// A text line against plain text, which is most of what a test asks of one.
impl PartialEq<str> for Tracked<String> {
    fn eq(&self, other: &str) -> bool {
        self.value == other
    }
}

impl PartialEq<&str> for Tracked<String> {
    fn eq(&self, other: &&str) -> bool {
        self.value == *other
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Tracked<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

impl<T: std::fmt::Display> std::fmt::Display for Tracked<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

#[cfg(all(test, feature = "editor"))]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_across_threads() {
        let ids: Vec<LineId> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| s.spawn(|| (0..5000).map(|_| LineId::fresh()).collect::<Vec<_>>()))
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn a_clone_is_the_same_line_and_equality_ignores_the_id() {
        let a = Tracked::new("x".to_string());
        let b = Tracked::new("x".to_string());
        assert_eq!(a.clone().line_id(), a.line_id());
        assert_ne!(a.line_id(), b.line_id());
        assert_eq!(a, b);
    }
}
