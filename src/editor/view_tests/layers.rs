//! Layer move: dragging a `ref` around, the thumbnails beside it, and the
//! subglyph menu.

use super::*;

/// `child` is a plain 4x4 glyph; `parent` owns its own 8x4 pixel grid and
/// additionally references `child` once via a `ref` line.
///
/// DocLines: 0 header child, 1 grid child (4x4), 2 blank,
///           3 header parent, 4 grid parent (8x4), 5 "ref child 4 0".
/// Item indices: child = 0, blank line = 1, parent = 2 (`DocumentItem` gives
/// blank lines their own slot).
fn composite_doc() -> String {
    let mut s = String::from("glyph child 4 4\n");
    for r in 0..4 {
        for c in 0..4 {
            s.push_str(if r < 2 && c < 2 { "@@" } else { ".." });
        }
        s.push('\n');
    }
    s.push('\n');
    s.push_str("glyph parent 8 4\n");
    for r in 0..4 {
        for c in 0..8 {
            s.push_str(if r < 2 && c < 2 { "@@" } else { ".." });
        }
        s.push('\n');
    }
    s.push_str("ref child 4 0\n");
    s
}

/// Drag from `start` by `cells` whole grid cells to the left, one cell per
/// frame. The pointer is moved to `start` *before* pressing (in its own frame)
/// so the teleport itself isn't misread as part of the drag delta.
fn drag_left_by_cells(h: &mut EditorHarness, start: egui::Pos2, cells: usize) {
    let grid_cell = h.snap().grid_cell;
    h.frame_with(vec![egui::Event::PointerMoved(start)], Modifiers::NONE);
    h.frame_with(
        vec![egui::Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
        Modifiers::NONE,
    );
    let mut pos = start;
    for _ in 0..cells {
        pos.x -= grid_cell;
        h.frame_with(vec![egui::Event::PointerMoved(pos)], Modifiers::NONE);
    }
    h.frame_with(
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
        Modifiers::NONE,
    );
}

/// Regression test: dragging a `ref` layer in `LayerMove` mode used to get
/// stuck after ~1 pixel because the "defer rederive" guard (meant to
/// tolerate transiently-invalid text while typing) also applied while
/// `LayerMove`-dragging, so the composite/document never re-derived past the
/// first drag step. The fix gates that guard to `EditMode::Normal` only.
#[test]
fn drag_layer_move_advances_full_distance_across_frames() {
    let mut h = EditorHarness::new(&composite_doc());
    enter_layer_move(&mut h, 4, 2, 0);

    // Keep the pointer off in empty space (well below any rendered content) so
    // the drag doesn't also trip text/grid click handling.
    drag_left_by_cells(&mut h, egui::pos2(500.0, 5000.0), 3);

    assert_eq!(
        h.text(5),
        "ref child 1 0",
        "ref offset should have advanced the full 3-cell drag distance, not stuck after 1 step"
    );
}

/// Regression test: dragging a layer *on the grid* past the left edge of its
/// glyph's own columns dropped out of `LayerMove` after one cell. The drag's
/// first frame resolved a click target, the pointer was already outside the
/// grid's columns by then, so it read as a click on the underlying text line
/// and reset the mode to `Normal`.
#[test]
fn drag_layer_move_on_grid_survives_leaving_the_grid_columns() {
    let mut h = EditorHarness::new(&composite_doc());
    enter_layer_move(&mut h, 4, 2, 0);

    let start = h.grid_cell_pos(4, 0, 0);
    drag_left_by_cells(&mut h, start, 3);

    assert_eq!(
        h.text(5),
        "ref child 1 0",
        "dragging on the grid should move the layer the full 3 cells"
    );
    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 2,
                layer_idx: 0
            }
        ),
        "the drag should not have kicked the editor out of LayerMove, got {:?}",
        h.state.mode
    );
}

/// Same as [`composite_doc`], but `parent` has no pixel grid of its own: it is
/// a ref-only composite of two side-by-side copies of `child`.
///
/// DocLines: 0 header child, 1 grid child (4x4), 2 blank,
///           3 header parent, 4 "ref child 0 0", 5 "ref child 4 0".
/// Item indices: child = 0, blank line = 1, parent = 2.
fn ref_only_composite_doc() -> String {
    let mut s = String::from("glyph child 4 4\n");
    for r in 0..4 {
        for c in 0..4 {
            s.push_str(if r < 2 && c < 2 { "@@" } else { ".." });
        }
        s.push('\n');
    }
    s.push('\n');
    s.push_str("glyph parent\n");
    s.push_str("ref child 0 0\n");
    s.push_str("ref child 4 0\n");
    s
}

/// Regression test: a subglyph layer of a ref-only composite could not be
/// dragged in `LayerMove` mode at all. The layer had to overlap the glyph's
/// pixel grid or another layer *at its destination*, and a ref-only glyph of two
/// adjacent parts satisfies that nowhere, so it was stuck in every direction.
#[test]
fn drag_layer_move_works_on_ref_only_glyph() {
    // The synthesized grid of a ref-only composite sits on its first ref line.
    let mut h = EditorHarness::new(&ref_only_composite_doc());
    enter_layer_move(&mut h, 4, 2, 0);

    let start = h.grid_cell_pos(4, 0, 0);
    drag_left_by_cells(&mut h, start, 3);

    assert_eq!(
        h.text(4),
        "ref child -3 0",
        "ref offset of a ref-only composite should follow the 3-cell drag"
    );
    assert_eq!(h.text(5), "ref child 4 0", "the other ref must not move");
}

/// Regression test: clicking a ref-layer thumbnail in the inline tools panel
/// used to do nothing, because `ui.interact()` (added for the right-click
/// context menu) consumed the click before the panel's own `click_pos`-based
/// hit test could see it. The fix reuses that same `ui.interact()` response
/// for left-click layer selection.
#[test]
fn click_ref_layer_thumbnail_selects_that_layer() {
    let mut h = EditorHarness::new(&composite_doc());

    // Enter GlyphEdit on the parent glyph (item_idx 2) to render its inline
    // tools panel (which shows the composite preview plus a ref thumbnail).
    h.click_grid_cell(4, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "expected GlyphEdit for item_idx 2, got {:?}",
        h.state.mode
    );

    // Let any scroll-into-view animation settle before computing screen
    // coordinates to click on.
    for _ in 0..10 {
        h.frame();
    }

    let ref_pos = h.ref_thumbnail_pos(2, 0);
    h.click_at(ref_pos);

    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 2,
                layer_idx: 0
            }
        ),
        "clicking the ref-layer thumbnail should select that layer, got {:?}",
        h.state.mode
    );
}

/// Regression test: a subglyph declared with `scale N` has a pixel grid that is
/// N times finer than its logical size, and the inline tools panel used to size
/// its thumbnail straight from that raw grid — so a `scale 2` subglyph was drawn
/// twice as large as the glyph it is a part of.
#[test]
fn scaled_subglyph_thumbnail_is_not_oversized() {
    let mut doc = String::from("glyph child 4 4 scale 2\n");
    for r in 0..8 {
        for c in 0..8 {
            doc.push_str(if r < 4 && c < 4 { "@@" } else { ".." });
        }
        doc.push('\n');
    }
    doc.push('\n');
    doc.push_str("glyph parent 8 4\n");
    for _ in 0..4 {
        for _ in 0..8 {
            doc.push_str("..");
        }
        doc.push('\n');
    }
    doc.push_str("ref child 4 0\n");

    let mut h = EditorHarness::new(&doc);
    h.click_grid_cell(4, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "expected GlyphEdit for item_idx 2, got {:?}",
        h.state.mode
    );
    for _ in 0..10 {
        h.frame();
    }

    let rect = h.ref_thumbnail_rect(2, 0);
    let expected = 4.0 * crate::editor::document_view::PREVIEW_SCALE * h.zoom as f32;
    assert!(
        (rect.height() - expected).abs() < 0.5,
        "a 4-logical-row `scale 2` subglyph thumbnail should be {expected} tall, got {}",
        rect.height()
    );
}

/// The same in reverse: the glyph *being edited* carries the `scale N`, so its
/// own composite preview (and with it the minimum thumbnail size every subglyph
/// is padded to) has to be measured in logical pixels too.
#[test]
fn scaled_parent_preview_is_not_oversized() {
    let mut doc = String::from("glyph child 4 4\n");
    for r in 0..4 {
        for c in 0..4 {
            doc.push_str(if r < 2 && c < 2 { "@@" } else { ".." });
        }
        doc.push('\n');
    }
    doc.push('\n');
    doc.push_str("glyph parent 8 4 scale 2\n");
    for _ in 0..8 {
        for _ in 0..16 {
            doc.push_str("..");
        }
        doc.push('\n');
    }
    doc.push_str("ref child 4 0\n");

    let mut h = EditorHarness::new(&doc);
    h.click_grid_cell(4, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "expected GlyphEdit for item_idx 2, got {:?}",
        h.state.mode
    );
    for _ in 0..10 {
        h.frame();
    }

    // The thumbnail is padded up to the composite preview, which is 4 logical
    // rows tall — 8 subcell rows must not read as 8 logical ones.
    let rect = h.ref_thumbnail_rect(2, 0);
    let expected = 4.0 * crate::editor::document_view::PREVIEW_SCALE * h.zoom as f32;
    assert!(
        (rect.height() - expected).abs() < 0.5,
        "a `scale 2` glyph's 4-logical-row preview should be {expected} tall, got {}",
        rect.height()
    );
}

/// The subglyph menu used to be reachable only by right-clicking the ref
/// thumbnail in the inline tools panel. Right-clicking the grid while that ref
/// layer is the selected one must offer it too.
///
/// The target here draws pixels alone, so its first command ("Inline once")
/// has no declaration to expand and flattens, exactly like the second one.
#[test]
fn right_click_grid_in_layer_move_offers_subglyph_menu() {
    let mut h = EditorHarness::new(&composite_doc());
    enter_layer_move(&mut h, 4, 2, 0);

    let cell = h.grid_cell_pos(4, 0, 0);
    h.right_click_at(cell);
    h.frame();

    // Click the first item of the context menu, which egui lays out just
    // inside the menu frame at the click position. Move the pointer there
    // first: pressing and moving in one frame reads as a layer drag.
    let item = cell + egui::vec2(24.0, 14.0);
    h.move_pointer(item);
    h.click_at(item);
    h.frame();

    assert!(
        h.grid(4).get(0, 4).is_bitmap_filled(),
        "the ref's ink should have been inlined into the parent's pixel grid"
    );
    assert!(
        !matches!(h.lines.get(5), Some(DocLine::Text(t)) if t.starts_with("ref ")),
        "the inlined ref line should be gone, lines: {:?}",
        h.lines
    );
}

/// The same menu's "Inline once" on a target that *is* composed: the ref line
/// is replaced by the target's own ref, rebased onto where it sat, and no ink
/// is flattened into the grid.
///
/// DocLines: 0 header leaf, 1 grid leaf, 2 blank, 3 header mid,
///           4 "ref leaf 1 0", 5 blank, 6 header parent, 7 grid parent,
///           8 "ref mid 4 0".
#[test]
fn inline_once_from_the_grid_menu_keeps_the_targets_ref() {
    let mut source = String::from("glyph leaf 4 4\n");
    for r in 0..4 {
        for c in 0..4 {
            source.push_str(if r < 2 && c < 2 { "@@" } else { ".." });
        }
        source.push('\n');
    }
    source.push_str("\nglyph mid\nref leaf 1 0\n\nglyph parent 8 4\n");
    for _ in 0..4 {
        source.push_str(&"..".repeat(8));
        source.push('\n');
    }
    source.push_str("ref mid 4 0\n");

    let mut h = EditorHarness::new(&source);
    enter_layer_move(&mut h, 7, 4, 0);

    let cell = h.grid_cell_pos(7, 0, 0);
    h.right_click_at(cell);
    h.frame();
    let item = cell + egui::vec2(24.0, 14.0);
    h.move_pointer(item);
    h.click_at(item);
    h.frame();

    let texts: Vec<&str> = h
        .lines
        .iter()
        .filter_map(|l| match l {
            DocLine::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        texts.contains(&"ref leaf 5 0"),
        "`mid`'s own ref should have taken its place: {texts:?}"
    );
    assert!(
        !texts.iter().any(|t| t.starts_with("ref mid")),
        "the expanded ref line survived: {texts:?}"
    );
    assert!(
        !h.grid(7).get(0, 4).is_bitmap_filled(),
        "nothing should have been flattened into the parent's grid"
    );
}

/// Like [`composite_doc`], but `parent` references `child` twice, so the layer
/// palette holds three slots: the pixel grid and two subglyphs.
///
/// DocLines: 0 header child, 1 grid child, 2 blank, 3 header parent,
///           4 grid parent, 5 "ref child 4 0", 6 "ref child 0 2".
fn two_ref_doc() -> String {
    let mut s = composite_doc();
    s.push_str("ref child 0 2\n");
    s
}

/// The palette keys are absolute selections, not mode-local ones: `1` and
/// `` ` `` switch to the pixel grid (slot 1) from *any* layer, and `2`..`9`
/// pick the 2nd..9th palette slot — the subglyph layers — from any mode.
#[test]
fn digit_keys_select_palette_slots_from_any_mode() {
    let mut h = EditorHarness::new(&two_ref_doc());
    h.click_grid_cell(4, 0, 0); // GlyphEdit on `parent`
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "should be in GlyphEdit, got {:?}",
        h.state.mode
    );

    // GlyphEdit → subglyph layers.
    h.key(Key::Num2);
    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 2,
                layer_idx: 0
            }
        ),
        "`2` should select the first subglyph layer, got {:?}",
        h.state.mode
    );
    h.key(Key::Num3);
    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 2,
                layer_idx: 1
            }
        ),
        "`3` should select the second subglyph layer, got {:?}",
        h.state.mode
    );

    // A slot past the last layer is a no-op.
    h.key(Key::Num4);
    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 2,
                layer_idx: 1
            }
        ),
        "`4` has no layer to select and must leave the mode alone, got {:?}",
        h.state.mode
    );

    // LayerMove → pixel grid, in either detail mode.
    h.key(Key::Backtick);
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2, .. }),
        "`` ` `` should switch to the pixel grid in PixelSelect, got {:?}",
        h.state.mode
    );
    h.key(Key::Num3);
    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 2,
                layer_idx: 1
            }
        ),
        "`3` should select a layer from PixelSelect too, got {:?}",
        h.state.mode
    );
    h.key(Key::Num1);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "`1` should switch to the pixel grid in GlyphEdit, got {:?}",
        h.state.mode
    );
}

/// Choosing a subglyph menu item leaves the pointer over the menu, and egui's
/// menu takes the click; the editor must still hold keyboard focus afterwards,
/// or the very next keystroke (`` ` ``, `1`, …) goes nowhere.
#[test]
fn subglyph_menu_on_grid_keeps_editor_focus() {
    let mut h = EditorHarness::new(&composite_doc());
    enter_layer_move(&mut h, 4, 2, 0);

    let cell = h.grid_cell_pos(4, 0, 0);
    h.right_click_at(cell);
    h.frame();

    let item = cell + egui::vec2(24.0, 14.0);
    h.move_pointer(item);
    h.click_at(item);
    h.frame();

    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "inlining should land in GlyphEdit, got {:?}",
        h.state.mode
    );
    assert!(
        h.editor_has_focus(),
        "the editor must keep focus after the context menu closes"
    );
    h.key(Key::Backtick);
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2, .. }),
        "Backtick right after inlining should enter PixelSelect, got {:?}",
        h.state.mode
    );
}

/// The same, entered from the ref thumbnail in the inline tools panel.
#[test]
fn subglyph_menu_on_thumbnail_keeps_editor_focus() {
    let mut h = EditorHarness::new(&composite_doc());
    enter_layer_move(&mut h, 4, 2, 0);

    let thumb = h.ref_thumbnail_rect(2, 0).center();
    h.right_click_at(thumb);
    h.frame();

    let item = thumb + egui::vec2(24.0, 14.0);
    h.move_pointer(item);
    h.click_at(item);
    h.frame();

    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "inlining should land in GlyphEdit, got {:?}",
        h.state.mode
    );
    assert!(
        h.editor_has_focus(),
        "the editor must keep focus after the context menu closes"
    );
    h.key(Key::Backtick);
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2, .. }),
        "Backtick right after inlining should enter PixelSelect, got {:?}",
        h.state.mode
    );
}

// ---------------------------------------------------------------------------
// Pixel selection mode tests
// ---------------------------------------------------------------------------

/// A `ref` to a glyph that draws colours of its own is painted in those
/// colours, not in the one colour a layer used to get. The rule is the font
/// build's (`ColorPiece` in `render::ttf_builder`'s `collect`): a colour
/// travels up through every `ref` that writes no `fill` of its own.
#[test]
fn a_ref_to_a_colored_glyph_is_painted_in_its_colors() {
    let src = "\
color red = #ff0000
color blue = #0000ff

glyph left 1 1
@@

glyph right 1 1
@@

glyph combo 2 1
ref left 0 0 fill red
ref right 1 0 fill blue

glyph outer 2 1
ref combo
";
    let h = EditorHarness::new(src);
    // `outer` states a box, so its (empty) grid line is where its composite
    // draws — the line right after the header.
    let grid_line = h
        .lines
        .iter()
        .position(|l| matches!(l, DocLine::Text(t) if t.trim() == "glyph outer 2 1"))
        .expect("the header is in the document")
        + 1;
    // The last rect covering the cell: paint order decides what is seen.
    let fill_at = |pos: egui::Pos2| {
        h.painted_rects()
            .iter()
            .rev()
            .find(|r| r.rect.contains(pos) && r.clip.contains(pos))
            .map(|r| r.fill)
            .expect("the cell is painted")
    };
    let red = egui::Color32::from_rgba_unmultiplied(0xff, 0, 0, 0xff);
    let blue = egui::Color32::from_rgba_unmultiplied(0, 0, 0xff, 0xff);
    assert_eq!(
        fill_at(h.grid_cell_pos(grid_line, 0, 0)),
        red,
        "the left cell keeps its target's red"
    );
    assert_eq!(
        fill_at(h.grid_cell_pos(grid_line, 0, 1)),
        blue,
        "the right cell keeps its target's blue"
    );
}

/// The caret alone reaches the two inline commands: right-clicking with it on
/// an IDC line offers them above the editing items, and choosing the first one
/// leaves the `ref`s the line stood for — with its comment on the first.
///
/// DocLines: 0 header left, 1 grid left, 2 blank, 3 header right, 4 grid right,
///           5 blank, 6 header comp, 7 grid comp, 8 the IDC line.
#[test]
fn caret_on_an_idc_line_offers_the_inline_commands() {
    let part = |name: &str| {
        let mut s = format!("glyph {name} 4 4\n@@......\n");
        for _ in 1..4 {
            s.push_str("........\n");
        }
        s
    };
    let source = format!(
        "{}\n{}\nglyph comp 8 4\n{}\u{2ff0} left right // both halves\n",
        part("left"),
        part("right"),
        "................\n".repeat(4),
    );

    let mut h = EditorHarness::new(&source);
    h.click_text(8, 0);
    h.frame();
    assert_eq!(h.cursor().line, 8);

    let pos = h.text_pos(8, 0);
    h.right_click_at(pos);
    h.frame();
    let item = pos + egui::vec2(24.0, 14.0);
    h.move_pointer(item);
    h.click_at(item);
    h.frame();

    assert_eq!(h.text(8), "ref left 0 0 // both halves");
    assert_eq!(h.text(9), "ref right 4 0");
}
