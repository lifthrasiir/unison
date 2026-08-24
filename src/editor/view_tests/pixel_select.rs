//! Pixel selection: framing, moving, floating and committing, and the
//! clipboard over a pixel grid.

use super::*;

/// Right-click erases to nothing; shift-right-click erases to a *hardblank*,
/// which is the same nothing but stays in the file as `$$`.
#[test]
fn shift_right_click_paints_a_hardblank() {
    let mut h = EditorHarness::new("glyph test 4 1\n@@@@@@@@");
    h.click_grid_cell(1, 0, 0); // enter GlyphEdit
    assert!(matches!(
        h.state.mode,
        EditMode::GlyphEdit { item_idx: 0, .. }
    ));
    h.frame(); // entering the mode relays out the grid; measure after it settles

    h.right_click_grid_cell_mod(1, 0, 0, Modifiers::SHIFT);
    h.right_click_grid_cell_mod(1, 0, 1, Modifiers::NONE);

    let grid = h.grid(1);
    assert!(
        grid.get(0, 0).is_hardblank(),
        "shift-right-click should leave a hardblank, got {:?}",
        grid.get(0, 0)
    );
    assert!(
        grid.get(0, 1).is_clear(),
        "a plain right-click still erases outright"
    );

    let mut out = Vec::new();
    crate::document_io::serialize_document(&h.doc, &mut out).unwrap();
    assert!(
        String::from_utf8(out).unwrap().contains("$$.."),
        "the hardblank has to survive into the file"
    );
}

#[test]
fn backtick_enters_pixel_select_and_num1_returns() {
    let mut h = make_pixel_select_harness();
    h.key(Key::Num1);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "Num1 should return to GlyphEdit"
    );
}

#[test]
fn escape_exits_pixel_select() {
    let mut h = make_pixel_select_harness();
    h.key(Key::Escape);
    assert!(
        matches!(h.state.mode, EditMode::Normal),
        "Escape should go to Normal"
    );
}

#[test]
fn drag_creates_grounded_selection() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 2, 2));
    assert!(!sel.is_floating());
}

#[test]
fn move_selection_makes_floating_and_clears_grid() {
    let mut h = make_pixel_select_harness();
    // Select the top-left 2x2 area
    h.drag_grid(1, (0, 0), (1, 1));

    // Now drag from inside the selection to move it
    h.drag_grid(1, (0, 0), (0, 2));

    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert!(sel.is_floating());
    // The original position (0,0)-(1,1) in grid should be cleared
    let grid = h.grid(1);
    assert!(
        grid.get(0, 0).is_clear(),
        "original cell should be empty after move"
    );
    assert!(grid.get(0, 1).is_clear());
}

#[test]
fn undo_move_restores_grid_and_grounded_state() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));
    h.drag_grid(1, (0, 0), (0, 2));

    // Should be floating now
    assert!(h.state.pixel_selection.as_ref().unwrap().is_floating());

    // Undo
    h.key_mod(Key::Z, Modifiers::COMMAND);
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection after undo");
    assert!(!sel.is_floating(), "should be grounded after undo");
    assert_eq!(
        (sel.row, sel.col),
        (0, 0),
        "should be back at original position"
    );

    // Grid should be restored
    let grid = h.grid(1);
    assert!(
        grid.get(0, 0).is_bitmap_filled(),
        "grid should be restored after undo"
    );
}

#[test]
fn mode_change_commits_floating_selection() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.drag_grid(1, (0, 0), (2, 0)); // move down by 2

    let sel = h.state.pixel_selection.as_ref().unwrap();
    assert!(sel.is_floating());

    // Switch to GlyphEdit (commits)
    h.key(Key::Num1);
    assert!(
        h.state.pixel_selection.is_none(),
        "selection should be cleared"
    );

    // The moved pixels should be merged into the grid at new position
    let grid = h.grid(1);
    assert!(
        grid.get(2, 0).is_bitmap_filled(),
        "moved pixel should be merged at new position"
    );
    // Original position should be empty
    assert!(
        grid.get(0, 0).is_clear(),
        "original position should be empty"
    );
}

#[test]
fn delete_grounded_fills_empty() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));

    // Delete
    h.key(Key::Delete);
    assert!(h.state.pixel_selection.is_none());

    let grid = h.grid(1);
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_clear());
    // Rest unchanged
    assert!(grid.get(0, 2).is_bitmap_filled());
}

#[test]
fn delete_floating_discards_no_merge() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.drag_grid(1, (0, 0), (2, 0)); // move down

    h.key(Key::Delete);
    assert!(h.state.pixel_selection.is_none());

    let grid = h.grid(1);
    // Original was cleared during float, and floating was discarded, so both are empty
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_clear());
    assert!(
        grid.get(2, 0).is_clear(),
        "floating pixels should not merge on delete"
    );
}

#[test]
fn undo_after_mode_change_commit_terminates() {
    // Move a selection (floating), then leave PixelSelect: reconcile commits
    // the float. Undoing that commit must not restore a floating selection
    // into a mode that cannot hold one — reconcile would commit it again,
    // pushing a fresh entry for every undo, and the stack would never drain.
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.drag_grid(1, (0, 0), (2, 0)); // move down -> floating
    h.key(Key::Num1); // back to GlyphEdit; reconcile commits the float
    assert!(h.state.pixel_selection.is_none());

    for _ in 0..10 {
        h.key_mod(Key::Z, Modifiers::COMMAND);
    }
    assert!(
        !h.state.undo.can_undo(),
        "undo must drain instead of regenerating entries"
    );

    let grid = h.grid(1);
    assert!(
        grid.get(0, 0).is_bitmap_filled(),
        "grid should be back to its original state"
    );
    assert!(grid.get(0, 1).is_bitmap_filled());
    assert!(grid.get(2, 0).is_clear());
}

/// A glyph with pixels, a ref placed by an explicit offset, a ref placed by
/// anchor matching (at `2 1`, so a wrong base position shows up), and the
/// anchor that places it — one of each thing a move-all touches.
///
/// Doc lines: 0/1 `part`, 2..4 `mark`, 5 header, 6 grid, 7/8 refs, 9 anchor.
fn make_move_all_harness() -> EditorHarness {
    let mut h = EditorHarness::new(
        "glyph part 2 2\n@@..\n....\n\
         glyph mark 2 2\n@@..\n....\nanchor -join 0 0\n\
         glyph test 4 3\n@@@@@@..\n..@@@@..\n........\n\
         ref part 1 0\nref mark\nanchor +join 2 1",
    );
    h.click_grid_cell(6, 0, 0);
    h.key(Key::Backtick);
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2, .. }),
        "should be in PixelSelect, got {:?}",
        h.state.mode
    );
    h
}

#[test]
fn cmd_drag_outside_selection_moves_all_layers() {
    let mut h = make_move_all_harness();
    // Empty bottom row, so nothing is selected there: Cmd+drag one cell right.
    h.drag_grid_mod(6, (2, 0), (2, 1), Modifiers::COMMAND);

    let grid = h.grid(6);
    assert!(grid.get(0, 0).is_clear(), "pixels should have moved right");
    assert!(grid.get(0, 1).is_bitmap_filled());
    assert!(grid.get(0, 2).is_bitmap_filled());
    assert!(grid.get(0, 3).is_bitmap_filled());
    assert!(grid.get(1, 1).is_clear());
    assert!(grid.get(1, 2).is_bitmap_filled());

    assert_eq!(
        h.text(7).trim(),
        "ref part 2 0",
        "explicit ref offset shifts"
    );
    assert_eq!(
        h.text(8).trim(),
        "ref mark 3 1",
        "an auto-placed ref moves from where the composite drew it (2 1)"
    );
    assert_eq!(h.text(9).trim(), "anchor +join 3 1", "anchor shifts");

    assert!(
        h.state.pixel_selection.is_none(),
        "a move-all leaves no selection behind"
    );
}

#[test]
fn cmd_drag_moves_all_layers_down() {
    let mut h = make_move_all_harness();
    h.drag_grid_mod(6, (0, 3), (1, 3), Modifiers::COMMAND);

    let grid = h.grid(6);
    assert!(grid.get(0, 0).is_clear(), "top row should be vacated");
    assert!(grid.get(1, 0).is_bitmap_filled());
    assert!(grid.get(2, 1).is_bitmap_filled());

    assert_eq!(h.text(7).trim(), "ref part 1 1");
    assert_eq!(h.text(8).trim(), "ref mark 2 2");
    assert_eq!(h.text(9).trim(), "anchor +join 2 2");
}

#[test]
fn cmd_drag_over_selection_still_moves_only_selection() {
    let mut h = make_move_all_harness();
    h.drag_grid(6, (0, 0), (1, 1)); // select the top-left 2x2
    h.drag_grid_mod(6, (0, 0), (0, 2), Modifiers::COMMAND);

    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("selection should survive its own move");
    assert!(sel.is_floating(), "dragging inside the selection moves it");
    assert_eq!((sel.row, sel.col), (0, 2));

    assert_eq!(h.text(7).trim(), "ref part 1 0", "layers must not move");
    assert_eq!(h.text(8).trim(), "ref mark");
    assert_eq!(h.text(9).trim(), "anchor +join 2 1");
}

#[test]
fn undo_move_all_restores_every_layer_at_once() {
    let mut h = make_move_all_harness();
    h.drag_grid_mod(6, (2, 0), (2, 2), Modifiers::COMMAND); // two cells right

    assert_eq!(h.text(7).trim(), "ref part 3 0");
    assert_eq!(h.text(8).trim(), "ref mark 4 1");
    assert_eq!(h.text(9).trim(), "anchor +join 4 1");

    h.key_mod(Key::Z, Modifiers::COMMAND);

    let grid = h.grid(6);
    assert!(grid.get(0, 0).is_bitmap_filled(), "pixels should be back");
    assert!(grid.get(0, 3).is_clear());
    assert_eq!(h.text(7).trim(), "ref part 1 0");
    assert_eq!(
        h.text(8).trim(),
        "ref mark",
        "undo restores the auto placement"
    );
    assert_eq!(h.text(9).trim(), "anchor +join 2 1");
    assert!(
        !h.state.undo.can_undo(),
        "the whole drag should be a single undo entry"
    );
}

#[test]
fn copy_produces_correct_text() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));

    // Copy (uses Event::Copy, same as Cmd+C)
    h.copy();
    let copied = h
        .last_copied_text
        .as_ref()
        .expect("should have copied text");
    assert_eq!(
        copied, "@@@@\n..@@",
        "copied text should match grid content"
    );
}

#[test]
fn paste_in_pixel_select_creates_floating() {
    let mut h = make_pixel_select_harness();
    h.paste("@@..\n..@@");

    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0, .. }),
        "should stay in PixelSelect"
    );
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert!(sel.is_floating());
    assert_eq!((sel.width, sel.height), (2, 2));
}

#[test]
fn paste_in_glyph_edit_switches_to_pixel_select() {
    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0); // enter GlyphEdit
    assert!(matches!(h.state.mode, EditMode::GlyphEdit { .. }));

    h.paste("@@\n@@");
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0, .. }),
        "paste should switch to PixelSelect"
    );
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert!(sel.is_floating());
}

#[test]
fn cut_copies_and_deletes() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));

    h.cut();
    let copied = h.last_copied_text.as_ref().expect("should have copied");
    assert_eq!(copied, "@@@@");
    assert!(h.state.pixel_selection.is_none());

    let grid = h.grid(1);
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_clear());
}

#[test]
fn paste_via_pixel_selection_function() {
    use crate::document_io::{derive_document, parse_doclines};
    use crate::editor::pixel_selection;

    let mut lines = parse_doclines("glyph test 3 2\n......\n......");
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let mut state = crate::editor::EditorState::new();
    state.mode = EditMode::GlyphEdit {
        item_idx: 0,
        selected_shape: crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
    };

    let ok = pixel_selection::paste_selection(&doc, &mut lines, &mut state, "@@..\n..@@");
    assert!(ok, "paste should succeed");
    assert!(matches!(
        state.mode,
        EditMode::PixelSelect { item_idx: 0, .. }
    ));
    let sel = state.pixel_selection.as_ref().unwrap();
    assert!(sel.is_floating());
    assert_eq!((sel.width, sel.height), (2, 2));
}

#[test]
fn paste_too_small_for_selection_fails() {
    use crate::document_io::{derive_document, parse_doclines};
    use crate::editor::pixel_selection;

    let mut lines = parse_doclines("glyph test 3 2\n......\n......");
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let mut state = crate::editor::EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };
    state.pixel_selection = Some(pixel_selection::PixelSelection {
        item_idx: 0,
        row: 0,
        col: 0,
        width: 3,
        height: 2,
        float_pixels: None,
    });

    let ok = pixel_selection::paste_selection(&doc, &mut lines, &mut state, "@@\n@@");
    assert!(
        !ok,
        "paste should fail when clipboard is smaller than selection"
    );
}

#[test]
fn right_click_cancels_selection() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    assert!(h.state.pixel_selection.is_some());

    let pos = h.grid_cell_pos(1, 0, 0);
    h.right_click_at(pos);
    assert!(
        h.state.pixel_selection.is_none(),
        "right click should cancel selection"
    );
}

#[test]
fn blur_commits_floating() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 0)); // select single cell
    h.drag_grid(1, (0, 0), (2, 0)); // move to row 2

    assert!(h.state.pixel_selection.as_ref().unwrap().is_floating());
    h.blur();
    assert!(
        h.state.pixel_selection.is_none(),
        "blur should commit and clear"
    );

    // Pixel should be merged at new position
    let grid = h.grid(1);
    assert!(grid.get(2, 0).is_bitmap_filled());
}

#[test]
fn on_demand_triangle_ref_reaches_renderer() {
    // A ref-only glyph pointing at an on-demand triangle must resolve with
    // exact custom details, expose them through the composite layers the
    // grid renderer draws from, and lay out one grid row per pixel row.
    let h = EditorHarness::new("glyph tri\nref 4x16-ul 0 0\n");
    assert_view_consistent(&h);

    // The offsetless form (auto-resolved placement) must reach the
    // renderer just the same.
    let h2 = EditorHarness::new("glyph tri2\nref 4x16-ul\n");
    assert_view_consistent(&h2);
    let resolved2 = h2.named_glyphs.get("tri2").expect("tri2 must resolve");
    assert!(
        !resolved2.grid.details.is_empty(),
        "offsetless triangle ref must resolve with details"
    );

    let resolved = h.named_glyphs.get("tri").expect("tri must resolve");
    assert!(
        !resolved.grid.details.is_empty(),
        "1:4 hypotenuse requires custom details in the resolved grid"
    );

    let rows = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| matches!(vl.kind, SnapKind::GridRow { .. }))
        .count();
    assert_eq!(rows, 16, "one grid row per pixel row");

    // The composite the renderer receives must carry the custom cells and
    // report them as filled (thumbnails and previews use that test).
    let comp = crate::editor::ref_composite::compute_composite(
        match &h.doc.items[0] {
            crate::document::DocumentItem::Glyph { body, .. } => body,
            other => panic!("expected glyph item, got {other:?}"),
        },
        &h.named_glyphs,
        &h.name_parts,
        &h.alt_index,
        &Default::default(),
    )
    .expect("composite for ref-only glyph");
    let layer = &comp.layers[0];
    assert!(
        !layer.grid.details.is_empty(),
        "composite layer must keep the custom details the renderer draws"
    );
    assert!(comp.any_layer_filled_at(0, 0), "right-angle corner filled");
    assert!(!comp.any_layer_filled_at(15, 3), "opposite corner empty");
}

// ---------------------------------------------------------------------------
// Horizontal grid scrolling
// ---------------------------------------------------------------------------

/// Leaving the pixel modes with Escape commits the floating pixels, and the
/// *derived* document has to catch up in that same frame: the deferred-reparse
/// rule is about a line the caret is still typing on, and a commit is not that
/// edit. Without this the grid kept painting the pre-commit shape until some
/// later edit happened to flush it.
#[test]
fn escape_commit_reaches_the_document() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.drag_grid(1, (0, 0), (2, 0)); // move down -> floating
    h.key(Key::Escape);
    h.frame();

    assert!(
        h.state.pixel_selection.is_none(),
        "Escape commits the float"
    );
    let lines_grid = h.grid(1).clone();
    let Some(crate::document::DocumentItem::Glyph { body, .. }) = h.doc.items.first() else {
        panic!("first item should be the glyph");
    };
    assert_eq!(
        body.pixels.as_ref(),
        Some(&lines_grid),
        "the derived document must hold the committed pixels"
    );
}
