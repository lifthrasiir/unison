//! The backreference shadow: the glyphs that refer to this one, drawn
//! behind it.

use super::*;

/// The backreference shadow: in pixel-selection mode a second `` ` `` draws
/// every glyph that refers to this one, placed where it puts this one, and the
/// drawn area grows to it. A third `` ` `` puts it away again.
#[test]
fn a_second_backtick_shadows_the_glyphs_that_refer_to_this_one() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph child 2 2
@@@@
@@@@

glyph parent 6 2
........@@@@
........@@@@
ref child 0 0
";
    let mut h = EditorHarness::new(source);
    let child_grid_line = 5;
    h.click_grid_cell(child_grid_line, 0, 0); // GlyphEdit on `child`
    assert_eq!(
        grid_extent_x(&h, child_grid_line),
        (0, 2),
        "the child's own extent"
    );

    // First `` ` ``: selection mode, and no shadow yet — a selection must not
    // resize the grid under the pointer.
    h.key(Key::Backtick);
    h.frame();
    assert!(
        matches!(
            h.state.mode,
            EditMode::PixelSelect {
                item_idx: 4,
                backrefs: false
            }
        ),
        "expected pixel selection with no shadow, got {:?}",
        h.state.mode
    );
    assert_eq!(grid_extent_x(&h, child_grid_line), (0, 2));

    // Second: `parent` refers to `child` at its own origin and is six columns
    // wide, so the drawn area grows to the whole of it.
    h.key(Key::Backtick);
    h.frame();
    assert!(
        matches!(
            h.state.mode,
            EditMode::PixelSelect {
                item_idx: 4,
                backrefs: true
            }
        ),
        "expected the backreference shadow to be on, got {:?}",
        h.state.mode
    );
    assert_eq!(grid_extent_x(&h, child_grid_line), (0, 6));

    // Third: off again.
    h.key(Key::Backtick);
    h.frame();
    assert!(
        matches!(
            h.state.mode,
            EditMode::PixelSelect {
                item_idx: 4,
                backrefs: false
            }
        ),
        "the third press should toggle the shadow off, got {:?}",
        h.state.mode
    );
    assert_eq!(grid_extent_x(&h, child_grid_line), (0, 2));
}

/// The toggle belongs to the mode: leaving pixel selection and coming back
/// starts with the shadow off, whatever it was before.
#[test]
fn leaving_pixel_selection_resets_the_backreference_shadow() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph child 2 2
@@@@
@@@@

glyph parent 6 2
........@@@@
........@@@@
ref child 0 0
";
    let mut h = EditorHarness::new(source);
    let child_grid_line = 5;
    h.click_grid_cell(child_grid_line, 0, 0);
    h.key(Key::Backtick);
    h.key(Key::Backtick);
    h.frame();
    assert_eq!(grid_extent_x(&h, child_grid_line), (0, 6), "shadow is on");

    // Out to the drawing mode and back in.
    h.key(Key::Num1);
    h.frame();
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 4, .. }),
        "mode {:?}",
        h.state.mode
    );
    assert_eq!(grid_extent_x(&h, child_grid_line), (0, 2));

    h.key(Key::Backtick);
    h.frame();
    assert!(
        matches!(
            h.state.mode,
            EditMode::PixelSelect {
                item_idx: 4,
                backrefs: false
            }
        ),
        "re-entering pixel selection must start with the shadow off, got {:?}",
        h.state.mode
    );
    assert_eq!(grid_extent_x(&h, child_grid_line), (0, 2));
}

/// A glyph nothing refers to has no shadow to show, and asking for one changes
/// nothing about the grid.
#[test]
fn the_backreference_shadow_of_an_unused_glyph_is_empty() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph lonely 2 2
@@@@
@@@@
";
    let mut h = EditorHarness::new(source);
    let grid_line = 5;
    h.click_grid_cell(grid_line, 0, 0);
    h.key(Key::Backtick);
    h.key(Key::Backtick);
    h.frame();
    assert!(
        matches!(
            h.state.mode,
            EditMode::PixelSelect {
                item_idx: 4,
                backrefs: true
            }
        ),
        "the toggle is the mode's, whether or not anything refers here: {:?}",
        h.state.mode
    );
    assert_eq!(grid_extent_x(&h, grid_line), (0, 2));
}
