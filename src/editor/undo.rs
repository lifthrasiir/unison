use crate::document::{DocLine, PixelGrid};
use crate::editor::EditMode;
use crate::editor::caret::{Caret, char_to_byte};
use crate::pixel::PixelShape;

const COALESCE_MS: u128 = 800;

#[derive(Clone, Debug)]
pub struct PixelChange {
    pub row: u16,
    pub col: u16,
    pub old: PixelShape,
    pub new: PixelShape,
}

#[derive(Clone, Debug)]
pub struct PixelSelectionSnapshot {
    pub item_idx: usize,
    pub row: i16,
    pub col: i16,
    pub width: u16,
    pub height: u16,
    pub float_pixels: Option<PixelGrid>,
}

#[derive(Clone, Debug)]
pub enum UndoOp {
    Text {
        line: usize,
        col: usize,
        old: String,
        new: String,
    },
    Lines {
        at: usize,
        old: Vec<DocLine>,
        new: Vec<DocLine>,
    },
    Pixels {
        line: usize,
        changes: Vec<PixelChange>,
    },
    PixelSelection {
        line: usize,
        pixel_changes: Vec<PixelChange>,
        mode_before: EditMode,
        mode_after: EditMode,
        before: Option<PixelSelectionSnapshot>,
        after: Option<PixelSelectionSnapshot>,
    },
    Compound(Vec<UndoOp>),
}

#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub op: UndoOp,
    pub caret_before: Caret,
    pub caret_after: Caret,
}

pub struct SelectionUndoCtx<'a> {
    pub mode: &'a mut EditMode,
    pub pixel_selection: &'a mut Option<crate::editor::pixel_selection::PixelSelection>,
}

pub struct UndoStack {
    entries: Vec<UndoEntry>,
    position: usize,
    last_push_time: std::time::Instant,
    saved_position: Option<usize>,
    /// Stepped whenever entries are dropped for a redo branch nobody can reach
    /// again. A [`SavePoint`] taken before that happened names a state the
    /// stack can no longer walk to, and this is how it is told apart from a
    /// position that merely moved; see [`UndoStack::mark_saved_at`].
    epoch: u64,
}

/// The state of a buffer at one moment, kept so that a write finishing later
/// can be credited to the revision it actually wrote.
///
/// A save is not instant — on a network share it is the slowest thing the
/// editor does — and the buffer moves on while it is in flight. Marking the
/// *current* position saved when the write lands would call edits made during
/// the write saved as well, and they are not on disk. So the position is taken
/// when the bytes are serialized and handed back at the end; see
/// [`crate::app::save`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavePoint {
    position: usize,
    epoch: u64,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            position: 0,
            last_push_time: std::time::Instant::now() - std::time::Duration::from_secs(10),
            saved_position: Some(0),
            epoch: 0,
        }
    }

    pub fn mark_saved(&mut self) {
        self.saved_position = Some(self.position);
        // A saved snapshot must stay on an undo-entry boundary. Otherwise a
        // quick follow-up edit can coalesce into the entry that was just
        // saved, leaving `position == saved_position` even though the text or
        // pixels no longer match the file on disk.
        self.break_coalesce();
    }

    /// The point a write about to be started is writing, with coalescing
    /// broken so that the next keystroke cannot fold into the entry this names
    /// — the same reason [`UndoStack::mark_saved`] breaks it.
    pub fn save_point(&mut self) -> SavePoint {
        self.break_coalesce();
        SavePoint {
            position: self.position,
            epoch: self.epoch,
        }
    }

    /// Records that the buffer as of `point` is what is on disk now.
    ///
    /// A point whose entries have since been dropped — the user undid past it
    /// and typed, so the redo branch it sat on is gone — names a state nothing
    /// can walk back to, and leaves the document unconditionally dirty rather
    /// than claiming a position that now means something else.
    pub fn mark_saved_at(&mut self, point: SavePoint) {
        if point.epoch == self.epoch {
            self.saved_position = Some(point.position);
        } else {
            self.saved_position = None;
        }
        if self.is_at_saved() {
            self.break_coalesce();
        }
    }

    pub fn is_at_saved(&self) -> bool {
        self.saved_position == Some(self.position)
    }

    fn truncate_and_invalidate(&mut self) {
        if self.entries.len() > self.position {
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.entries.truncate(self.position);
        if let Some(sp) = self.saved_position
            && sp > self.position
        {
            self.saved_position = None;
        }
    }

    pub fn break_coalesce(&mut self) {
        self.last_push_time = std::time::Instant::now() - std::time::Duration::from_secs(10);
    }

    pub fn push_text(
        &mut self,
        line: usize,
        col: usize,
        old: String,
        new: String,
        caret_before: Caret,
        caret_after: Caret,
    ) {
        if old == new {
            return;
        }
        // Separate copies for the merge attempt, since `old`/`new` are also
        // needed (by move) to build a fresh entry if merging doesn't apply.
        let old_for_merge = old.clone();
        let new_for_merge = new.clone();

        self.push_coalescing(
            caret_before,
            caret_after,
            move |entry| {
                let UndoOp::Text {
                    line: prev_line,
                    col: prev_col,
                    old: prev_old,
                    new: prev_new,
                } = &mut entry.op
                else {
                    return false;
                };
                if *prev_line != line {
                    return false;
                }
                let prev_end = *prev_col + prev_new.chars().count();
                // Typing forward
                if col == prev_end && old_for_merge.is_empty() {
                    prev_new.push_str(&new_for_merge);
                    return true;
                }
                // Backspace: old is the deleted char, new is empty,
                // col is where the char was (before current cursor)
                if !old_for_merge.is_empty()
                    && new_for_merge.is_empty()
                    && col + old_for_merge.chars().count() == *prev_col
                {
                    let mut merged_old = old_for_merge.clone();
                    merged_old.push_str(prev_old);
                    *prev_old = merged_old;
                    *prev_col = col;
                    return true;
                }
                // Replace chain: the same span rewritten over and over, so
                // this edit's `old` is exactly what the previous one wrote.
                // A `ref` drag rewrites its whole line this way; Alt+wheel
                // rewrites one number in place. Either is one edit to undo.
                if *prev_col == col && *prev_new == old_for_merge {
                    *prev_new = new_for_merge.clone();
                    return true;
                }
                false
            },
            move || UndoOp::Text {
                line,
                col,
                old,
                new,
            },
        );
    }

    pub fn push_lines(
        &mut self,
        at: usize,
        old: Vec<DocLine>,
        new: Vec<DocLine>,
        caret_before: Caret,
        caret_after: Caret,
    ) {
        self.truncate_and_invalidate();
        self.entries.push(UndoEntry {
            op: UndoOp::Lines { at, old, new },
            caret_before,
            caret_after,
        });
        self.position = self.entries.len();
    }

    /// Replace a block of lines, folding into a preceding replacement of the
    /// same block that this one continues (its `new` is exactly this one's
    /// `old`). A drag rewrites its block once per whole-cell step; the whole
    /// drag is one undo, the way the `Text` replace chain handles a `ref` drag.
    pub fn push_lines_replacing(
        &mut self,
        at: usize,
        old: Vec<DocLine>,
        new: Vec<DocLine>,
        caret_before: Caret,
        caret_after: Caret,
    ) {
        if old == new {
            return;
        }
        let old_for_merge = old.clone();
        let new_for_merge = new.clone();

        self.push_coalescing(
            caret_before,
            caret_after,
            move |entry| {
                let UndoOp::Lines {
                    at: prev_at,
                    new: prev_new,
                    ..
                } = &mut entry.op
                else {
                    return false;
                };
                if *prev_at != at || *prev_new != old_for_merge {
                    return false;
                }
                *prev_new = new_for_merge.clone();
                true
            },
            move || UndoOp::Lines { at, old, new },
        );
    }

    /// Record a structural change that is a *consequence* of the last user
    /// edit rather than an edit of its own — the grid resize/creation/demotion
    /// `reconcile` performs once the caret leaves a glyph header.
    ///
    /// It folds into the preceding entry, so one undo takes the header text and
    /// its grid back together. Undoing them separately would leave a `18 16`
    /// header over a 16-wide grid, which the reparse renders as an empty grid.
    pub fn push_derived_lines(
        &mut self,
        at: usize,
        old: Vec<DocLine>,
        new: Vec<DocLine>,
        caret_before: Caret,
        caret_after: Caret,
    ) {
        // Only foldable at the tip of the stack, and never onto the entry the
        // saved snapshot points at: merging there would keep `is_at_saved`
        // true even though `lines` no longer match the file on disk.
        let foldable = self.position == self.entries.len()
            && self.position > 0
            && self.saved_position != Some(self.position);
        if !foldable {
            self.push_lines(at, old, new, caret_before, caret_after);
            return;
        }

        let op = UndoOp::Lines { at, old, new };
        let entry = &mut self.entries[self.position - 1];
        match &mut entry.op {
            UndoOp::Compound(ops) => ops.push(op),
            other => {
                let prev = std::mem::replace(other, UndoOp::Compound(Vec::new()));
                let UndoOp::Compound(ops) = other else {
                    unreachable!()
                };
                ops.push(prev);
                ops.push(op);
            }
        }
        entry.caret_after = caret_after;
    }

    pub fn push_compound(&mut self, ops: Vec<UndoOp>, caret_before: Caret, caret_after: Caret) {
        self.truncate_and_invalidate();
        self.entries.push(UndoEntry {
            op: UndoOp::Compound(ops),
            caret_before,
            caret_after,
        });
        self.position = self.entries.len();
    }

    pub fn push_pixel(
        &mut self,
        line: usize,
        change: PixelChange,
        caret_before: Caret,
        caret_after: Caret,
    ) {
        if change.old == change.new {
            return;
        }
        // Separate copy for the merge attempt, since `change` is also needed
        // (by move) to build a fresh entry if merging doesn't apply.
        let change_for_merge = change.clone();

        self.push_coalescing(
            caret_before,
            caret_after,
            move |entry| {
                let UndoOp::Pixels {
                    line: prev_line,
                    changes,
                } = &mut entry.op
                else {
                    return false;
                };
                if *prev_line != line {
                    return false;
                }
                if let Some(existing) = changes
                    .iter_mut()
                    .find(|c| c.row == change_for_merge.row && c.col == change_for_merge.col)
                {
                    existing.new = change_for_merge.new;
                } else {
                    changes.push(change_for_merge);
                }
                true
            },
            move || UndoOp::Pixels {
                line,
                changes: vec![change],
            },
        );
    }

    /// Shared coalescing skeleton used by `push_text` and `push_pixel`:
    /// if enough time has passed since the last push, or the previous entry
    /// can't absorb this change, start a fresh undo entry (truncating any
    /// redo history); otherwise merge into the previous entry in place.
    fn push_coalescing(
        &mut self,
        caret_before: Caret,
        caret_after: Caret,
        try_merge: impl FnOnce(&mut UndoEntry) -> bool,
        make_op: impl FnOnce() -> UndoOp,
    ) {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_push_time).as_millis();
        self.last_push_time = now;

        if elapsed < COALESCE_MS
            && self.position > 0
            && let Some(entry) = self.entries.get_mut(self.position - 1)
            && try_merge(entry)
        {
            entry.caret_after = caret_after;
            return;
        }

        self.truncate_and_invalidate();
        self.entries.push(UndoEntry {
            op: make_op(),
            caret_before,
            caret_after,
        });
        self.position = self.entries.len();
    }

    // Three before/after pairs; a struct per pair would read worse at the call site.
    #[expect(clippy::too_many_arguments)]
    pub fn push_pixel_selection(
        &mut self,
        line: usize,
        pixel_changes: Vec<PixelChange>,
        mode_before: EditMode,
        mode_after: EditMode,
        before: Option<PixelSelectionSnapshot>,
        after: Option<PixelSelectionSnapshot>,
        caret_before: Caret,
        caret_after: Caret,
    ) {
        let pixel_changes_for_merge = pixel_changes.clone();
        let after_for_merge = after.clone();
        let mode_after_for_merge = mode_after.clone();

        self.push_coalescing(
            caret_before,
            caret_after,
            move |entry| {
                let UndoOp::PixelSelection {
                    line: prev_line,
                    pixel_changes: _,
                    after: prev_after,
                    mode_after: prev_mode_after,
                    ..
                } = &mut entry.op
                else {
                    return false;
                };
                if *prev_line != line {
                    return false;
                }
                // Only coalesce consecutive floating moves (no new pixel changes)
                if !pixel_changes_for_merge.is_empty() {
                    return false;
                }
                if prev_after.as_ref().is_none_or(|s| s.float_pixels.is_none()) {
                    return false;
                }
                *prev_after = after_for_merge;
                *prev_mode_after = mode_after_for_merge;
                true
            },
            move || UndoOp::PixelSelection {
                line,
                pixel_changes,
                mode_before,
                mode_after,
                before,
                after,
            },
        );
    }

    pub fn push_pixel_batch(
        &mut self,
        line: usize,
        changes: Vec<PixelChange>,
        caret_before: Caret,
        caret_after: Caret,
    ) {
        if changes.is_empty() {
            return;
        }
        self.truncate_and_invalidate();
        self.entries.push(UndoEntry {
            op: UndoOp::Pixels { line, changes },
            caret_before,
            caret_after,
        });
        self.position = self.entries.len();
        self.break_coalesce();
    }

    pub fn can_undo(&self) -> bool {
        self.position > 0
    }

    pub fn can_redo(&self) -> bool {
        self.position < self.entries.len()
    }

    pub fn undo(&mut self, lines: &mut Vec<DocLine>) -> Option<Caret> {
        self.undo_with_sel(lines, None)
    }

    pub fn undo_with_sel(
        &mut self,
        lines: &mut Vec<DocLine>,
        mut sel: Option<SelectionUndoCtx>,
    ) -> Option<Caret> {
        if self.position == 0 {
            return None;
        }
        self.position -= 1;
        let entry = &self.entries[self.position];
        let caret = entry.caret_before;
        apply_op(&entry.op, lines, Direction::Undo, &mut sel);
        self.break_coalesce();
        Some(caret)
    }

    pub fn redo(&mut self, lines: &mut Vec<DocLine>) -> Option<Caret> {
        self.redo_with_sel(lines, None)
    }

    pub fn redo_with_sel(
        &mut self,
        lines: &mut Vec<DocLine>,
        mut sel: Option<SelectionUndoCtx>,
    ) -> Option<Caret> {
        if self.position >= self.entries.len() {
            return None;
        }
        let entry = &self.entries[self.position];
        let caret = entry.caret_after;
        apply_op(&entry.op, lines, Direction::Redo, &mut sel);
        self.position += 1;
        self.break_coalesce();
        Some(caret)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Undo,
    Redo,
}

/// Applies one op in the given direction. Undo applies each op's `old` side
/// (nested ops in reverse order); redo applies the `new` side (in order).
/// One shared implementation so a new op kind (or a new nesting rule) cannot
/// be handled asymmetrically between undo and redo.
fn apply_op(
    op: &UndoOp,
    lines: &mut Vec<DocLine>,
    dir: Direction,
    sel: &mut Option<SelectionUndoCtx>,
) {
    match op {
        UndoOp::Text {
            line,
            col,
            old,
            new,
        } => {
            let (remove, insert) = match dir {
                Direction::Undo => (new, old),
                Direction::Redo => (old, new),
            };
            if let Some(DocLine::Text(s)) = lines.get_mut(*line) {
                let byte_start = char_to_byte(s, *col);
                let byte_end = char_to_byte(s, *col + remove.chars().count());
                s.replace_range(byte_start..byte_end, insert);
            }
        }
        UndoOp::Lines { at, old, new } => {
            let (remove, insert) = match dir {
                Direction::Undo => (new, old),
                Direction::Redo => (old, new),
            };
            let end = (*at + remove.len()).min(lines.len());
            lines.splice(*at..end, insert.iter().cloned());
        }
        UndoOp::Pixels { line, changes } => {
            if let Some(DocLine::Grid(grid)) = lines.get_mut(*line) {
                apply_pixel_changes(grid, changes, dir);
            }
        }
        UndoOp::Compound(ops) => {
            let mut each = |op| apply_op(op, lines, dir, sel);
            match dir {
                Direction::Undo => ops.iter().rev().for_each(&mut each),
                Direction::Redo => ops.iter().for_each(&mut each),
            }
        }
        UndoOp::PixelSelection {
            line,
            pixel_changes,
            mode_before,
            mode_after,
            before,
            after,
        } => {
            if let Some(DocLine::Grid(grid)) = lines.get_mut(*line) {
                apply_pixel_changes(grid, pixel_changes, dir);
            }
            let (mode, snapshot) = match dir {
                Direction::Undo => (mode_before, before),
                Direction::Redo => (mode_after, after),
            };
            if let Some(ctx) = sel.as_mut() {
                *ctx.mode = mode.clone();
                *ctx.pixel_selection = snapshot
                    .as_ref()
                    .map(crate::editor::pixel_selection::PixelSelection::from_snapshot);
            }
        }
    }
}

fn apply_pixel_changes(grid: &mut PixelGrid, changes: &[PixelChange], dir: Direction) {
    match dir {
        Direction::Undo => {
            for ch in changes.iter().rev() {
                grid.set(ch.row, ch.col, ch.old);
            }
        }
        Direction::Redo => {
            for ch in changes {
                grid.set(ch.row, ch.col, ch.new);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PixelGrid;

    fn text(s: &str) -> DocLine {
        DocLine::Text(s.to_string())
    }
    fn grid(w: u16, h: u16) -> DocLine {
        DocLine::Grid(PixelGrid::new(w, h))
    }
    fn c(line: usize, col: usize) -> Caret {
        Caret::new(line, col)
    }

    #[test]
    fn text_undo_redo() {
        let mut lines = vec![text("hello")];
        let mut undo = UndoStack::new();

        // Delete 'o' at col 4
        undo.push_text(0, 4, "o".into(), "".into(), c(0, 5), c(0, 4));
        lines[0] = DocLine::Text("hell".into());

        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines[0], text("hello"));
        assert_eq!(caret, c(0, 5));

        let caret = undo.redo(&mut lines).unwrap();
        assert_eq!(lines[0], text("hell"));
        assert_eq!(caret, c(0, 4));
    }

    #[test]
    fn lines_undo_redo_insert() {
        let mut lines = vec![text("a"), text("b")];
        let mut undo = UndoStack::new();

        // Insert a grid between lines 0 and 1
        undo.push_lines(1, vec![], vec![grid(2, 2)], c(0, 1), c(1, 0));
        lines.insert(1, grid(2, 2));
        assert_eq!(lines.len(), 3);

        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], text("a"));
        assert_eq!(lines[1], text("b"));
        assert_eq!(caret, c(0, 1));

        let caret = undo.redo(&mut lines).unwrap();
        assert_eq!(lines.len(), 3);
        assert!(matches!(lines[1], DocLine::Grid(_)));
        assert_eq!(caret, c(1, 0));
    }

    #[test]
    fn derived_lines_fold_into_the_preceding_edit() {
        let mut lines = vec![text("glyph foo 8 2"), grid(8, 2)];
        let mut undo = UndoStack::new();

        // The user edit...
        undo.push_text(0, 10, "8".into(), "18".into(), c(0, 11), c(0, 12));
        lines[0] = text("glyph foo 18 2");
        // ...and the resize it implies.
        undo.push_derived_lines(1, vec![grid(8, 2)], vec![grid(18, 2)], c(0, 12), c(0, 12));
        lines[1] = grid(18, 2);

        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("glyph foo 8 2"), grid(8, 2)]);
        assert_eq!(caret, c(0, 11));
        assert!(!undo.can_undo(), "both halves belong to one entry");

        undo.redo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("glyph foo 18 2"), grid(18, 2)]);
        assert!(!undo.can_redo());
    }

    #[test]
    fn derived_lines_stay_separate_at_the_saved_snapshot() {
        let mut lines = vec![text("glyph foo 8 2")];
        let mut undo = UndoStack::new();

        undo.push_text(0, 0, "".into(), "x".into(), c(0, 0), c(0, 1));
        undo.mark_saved();
        assert!(undo.is_at_saved());

        // Folding here would leave the document looking saved despite the
        // grid insertion, so it has to become its own entry.
        undo.push_derived_lines(1, vec![], vec![grid(8, 2)], c(0, 1), c(0, 1));
        lines.push(grid(8, 2));
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(undo.is_at_saved());
    }

    #[test]
    fn lines_undo_redo_split() {
        let mut lines = vec![text("abcd")];
        let mut undo = UndoStack::new();

        // Split at col 2: "abcd" -> "ab", "cd"
        undo.push_lines(
            0,
            vec![text("abcd")],
            vec![text("ab"), text("cd")],
            c(0, 2),
            c(1, 0),
        );
        lines.splice(0..1, vec![text("ab"), text("cd")]);

        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("abcd")]);
        assert_eq!(caret, c(0, 2));

        let caret = undo.redo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("ab"), text("cd")]);
        assert_eq!(caret, c(1, 0));
    }

    #[test]
    fn pixel_undo_redo() {
        let mut lines = vec![text("glyph foo 2 2"), grid(2, 2)];
        let mut undo = UndoStack::new();

        let old = PixelShape::EMPTY;
        let new = PixelShape::new(0, true);

        undo.push_pixel(
            1,
            PixelChange {
                row: 0,
                col: 1,
                old,
                new,
            },
            c(1, 0),
            c(1, 0),
        );
        if let DocLine::Grid(g) = &mut lines[1] {
            g.set(0, 1, new);
        }

        let caret = undo.undo(&mut lines).unwrap();
        if let DocLine::Grid(g) = &lines[1] {
            assert_eq!(g.get(0, 1), old);
        }
        assert_eq!(caret, c(1, 0));

        let caret = undo.redo(&mut lines).unwrap();
        if let DocLine::Grid(g) = &lines[1] {
            assert_eq!(g.get(0, 1), new);
        }
        assert_eq!(caret, c(1, 0));
    }

    #[test]
    fn pixel_coalesce_same_cell() {
        let mut lines = vec![grid(2, 2)];
        let mut undo = UndoStack::new();

        let s1 = PixelShape::EMPTY;
        let s2 = PixelShape::new(0, true);
        let s3 = PixelShape(1);

        undo.push_pixel(
            0,
            PixelChange {
                row: 0,
                col: 0,
                old: s1,
                new: s2,
            },
            c(0, 0),
            c(0, 0),
        );
        undo.push_pixel(
            0,
            PixelChange {
                row: 0,
                col: 0,
                old: s2,
                new: s3,
            },
            c(0, 0),
            c(0, 0),
        );

        // Should have coalesced into one entry
        assert_eq!(undo.position, 1);

        if let DocLine::Grid(g) = &mut lines[0] {
            g.set(0, 0, s3);
        }

        let _ = undo.undo(&mut lines);
        if let DocLine::Grid(g) = &lines[0] {
            assert_eq!(g.get(0, 0), s1); // back to original
        }
    }

    #[test]
    fn same_span_replacements_coalesce_into_one_entry() {
        // A span rewritten over and over — a `ref` drag, or Alt+wheel
        // stepping one number — is one edit, not one per step.
        let mut lines = vec![text("h 16")];
        let mut undo = UndoStack::new();

        undo.push_text(0, 2, "16".into(), "17".into(), c(0, 4), c(0, 4));
        lines[0] = text("h 17");
        undo.push_text(0, 2, "17".into(), "18".into(), c(0, 4), c(0, 4));
        lines[0] = text("h 18");
        undo.push_text(0, 2, "18".into(), "19".into(), c(0, 4), c(0, 4));
        lines[0] = text("h 19");
        assert_eq!(undo.position, 1);

        undo.undo(&mut lines);
        assert_eq!(lines[0], text("h 16"));
        assert!(!undo.can_undo());
    }

    #[test]
    fn a_broken_coalesce_splits_a_replacement_chain() {
        let mut lines = vec![text("h 16")];
        let mut undo = UndoStack::new();

        undo.push_text(0, 2, "16".into(), "17".into(), c(0, 4), c(0, 4));
        lines[0] = text("h 17");
        undo.break_coalesce();
        undo.push_text(0, 2, "17".into(), "18".into(), c(0, 4), c(0, 4));
        lines[0] = text("h 18");
        assert_eq!(undo.position, 2);

        undo.undo(&mut lines);
        assert_eq!(lines[0], text("h 17"));
        undo.undo(&mut lines);
        assert_eq!(lines[0], text("h 16"));
    }

    #[test]
    fn undo_empty_returns_none() {
        let mut lines = vec![text("a")];
        let mut undo = UndoStack::new();
        assert!(undo.undo(&mut lines).is_none());
    }

    #[test]
    fn redo_empty_returns_none() {
        let mut lines = vec![text("a")];
        let mut undo = UndoStack::new();
        assert!(undo.redo(&mut lines).is_none());
    }

    #[test]
    fn undo_truncates_redo_on_new_push() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        undo.break_coalesce();
        undo.push_text(0, 4, "".into(), "e".into(), c(0, 4), c(0, 5));
        lines[0] = DocLine::Text("abcde".into());

        // Undo "e"
        undo.undo(&mut lines);
        assert_eq!(lines[0], text("abcd"));

        // New edit — should truncate the redo for "e"
        undo.push_text(0, 4, "".into(), "f".into(), c(0, 4), c(0, 5));
        lines[0] = DocLine::Text("abcdf".into());

        assert!(undo.redo(&mut lines).is_none());
    }

    // -----------------------------------------------------------------------
    // Saved-position (dirty flag) tests
    // -----------------------------------------------------------------------

    #[test]
    fn new_stack_is_at_saved() {
        let undo = UndoStack::new();
        assert!(undo.is_at_saved());
    }

    #[test]
    fn edit_makes_not_saved() {
        let mut undo = UndoStack::new();
        undo.push_text(0, 0, "".into(), "a".into(), c(0, 0), c(0, 1));
        assert!(!undo.is_at_saved());
    }

    #[test]
    fn undo_all_text_restores_saved() {
        let mut lines = vec![text("hello")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 5, "".into(), " world".into(), c(0, 5), c(0, 11));
        lines[0] = DocLine::Text("hello world".into());
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert_eq!(lines[0], text("hello"));
        assert!(undo.is_at_saved());
    }

    #[test]
    fn undo_all_lines_restores_saved() {
        let mut lines = vec![text("a"), text("b")];
        let mut undo = UndoStack::new();

        undo.push_lines(1, vec![], vec![grid(2, 2)], c(0, 1), c(1, 0));
        lines.insert(1, grid(2, 2));
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());
    }

    #[test]
    fn undo_all_pixels_restores_saved() {
        let mut lines = vec![grid(2, 2)];
        let mut undo = UndoStack::new();

        let s0 = PixelShape::EMPTY;
        let s1 = PixelShape::new(0, true);

        undo.push_pixel(
            0,
            PixelChange {
                row: 0,
                col: 0,
                old: s0,
                new: s1,
            },
            c(0, 0),
            c(0, 0),
        );
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());
    }

    #[test]
    fn multiple_edits_undo_all_restores_saved() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        undo.break_coalesce();
        undo.push_text(0, 4, "".into(), "e".into(), c(0, 4), c(0, 5));
        lines[0] = DocLine::Text("abcde".into());

        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());
        assert_eq!(lines[0], text("abc"));
    }

    #[test]
    fn redo_after_undo_leaves_not_saved() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());

        undo.redo(&mut lines);
        assert!(!undo.is_at_saved());
    }

    /// A write started at one revision and landing after two more edits is
    /// credited to the revision it wrote, not to the buffer as it stands. See
    /// [`crate::app::save`].
    #[test]
    fn a_save_point_credits_the_revision_it_was_taken_at() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        let point = undo.save_point();

        undo.break_coalesce();
        undo.push_text(0, 4, "".into(), "e".into(), c(0, 4), c(0, 5));
        lines[0] = DocLine::Text("abcde".into());

        undo.mark_saved_at(point);
        assert!(
            !undo.is_at_saved(),
            "the edit made while the write was in flight is not on disk"
        );

        undo.undo(&mut lines);
        assert!(undo.is_at_saved(), "back at the revision the write carried");
    }

    /// The redo branch a save point sat on can be thrown away while the write
    /// is in flight — undo past it, then type. Nothing can walk back to what
    /// was written, so the point names nothing and the document stays dirty.
    #[test]
    fn a_save_point_on_a_dropped_redo_branch_credits_nothing() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        let point = undo.save_point();

        undo.undo(&mut lines);
        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "z".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcz".into());

        undo.mark_saved_at(point);
        assert!(!undo.is_at_saved());
        undo.undo(&mut lines);
        assert!(!undo.is_at_saved(), "and no position on the stack is it");
        undo.redo(&mut lines);
        assert!(!undo.is_at_saved());
    }

    #[test]
    fn mark_saved_then_undo_redo_back() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        undo.mark_saved();
        assert!(undo.is_at_saved());

        undo.break_coalesce();
        undo.push_text(0, 4, "".into(), "e".into(), c(0, 4), c(0, 5));
        lines[0] = DocLine::Text("abcde".into());
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(!undo.is_at_saved());

        undo.redo(&mut lines);
        assert!(undo.is_at_saved());
    }

    #[test]
    fn edit_after_save_does_not_coalesce_across_saved_position() {
        let mut undo = UndoStack::new();

        undo.push_text(0, 0, "".into(), "a".into(), c(0, 0), c(0, 1));
        undo.mark_saved();
        assert!(undo.is_at_saved());

        // This is adjacent and immediate, so it would normally coalesce with
        // the preceding insertion.
        undo.push_text(0, 1, "".into(), "b".into(), c(0, 1), c(0, 2));

        assert!(!undo.is_at_saved());
        assert_eq!(undo.position, 2);
    }

    #[test]
    fn truncate_invalidates_saved_position() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        undo.break_coalesce();
        undo.push_text(0, 4, "".into(), "e".into(), c(0, 4), c(0, 5));
        lines[0] = DocLine::Text("abcde".into());

        undo.mark_saved();
        assert!(undo.is_at_saved());

        undo.undo(&mut lines);
        undo.undo(&mut lines);

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "x".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcx".into());

        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(!undo.is_at_saved());
    }

    #[test]
    fn truncate_preserves_saved_at_zero() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());

        undo.break_coalesce();
        undo.push_text(0, 3, "".into(), "x".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcx".into());
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());
    }

    #[test]
    fn coalesced_text_undo_restores_saved() {
        let mut lines = vec![text("ab")];
        let mut undo = UndoStack::new();

        // Type "c" then "d" fast (coalesced into one entry)
        undo.push_text(0, 2, "".into(), "c".into(), c(0, 2), c(0, 3));
        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());
        assert_eq!(undo.position, 1);

        undo.undo(&mut lines);
        assert_eq!(lines[0], text("ab"));
        assert!(undo.is_at_saved());
    }

    #[test]
    fn coalesced_pixel_undo_restores_saved() {
        let mut lines = vec![grid(2, 2)];
        let mut undo = UndoStack::new();

        let s0 = PixelShape::EMPTY;
        let s1 = PixelShape::new(0, true);
        let s2 = PixelShape(1);

        undo.push_pixel(
            0,
            PixelChange {
                row: 0,
                col: 0,
                old: s0,
                new: s1,
            },
            c(0, 0),
            c(0, 0),
        );
        undo.push_pixel(
            0,
            PixelChange {
                row: 0,
                col: 1,
                old: s0,
                new: s2,
            },
            c(0, 0),
            c(0, 0),
        );
        assert_eq!(undo.position, 1);

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());
    }

    #[test]
    fn mixed_ops_undo_all_restores_saved() {
        let mut lines = vec![text("hello"), grid(2, 2), text("world")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 5, "".into(), "!".into(), c(0, 5), c(0, 6));
        lines[0] = DocLine::Text("hello!".into());

        undo.break_coalesce();
        let s0 = PixelShape::EMPTY;
        let s1 = PixelShape::new(0, true);
        undo.push_pixel(
            1,
            PixelChange {
                row: 0,
                col: 0,
                old: s0,
                new: s1,
            },
            c(1, 0),
            c(1, 0),
        );
        if let DocLine::Grid(g) = &mut lines[1] {
            g.set(0, 0, s1);
        }

        undo.push_lines(3, vec![], vec![text("extra")], c(2, 5), c(3, 0));
        lines.push(text("extra"));

        assert!(!undo.is_at_saved());

        undo.undo(&mut lines); // undo lines insert
        undo.undo(&mut lines); // undo pixel
        undo.undo(&mut lines); // undo text

        assert!(undo.is_at_saved());
        assert_eq!(lines[0], text("hello"));
        if let DocLine::Grid(g) = &lines[1] {
            assert_eq!(g.get(0, 0), s0);
        }
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn undo_redo_undo_cycle_restores_saved() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();

        undo.push_text(0, 3, "".into(), "d".into(), c(0, 3), c(0, 4));
        lines[0] = DocLine::Text("abcd".into());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());

        undo.redo(&mut lines);
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());
    }

    #[test]
    fn save_at_middle_then_full_cycle() {
        let mut lines = vec![text("a")];
        let mut undo = UndoStack::new();

        undo.break_coalesce();
        undo.push_text(0, 1, "".into(), "b".into(), c(0, 1), c(0, 2));
        lines[0] = DocLine::Text("ab".into());

        undo.mark_saved();

        undo.break_coalesce();
        undo.push_text(0, 2, "".into(), "c".into(), c(0, 2), c(0, 3));
        lines[0] = DocLine::Text("abc".into());
        assert!(!undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(undo.is_at_saved());

        undo.undo(&mut lines);
        assert!(!undo.is_at_saved());

        undo.redo(&mut lines);
        assert!(undo.is_at_saved());

        undo.redo(&mut lines);
        assert!(!undo.is_at_saved());
    }

    #[test]
    fn compound_undo_redo_applies_non_line_ops() {
        // A compound entry may nest any op kind; text ops inside it must
        // participate in undo/redo like top-level ones, in reverse order on
        // undo and forward order on redo.
        let mut lines = vec![text("ab"), text("cd")];
        let mut undo = UndoStack::new();

        undo.push_compound(
            vec![
                UndoOp::Text {
                    line: 0,
                    col: 2,
                    old: "".into(),
                    new: "X".into(),
                },
                UndoOp::Lines {
                    at: 1,
                    old: vec![text("cd")],
                    new: vec![text("c"), text("d")],
                },
            ],
            c(0, 2),
            c(1, 1),
        );
        lines[0] = text("abX");
        lines.splice(1..2, vec![text("c"), text("d")]);

        undo.undo(&mut lines);
        assert_eq!(lines, vec![text("ab"), text("cd")]);

        undo.redo(&mut lines);
        assert_eq!(lines, vec![text("abX"), text("c"), text("d")]);
    }

    #[test]
    fn noop_push_does_not_affect_saved() {
        let mut undo = UndoStack::new();
        assert!(undo.is_at_saved());

        // push_text with old == new is a no-op
        undo.push_text(0, 0, "x".into(), "x".into(), c(0, 0), c(0, 0));
        assert!(undo.is_at_saved());

        // push_pixel with old == new is a no-op
        let s = PixelShape::EMPTY;
        undo.push_pixel(
            0,
            PixelChange {
                row: 0,
                col: 0,
                old: s,
                new: s,
            },
            c(0, 0),
            c(0, 0),
        );
        assert!(undo.is_at_saved());
    }
}
