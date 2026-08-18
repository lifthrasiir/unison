//! The band a grid is drawn in: horizontal scrolling, its scrollbar, and
//! what a click outside the grid may and may not paint.

use super::*;

/// A glyph far too wide for the band, followed by a narrow one.
///
/// DocLines: 0 header wide, 1 grid 90x2, 2 blank, 3 header narrow, 4 grid 4x1.
fn wide_doc() -> String {
    let mut s = String::from("glyph wide 90 2\n");
    s.push_str(&"@@".repeat(90));
    s.push('\n');
    s.push_str(&"..".repeat(90));
    s.push('\n');
    s.push('\n');
    s.push_str("glyph narrow 4 1\n@@@@@@@@\n");
    s
}

#[test]
fn band_leaves_room_for_the_inline_tool_panel_while_editing() {
    let mut h = EditorHarness::new(&wide_doc());
    h.click_grid_cell(1, 0, 0);
    h.frame();
    let snap = h.snap();
    let content_w = 90.0 * snap.grid_cell;
    assert!(
        content_w > snap.strip.w,
        "the wide glyph must overflow the band ({content_w} <= {})",
        snap.strip.w
    );
    // The full inline tool panel still fits to the right of the band.
    let panel_w = crate::editor::glyph_widget::palette_cols() as f32
        * crate::editor::document_view::INLINE_PALETTE_CELL;
    assert!(
        snap.strip.right() + panel_w <= 1000.0,
        "band right edge {} leaves no room for a {panel_w}pt panel",
        snap.strip.right()
    );
}

/// Outside glyph editing there is no inline tool panel, so the band must not
/// keep room for one: a grid that fits the editor's full width should not be
/// scrolled off it just because a panel *might* appear later.
#[test]
fn band_uses_the_full_width_when_not_editing() {
    let mut h = EditorHarness::new(&wide_doc());
    let idle = h.snap().strip.clone();

    h.click_grid_cell(1, 0, 0);
    h.frame();
    let editing = h.snap().strip.clone();

    let reserved = crate::editor::document_view::inline_panel_reserved_width(1.0);
    assert!(reserved > 0.0);
    assert!(
        (idle.w - editing.w - reserved).abs() < 0.01,
        "idle band {} should be exactly {reserved} wider than the editing band {}",
        idle.w,
        editing.w
    );
}

#[test]
fn wide_grid_scrolls_horizontally_while_narrow_one_stays_put() {
    let mut h = EditorHarness::new(&wide_doc());
    let cell = h.snap().grid_cell;
    let band_x = h.snap().strip.x;

    assert_eq!(h.snap().grid_row_x(0, 90), band_x, "starts unscrolled");

    h.state.grid_scroll_x = 3.0 * cell;
    h.frame();
    assert_eq!(
        h.snap().grid_row_x(0, 90),
        band_x - 3.0 * cell,
        "the overflowing grid shifts left by the scroll offset"
    );
    assert_eq!(
        h.snap().grid_row_x(0, 4),
        band_x,
        "a grid that fits is unaffected by the shared offset"
    );
}

#[test]
fn grid_scroll_is_clamped_to_the_overflow() {
    let mut h = EditorHarness::new(&wide_doc());
    h.state.grid_scroll_x = 100_000.0;
    h.frame();
    let snap = h.snap();
    let overflow = 90.0 * snap.grid_cell - snap.strip.w;
    assert!((h.state.grid_scroll_x - overflow).abs() < 0.01);
    assert_eq!(
        snap.grid_row_x(0, 90),
        snap.strip.x - overflow,
        "the last column sits flush with the band's right edge"
    );
}

#[test]
fn clicks_past_the_band_do_not_paint_pixels() {
    let mut h = EditorHarness::new(&wide_doc());
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit, got {:?}",
        h.state.mode
    );

    h.frame();
    // A column that only exists to the right of the band: clicking where it
    // would be lands on the inline tool panel, and must not paint.
    let row_y = h.grid_cell_pos(1, 0, 0).y;
    let snap = h.snap();
    let past_band = snap.strip.right() + snap.grid_cell;
    let hidden_col = ((past_band - snap.strip.x) / snap.grid_cell) as u16;

    let before = h.grid(1).get(0, hidden_col);
    h.right_click_at(egui::pos2(past_band, row_y));
    assert_eq!(
        h.grid(1).get(0, hidden_col),
        before,
        "a click outside the band must not reach the grid"
    );
}

#[test]
fn dragging_to_the_band_edge_auto_scrolls() {
    let mut h = EditorHarness::new(&wide_doc());
    h.click_grid_cell(1, 0, 0);
    assert_eq!(h.state.grid_scroll_x, 0.0);

    h.frame();
    let start = h.grid_cell_pos(1, 0, 0);
    let edge_x = h.snap().strip.right() - 2.0;
    let row_y = start.y;

    // Press inside the grid, then hold at the right edge for a few frames.
    h.press_at(start);
    for _ in 0..5 {
        h.move_pointer(egui::pos2(edge_x, row_y));
    }
    assert!(
        h.state.grid_scroll_x > 0.0,
        "holding a drag at the right edge should scroll the band"
    );
    let scrolled = h.state.grid_scroll_x;
    h.release_at(egui::pos2(edge_x, row_y));
    h.frame();
    assert_eq!(
        h.state.grid_scroll_x, scrolled,
        "auto-scroll stops once the button is released"
    );
}

#[test]
fn dragging_in_the_inline_tool_panel_does_not_auto_scroll() {
    let mut h = EditorHarness::new(&wide_doc());
    h.click_grid_cell(1, 0, 0);
    h.frame();
    let row_y = h.grid_cell_pos(1, 0, 0).y;
    // The inline tool panel lives past the band's right edge, i.e. inside the
    // auto-scroll edge zone. A gesture starting there is not a grid drag.
    let panel_x = h.snap().strip.right() + 20.0;

    h.press_at(egui::pos2(panel_x, row_y));
    for _ in 0..5 {
        h.move_pointer(egui::pos2(panel_x, row_y));
    }
    assert_eq!(
        h.state.grid_scroll_x, 0.0,
        "a press in the inline tool panel must not auto-scroll the band"
    );
    h.release_at(egui::pos2(panel_x, row_y));
}

#[test]
fn scrollbar_sits_below_the_grid_and_drags_it() {
    let mut h = EditorHarness::new(&wide_doc());
    h.click_grid_cell(1, 0, 0);
    h.frame();
    let bar = *h
        .snap()
        .strip
        .bars
        .first()
        .expect("an overflowing grid gets a scrollbar");

    let block_bottom = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == 1)
        .map(|vl| vl.y + vl.height)
        .fold(f32::MIN, f32::max);
    assert!(
        bar.min.y >= block_bottom,
        "the scrollbar must not cover the grid ({} < {block_bottom})",
        bar.min.y
    );

    let y = bar.center().y;
    h.press_at(egui::pos2(bar.min.x + 20.0, y));
    h.move_pointer(egui::pos2(bar.min.x + 220.0, y));
    h.release_at(egui::pos2(bar.min.x + 220.0, y));
    h.frame();
    assert!(
        h.state.grid_scroll_x > 0.0,
        "dragging the thumb right should scroll the band"
    );
}

#[test]
fn scrollbar_of_an_overlong_grid_is_pulled_into_view() {
    // Taller than the 2400pt test viewport, so its bottom edge — where the
    // scrollbar would normally go — is off screen.
    let mut src = String::from("glyph tall 90 200\n");
    for _ in 0..200 {
        src.push_str(&"@@".repeat(90));
        src.push('\n');
    }
    let mut h = EditorHarness::new(&src);
    // Taller than twice the font height, so it opens folded; this test is
    // about the block open.
    h.click_fold_marker(0);
    h.click_grid_cell(1, 0, 0);
    h.frame();
    let bar = *h
        .snap()
        .strip
        .bars
        .first()
        .expect("an overflowing grid gets a scrollbar");
    let block_bottom = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == 1)
        .map(|vl| vl.y + vl.height)
        .fold(f32::MIN, f32::max);

    assert!(
        block_bottom > 2400.0,
        "the grid should overflow the viewport"
    );
    assert!(
        bar.max.y <= 2400.0 && bar.min.y >= 0.0,
        "the scrollbar must be pulled back into view, got {bar:?}"
    );
}

#[test]
fn scrollbar_drag_passing_over_the_grid_does_not_paint() {
    let mut h = EditorHarness::new(&wide_doc());
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit, got {:?}",
        h.state.mode
    );
    h.frame();

    let before = h.grid(1).clone();
    // Row 1 is empty, so any stray paint shows up.
    let cell = h.grid_cell_pos(1, 1, 2);
    let bar = *h.snap().strip.bars.first().expect("scrollbar");

    // Grab the thumb, then sweep up across the grid rows and back down.
    h.press_at(egui::pos2(bar.min.x + 20.0, bar.center().y));
    h.move_pointer(egui::pos2(bar.min.x + 120.0, cell.y));
    h.move_pointer(egui::pos2(bar.min.x + 220.0, cell.y));
    h.move_pointer(egui::pos2(bar.min.x + 220.0, bar.center().y));
    h.release_at(egui::pos2(bar.min.x + 220.0, bar.center().y));
    h.frame();

    assert!(
        h.state.grid_scroll_x > 0.0,
        "the thumb drag should still scroll while off the bar"
    );
    for r in 0..before.height {
        for c in 0..before.width {
            assert_eq!(
                h.grid(1).get(r, c),
                before.get(r, c),
                "scrollbar drag painted over cell ({r}, {c})"
            );
        }
    }
}

/// Regression test: a menu drawn over the grid closes on the press, and the
/// grid — which reads the raw pointer state rather than a click — took the
/// still-held button on the following frame for a paint stroke, filling the
/// cell the menu entry had covered. Painting must follow only a gesture that
/// began on the grid itself.
#[test]
fn clicking_a_menu_over_the_grid_does_not_paint() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit, got {:?}",
        h.state.mode
    );
    h.frame();

    let cell = h.grid_cell_pos(1, 2, 3);
    assert!(h.grid(1).get(2, 3).is_clear(), "cell (2, 3) starts empty");

    // A menu covering that cell, then a click on the entry over it: the menu
    // closes on the press, and the button comes up a frame later — by then
    // nothing is between the pointer and the grid.
    h.menu_overlay = Some(egui::Rect::from_center_size(cell, egui::vec2(120.0, 60.0)));
    h.frame();
    h.press_at(cell);
    h.menu_overlay = None;
    h.frame();
    h.release_at(cell);
    h.frame();

    assert!(
        h.grid(1).get(2, 3).is_clear(),
        "clicking a menu entry must not paint the cell underneath it"
    );
}

/// The other half of [`clicking_a_menu_over_the_grid_does_not_paint`]: a press
/// that does land on the grid still paints on the very frame it arrives.
#[test]
fn a_press_starting_on_the_grid_paints() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_grid_cell(1, 0, 0);
    h.frame();

    let cell = h.grid_cell_pos(1, 2, 3);
    h.press_at(cell);
    h.release_at(cell);
    h.frame();

    assert!(
        !h.grid(1).get(2, 3).is_clear(),
        "a press on the grid should paint the cell under it"
    );
}

#[test]
fn scrollbar_only_shows_for_the_glyph_being_edited() {
    let mut h = EditorHarness::new(&wide_doc());
    assert!(
        h.snap().strip.bars.is_empty(),
        "no scrollbar outside grid edit mode -- it would cover the next line"
    );

    h.click_grid_cell(1, 0, 0);
    h.frame();
    assert_eq!(
        h.snap().strip.bars.len(),
        1,
        "the edited glyph gets a scrollbar"
    );

    // Leaving edit mode takes it away again.
    h.key(Key::Escape);
    assert!(matches!(h.state.mode, EditMode::Normal));
    assert!(h.snap().strip.bars.is_empty());
}
