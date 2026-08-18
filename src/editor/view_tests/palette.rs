//! The shape palette: rotation, fill, and the shortcuts that reach into it.

use super::*;

fn selected_shape(h: &EditorHarness) -> crate::pixel::PixelShape {
    match h.state.mode {
        EditMode::GlyphEdit { selected_shape, .. } => selected_shape,
        ref other => panic!("not editing pixels: {other:?}"),
    }
}

/// A 4×2 glyph, entered in GlyphEdit mode with the pointer over its grid.
fn palette_harness() -> EditorHarness {
    let mut h = EditorHarness::new("glyph test 4 2\n........\n........\n");
    h.click_grid_cell(1, 0, 0);
    h.frame();
    h
}

#[test]
fn wheel_over_the_grid_rotates_the_selected_shape() {
    use crate::editor::glyph_widget::rotate_shape;

    let mut h = palette_harness();
    // `f` picks HALF1 outright, so the starting orientation is known.
    h.key(Key::F);
    h.frame();
    let start = selected_shape(&h);
    assert_eq!(start.shape_id(), crate::pixel::PX_HALF1);
    let start_rotation = h.state.shape_rotation;

    eprintln!("pos now = {:?}", h.grid_cell_pos(1, 0, 1));
    for i in 0..3 {
        h.frame();
        eprintln!(
            "pos after {} frames = {:?}",
            i + 1,
            h.grid_cell_pos(1, 0, 1)
        );
    }
    let pos = h.grid_cell_pos(1, 0, 1);
    h.wheel_at_mod(pos, false, Modifiers::NONE);

    assert_eq!(
        selected_shape(&h),
        rotate_shape(start, 1),
        "a wheel notch should turn the shape a quarter clockwise"
    );
    assert_eq!(h.state.shape_rotation, (start_rotation + 1) % 4);
}

#[test]
fn shift_wheel_picks_another_shape_at_the_same_rotation() {
    use crate::editor::glyph_widget::{palette_shapes, rotate_shape, shape_orbit};

    let mut h = palette_harness();
    h.key(Key::F);
    h.frame();
    eprintln!("pos now = {:?}", h.grid_cell_pos(1, 0, 1));
    for i in 0..3 {
        h.frame();
        eprintln!(
            "pos after {} frames = {:?}",
            i + 1,
            h.grid_cell_pos(1, 0, 1)
        );
    }
    let pos = h.grid_cell_pos(1, 0, 1);

    // Rotate twice, then step to the neighbouring palette cell.
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    let rotation = h.state.shape_rotation;
    let before = shape_orbit(selected_shape(&h)).unwrap().0;

    h.wheel_at_mod(pos, false, Modifiers::SHIFT);

    let (cell, _) = shape_orbit(selected_shape(&h)).unwrap();
    assert_eq!(cell, before + 1, "shift+wheel walks the palette");
    assert_eq!(
        h.state.shape_rotation, rotation,
        "the rotation is remembered across a shape change"
    );
    assert_eq!(
        selected_shape(&h),
        rotate_shape(palette_shapes()[cell], rotation as i32),
        "the new shape arrives already rotated"
    );
}

#[test]
fn the_whole_palette_rotates_with_the_wheel() {
    use crate::editor::glyph_widget::{palette_shapes, rotate_shape};

    let mut h = palette_harness();
    eprintln!("pos now = {:?}", h.grid_cell_pos(1, 0, 1));
    for i in 0..3 {
        h.frame();
        eprintln!(
            "pos after {} frames = {:?}",
            i + 1,
            h.grid_cell_pos(1, 0, 1)
        );
    }
    let pos = h.grid_cell_pos(1, 0, 1);
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    let rotation = h.state.shape_rotation;
    assert_eq!(rotation, 1);

    // Clicking a palette cell yields that cell *as drawn*, i.e. rotated —
    // which is what makes the rotation visible in every cell, not just the
    // preview under the cursor.
    let cell = 3;
    let click = h.palette_cell_pos(cell);
    h.click_at(click);
    h.frame();
    assert_eq!(
        selected_shape(&h),
        rotate_shape(palette_shapes()[cell], rotation as i32)
    );
}

/// The palette wraps at the slants, so the families added after them are only
/// reachable if the second row is laid out and hit-tested at all.
#[test]
fn a_second_row_palette_cell_can_be_picked() {
    use crate::editor::glyph_widget::{palette_row_col, palette_shapes, shape_orbit};
    use crate::pixel::{PX_HOUSE2, PixelShape};

    let mut h = palette_harness();
    let (cell, _) = shape_orbit(PixelShape::new(PX_HOUSE2, true)).unwrap();
    assert_eq!(palette_row_col(cell).0, 1, "cell {cell} should be on row 1");

    h.click_at(h.palette_cell_pos(cell));
    h.frame();
    assert_eq!(selected_shape(&h), palette_shapes()[cell]);
}

/// Clicking the cell that is already selected is not a no-op: it flips the
/// fill, so the shape and its complement are one click apart.
#[test]
fn clicking_the_selected_palette_cell_toggles_the_fill() {
    use crate::editor::glyph_widget::shape_orbit;

    let mut h = palette_harness();
    let (cell, _) = shape_orbit(selected_shape(&h)).unwrap();
    let click = h.palette_cell_pos(cell);

    let before = selected_shape(&h);
    h.click_at(click);
    h.frame();
    assert_eq!(
        selected_shape(&h),
        before.with_fill_toggled(),
        "re-clicking the selected cell flips the fill"
    );

    h.click_at(click);
    h.frame();
    assert_eq!(selected_shape(&h), before, "and flips it back");
}

/// The selected cell shows the fill the selection actually carries, so the
/// toggling is visible: click the same cell and it alternates between the solid
/// and the dimmed drawing. Every other cell keeps the palette's own fill.
#[test]
fn the_selected_palette_cell_is_drawn_with_the_selections_fill() {
    use crate::editor::glyph_widget::{palette_shapes, shape_orbit};

    let mut h = palette_harness();
    let (cell, _) = shape_orbit(selected_shape(&h)).unwrap();
    let other = if cell == 0 { 2 } else { 0 };
    let other_filled = palette_shapes()[other].is_bitmap_filled();
    let click = h.palette_cell_pos(cell);

    assert!(h.palette_cell_filled(cell), "starts out filled");

    h.click_at(click);
    h.frame();
    assert!(!selected_shape(&h).is_bitmap_filled());
    assert!(
        !h.palette_cell_filled(cell),
        "the selected cell follows the selection into unfilled"
    );
    assert_eq!(
        h.palette_cell_filled(other),
        other_filled,
        "an unselected cell keeps the palette's own fill"
    );

    h.click_at(click);
    h.frame();
    assert!(h.palette_cell_filled(cell), "and back to filled");
}

/// Only the *selected* cell toggles; picking a different one still yields that
/// cell's own fill, unchanged from the palette.
#[test]
fn clicking_another_palette_cell_takes_its_own_fill() {
    use crate::editor::glyph_widget::palette_shapes;

    let mut h = palette_harness();
    let cell = 1; // PX_DOT, the one unfilled representative on row 0
    assert!(!palette_shapes()[cell].is_bitmap_filled());

    h.click_at(h.palette_cell_pos(cell));
    h.frame();
    assert_eq!(selected_shape(&h), palette_shapes()[cell]);
}

#[test]
fn a_shape_shortcut_pulls_the_palette_rotation_with_it() {
    let mut h = palette_harness();
    eprintln!("pos now = {:?}", h.grid_cell_pos(1, 0, 1));
    for i in 0..3 {
        h.frame();
        eprintln!(
            "pos after {} frames = {:?}",
            i + 1,
            h.grid_cell_pos(1, 0, 1)
        );
    }
    let pos = h.grid_cell_pos(1, 0, 1);
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    h.wheel_at_mod(pos, false, Modifiers::NONE);

    // `a` names PX_HALF3 absolutely; the palette adopts its orientation
    // instead of leaving the two disagreeing.
    h.key(Key::A);
    h.frame();
    let shape = selected_shape(&h);
    assert_eq!(shape.shape_id(), crate::pixel::PX_HALF3);
    assert_eq!(
        h.state.shape_rotation,
        crate::editor::glyph_widget::shape_orbit(shape).unwrap().1
    );
}

// -- edit border under scroll -------------------------------------------------

/// The edit-mode border spans the whole glyph but used to be drawn only from
/// the grid's *top* row. With that row scrolled above the viewport (culled),
/// the border vanished even though most of the glyph was still on screen.
#[test]
fn edit_border_survives_the_top_grid_row_being_culled() {
    let mut source = String::from("glyph tall 16 300\n");
    for _ in 0..300 {
        source.push_str("@@..............................\n");
    }
    source.push_str("\nglyph small 2 2\n@@@@\n@@@@\n");
    let mut h = EditorHarness::new(&source);
    // Taller than twice the font height, so it opens folded; this test is
    // about the block open.
    h.click_fold_marker(0);
    h.state.mode = EditMode::GlyphEdit {
        item_idx: 0,
        selected_shape: crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
    };
    h.frame();
    let full_border = h
        .edit_border_rect()
        .expect("border must be painted with the whole grid in view");

    // Scroll to the end of the document: the tall grid's top rows are culled,
    // its bottom rows still visible. The caret move drops back to Normal
    // mode, so re-enter GlyphEdit once the scroll has settled.
    h.state.goto_line(6);
    h.frame();
    h.frame();
    assert!(
        h.scroll_y() > 1000.0,
        "expected a scroll; y={}",
        h.scroll_y()
    );
    h.state.mode = EditMode::GlyphEdit {
        item_idx: 0,
        selected_shape: crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
    };
    h.frame();
    assert!(
        h.scroll_y() > 1000.0,
        "scroll must survive; y={}",
        h.scroll_y()
    );

    let border = h
        .edit_border_rect()
        .expect("border must still be painted when the grid's top row is culled");
    // The border still describes the whole glyph — its top is above the
    // viewport, and its size is unchanged.
    assert!(
        border.min.y < 0.0,
        "border top should be off-screen: {border:?}"
    );
    assert_eq!(border.size(), full_border.size());
}

/// Clicking a menu-bar item hands egui's keyboard focus to that button, and the
/// editor is left with none: every key after it — Ctrl+V included — goes
/// nowhere until the user clicks back into the grid. An action dispatched from
/// a menu therefore has to hand the focus back, or the document it just changed
/// takes no keyboard input at all.
#[test]
fn a_menu_action_leaves_the_editor_ready_for_keys() {
    use crate::editor::pixel_selection;

    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0);
    assert!(matches!(h.state.mode, EditMode::GlyphEdit { .. }));
    assert!(h.editor_has_focus());

    // Opening the menu takes the focus away.
    h.blur();
    assert!(!h.editor_has_focus());

    // What Edit ▸ Adjust scale does to the document.
    assert!(pixel_selection::handle_adjust_scale(
        &h.doc,
        &mut h.lines,
        &mut h.state,
        2
    ));
    crate::editor::document_view::flush_document_changes(&mut h.lines, &mut h.doc, &mut h.state);
    h.state.refocus();
    h.frame();

    assert!(
        h.editor_has_focus(),
        "the editor should have the keyboard back"
    );
    h.paste("@@\n@@");
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0, .. }),
        "paste should switch to PixelSelect, mode = {:?}",
        h.state.mode
    );
}

/// A menu-bar click takes the editor's keyboard focus (see
/// [`crate::editor::EditorState::refocus`]) before any of the menu's entries
/// can run, and an unfocused editor commits its floating selection away. The
/// selection is what the Selection menu acts on, so it has to survive the trip
/// into the menu — otherwise every entry there is greyed out by the time the
/// user reaches it. `blur_commits_floating` covers the other side: a focus loss
/// with no menu open still commits.
#[test]
fn a_floating_selection_survives_the_menu_that_acts_on_it() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 0)); // select a single cell
    h.drag_grid(1, (0, 0), (2, 0)); // and float it down to row 2
    assert!(h.state.pixel_selection.as_ref().unwrap().is_floating());

    // Opening a menu over the editor: the focus goes to the menu button.
    h.menu_open = true;
    h.blur();
    assert!(
        h.state
            .pixel_selection
            .as_ref()
            .is_some_and(|s| s.is_floating()),
        "the selection the menu acts on should still be there"
    );

    // Once the menu is gone, an editor that never got the focus back commits
    // it as any other unfocused editor does.
    h.menu_open = false;
    h.frame();
    assert!(h.state.pixel_selection.is_none(), "menu closed: commit");
    assert!(h.grid(1).get(2, 0).is_bitmap_filled());
}

// ---------------------------------------------------------------------------
// Implicit whole-grid selection, Ctrl+A, and shift-click extension
// ---------------------------------------------------------------------------
