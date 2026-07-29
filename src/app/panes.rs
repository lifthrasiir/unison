//! The editor pane model: which document each side of a vertical split shows,
//! which side has the focus, and where the divider sits.
//!
//! A *pane* is one whole editor surface — the document text, the glyph grids,
//! the minimap and the inline tool palette — not a sub-widget of one. The panes
//! are views onto [`UniformApp::open_documents`]; closing a pane detaches it
//! from its document but leaves the document open (it keeps its edits, its undo
//! stack and its dirty flag, exactly as a document already does today when a
//! second file is opened over it).
//!
//! Two rules shape everything here and are what the tests below pin down:
//!
//! - **At most one placeholder.** A pane with no document shows the "select a
//!   file" placeholder, and two of those at once would leave the sidebar with
//!   no way to say which one an opened file lands in. Splitting is therefore
//!   only offered from a single pane that *has* a document, which is the only
//!   operation that could ever produce a second placeholder.
//! - **A document is shown by at most one pane.** Two live editors over one
//!   document would have to be kept in sync line by line; instead, opening a
//!   file that is already on screen moves the focus to the pane showing it.
//!
//! [`UniformApp::open_documents`]: super::UniformApp

/// Which side of the split a new pane goes to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SplitSide {
    Left,
    Right,
}

/// How far the divider may be dragged before the drop closes a pane, as a
/// fraction of the editor area's width.
pub(super) const CLOSE_EDGE_FRACTION: f32 = 0.08;

/// The narrowest a pane may get by dragging the divider, in points. Below this
/// the drag is still tracked (so releasing near an edge closes that pane) but
/// the layout stops following it.
pub(super) const MIN_PANE_WIDTH: f32 = 120.0;

/// One editor surface.
pub(super) struct Pane {
    /// Index into `UniformApp::open_documents`, or `None` for the placeholder.
    pub(super) doc_idx: Option<usize>,
    /// The editor zoom level. Per pane rather than per window: the two panes
    /// are independent contexts, and Cmd/Ctrl + wheel already picks its target
    /// by what the pointer is over.
    pub(super) zoom_level: u32,
    /// This pane's screen rect as of the last frame, for pointer-based zoom
    /// routing. `None` while the pane shows the placeholder, which is not an
    /// editor and so must not make Cmd/Ctrl + wheel zoom anything.
    pub(super) view_rect: Option<egui::Rect>,
}

impl Pane {
    fn placeholder(zoom_level: u32) -> Self {
        Self { doc_idx: None, zoom_level, view_rect: None }
    }
}

/// The one or two panes of the editor area, and which of them has the focus.
pub(super) struct Panes {
    /// Left to right; always one or two entries.
    list: Vec<Pane>,
    /// Index into `list` of the pane that last held the keyboard focus. This
    /// is what "the active document" means for the menus, the status bar and
    /// the window title.
    focus: usize,
    /// Fraction of the editor area given to the left pane while split.
    pub(super) split_ratio: f32,
}

impl Panes {
    /// A single placeholder pane, the state the application starts in.
    pub(super) fn new() -> Self {
        Self {
            list: vec![Pane::placeholder(1)],
            focus: 0,
            split_ratio: 0.5,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.list.len()
    }

    pub(super) fn is_split(&self) -> bool {
        self.list.len() > 1
    }

    pub(super) fn get(&self, idx: usize) -> Option<&Pane> {
        self.list.get(idx)
    }

    pub(super) fn get_mut(&mut self, idx: usize) -> Option<&mut Pane> {
        self.list.get_mut(idx)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &Pane> {
        self.list.iter()
    }

    pub(super) fn focus(&self) -> usize {
        self.focus
    }

    pub(super) fn set_focus(&mut self, idx: usize) {
        if idx < self.list.len() {
            self.focus = idx;
        }
    }

    pub(super) fn focused(&self) -> &Pane {
        &self.list[self.focus]
    }

    /// The document the rest of the application treats as active.
    pub(super) fn active_doc_idx(&self) -> Option<usize> {
        self.focused().doc_idx
    }

    /// The pane currently showing `doc_idx`, if any.
    pub(super) fn pane_showing(&self, doc_idx: usize) -> Option<usize> {
        self.list.iter().position(|p| p.doc_idx == Some(doc_idx))
    }

    /// Where a newly opened file goes: the placeholder pane if one is up,
    /// otherwise the pane that last had the focus.
    pub(super) fn open_target(&self) -> usize {
        self.list
            .iter()
            .position(|p| p.doc_idx.is_none())
            .unwrap_or(self.focus)
    }

    /// Shows `doc_idx`, moving the focus to the pane that ends up with it.
    /// A document already on screen is *not* duplicated — the focus simply
    /// moves to the pane that has it.
    pub(super) fn show_document(&mut self, doc_idx: usize) {
        if let Some(existing) = self.pane_showing(doc_idx) {
            self.focus = existing;
            return;
        }
        let target = self.open_target();
        self.list[target].doc_idx = Some(doc_idx);
        self.focus = target;
    }

    /// Whether a new split may be opened. Only from a single pane that has a
    /// document: splitting a placeholder would give two of them, and a third
    /// pane is out of scope.
    pub(super) fn can_split(&self) -> bool {
        self.list.len() == 1 && self.list[0].doc_idx.is_some()
    }

    /// Opens a placeholder pane on `side`, moving the existing pane to the
    /// other side, and focuses the new pane. No-op unless [`Self::can_split`].
    pub(super) fn split(&mut self, side: SplitSide) {
        if !self.can_split() {
            return;
        }
        let new_pane = Pane::placeholder(self.list[0].zoom_level);
        match side {
            SplitSide::Left => {
                self.list.insert(0, new_pane);
                self.focus = 0;
            }
            SplitSide::Right => {
                self.list.push(new_pane);
                self.focus = 1;
            }
        }
        self.split_ratio = 0.5;
    }

    /// The pane the focus would move to on `side`, if there is one. Moving is
    /// a step, not a wrap: from the left pane there is nothing further left,
    /// and a single pane has nowhere to go at all.
    fn focus_target(&self, side: SplitSide) -> Option<usize> {
        let target = match side {
            SplitSide::Left => self.focus.checked_sub(1)?,
            SplitSide::Right => self.focus + 1,
        };
        (target < self.list.len()).then_some(target)
    }

    /// Whether the focus can move one pane towards `side`.
    pub(super) fn can_focus_side(&self, side: SplitSide) -> bool {
        self.focus_target(side).is_some()
    }

    /// Moves the focus one pane towards `side`. No-op at that end of the split.
    pub(super) fn focus_side(&mut self, side: SplitSide) {
        if let Some(target) = self.focus_target(side) {
            self.focus = target;
        }
    }

    /// Whether the two panes can be exchanged.
    pub(super) fn can_swap(&self) -> bool {
        self.list.len() == 2
    }

    /// Exchanges the two panes, keeping the focus on the same *content* and
    /// mirroring the divider so neither pane changes width.
    pub(super) fn swap(&mut self) {
        if !self.can_swap() {
            return;
        }
        self.list.swap(0, 1);
        self.focus = 1 - self.focus;
        self.split_ratio = 1.0 - self.split_ratio;
    }

    /// Closes pane `idx`. With two panes the other one takes over the whole
    /// area; with one, the pane drops its document and becomes the
    /// placeholder. The document itself stays open either way.
    pub(super) fn close(&mut self, idx: usize) {
        if idx >= self.list.len() {
            return;
        }
        if self.list.len() > 1 {
            self.list.remove(idx);
            self.focus = 0;
            self.split_ratio = 0.5;
        } else {
            self.list[0].doc_idx = None;
            self.list[0].view_rect = None;
        }
    }

    /// Whether the close command does anything: a lone placeholder has
    /// nothing to close.
    pub(super) fn can_close(&self) -> bool {
        self.is_split() || self.focused().doc_idx.is_some()
    }

    /// The pane a divider dropped at `ratio` closes, if any. Dragging the
    /// divider onto either edge and releasing there means "close the pane I
    /// just collapsed".
    pub(super) fn pane_closed_by_ratio(&self, ratio: f32) -> Option<usize> {
        if !self.is_split() {
            return None;
        }
        if ratio <= CLOSE_EDGE_FRACTION {
            Some(0)
        } else if ratio >= 1.0 - CLOSE_EDGE_FRACTION {
            Some(1)
        } else {
            None
        }
    }
}

/// A pane command requested by the menu or its accelerator, dispatched after
/// the panes have been laid out (so this frame's editor input has landed and
/// the focus is up to date).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum PaneAction {
    #[default]
    None,
    Close,
    Split(SplitSide),
    Swap,
    /// Move the keyboard focus to the pane on that side.
    Focus(SplitSide),
}

impl super::UniformApp {
    /// Closes the focused pane. The document stays open — it keeps its edits,
    /// its undo stack and its dirty flag, and reopening it from the sidebar
    /// brings all of that back.
    pub(super) fn close_focused_pane(&mut self) {
        if !self.panes.can_close() {
            return;
        }
        let focus = self.panes.focus();
        if let Some(doc) = self.pane_doc_mut(focus) {
            doc.flush_pending_changes();
        }
        self.panes.close(focus);
    }

    pub(super) fn split_focused_pane(&mut self, side: SplitSide) {
        self.panes.split(side);
    }

    pub(super) fn swap_panes(&mut self) {
        self.panes.swap();
    }

    /// Runs `action`; returns whether the pane layout changed, so the caller
    /// can hand the keyboard focus to the pane that ended up with it.
    pub(super) fn apply_pane_action(&mut self, action: PaneAction) -> bool {
        match action {
            PaneAction::None => return false,
            PaneAction::Close => self.close_focused_pane(),
            PaneAction::Split(side) => self.split_focused_pane(side),
            PaneAction::Swap => self.swap_panes(),
            PaneAction::Focus(side) => self.panes.focus_side(side),
        }
        true
    }

    /// Gives the keyboard focus to the focused pane's editor. A pane that
    /// closes takes its editor widget — and with it egui's focus — away, so
    /// without this the survivor would need a click before it took a keystroke.
    pub(super) fn focus_pane_editor(&mut self, ctx: &egui::Context) {
        let focus = self.panes.focus();
        if let Some(canvas_id) = self
            .pane_doc_mut(focus)
            .and_then(|doc| doc.editor_state.canvas_id)
        {
            ctx.memory_mut(|m| m.request_focus(canvas_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two structural invariants, checked after every operation below.
    fn check_invariants(panes: &Panes) {
        assert!((1..=2).contains(&panes.len()), "pane count out of range");
        assert!(panes.focus() < panes.len(), "focus points at no pane");
        let placeholders = panes.iter().filter(|p| p.doc_idx.is_none()).count();
        assert!(placeholders <= 1, "two placeholders at once");
        if let (Some(a), Some(b)) = (panes.get(0), panes.get(1))
            && a.doc_idx.is_some()
        {
            assert_ne!(a.doc_idx, b.doc_idx, "one document shown by both panes");
        }
    }

    fn split_with(a: usize, b: usize) -> Panes {
        let mut panes = Panes::new();
        panes.show_document(a);
        panes.split(SplitSide::Right);
        panes.show_document(b);
        check_invariants(&panes);
        panes
    }

    #[test]
    fn starts_as_a_single_placeholder() {
        let panes = Panes::new();
        check_invariants(&panes);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes.active_doc_idx(), None);
        assert!(!panes.can_split());
        assert!(!panes.can_swap());
        assert!(!panes.can_close());
    }

    #[test]
    fn a_placeholder_takes_the_opened_document_before_the_focused_pane() {
        let mut panes = Panes::new();
        panes.show_document(0);
        panes.split(SplitSide::Right);
        // The split focuses the new placeholder, but even after focusing the
        // other side an open still lands in the placeholder.
        panes.set_focus(0);
        assert_eq!(panes.open_target(), 1);
        panes.show_document(7);
        check_invariants(&panes);
        assert_eq!(panes.get(1).unwrap().doc_idx, Some(7));
        assert_eq!(panes.focus(), 1);
    }

    #[test]
    fn with_both_panes_filled_an_open_replaces_the_focused_one() {
        let mut panes = split_with(3, 4);
        panes.set_focus(0);
        panes.show_document(9);
        check_invariants(&panes);
        assert_eq!(panes.get(0).unwrap().doc_idx, Some(9));
        assert_eq!(panes.get(1).unwrap().doc_idx, Some(4));
    }

    #[test]
    fn opening_a_document_that_is_already_shown_only_moves_the_focus() {
        let mut panes = split_with(3, 4);
        panes.set_focus(1);
        panes.show_document(3);
        check_invariants(&panes);
        assert_eq!(panes.focus(), 0);
        assert_eq!(panes.get(0).unwrap().doc_idx, Some(3));
        assert_eq!(panes.get(1).unwrap().doc_idx, Some(4));
    }

    #[test]
    fn splitting_is_only_offered_where_it_cannot_make_a_second_placeholder() {
        // A lone placeholder: splitting it would give two.
        let mut panes = Panes::new();
        assert!(!panes.can_split());
        panes.split(SplitSide::Left);
        assert_eq!(panes.len(), 1);

        // A single pane with a document: the one case that splits.
        panes.show_document(0);
        assert!(panes.can_split());
        panes.split(SplitSide::Left);
        check_invariants(&panes);
        assert_eq!(panes.len(), 2);
        assert_eq!(panes.get(0).unwrap().doc_idx, None);
        assert_eq!(panes.get(1).unwrap().doc_idx, Some(0));
        assert_eq!(panes.focus(), 0, "the new placeholder takes the focus");

        // Already split, either with a placeholder or not: no third pane.
        assert!(!panes.can_split());
        panes.split(SplitSide::Right);
        assert_eq!(panes.len(), 2);
        panes.show_document(1);
        assert!(!panes.can_split());
        panes.split(SplitSide::Right);
        check_invariants(&panes);
        assert_eq!(panes.len(), 2);
    }

    #[test]
    fn split_right_puts_the_new_pane_on_the_right() {
        let mut panes = Panes::new();
        panes.show_document(5);
        panes.split(SplitSide::Right);
        check_invariants(&panes);
        assert_eq!(panes.get(0).unwrap().doc_idx, Some(5));
        assert_eq!(panes.get(1).unwrap().doc_idx, None);
        assert_eq!(panes.focus(), 1);
    }

    #[test]
    fn swap_keeps_the_focus_on_its_content_and_mirrors_the_divider() {
        let mut panes = split_with(3, 4);
        panes.split_ratio = 0.3;
        panes.set_focus(0);
        panes.swap();
        check_invariants(&panes);
        assert_eq!(panes.get(0).unwrap().doc_idx, Some(4));
        assert_eq!(panes.get(1).unwrap().doc_idx, Some(3));
        assert_eq!(panes.focus(), 1, "focus follows document 3");
        assert!((panes.split_ratio - 0.7).abs() < 1e-6);
    }

    #[test]
    fn the_focus_steps_between_panes_and_stops_at_the_ends() {
        let mut panes = split_with(3, 4);
        panes.set_focus(1);
        assert!(panes.can_focus_side(SplitSide::Left));
        assert!(!panes.can_focus_side(SplitSide::Right));
        panes.focus_side(SplitSide::Left);
        check_invariants(&panes);
        assert_eq!(panes.focus(), 0);

        // At the left end, moving further left does nothing.
        assert!(!panes.can_focus_side(SplitSide::Left));
        panes.focus_side(SplitSide::Left);
        assert_eq!(panes.focus(), 0);

        assert!(panes.can_focus_side(SplitSide::Right));
        panes.focus_side(SplitSide::Right);
        check_invariants(&panes);
        assert_eq!(panes.focus(), 1);

        // A single pane has nowhere to go in either direction.
        let mut single = Panes::new();
        single.show_document(0);
        assert!(!single.can_focus_side(SplitSide::Left));
        assert!(!single.can_focus_side(SplitSide::Right));
        single.focus_side(SplitSide::Right);
        check_invariants(&single);
        assert_eq!(single.focus(), 0);
    }

    #[test]
    fn closing_one_of_two_panes_leaves_the_other_full_width() {
        let mut panes = split_with(3, 4);
        panes.close(0);
        check_invariants(&panes);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes.active_doc_idx(), Some(4));

        // And closing the last one leaves the placeholder, not an empty area.
        assert!(panes.can_close());
        panes.close(0);
        check_invariants(&panes);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes.active_doc_idx(), None);
        assert!(!panes.can_close());
    }

    #[test]
    fn closing_the_document_pane_of_a_split_leaves_a_single_placeholder() {
        let mut panes = Panes::new();
        panes.show_document(2);
        panes.split(SplitSide::Right);
        panes.close(0);
        check_invariants(&panes);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes.active_doc_idx(), None);
    }

    #[test]
    fn a_divider_dropped_at_an_edge_closes_the_collapsed_pane() {
        let panes = split_with(3, 4);
        assert_eq!(panes.pane_closed_by_ratio(0.0), Some(0));
        assert_eq!(panes.pane_closed_by_ratio(1.0), Some(1));
        assert_eq!(panes.pane_closed_by_ratio(0.5), None);
        assert_eq!(panes.pane_closed_by_ratio(CLOSE_EDGE_FRACTION * 2.0), None);
        // A single pane has no divider to drop.
        assert_eq!(Panes::new().pane_closed_by_ratio(0.0), None);
    }
}
