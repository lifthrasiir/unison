//! End-to-end GUI behavior tests for the document editor.
//!
//! These drive the real `show_document` frame loop through
//! [`EditorHarness`]: synthetic keyboard/mouse input goes in, and the
//! assertions read both the editor/document state and the rendered layout
//! (visual lines, grid rows, gutter line numbers) captured per frame.

use crate::document::DocLine;
use crate::editor::EditMode;
use crate::editor::caret::Caret;
use crate::editor::harness::EditorHarness;
use egui::{Key, Modifiers};

/// glyph foo 16 16 with a filled diagonal, a blank line, then glyph bar 4 2.
///
/// DocLines: 0 header foo, 1 grid 16x16, 2 blank, 3 header bar, 4 grid 4x2.
/// Source lines: 1, 2..=17, 18, 19, 20..=21.
fn sample_doc() -> String {
    let mut s = String::from("glyph foo 16 16\n");
    for r in 0..16 {
        for c in 0..16 {
            s.push_str(if r == c { "@@" } else { ".." });
        }
        s.push('\n');
    }
    s.push('\n');
    s.push_str("glyph bar 4 2\n@@......\n......@@\n");
    s
}

fn cmd_z(h: &mut EditorHarness) {
    h.key_mod(Key::Z, Modifiers::COMMAND);
}

fn undo_all(h: &mut EditorHarness) {
    let mut guard = 0;
    while h.state.undo.can_undo() {
        cmd_z(h);
        guard += 1;
        assert!(guard < 100, "undo did not converge");
    }
}

#[test]
fn initial_layout_has_expected_gutter_and_grid_rows() {
    let h = EditorHarness::new(&sample_doc());

    assert_eq!(h.grid_row_count(1), 16);
    assert_eq!(h.grid_row_count(4), 2);
    assert_eq!(h.gutter_of(0), Some(1));
    assert_eq!(h.gutter_of(2), Some(18));
    assert_eq!(h.gutter_of(3), Some(19));
    // Every visual line carries a consecutive source line number.
    assert_eq!(h.gutter_numbers(), (1..=21).collect::<Vec<_>>());
}

#[test]
fn click_places_caret_and_focuses_editor() {
    let mut h = EditorHarness::new(&sample_doc());
    assert!(!h.state.is_active());

    h.click_text(0, 6);
    assert!(h.state.is_active());
    assert_eq!(h.cursor(), Caret::new(0, 6));

    h.click_text(3, 0);
    assert_eq!(h.cursor(), Caret::new(3, 0));

    h.click_text(0, 15); // end of "glyph foo 16 16"
    assert_eq!(h.cursor(), Caret::new(0, 15));
}

#[test]
fn arrow_keys_traverse_grid_lines() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 5);

    h.key(Key::ArrowDown);
    assert_eq!(h.cursor(), Caret::new(1, 0), "grid line clamps caret to col 0");
    h.key(Key::ArrowDown);
    assert_eq!(h.cursor(), Caret::new(2, 0));
    h.key(Key::ArrowUp);
    h.key(Key::ArrowUp);
    assert_eq!(h.cursor(), Caret::new(0, 0));

    h.key(Key::End);
    assert_eq!(h.cursor(), Caret::new(0, 15));
    h.key(Key::Home);
    assert_eq!(h.cursor(), Caret::new(0, 0));
}

#[test]
fn typing_inserts_text_and_marks_document_dirty() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(2, 0);
    assert!(!h.doc.dirty);

    h.type_text("// note");
    assert_eq!(h.text(2), "// note");
    assert_eq!(h.cursor(), Caret::new(2, 7));
    assert!(h.doc.dirty);

    undo_all(&mut h);
    assert_eq!(h.text(2), "");
    assert!(!h.doc.dirty);
}

/// The scenario from the editor's core contract: change `glyph foo 16 16`
/// to `glyph foo 16 8`, and the grid must keep its old size while the
/// header line is still being edited, then shrink (dropping overflowing
/// pixels) once the caret leaves the line — pulling all following lines up
/// and renumbering the gutter — and undo must restore all of it.
#[test]
fn header_height_edit_resizes_grid_when_caret_leaves_line() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // Caret at the end of "glyph foo 16 16"; delete "16", type "8".
    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.text(0), "glyph foo 16 8");

    // While the caret is still on the header line the resize is deferred:
    // the grid widget and the gutter numbering are unchanged.
    assert_eq!(h.grid_row_count(1), 16);
    assert_eq!(h.gutter_of(3), Some(19));
    assert_eq!(h.grid(1).height, 16);

    // Move the caret off the header line — now the grid resizes.
    h.key(Key::ArrowDown);
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (16, 8));
    assert!(!grid.get(5, 5).is_empty(), "surviving pixel kept");
    assert_eq!(h.grid_row_count(1), 8, "grid widget shrank to 8 rows");

    // All following lines moved up by 8 source lines.
    assert_eq!(h.gutter_of(2), Some(10));
    assert_eq!(h.gutter_of(3), Some(11));
    assert_eq!(h.gutter_numbers(), (1..=13).collect::<Vec<_>>());

    // Undo everything: header text, grid size, truncated pixels, gutter.
    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_eq!(h.text(0), "glyph foo 16 16");
    assert_eq!(h.grid_row_count(1), 16);
    assert!(!h.grid(1).get(12, 12).is_empty(), "truncated pixel restored");
    assert_eq!(h.gutter_of(3), Some(19));
    assert_eq!(h.gutter_numbers(), (1..=21).collect::<Vec<_>>());
}

/// Same as above, but the deferred resize is flushed by the editor losing
/// keyboard focus rather than by caret movement.
#[test]
fn header_height_edit_resizes_grid_on_focus_loss() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.grid_row_count(1), 16, "still deferred while focused");

    h.blur();
    h.frame();
    assert_eq!(h.grid(1).height, 8);
    assert_eq!(h.grid_row_count(1), 8);
    assert_eq!(h.gutter_of(3), Some(11));
}

#[test]
fn growing_header_height_expands_grid_and_gutter() {
    let mut h = EditorHarness::new(&sample_doc());

    // "glyph bar 4 2" -> "glyph bar 4 12"
    h.click_text(3, 12); // just before the "2"
    h.type_text("1");
    assert_eq!(h.text(3), "glyph bar 4 12");
    assert_eq!(h.grid_row_count(4), 2, "deferred while editing header");

    h.key(Key::ArrowUp);
    assert_eq!(h.grid(4).height, 12);
    assert_eq!(h.grid_row_count(4), 12);
    // bar's grid rows now cover source lines 20..=31.
    assert_eq!(h.gutter_numbers(), (1..=31).collect::<Vec<_>>());

    undo_all(&mut h);
    assert_eq!(h.grid(4).height, 2);
    assert_eq!(h.gutter_numbers(), (1..=21).collect::<Vec<_>>());
}

#[test]
fn clicking_grid_enters_glyph_edit_and_escape_exits() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_grid_cell(1, 5, 5);
    assert!(
        matches!(h.mode(), EditMode::GlyphEdit { item_idx: 0, .. }),
        "clicking foo's grid enters GlyphEdit for item 0, got {:?}",
        h.mode()
    );

    h.key(Key::Escape);
    assert!(matches!(h.mode(), EditMode::Normal));
}

#[test]
fn painting_a_pixel_in_glyph_edit_mode_and_undo() {
    let mut h = EditorHarness::new(&sample_doc());

    // First click enters GlyphEdit (and is suppressed as a paint).
    h.click_grid_cell(1, 2, 10);
    assert!(matches!(h.mode(), EditMode::GlyphEdit { item_idx: 0, .. }));
    assert!(h.grid(1).get(2, 10).is_empty(), "entering click must not paint");

    // Second click paints with the selected shape.
    h.click_grid_cell(1, 2, 10);
    assert!(!h.grid(1).get(2, 10).is_empty(), "click paints the cell");
    assert!(h.doc.dirty);

    cmd_z(&mut h);
    assert!(h.grid(1).get(2, 10).is_empty(), "undo clears the painted cell");
}

#[test]
fn deleting_glyph_header_demotes_grid_to_text_and_undo_restores() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // Select the whole "glyph bar 4 2" line and delete it.
    h.click_text(3, 0);
    h.key_mod(Key::End, Modifiers::SHIFT);
    assert!(h.state.selection_range().is_some());
    h.key(Key::Backspace);

    // While the caret still sits on the (now empty) header line the grid is
    // left alone — the user may be about to retype the header.
    assert_eq!(h.text(3), "");
    assert!(matches!(h.lines[4], DocLine::Grid(_)));

    // Leaving the line orphans the grid, demoting it back to two text rows.
    h.key(Key::ArrowUp);
    assert_eq!(h.text(4), "@@......");
    assert_eq!(h.text(5), "......@@");
    assert!(matches!(h.lines[4], DocLine::Text(_)));

    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_eq!(h.grid_row_count(4), 2);
}

#[test]
fn newline_shifts_following_gutter_numbers() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_text(2, 0);
    h.key(Key::Enter);
    // "glyph bar" moved from doc line 3 to 4, source line 19 -> 20.
    assert_eq!(h.text(4), "glyph bar 4 2");
    assert_eq!(h.gutter_of(4), Some(20));

    undo_all(&mut h);
    assert_eq!(h.text(3), "glyph bar 4 2");
    assert_eq!(h.gutter_of(3), Some(19));
}

#[test]
fn paste_single_line_and_undo() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(2, 0);

    h.paste("hello");
    assert_eq!(h.text(2), "hello");
    assert_eq!(h.cursor(), Caret::new(2, 5));

    cmd_z(&mut h);
    assert_eq!(h.text(2), "");
    assert_eq!(h.cursor(), Caret::new(2, 0));
}

#[test]
fn paste_multiline_undoes_atomically() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(2, 0);
    let original_lines = h.lines.clone();

    h.paste("line1\nline2\nline3");
    assert_eq!(h.text(2), "line1");
    assert_eq!(h.text(3), "line2");
    assert_eq!(h.text(4), "line3");
    assert_eq!(h.cursor(), Caret::new(4, 5));
    assert_eq!(h.lines.len(), original_lines.len() + 2);

    // Single undo must revert the entire multi-line paste at once.
    cmd_z(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_eq!(h.cursor(), Caret::new(2, 0));
}

#[test]
fn paste_multiline_into_middle_of_text_and_undo() {
    let mut h = EditorHarness::new("abc\ndef\n");
    h.click_text(0, 1); // between 'a' and 'bc'

    h.paste("X\nY\nZ");
    assert_eq!(h.text(0), "aX");
    assert_eq!(h.text(1), "Y");
    assert_eq!(h.text(2), "Zbc");
    assert_eq!(h.cursor(), Caret::new(2, 1));

    cmd_z(&mut h);
    assert_eq!(h.text(0), "abc");
    assert_eq!(h.cursor(), Caret::new(0, 1));
}

#[test]
fn paste_multiline_with_crlf_and_undo() {
    let mut h = EditorHarness::new("hello\n");
    let n = h.lines.len();
    h.click_text(0, 5);

    h.paste(" world\r\nfoo\r\nbar");
    assert_eq!(h.text(0), "hello world");
    assert_eq!(h.text(1), "foo");
    assert_eq!(h.text(2), "bar");

    cmd_z(&mut h);
    assert_eq!(h.text(0), "hello");
    assert_eq!(h.lines.len(), n);
}

#[test]
fn paste_over_selection_undoes_atomically() {
    let mut h = EditorHarness::new("abcdef\nghijkl\n");
    let original_lines = h.lines.clone();

    // Select "cdef\nghi" (from (0,2) to (1,3))
    h.click_text(0, 2);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    assert!(h.state.selection_range().is_some());

    h.paste("XY");
    assert_eq!(h.text(0), "abXYjkl");
    assert_eq!(h.lines.len(), 1);

    // Single undo must revert both the selection deletion and the paste.
    cmd_z(&mut h);
    assert_eq!(h.lines, original_lines);
}

#[test]
fn paste_multiline_over_selection_undoes_atomically() {
    let mut h = EditorHarness::new("abcdef\nghijkl\nmnopqr\n");
    let original_lines = h.lines.clone();

    // Select "cdef\nghijkl\nmno" (from (0,2) to (2,3))
    h.click_text(0, 2);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    assert!(h.state.selection_range().is_some());

    h.paste("1\n2\n3");
    assert_eq!(h.text(0), "ab1");
    assert_eq!(h.text(1), "2");
    assert_eq!(h.text(2), "3pqr");

    cmd_z(&mut h);
    assert_eq!(h.lines, original_lines);
}

#[test]
fn select_all_and_shift_arrow_selection() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 0);

    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    let (lo, hi) = h.state.selection_range().expect("selection");
    assert_eq!((lo, hi), (Caret::new(0, 0), Caret::new(0, 2)));

    // Plain arrow clears the selection.
    h.key(Key::ArrowLeft);
    assert!(h.state.selection_range().is_none());

    h.key_mod(Key::A, Modifiers::COMMAND);
    let (lo, hi) = h.state.selection_range().expect("select all");
    assert_eq!(lo, Caret::new(0, 0));
    assert_eq!(hi.line, 4, "select-all extends to the last doc line");
}

