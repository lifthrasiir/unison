//! The gutter's marks of what the buffer changed since it was saved; see
//! [`crate::editor::change_marks`].

use super::*;
use crate::editor::change_marks::MarkKind;
use crate::editor::document_view::LEFT_PAD;

/// The marks as painted, one frame after the input so that the frame that
/// made the edit has been followed by one drawn from it.
fn marks(h: &mut EditorHarness) -> Vec<(MarkKind, egui::Rect)> {
    h.frame();
    h.change_marks()
}

/// The top and bottom of the rows drawing buffer line `line`.
fn rows_of(h: &EditorHarness, line: usize) -> (f32, f32) {
    let rows: Vec<_> = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == line && !matches!(vl.kind, SnapKind::RefImage { .. }))
        .collect();
    let (first, last) = (rows.first().unwrap(), rows.last().unwrap());
    (first.y, last.y + last.height)
}

#[track_caller]
fn assert_covers(rect: egui::Rect, (top, bottom): (f32, f32)) {
    assert!(
        (rect.min.y - top).abs() <= 1.0 && (rect.max.y - bottom).abs() <= 1.0,
        "mark {:?} does not cover the rows {top}..{bottom}",
        rect.y_range()
    );
}

/// A bar sits in the middle of the gap between the numbers and the text.
#[track_caller]
fn assert_in_gap(h: &EditorHarness, rect: egui::Rect) {
    let text_x = h.snap().origin_x;
    assert!(
        rect.max.x < text_x + LEFT_PAD && rect.min.x > text_x - LEFT_PAD * 4.0,
        "mark {:?} is not in the gap before x = {text_x}",
        rect.x_range()
    );
}

#[test]
fn a_buffer_as_loaded_has_no_marks() {
    let mut h = EditorHarness::new(&sample_doc());
    assert!(marks(&mut h).is_empty());
}

#[test]
fn a_line_typed_into_is_modified_until_it_is_undone() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(3, 0);
    h.type_text("// ");
    let m = marks(&mut h);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].0, MarkKind::Modified);
    assert_covers(m[0].1, rows_of(&h, 3));
    assert_in_gap(&h, m[0].1);

    undo_all(&mut h);
    assert!(marks(&mut h).is_empty());
}

#[test]
fn a_line_typed_back_to_what_was_saved_is_unmarked() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(3, 0);
    h.type_text("x");
    assert_eq!(marks(&mut h).len(), 1);
    h.key(Key::Backspace);
    assert!(marks(&mut h).is_empty());
}

#[test]
fn an_inserted_line_is_added() {
    let mut h = EditorHarness::new(&sample_doc());
    // Enter at the start of a header pushes it down under a fresh line.
    h.click_text(3, 0);
    h.key(Key::Enter);
    assert_eq!(h.text(4), "glyph bar 4 2");
    let m = marks(&mut h);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].0, MarkKind::Added);
    assert_covers(m[0].1, rows_of(&h, 3));
}

#[test]
fn a_deleted_line_is_a_triangle_on_the_boundary_it_left() {
    let mut h = EditorHarness::new(&sample_doc());
    // Delete on the blank line joins the next one into it, which is the blank
    // line going.
    h.click_text(2, 0);
    h.key(Key::Delete);
    assert_eq!(h.text(2), "glyph bar 4 2");
    let m = marks(&mut h);
    assert_eq!(m.len(), 1, "{m:?}");
    let (kind, rect) = m[0];
    assert_eq!(kind, MarkKind::Deleted);
    // Between the grid above and the line below, pointing at the text and
    // flush with where the gap ends.
    let boundary = rows_of(&h, 2).0;
    assert!(
        (rect.center().y - boundary).abs() <= 1.0,
        "{rect:?} vs {boundary}"
    );
    assert!((rect.max.x - (h.snap().origin_x + LEFT_PAD)).abs() <= 0.01);
    assert!(rect.width() > 0.0 && rect.width() < rect.height());
}

/// A grid is one line of the buffer, so one changed pixel marks all of it.
#[test]
fn a_changed_grid_is_marked_along_its_whole_height() {
    let mut h = EditorHarness::new(&sample_doc());
    let before = h.grid(4).clone();
    h.click_grid_cell(4, 0, 1);
    h.click_grid_cell(4, 0, 1);
    assert_ne!(*h.grid(4), before, "the click painted a pixel");
    let m = marks(&mut h);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].0, MarkKind::Modified);
    let (top, bottom) = rows_of(&h, 4);
    assert!(
        bottom - top >= 2.0 * h.snap().grid_cell,
        "two rows at least"
    );
    assert_covers(m[0].1, (top, bottom));
}

/// A heading is taller than a row, and so is its mark.
#[test]
fn a_taller_line_has_a_taller_mark() {
    let mut h = EditorHarness::new("# title\nplain\n");
    h.click_text(0, 7);
    h.type_text("s");
    let m = marks(&mut h);
    assert_eq!(m.len(), 1, "{m:?}");
    let (top, bottom) = rows_of(&h, 0);
    assert!(bottom - top > h.snap().row_height);
    assert_covers(m[0].1, (top, bottom));
}

/// Neighbouring changed lines are one bar rather than a stack of them.
#[test]
fn adjacent_additions_are_one_bar() {
    let mut h = EditorHarness::new("a\nb\n");
    h.click_text(0, 1);
    h.key(Key::Enter);
    h.type_text("x");
    h.key(Key::Enter);
    h.type_text("y");
    let m = marks(&mut h);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].0, MarkKind::Added);
    assert_covers(m[0].1, (rows_of(&h, 1).0, rows_of(&h, 2).1));
}
