//! Navigation history — the reverse of Ctrl/Cmd+click "go to symbol".
//!
//! The stack behaves exactly like the undo stack: entries accumulate, going
//! back rewinds a position without discarding anything, and pushing a new entry
//! while rewound replaces everything after the current position.
//!
//! What is unlike an undo stack is that each entry stores **two** positions.
//! Following a link is asymmetric: the jump starts at the link and ends at its
//! target, so going back has to reach the link while going forward again has to
//! reach the target. One position per entry would make one of the two
//! directions land a step off.
//!
//! ```text
//!   entries:  [ A ] [ B ] [ C ]
//!                          ^ pos == 3
//!   back  -> pos 2, caret at C.from (the link that led into C's target)
//!   back  -> pos 1, caret at B.from
//!   fwd   -> caret at B.to (the target that link led to), pos 2
//! ```
//!
//! Positions are plain `(document, line, column)` triples and are **not**
//! rewritten when the document is edited underneath them; see
//! [`NavHistory`]'s note. Documents are identified by their index into
//! `UniformApp::open_documents`, which is only ever appended to — no index it
//! hands out can later mean a different file. Opening another folder does clear
//! that list, and clears this history with it.

/// A remembered caret position in one open document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NavLoc {
    /// Index into `UniformApp::open_documents`.
    pub doc_idx: usize,
    pub line: usize,
    pub col: usize,
}

impl NavLoc {
    pub(super) fn new(doc_idx: usize, line: usize, col: usize) -> Self {
        Self { doc_idx, line, col }
    }
}

/// One followed link: where it was written, and where it led.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NavEntry {
    /// The link itself — where "go back" returns to.
    pub from: NavLoc,
    /// The link's target — where "go forward" returns to.
    pub to: NavLoc,
}

/// The go-back/go-forward stack.
///
/// Note: an edit that inserts or deletes lines does *not* shift the positions
/// already recorded here, so a jump remembered before such an edit can come
/// back a few lines off. Navigation clamps to the document, so the position is
/// always valid, just possibly stale.
#[derive(Default)]
pub(super) struct NavHistory {
    entries: Vec<NavEntry>,
    /// Number of entries not yet rewound; `entries[..pos]` are behind us.
    pos: usize,
}

impl NavHistory {
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Records a jump, discarding any entries that were rewound past.
    pub(super) fn push(&mut self, entry: NavEntry) {
        self.entries.truncate(self.pos);
        self.entries.push(entry);
        self.pos = self.entries.len();
    }

    pub(super) fn can_go_back(&self) -> bool {
        self.pos > 0
    }

    pub(super) fn can_go_forward(&self) -> bool {
        self.pos < self.entries.len()
    }

    /// Rewinds one entry and reports the link position to return to.
    pub(super) fn go_back(&mut self) -> Option<NavLoc> {
        self.pos = self.pos.checked_sub(1)?;
        Some(self.entries[self.pos].from)
    }

    /// Replays the entry last rewound and reports its target position.
    pub(super) fn go_forward(&mut self) -> Option<NavLoc> {
        let to = self.entries.get(self.pos)?.to;
        self.pos += 1;
        Some(to)
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(from_line: usize, to_line: usize) -> NavEntry {
        NavEntry {
            from: NavLoc::new(0, from_line, 4),
            to: NavLoc::new(1, to_line, 0),
        }
    }

    #[test]
    fn empty_history_goes_nowhere() {
        let mut h = NavHistory::new();
        assert!(!h.can_go_back());
        assert!(!h.can_go_forward());
        assert_eq!(h.go_back(), None);
        assert_eq!(h.go_forward(), None);
    }

    #[test]
    fn back_reaches_the_link_and_forward_the_target() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        assert!(h.can_go_back());
        assert!(!h.can_go_forward());

        // Back lands on the link that was followed...
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 10, 4)));
        assert!(!h.can_go_back());
        assert!(h.can_go_forward());

        // ...and forward on the target that link led to. The asymmetry is the
        // whole reason an entry stores two positions.
        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 100, 0)));
        assert!(!h.can_go_forward());
    }

    #[test]
    fn walks_a_chain_of_jumps_in_both_directions() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        h.push(entry(20, 200));
        h.push(entry(30, 300));

        assert_eq!(h.go_back(), Some(NavLoc::new(0, 30, 4)));
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 20, 4)));
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 10, 4)));
        assert_eq!(h.go_back(), None);

        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 100, 0)));
        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 200, 0)));
        assert_eq!(h.go_forward(), Some(NavLoc::new(1, 300, 0)));
        assert_eq!(h.go_forward(), None);
    }

    #[test]
    fn a_new_jump_replaces_the_rewound_tail() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        h.push(entry(20, 200));
        h.push(entry(30, 300));
        h.go_back();
        h.go_back();
        assert!(h.can_go_forward());

        h.push(entry(40, 400));
        assert!(!h.can_go_forward(), "entries after the rewind point are gone");
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 40, 4)));
        assert_eq!(h.go_back(), Some(NavLoc::new(0, 10, 4)));
        assert_eq!(h.go_back(), None);
    }

    #[test]
    fn clear_drops_everything() {
        let mut h = NavHistory::new();
        h.push(entry(10, 100));
        h.go_back();
        h.clear();
        assert!(!h.can_go_back());
        assert!(!h.can_go_forward());
    }
}
