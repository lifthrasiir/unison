//! The anchor shadow: the glyphs that would attach to the selected anchor,
//! drawn behind it.

use super::*;

/// Selecting an `anchor` layer shadows every glyph that can attach there, and
/// the drawn area grows to the shadow: a two-column mark otherwise shows none
/// of the base it lands on, which is the whole point of looking at it.
#[test]
fn selecting_an_anchor_shadows_the_glyphs_that_attach_to_it() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph mark 2 1
@@@@
anchor -above 0 1

glyph base 8 4
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@
anchor +above 2 -1
";
    let mut h = EditorHarness::new(source);
    let grid_line = 5;
    assert_eq!(
        grid_extent_x(&h, grid_line),
        (0, 2),
        "the mark's own extent"
    );

    // The mark has no refs, so layer 0 is its `-above` anchor.
    enter_layer_move(&mut h, grid_line, 4, 0);
    h.frame();

    // `base` is placed so its `+above` lands on the mark's `-above`: two
    // columns left of the mark's origin, eight columns wide.
    assert_eq!(grid_extent_x(&h, grid_line), (-2, 6));
}

/// The shadow is the *union* of every candidate, not just the first one found,
/// and it is drawn only while the anchor layer is the selected one.
#[test]
fn the_anchor_shadow_unions_every_candidate_and_only_while_selected() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph mark 2 1
@@@@
anchor -above 0 1

glyph left 4 2
@@@@@@@@
@@@@@@@@
anchor +above 2 -1

glyph right 4 2
@@@@@@@@
@@@@@@@@
anchor +above 0 -1
";
    let mut h = EditorHarness::new(source);
    let grid_line = 5;
    enter_layer_move(&mut h, grid_line, 4, 0);
    h.frame();

    // `left` reaches two columns left of the origin, `right` four to its
    // right; the union spans both.
    assert_eq!(grid_extent_x(&h, grid_line), (-2, 4));

    // Back on the pixel layer the shadow is gone, and so is the room made
    // for it.
    let hover = h.grid_cell_pos(grid_line, 0, 0);
    h.frame_with(
        vec![
            egui::Event::PointerMoved(hover),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, 1.0),
                modifiers: Modifiers::COMMAND,
            },
        ],
        Modifiers::COMMAND,
    );
    h.frame();
    assert!(matches!(h.state.mode, EditMode::GlyphEdit { .. }));
    assert_eq!(grid_extent_x(&h, grid_line), (0, 2));
}

/// An anchor inherited through an `inherit` ref is a selectable layer like a
/// declared point: it comes after the declared points in the cycle order, and
/// selecting it draws the anchor shadow of the glyphs that could attach there.
#[test]
fn an_inherited_anchor_is_a_selectable_layer_with_a_shadow() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph base 4 2
@@@@@@@@
@@@@@@@@
anchor +above 3 0

glyph mark 2 1
@@@@
anchor -above 0 1

glyph comp 4 2
........
........
ref base inherit
";
    let mut h = EditorHarness::new(source);
    let comp_grid_line = 13;
    assert_eq!(
        grid_extent_x(&h, comp_grid_line),
        (0, 4),
        "comp's own extent"
    );

    // comp has one ref and no declared points, so layer 1 is the inherited
    // `+above`.
    enter_layer_move(&mut h, comp_grid_line, 8, 1);
    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 8,
                layer_idx: 1
            }
        ),
        "expected the inherited anchor layer, got {:?}",
        h.state.mode
    );
    h.frame();

    // `mark` shadows with its `-above` on the inherited `+above` at column 3,
    // so it reaches one column past comp's own right edge.
    assert_eq!(grid_extent_x(&h, comp_grid_line), (0, 5));
}

/// An inherited anchor outside the ink widens the drawn area exactly like a
/// declared one — otherwise it is a palette layer with no visible mark.
#[test]
fn an_inherited_anchor_above_the_grid_widens_the_drawn_area() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph base 4 2
@@@@@@@@
@@@@@@@@
anchor +above 2 -2

glyph comp 4 2
........
........
ref base inherit
";
    let mut h = EditorHarness::new(source);
    h.frame();
    let comp_grid_line = 9;
    let rows: Vec<i16> = h
        .snap()
        .vlines
        .iter()
        .filter_map(|vl| match vl.kind {
            SnapKind::GridRow { row, .. } if vl.doc_line == comp_grid_line => Some(row),
            _ => None,
        })
        .collect();
    assert!(
        rows.contains(&-2),
        "the drawn area must reach the inherited +above at row -2, got rows {rows:?}",
    );
}

// ---------------------------------------------------------------------------
// Shape palette: rotation and shape choice are orthogonal
// ---------------------------------------------------------------------------
