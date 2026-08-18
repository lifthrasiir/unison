//! Typing into the document, and the deferred grid resize a header edit
//! schedules.

use super::*;

#[test]
fn click_then_type_places_text() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 6);
    assert_eq!(h.cursor(), Caret::new(0, 6));
    h.type_text("X");
    assert_eq!(h.text(0), "glyph Xfoo 16 16");
}

#[test]
fn typing_inserts_text_and_marks_document_dirty() {
    let mut h = EditorHarness::new(&sample_doc());
    assert!(!h.doc.dirty);

    h.click_text(0, 0);
    h.type_text("// ");
    assert_eq!(h.text(0), "// glyph foo 16 16");
    assert!(h.doc.dirty);

    undo_all(&mut h);
    assert_eq!(h.text(0), "glyph foo 16 16");
    assert!(!h.doc.dirty);
}

#[test]
fn delete_key_updates_immediately() {
    let mut h = EditorHarness::new(&sample_doc());
    // Place cursor at start of "glyph foo 16 16"
    h.click_text(0, 0);
    h.key(Key::Delete);
    assert_eq!(h.text(0), "lyph foo 16 16");
    assert!(h.doc.dirty);
}

#[test]
fn click_grid_enters_glyph_edit() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit for item_idx 0"
    );
}

#[test]
fn header_height_edit_resizes_grid_when_caret_leaves_line() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // "glyph foo 16 16" -> "glyph foo 16 8"
    // Select trailing "16" by navigating to end, backspace twice.
    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.text(0), "glyph foo 16 8");

    // Grid hasn't changed yet — resize is deferred while the caret is on the
    // header line.
    assert_eq!(
        h.grid_row_count(1),
        16,
        "grid is still 16 rows while deferred"
    );
    assert_eq!(h.gutter_of(3), Some(19), "gutter hasn't changed yet");

    // Move the caret off the header line — now the grid resizes.
    h.key(Key::ArrowDown);
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (16, 8));
    assert!(!grid.get(5, 5).is_clear(), "surviving pixel kept");
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
    assert!(
        !h.grid(1).get(12, 12).is_clear(),
        "truncated pixel restored"
    );
    assert_eq!(h.gutter_of(3), Some(19));
    assert_eq!(h.gutter_numbers(), (1..=21).collect::<Vec<_>>());
}

/// The grid resize is a consequence of the header edit, not a separate user
/// action: one undo has to take the text *and* the grid back together. Undoing
/// only the resize would leave a `18 16` header over an 8-wide grid, which the
/// reparse then renders as an empty 18-wide grid.
#[test]
fn header_dimension_edit_undoes_in_one_step() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // "glyph foo 16 16" -> "glyph foo 18 16"
    h.click_text(0, 12); // just after the width "16"
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("18");
    assert_eq!(h.text(0), "glyph foo 18 16");

    h.key(Key::ArrowDown);
    assert_eq!((h.grid(1).width, h.grid(1).height), (18, 16));

    cmd_z(&mut h);
    assert_eq!(
        h.text(0),
        "glyph foo 16 16",
        "header text restored by one undo"
    );
    assert_eq!(h.lines, original_lines, "grid restored by the same undo");
    assert_eq!(h.grid_row_count(1), 16);
    assert!(
        !h.state.undo.can_undo(),
        "no leftover undo entry for the resize"
    );

    // Redo has to bring both sides back in one step too.
    h.key_mod(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.text(0), "glyph foo 18 16");
    assert_eq!((h.grid(1).width, h.grid(1).height), (18, 16));
    assert!(
        !h.state.undo.can_redo(),
        "no leftover redo entry for the resize"
    );
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

/// Same as above, but the deferred resize is flushed by clicking straight into
/// the grid: entering a pixel mode leaves the header line just as surely as
/// moving the caret off it does.
#[test]
fn header_height_edit_resizes_grid_on_grid_click() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.grid_row_count(1), 16, "still deferred while editing");

    // Click into the grid without moving the caret off the header line first.
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit for item_idx 0"
    );
    assert_eq!(
        h.grid(1).height,
        8,
        "header edit applied on entering the grid"
    );
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
}
