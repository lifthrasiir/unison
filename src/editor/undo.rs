use crate::document::DocLine;
use crate::editor::caret::{char_to_byte, Caret};
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
}

#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub op: UndoOp,
    pub caret_before: Caret,
    pub caret_after: Caret,
}

pub struct UndoStack {
    entries: Vec<UndoEntry>,
    position: usize,
    last_push_time: std::time::Instant,
    saved_position: Option<usize>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            position: 0,
            last_push_time: std::time::Instant::now() - std::time::Duration::from_secs(10),
            saved_position: Some(0),
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

    pub fn is_at_saved(&self) -> bool {
        self.saved_position == Some(self.position)
    }

    fn truncate_and_invalidate(&mut self) {
        self.entries.truncate(self.position);
        if let Some(sp) = self.saved_position
            && sp > self.position {
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
                if !old_for_merge.is_empty() && new_for_merge.is_empty()
                    && col + old_for_merge.chars().count() == *prev_col
                {
                    let mut merged_old = old_for_merge.clone();
                    merged_old.push_str(prev_old);
                    *prev_old = merged_old;
                    *prev_col = col;
                    return true;
                }
                // Whole-line replace chain (ref drag): prev.new == current old
                if *prev_col == 0 && col == 0 && *prev_new == old_for_merge {
                    *prev_new = new_for_merge.clone();
                    return true;
                }
                false
            },
            move || UndoOp::Text { line, col, old, new },
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
                let UndoOp::Pixels { line: prev_line, changes } = &mut entry.op else {
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

        if elapsed < COALESCE_MS && self.position > 0
            && let Some(entry) = self.entries.get_mut(self.position - 1)
                && try_merge(entry) {
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

    pub fn can_undo(&self) -> bool {
        self.position > 0
    }

    pub fn can_redo(&self) -> bool {
        self.position < self.entries.len()
    }

    pub fn undo(&mut self, lines: &mut Vec<DocLine>) -> Option<Caret> {
        if self.position == 0 {
            return None;
        }
        self.position -= 1;
        let entry = &self.entries[self.position];
        let caret = entry.caret_before;

        match &entry.op {
            UndoOp::Text { line, col, old, new } => {
                if let Some(DocLine::Text(s)) = lines.get_mut(*line) {
                    let byte_start = char_to_byte(s, *col);
                    let byte_end = char_to_byte(s, *col + new.chars().count());
                    s.replace_range(byte_start..byte_end, old);
                }
            }
            UndoOp::Lines { at, old, new } => {
                let end = (*at + new.len()).min(lines.len());
                lines.splice(*at..end, old.iter().cloned());
            }
            UndoOp::Pixels { line, changes } => {
                if let Some(DocLine::Grid(grid)) = lines.get_mut(*line) {
                    for ch in changes.iter().rev() {
                        grid.set(ch.row, ch.col, ch.old);
                    }
                }
            }
        }
        self.break_coalesce();
        Some(caret)
    }

    pub fn redo(&mut self, lines: &mut Vec<DocLine>) -> Option<Caret> {
        if self.position >= self.entries.len() {
            return None;
        }
        let entry = &self.entries[self.position];
        let caret = entry.caret_after;

        match &entry.op {
            UndoOp::Text { line, col, old, new } => {
                if let Some(DocLine::Text(s)) = lines.get_mut(*line) {
                    let byte_start = char_to_byte(s, *col);
                    let byte_end = char_to_byte(s, *col + old.chars().count());
                    s.replace_range(byte_start..byte_end, new);
                }
            }
            UndoOp::Lines { at, old, new } => {
                let end = (*at + old.len()).min(lines.len());
                lines.splice(*at..end, new.iter().cloned());
            }
            UndoOp::Pixels { line, changes } => {
                if let Some(DocLine::Grid(grid)) = lines.get_mut(*line) {
                    for ch in changes {
                        grid.set(ch.row, ch.col, ch.new);
                    }
                }
            }
        }
        self.position += 1;
        self.break_coalesce();
        Some(caret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PixelGrid;

    fn text(s: &str) -> DocLine { DocLine::Text(s.to_string()) }
    fn grid(w: u16, h: u16) -> DocLine { DocLine::Grid(PixelGrid::new(w, h)) }
    fn c(line: usize, col: usize) -> Caret { Caret::new(line, col) }

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

        undo.push_pixel(1, PixelChange { row: 0, col: 1, old, new }, c(1, 0), c(1, 0));
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

        undo.push_pixel(0, PixelChange { row: 0, col: 0, old: s1, new: s2 }, c(0, 0), c(0, 0));
        undo.push_pixel(0, PixelChange { row: 0, col: 0, old: s2, new: s3 }, c(0, 0), c(0, 0));

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

        undo.push_pixel(0, PixelChange { row: 0, col: 0, old: s0, new: s1 }, c(0, 0), c(0, 0));
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

        undo.push_pixel(0, PixelChange { row: 0, col: 0, old: s0, new: s1 }, c(0, 0), c(0, 0));
        undo.push_pixel(0, PixelChange { row: 0, col: 1, old: s0, new: s2 }, c(0, 0), c(0, 0));
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
        undo.push_pixel(1, PixelChange { row: 0, col: 0, old: s0, new: s1 }, c(1, 0), c(1, 0));
        if let DocLine::Grid(g) = &mut lines[1] { g.set(0, 0, s1); }

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
    fn noop_push_does_not_affect_saved() {
        let mut undo = UndoStack::new();
        assert!(undo.is_at_saved());

        // push_text with old == new is a no-op
        undo.push_text(0, 0, "x".into(), "x".into(), c(0, 0), c(0, 0));
        assert!(undo.is_at_saved());

        // push_pixel with old == new is a no-op
        let s = PixelShape::EMPTY;
        undo.push_pixel(0, PixelChange { row: 0, col: 0, old: s, new: s }, c(0, 0), c(0, 0));
        assert!(undo.is_at_saved());
    }
}
