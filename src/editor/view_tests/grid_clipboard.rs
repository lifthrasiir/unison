//! Copy, cut, delete and select-all with nothing framed, over the pixel
//! grid rather than over text.

use super::*;

fn make_glyph_edit_harness() -> EditorHarness {
    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0); // enter GlyphEdit
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "should be in GlyphEdit"
    );
    h
}

#[track_caller]
fn assert_grid_empty(h: &EditorHarness) {
    let grid = h.grid(1);
    for r in 0..grid.height {
        for c in 0..grid.width {
            assert!(grid.get(r, c).is_clear(), "cell {r},{c} should be empty");
        }
    }
}

#[test]
fn copy_without_selection_copies_whole_grid_in_pixel_select() {
    let mut h = make_pixel_select_harness();
    assert!(h.state.pixel_selection.is_none());
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some(WHOLE_GRID_TEXT));
    assert!(
        h.state.pixel_selection.is_none(),
        "an implicit copy should not leave a selection behind"
    );
}

#[test]
fn copy_without_selection_copies_whole_grid_in_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some(WHOLE_GRID_TEXT));
}

#[test]
fn cut_without_selection_takes_whole_grid_in_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.cut();
    assert_eq!(h.last_copied_text.as_deref(), Some(WHOLE_GRID_TEXT));
    assert_grid_empty(&h);
}

#[test]
fn delete_without_selection_clears_whole_grid() {
    let mut h = make_pixel_select_harness();
    h.key(Key::Delete);
    assert_grid_empty(&h);

    // ...and it is one undo step.
    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert!(h.grid(1).get(0, 0).is_bitmap_filled());
}

#[test]
fn delete_without_selection_clears_whole_grid_in_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.key(Key::Delete);
    assert_grid_empty(&h);
}

#[test]
fn copy_with_selection_is_unaffected() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some("@@@@"));
}

#[test]
fn ctrl_a_selects_the_whole_grid_from_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.key_mod(Key::A, Modifiers::COMMAND);
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0, .. }),
        "Ctrl+A should enter PixelSelect, got {:?}",
        h.state.mode
    );
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 4, 3));
    assert!(!sel.is_floating());
}

#[test]
fn ctrl_a_selects_the_whole_grid_from_pixel_select() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.key_mod(Key::A, Modifiers::COMMAND);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 4, 3));
}

#[test]
fn ctrl_a_does_not_select_text_in_a_pixel_mode() {
    let mut h = make_pixel_select_harness();
    h.key_mod(Key::A, Modifiers::COMMAND);
    assert!(
        h.state.selection_anchor.is_none(),
        "Ctrl+A in a pixel mode must not select the document text"
    );
}

#[test]
fn shift_click_extends_the_selection_outward() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1)); // 2x1 at the top-left
    h.click_grid_cell_mod(1, 2, 3, Modifiers::SHIFT);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!(
        (sel.row, sel.col, sel.width, sel.height),
        (0, 0, 4, 3),
        "shift-click should keep the drag's start point and move the end point"
    );
    assert!(!sel.is_floating(), "extending must not float the selection");
}

#[test]
fn shift_click_inside_the_selection_shrinks_it() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (2, 3)); // the whole grid
    // A plain click here would start a move drag; with Shift the cell becomes
    // the new end point instead.
    h.click_grid_cell_mod(1, 1, 1, Modifiers::SHIFT);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 2, 2));
    assert!(!sel.is_floating());
}

#[test]
fn shift_click_extends_from_the_drag_start_not_the_corner() {
    let mut h = make_pixel_select_harness();
    // Drag upward-left: the rect is (0,0)-(1,1) but the drag *started* at (1,1).
    h.drag_grid(1, (1, 1), (0, 0));
    let sel = h.state.pixel_selection.as_ref().unwrap();
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 2, 2));

    h.click_grid_cell_mod(1, 2, 3, Modifiers::SHIFT);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!(
        (sel.row, sel.col, sel.width, sel.height),
        (1, 1, 3, 2),
        "the fixed corner should be the drag's start cell (1,1)"
    );
}

#[test]
fn plain_click_inside_the_selection_still_moves_it() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));
    h.drag_grid(1, (0, 0), (0, 2)); // plain drag from inside: move
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert!(sel.is_floating(), "a plain drag from inside still moves");
}

// The Edit menu acts through `apply_edit_action`, so these drive that entry
// point rather than the keys — the point is that both readings agree.

#[test]
fn the_edit_menu_offers_the_pixel_grid_in_a_pixel_mode() {
    let h = make_glyph_edit_harness();
    let caps = h.state.edit_menu_caps(&h.doc);
    assert!(caps.can_edit, "the menu must not be dead in a pixel mode");
    assert!(
        caps.has_selection,
        "the implicit whole-grid selection is what Cut/Copy/Delete would take"
    );

    // A ref-only glyph has no pixels to take.
    let mut h2 = EditorHarness::new("glyph part 2 1\n@@@@\nglyph whole\nref part 0 0");
    h2.click_grid_cell(1, 0, 0);
    h2.state.mode = EditMode::PixelSelect {
        item_idx: 1,
        backrefs: false,
    };
    h2.frame();
    assert!(!h2.state.edit_menu_caps(&h2.doc).has_selection);
}

#[test]
fn the_edit_menu_copy_and_delete_take_the_whole_grid() {
    use crate::edit_menu::EditAction;
    let ctx = egui::Context::default();
    let mut h = make_glyph_edit_harness();

    h.state
        .apply_edit_action(EditAction::Copy, &h.doc, &mut h.lines, &ctx);
    assert_eq!(
        ctx.output(|o| o.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })),
        Some(WHOLE_GRID_TEXT.to_string())
    );

    h.state
        .apply_edit_action(EditAction::Delete, &h.doc, &mut h.lines, &ctx);
    h.frame();
    assert_grid_empty(&h);
}

#[test]
fn the_edit_menu_select_all_frames_the_grid() {
    use crate::edit_menu::EditAction;
    let mut h = make_glyph_edit_harness();
    h.state.apply_edit_action(
        EditAction::SelectAll,
        &h.doc,
        &mut h.lines,
        &egui::Context::default(),
    );
    assert!(matches!(
        h.state.mode,
        EditMode::PixelSelect { item_idx: 0, .. }
    ));
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 4, 3));
    assert!(
        h.state.selection_anchor.is_none(),
        "it must not select the document text as well"
    );
}

// ---------------------------------------------------------------------------
// Resize modes: the box (F2) and the canvas (under the backreference shadow)
// ---------------------------------------------------------------------------
