use crate::document::{DocLine, PixelGrid};
use crate::document_io::{encode_grid_row, glyph_header_dims, tokenize_tokens};
use crate::editor::caret::Caret;
use crate::editor::undo::UndoStack;

/// Parse a `glyph NAME W H` header line and return its pixel dimensions,
/// or `None` if the line isn't a header that expects pixel rows (e.g.
/// `glyph NAME = ALIAS` or `glyph NAME` with no dimensions).
pub fn parse_glyph_header_dims(s: &str) -> Option<(u16, u16)> {
    let trimmed = s.trim();
    let tokens = tokenize_tokens(trimmed).ok()?;
    if tokens.first().is_none_or(|t| t != "glyph") {
        return None;
    }
    let dims = glyph_header_dims(&tokens[1..])?;
    Some((dims.width, dims.height))
}

pub fn reconcile(
    lines: &mut Vec<DocLine>,
    undo: &mut UndoStack,
    caret: Caret,
) -> Option<Caret> {
    // Pass A: header/grid mismatch — resize or create
    for i in 0..lines.len() {
        if let DocLine::Text(t) = &lines[i]
            && let Some((w, h)) = parse_glyph_header_dims(t) {
                match lines.get(i + 1) {
                    Some(DocLine::Grid(g)) if g.width == w && g.height == h => {
                        // Dimensions match — nothing to do
                    }
                    Some(DocLine::Grid(g)) => {
                        // Resize
                        let old_grid = g.clone();
                        let mut resized = old_grid.clone();
                        resized.resize(w, h);
                        undo.break_coalesce();
                        undo.push_derived_lines(
                            i + 1,
                            vec![DocLine::Grid(old_grid)],
                            vec![DocLine::Grid(resized.clone())],
                            caret,
                            caret,
                        );
                        undo.break_coalesce();
                        lines[i + 1] = DocLine::Grid(resized);
                        return Some(caret);
                    }
                    _ => {
                        // No grid follows — insert empty
                        let empty = PixelGrid::new(w, h);
                        let caret_after = caret_after_splice(caret, i + 1, 0, 1);
                        undo.break_coalesce();
                        undo.push_derived_lines(
                            i + 1,
                            vec![],
                            vec![DocLine::Grid(empty.clone())],
                            caret,
                            caret_after,
                        );
                        undo.break_coalesce();
                        lines.insert(i + 1, DocLine::Grid(empty));
                        return Some(caret_after);
                    }
                }
            }
    }

    // Pass B: orphaned grid demotion
    for i in 0..lines.len() {
        if let DocLine::Grid(g) = &lines[i] {
            let valid_header = i > 0
                && matches!(&lines[i - 1], DocLine::Text(t)
                    if parse_glyph_header_dims(t).is_some());

            if !valid_header {
                let rows: Vec<DocLine> = (0..g.height)
                    .map(|r| DocLine::Text(encode_grid_row(g, r)))
                    .collect();
                let caret_after = caret_after_splice(caret, i, 1, rows.len());
                undo.break_coalesce();
                undo.push_derived_lines(i, vec![lines[i].clone()], rows.clone(), caret, caret_after);
                undo.break_coalesce();
                lines.splice(i..=i, rows);
                return Some(caret_after);
            }
        }
    }

    None
}

/// Keep a caret attached to the same logical content when a line range is
/// replaced. `old_len == 0` represents an insertion.
fn caret_after_splice(caret: Caret, at: usize, old_len: usize, new_len: usize) -> Caret {
    if caret.line < at {
        return caret;
    }

    let old_end = at.saturating_add(old_len);
    if caret.line >= old_end {
        let line = if new_len >= old_len {
            caret.line.saturating_add(new_len - old_len)
        } else {
            caret.line.saturating_sub(old_len - new_len)
        };
        return Caret::new(line, caret.col);
    }

    // The caret was inside the replaced range. A grid caret always has
    // column zero; map it to the corresponding replacement line, or to the
    // splice boundary when the range was removed entirely.
    let relative = caret.line - at;
    let line = if new_len == 0 {
        at
    } else {
        at + relative.min(new_len - 1)
    };
    Caret::new(line, caret.col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::PixelShape;

    fn text(s: &str) -> DocLine { DocLine::Text(s.to_string()) }
    fn grid(w: u16, h: u16) -> DocLine { DocLine::Grid(PixelGrid::new(w, h)) }
    fn c(line: usize, col: usize) -> Caret { Caret::new(line, col) }

    #[test]
    fn parse_header_dims_basic() {
        assert_eq!(parse_glyph_header_dims("glyph foo 8 16"), Some((8, 16)));
        assert_eq!(parse_glyph_header_dims("glyph foo 8 16 sticky"), Some((8, 16)));
        assert_eq!(parse_glyph_header_dims("glyph uni0041 = test"), None);
        assert_eq!(parse_glyph_header_dims("meta height 16"), None);
        assert_eq!(parse_glyph_header_dims("// comment"), None);
        assert_eq!(parse_glyph_header_dims("glyph composite"), None);
    }

    #[test]
    fn reconcile_matching_dims_noop() {
        let mut lines = vec![text("glyph foo 2 2"), grid(2, 2)];
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_none());
    }

    #[test]
    fn reconcile_resize_grid() {
        let mut lines = vec![text("glyph foo 4 3"), grid(2, 2)];
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_some());

        let g = lines[1].as_grid().unwrap();
        assert_eq!(g.width, 4);
        assert_eq!(g.height, 3);
    }

    #[test]
    fn reconcile_resize_preserves_pixels() {
        let mut g = PixelGrid::new(2, 2);
        g.set(0, 0, PixelShape::new(0, true));
        g.set(1, 1, PixelShape::new(0, true));

        let mut lines = vec![text("glyph foo 3 3"), DocLine::Grid(g)];
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_some());

        let g = lines[1].as_grid().unwrap();
        assert_eq!(g.width, 3);
        assert_eq!(g.height, 3);
        assert_eq!(g.get(0, 0), PixelShape::new(0, true));
        assert_eq!(g.get(1, 1), PixelShape::new(0, true));
        assert_eq!(g.get(2, 2), PixelShape::EMPTY);
    }

    #[test]
    fn reconcile_create_empty_grid() {
        let mut lines = vec![text("glyph foo 4 3"), text("// next")];
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_some());

        assert_eq!(lines.len(), 3);
        let g = lines[1].as_grid().unwrap();
        assert_eq!(g.width, 4);
        assert_eq!(g.height, 3);
        assert_eq!(lines[2], text("// next"));
    }

    #[test]
    fn reconcile_create_grid_at_end() {
        let mut lines = vec![text("glyph bar 2 1")];
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_some());
        assert_eq!(lines.len(), 2);
        let g = lines[1].as_grid().unwrap();
        assert_eq!(g.width, 2);
        assert_eq!(g.height, 1);
    }

    #[test]
    fn reconcile_insert_keeps_caret_on_following_content() {
        let mut lines = vec![
            text("glyph foo 2 1"),
            text("// first"),
            text("// caret stays here"),
        ];
        let mut undo = UndoStack::new();

        let caret_after = reconcile(&mut lines, &mut undo, c(2, 4)).unwrap();
        assert_eq!(caret_after, c(3, 4));
        assert_eq!(lines[3], text("// caret stays here"));

        let undo_caret = undo.undo(&mut lines).unwrap();
        assert_eq!(undo_caret, c(2, 4));
        assert_eq!(lines[2], text("// caret stays here"));

        let redo_caret = undo.redo(&mut lines).unwrap();
        assert_eq!(redo_caret, c(3, 4));
        assert_eq!(lines[3], text("// caret stays here"));
    }

    #[test]
    fn reconcile_demote_orphan_grid() {
        let mut g = PixelGrid::new(2, 1);
        g.set(0, 0, PixelShape::new(0, true));

        let mut lines = vec![text("// not a header"), DocLine::Grid(g)];
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_some());

        assert_eq!(lines.len(), 2);
        assert!(matches!(&lines[1], DocLine::Text(s) if s == "__.."));
    }

    #[test]
    fn reconcile_demote_multi_row_grid() {
        let mut g = PixelGrid::new(2, 2);
        g.set(0, 0, PixelShape::new(0, true));
        g.set(1, 1, PixelShape::new(0, true));

        let mut lines = vec![DocLine::Grid(g)]; // orphan at line 0
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_some());

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], text("__.."));
        assert_eq!(lines[1], text("..__"));
    }

    #[test]
    fn reconcile_demote_keeps_caret_on_following_content() {
        let mut g = PixelGrid::new(2, 2);
        g.set(0, 0, PixelShape::new(0, true));
        let mut lines = vec![
            text("// not a header"),
            DocLine::Grid(g),
            text("// caret stays here"),
        ];
        let mut undo = UndoStack::new();

        let caret_after = reconcile(&mut lines, &mut undo, c(2, 4)).unwrap();
        assert_eq!(caret_after, c(3, 4));
        assert_eq!(lines[3], text("// caret stays here"));

        let undo_caret = undo.undo(&mut lines).unwrap();
        assert_eq!(undo_caret, c(2, 4));
        assert_eq!(lines[2], text("// caret stays here"));

        let redo_caret = undo.redo(&mut lines).unwrap();
        assert_eq!(redo_caret, c(3, 4));
        assert_eq!(lines[3], text("// caret stays here"));
    }

    #[test]
    fn reconcile_alias_no_grid() {
        let mut lines = vec![text("glyph uni0041 = test")];
        let mut undo = UndoStack::new();
        assert!(reconcile(&mut lines, &mut undo, c(0, 0)).is_none());
    }

    #[test]
    fn reconcile_undo_resize() {
        let original_grid = grid(2, 2);
        let mut lines = vec![text("glyph foo 4 3"), original_grid.clone()];
        let mut undo = UndoStack::new();

        let _ = reconcile(&mut lines, &mut undo, c(0, 0));
        let g = lines[1].as_grid().unwrap();
        assert_eq!((g.width, g.height), (4, 3));

        let _ = undo.undo(&mut lines);
        assert_eq!(lines[1], original_grid);
    }

    #[test]
    fn reconcile_undo_create() {
        let mut lines = vec![text("glyph foo 2 2"), text("next")];
        let mut undo = UndoStack::new();

        let _ = reconcile(&mut lines, &mut undo, c(0, 0));
        assert_eq!(lines.len(), 3);

        let _ = undo.undo(&mut lines);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], text("next"));
    }

    #[test]
    fn reconcile_undo_demote() {
        let mut g = PixelGrid::new(2, 1);
        g.set(0, 0, PixelShape::new(0, true));
        let original = DocLine::Grid(g);

        let mut lines = vec![text("// comment"), original.clone()];
        let mut undo = UndoStack::new();

        let _ = reconcile(&mut lines, &mut undo, c(0, 0));
        assert!(matches!(&lines[1], DocLine::Text(_)));

        let _ = undo.undo(&mut lines);
        assert_eq!(lines[1], original);
    }

    #[test]
    fn reconcile_loop_converges() {
        let mut lines = vec![
            text("glyph foo 2 2"),
            // Missing grid — should be created
            text("glyph bar 3 1"),
            // Missing grid — should be created
        ];
        let mut undo = UndoStack::new();

        let mut iters = 0;
        while reconcile(&mut lines, &mut undo, c(0, 0)).is_some() {
            iters += 1;
            assert!(iters < 10, "reconcile didn't converge");
        }

        // Should have inserted two grids
        assert!(matches!(lines[1], DocLine::Grid(_)));
        assert!(matches!(lines[3], DocLine::Grid(_)));
    }
}
