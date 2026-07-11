use crate::document::DocLine;
use crate::editor::caret::{Caret, char_to_byte};
use crate::editor::undo::UndoStack;

#[allow(clippy::ptr_arg)]
pub fn insert_str(lines: &mut Vec<DocLine>, undo: &mut UndoStack, caret: Caret, s: &str) -> Caret {
    if s.is_empty() {
        return caret;
    }
    let DocLine::Text(t) = &mut lines[caret.line] else {
        return caret;
    };

    let insert_chars = s.chars().count();
    let byte = char_to_byte(t, caret.col);
    let after = Caret::new(caret.line, caret.col + insert_chars);

    undo.push_text(
        caret.line,
        caret.col,
        String::new(),
        s.to_string(),
        caret,
        after,
    );
    t.insert_str(byte, s);
    after
}

pub fn insert_newline(lines: &mut Vec<DocLine>, undo: &mut UndoStack, caret: Caret) -> Caret {
    let DocLine::Text(t) = &lines[caret.line] else {
        return caret;
    };

    let byte = char_to_byte(t, caret.col);
    let before_str = t[..byte].to_string();
    let after_str = t[byte..].to_string();

    let old = vec![lines[caret.line].clone()];
    let new = vec![DocLine::Text(before_str), DocLine::Text(after_str)];
    let new_caret = Caret::new(caret.line + 1, 0);

    undo.push_lines(caret.line, old, new.clone(), caret, new_caret);
    lines.splice(caret.line..=caret.line, new);
    new_caret
}

pub fn backspace(lines: &mut Vec<DocLine>, undo: &mut UndoStack, caret: Caret) -> Caret {
    // Grid line with no selection: no-op
    if matches!(lines.get(caret.line), Some(DocLine::Grid(_))) {
        return caret;
    }

    if caret.col > 0 {
        let DocLine::Text(t) = &mut lines[caret.line] else {
            return caret;
        };
        let b0 = char_to_byte(t, caret.col - 1);
        let b1 = char_to_byte(t, caret.col);
        let removed = t[b0..b1].to_string();
        let new_caret = Caret::new(caret.line, caret.col - 1);
        undo.push_text(
            caret.line,
            caret.col - 1,
            removed,
            String::new(),
            caret,
            new_caret,
        );
        t.replace_range(b0..b1, "");
        return new_caret;
    }

    // col == 0, need to join with previous line or move onto grid
    if caret.line == 0 {
        return caret;
    }

    match &lines[caret.line - 1] {
        DocLine::Grid(_) => {
            // Just move caret onto the grid (select it), no edit
            Caret::new(caret.line - 1, 0)
        }
        DocLine::Text(prev) => {
            let join_col = prev.chars().count();
            let cur = lines[caret.line].as_text().unwrap().to_string();
            let merged = format!("{prev}{cur}");
            let old = vec![lines[caret.line - 1].clone(), lines[caret.line].clone()];
            let new = vec![DocLine::Text(merged)];
            let new_caret = Caret::new(caret.line - 1, join_col);
            undo.push_lines(caret.line - 1, old, new.clone(), caret, new_caret);
            lines.splice(caret.line - 1..=caret.line, new);
            new_caret
        }
    }
}

pub fn delete(lines: &mut Vec<DocLine>, undo: &mut UndoStack, caret: Caret) -> Caret {
    // Grid line with no selection: no-op
    if matches!(lines.get(caret.line), Some(DocLine::Grid(_))) {
        return caret;
    }

    if let Some(DocLine::Text(t)) = lines.get(caret.line) {
        let len = t.chars().count();
        if caret.col < len {
            let t = lines[caret.line].as_text_mut().unwrap();
            let b0 = char_to_byte(t, caret.col);
            let b1 = char_to_byte(t, caret.col + 1);
            let removed = t[b0..b1].to_string();
            undo.push_text(caret.line, caret.col, removed, String::new(), caret, caret);
            t.replace_range(b0..b1, "");
            return caret;
        }
    }

    // At end of text line, need to join with next line or skip grid
    if caret.line + 1 >= lines.len() {
        return caret;
    }

    match &lines[caret.line + 1] {
        DocLine::Grid(_) => {
            // Move caret onto grid (select it), no edit
            Caret::new(caret.line + 1, 0)
        }
        DocLine::Text(next) => {
            let cur = lines[caret.line].as_text().unwrap().to_string();
            let merged = format!("{cur}{next}");
            let old = vec![lines[caret.line].clone(), lines[caret.line + 1].clone()];
            let new = vec![DocLine::Text(merged)];
            undo.push_lines(caret.line, old, new.clone(), caret, caret);
            lines.splice(caret.line..=caret.line + 1, new);
            caret
        }
    }
}

pub fn delete_selection(
    lines: &mut Vec<DocLine>,
    undo: &mut UndoStack,
    cursor: Caret,
    anchor: Caret,
) -> Caret {
    let lo = cursor.min(anchor);
    let hi = cursor.max(anchor);

    if lo == hi {
        return lo;
    }

    if lo.line == hi.line {
        // Same line
        if matches!(lines.get(lo.line), Some(DocLine::Grid(_))) {
            // Grid: selection within a grid is always the whole grid.
            // Delete the grid line entirely.
            let old = vec![lines[lo.line].clone()];
            undo.push_lines(lo.line, old, vec![], lo, lo);
            lines.remove(lo.line);
            return Caret::new(lo.line.min(lines.len().saturating_sub(1)), 0);
        }
        let DocLine::Text(t) = &mut lines[lo.line] else {
            return lo;
        };
        let b0 = char_to_byte(t, lo.col);
        let b1 = char_to_byte(t, hi.col);
        let removed = t[b0..b1].to_string();
        undo.push_text(lo.line, lo.col, removed, String::new(), cursor, lo);
        t.replace_range(b0..b1, "");
        return lo;
    }

    // Multi-line selection
    let prefix = match &lines[lo.line] {
        DocLine::Text(s) => {
            let byte = char_to_byte(s, lo.col);
            s[..byte].to_string()
        }
        DocLine::Grid(_) => String::new(),
    };

    let suffix = match &lines[hi.line] {
        DocLine::Text(s) => {
            let byte = char_to_byte(s, hi.col);
            s[byte..].to_string()
        }
        DocLine::Grid(_) => String::new(),
    };

    let old: Vec<DocLine> = lines[lo.line..=hi.line].to_vec();
    let merged = format!("{prefix}{suffix}");
    let new = vec![DocLine::Text(merged)];
    let new_caret = Caret::new(lo.line, lo.col);

    undo.push_lines(lo.line, old, new.clone(), cursor, new_caret);
    lines.splice(lo.line..=hi.line, new);
    new_caret
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
    fn insert_into_text() {
        let mut lines = vec![text("ac")];
        let mut undo = UndoStack::new();
        let caret = insert_str(&mut lines, &mut undo, c(0, 1), "b");
        assert_eq!(lines[0], text("abc"));
        assert_eq!(caret, c(0, 2));
    }

    #[test]
    fn insert_on_grid_is_noop() {
        let mut lines = vec![grid(2, 2)];
        let mut undo = UndoStack::new();
        let caret = insert_str(&mut lines, &mut undo, c(0, 0), "x");
        assert_eq!(caret, c(0, 0));
        assert!(matches!(lines[0], DocLine::Grid(_)));
    }

    #[test]
    fn insert_newline_splits() {
        let mut lines = vec![text("abcd")];
        let mut undo = UndoStack::new();
        let caret = insert_newline(&mut lines, &mut undo, c(0, 2));
        assert_eq!(lines, vec![text("ab"), text("cd")]);
        assert_eq!(caret, c(1, 0));
    }

    #[test]
    fn insert_newline_at_start() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();
        let caret = insert_newline(&mut lines, &mut undo, c(0, 0));
        assert_eq!(lines, vec![text(""), text("abc")]);
        assert_eq!(caret, c(1, 0));
    }

    #[test]
    fn insert_newline_at_end() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();
        let caret = insert_newline(&mut lines, &mut undo, c(0, 3));
        assert_eq!(lines, vec![text("abc"), text("")]);
        assert_eq!(caret, c(1, 0));
    }

    #[test]
    fn insert_newline_on_grid_is_noop() {
        let mut lines = vec![grid(2, 2)];
        let mut undo = UndoStack::new();
        let caret = insert_newline(&mut lines, &mut undo, c(0, 0));
        assert_eq!(caret, c(0, 0));
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn backspace_within_text() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();
        let caret = backspace(&mut lines, &mut undo, c(0, 2));
        assert_eq!(lines[0], text("ac"));
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn backspace_joins_text_lines() {
        let mut lines = vec![text("ab"), text("cd")];
        let mut undo = UndoStack::new();
        let caret = backspace(&mut lines, &mut undo, c(1, 0));
        assert_eq!(lines, vec![text("abcd")]);
        assert_eq!(caret, c(0, 2));
    }

    #[test]
    fn backspace_before_grid_selects_grid() {
        let mut lines = vec![text("ab"), grid(2, 2), text("cd")];
        let mut undo = UndoStack::new();
        let caret = backspace(&mut lines, &mut undo, c(2, 0));
        // Should just move caret to grid, not delete anything
        assert_eq!(caret, c(1, 0));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn backspace_on_grid_is_noop() {
        let mut lines = vec![text("ab"), grid(2, 2), text("cd")];
        let mut undo = UndoStack::new();
        let caret = backspace(&mut lines, &mut undo, c(1, 0));
        assert_eq!(caret, c(1, 0));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn backspace_at_start_is_noop() {
        let mut lines = vec![text("ab")];
        let mut undo = UndoStack::new();
        let caret = backspace(&mut lines, &mut undo, c(0, 0));
        assert_eq!(caret, c(0, 0));
    }

    #[test]
    fn delete_within_text() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();
        let caret = delete(&mut lines, &mut undo, c(0, 1));
        assert_eq!(lines[0], text("ac"));
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn delete_joins_text_lines() {
        let mut lines = vec![text("ab"), text("cd")];
        let mut undo = UndoStack::new();
        let caret = delete(&mut lines, &mut undo, c(0, 2));
        assert_eq!(lines, vec![text("abcd")]);
        assert_eq!(caret, c(0, 2));
    }

    #[test]
    fn delete_at_end_before_grid_selects_grid() {
        let mut lines = vec![text("ab"), grid(2, 2), text("cd")];
        let mut undo = UndoStack::new();
        let caret = delete(&mut lines, &mut undo, c(0, 2));
        assert_eq!(caret, c(1, 0));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn delete_on_grid_is_noop() {
        let mut lines = vec![text("ab"), grid(2, 2), text("cd")];
        let mut undo = UndoStack::new();
        let caret = delete(&mut lines, &mut undo, c(1, 0));
        assert_eq!(caret, c(1, 0));
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn delete_selection_same_line() {
        let mut lines = vec![text("abcde")];
        let mut undo = UndoStack::new();
        let caret = delete_selection(&mut lines, &mut undo, c(0, 4), c(0, 1));
        assert_eq!(lines[0], text("ae"));
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn delete_selection_multi_line() {
        let mut lines = vec![text("abc"), text("def"), text("ghi")];
        let mut undo = UndoStack::new();
        let caret = delete_selection(&mut lines, &mut undo, c(0, 1), c(2, 2));
        assert_eq!(lines, vec![text("ai")]);
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn delete_selection_spanning_grid() {
        let mut lines = vec![text("abc"), grid(2, 2), text("def")];
        let mut undo = UndoStack::new();
        let caret = delete_selection(&mut lines, &mut undo, c(0, 1), c(2, 2));
        assert_eq!(lines, vec![text("af")]);
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn delete_selection_just_grid() {
        let mut lines = vec![text("ab"), grid(2, 2), text("cd")];
        let mut undo = UndoStack::new();
        let caret = delete_selection(&mut lines, &mut undo, c(1, 0), c(1, 0));
        // lo == hi, no-op
        assert_eq!(caret, c(1, 0));
        assert_eq!(lines.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Undo round-trip tests for editing operations
    // -----------------------------------------------------------------------

    #[test]
    fn undo_insert_char() {
        let mut lines = vec![text("ac")];
        let mut undo = UndoStack::new();
        let _ = insert_str(&mut lines, &mut undo, c(0, 1), "b");
        assert_eq!(lines[0], text("abc"));
        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines[0], text("ac"));
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn undo_insert_newline() {
        let mut lines = vec![text("abcd")];
        let mut undo = UndoStack::new();
        let _ = insert_newline(&mut lines, &mut undo, c(0, 2));
        assert_eq!(lines, vec![text("ab"), text("cd")]);
        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("abcd")]);
        assert_eq!(caret, c(0, 2));
    }

    #[test]
    fn undo_backspace() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();
        let _ = backspace(&mut lines, &mut undo, c(0, 2));
        assert_eq!(lines[0], text("ac"));
        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines[0], text("abc"));
        assert_eq!(caret, c(0, 2));
    }

    #[test]
    fn undo_backspace_join() {
        let mut lines = vec![text("ab"), text("cd")];
        let mut undo = UndoStack::new();
        let _ = backspace(&mut lines, &mut undo, c(1, 0));
        assert_eq!(lines, vec![text("abcd")]);
        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("ab"), text("cd")]);
        assert_eq!(caret, c(1, 0));
    }

    #[test]
    fn undo_delete() {
        let mut lines = vec![text("abc")];
        let mut undo = UndoStack::new();
        let _ = delete(&mut lines, &mut undo, c(0, 1));
        assert_eq!(lines[0], text("ac"));
        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines[0], text("abc"));
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn undo_delete_selection_multiline() {
        let original = vec![text("abc"), text("def"), text("ghi")];
        let mut lines = original.clone();
        let mut undo = UndoStack::new();
        let _ = delete_selection(&mut lines, &mut undo, c(0, 1), c(2, 2));
        assert_eq!(lines, vec![text("ai")]);
        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, original);
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn undo_delete_selection_spanning_grid() {
        let original = vec![text("abc"), grid(2, 2), text("def")];
        let mut lines = original.clone();
        let mut undo = UndoStack::new();
        let _ = delete_selection(&mut lines, &mut undo, c(0, 1), c(2, 2));
        assert_eq!(lines, vec![text("af")]);
        let caret = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, original);
        assert_eq!(caret, c(0, 1));
    }

    #[test]
    fn undo_redo_full_cycle() {
        let mut lines = vec![text("hello")];
        let mut undo = UndoStack::new();

        // Type " world"
        undo.break_coalesce();
        let c1 = insert_str(&mut lines, &mut undo, c(0, 5), " world");
        assert_eq!(lines[0], text("hello world"));
        assert_eq!(c1, c(0, 11));

        // Enter
        let c2 = insert_newline(&mut lines, &mut undo, c1);
        assert_eq!(lines, vec![text("hello world"), text("")]);
        assert_eq!(c2, c(1, 0));

        // Undo enter
        let c3 = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("hello world")]);
        assert_eq!(c3, c(0, 11));

        // Undo " world"
        let c4 = undo.undo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("hello")]);
        assert_eq!(c4, c(0, 5));

        // Redo " world"
        let c5 = undo.redo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("hello world")]);
        assert_eq!(c5, c(0, 11));

        // Redo enter
        let c6 = undo.redo(&mut lines).unwrap();
        assert_eq!(lines, vec![text("hello world"), text("")]);
        assert_eq!(c6, c(1, 0));
    }
}
