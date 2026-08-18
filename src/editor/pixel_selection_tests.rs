//! Tests for [`crate::editor::pixel_selection`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items.

use super::*;
use crate::editor::caret::Caret;

fn c(line: usize, col: usize) -> Caret {
    Caret::new(line, col)
}

#[test]
fn parse_pixel_rect_valid() {
    let text = "@@..\n..@@";
    let grid = parse_pixel_rect(text).unwrap();
    assert_eq!(grid.width, 2);
    assert_eq!(grid.height, 2);
    assert!(grid.get(0, 0).is_bitmap_filled());
    assert!(grid.get(0, 1).is_clear());
    assert!(grid.get(1, 0).is_clear());
    assert!(grid.get(1, 1).is_bitmap_filled());
}

#[test]
fn parse_pixel_rect_invalid() {
    assert!(parse_pixel_rect("").is_none());
    assert!(parse_pixel_rect("@").is_none()); // odd length
    assert!(parse_pixel_rect("@@\n@").is_none()); // inconsistent lengths
    assert!(parse_pixel_rect("ZZ").is_none()); // invalid shape chars
}

#[test]
fn selection_contains() {
    let sel = PixelSelection {
        item_idx: 0,
        row: 2,
        col: 3,
        width: 4,
        height: 3,
        float_pixels: None,
    };
    assert!(sel.contains(2, 3));
    assert!(sel.contains(4, 6));
    assert!(!sel.contains(1, 3));
    assert!(!sel.contains(5, 3));
    assert!(!sel.contains(2, 2));
    assert!(!sel.contains(2, 7));
}

#[test]
fn snapshot_roundtrip() {
    let sel = PixelSelection {
        item_idx: 5,
        row: -1,
        col: 3,
        width: 2,
        height: 4,
        float_pixels: Some(PixelGrid::new(2, 4)),
    };
    let snap = sel.to_snapshot();
    let restored = PixelSelection::from_snapshot(&snap);
    assert_eq!(sel, restored);
}

#[test]
fn copy_grounded_selection() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n@@@@..\n..@@@@";
    let lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();
    let sel = PixelSelection {
        item_idx: 0,
        row: 0,
        col: 1,
        width: 2,
        height: 2,
        float_pixels: None,
    };
    let text = copy_selection(&doc, &lines, &sel).unwrap();
    assert_eq!(text, "@@..\n@@@@");
}

#[test]
fn copy_floating_selection() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n......\n......";
    let lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();
    let mut float = PixelGrid::new(2, 1);
    float.set(0, 0, PixelShape::new(pixel::PX_ALMOSTFULL, true));
    float.set(0, 1, PixelShape::EMPTY);
    let sel = PixelSelection {
        item_idx: 0,
        row: 0,
        col: 0,
        width: 2,
        height: 1,
        float_pixels: Some(float),
    };
    let text = copy_selection(&doc, &lines, &sel).unwrap();
    assert_eq!(text, "@@..");
}

#[test]
fn delete_grounded_clears_pixels() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n@@@@@@\n@@@@@@";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };
    state.pixel_selection = Some(PixelSelection {
        item_idx: 0,
        row: 0,
        col: 0,
        width: 2,
        height: 1,
        float_pixels: None,
    });

    handle_delete_selection(&doc, &mut lines, &mut state);
    assert!(state.pixel_selection.is_none());

    let DocLine::Grid(grid) = &lines[1] else {
        panic!("expected grid");
    };
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_clear());
    assert!(grid.get(0, 2).is_bitmap_filled());
    assert!(grid.get(1, 0).is_bitmap_filled());
}

#[test]
fn delete_floating_discards() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n......\n......";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut float = PixelGrid::new(2, 1);
    float.set(0, 0, PixelShape::new(pixel::PX_ALMOSTFULL, true));

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };
    state.pixel_selection = Some(PixelSelection {
        item_idx: 0,
        row: 1,
        col: 1,
        width: 2,
        height: 1,
        float_pixels: Some(float),
    });

    handle_delete_selection(&doc, &mut lines, &mut state);
    assert!(state.pixel_selection.is_none());

    // Grid should remain all empty — floating pixels were discarded, not merged
    let DocLine::Grid(grid) = &lines[1] else {
        panic!("expected grid");
    };
    for r in 0..2 {
        for c in 0..3 {
            assert!(grid.get(r, c).is_clear());
        }
    }
}

#[test]
fn commit_floating_merges_overwrite() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n@@@@@@\n@@@@@@";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut float = PixelGrid::new(2, 1);
    float.set(0, 0, PixelShape::EMPTY);
    float.set(0, 1, PixelShape::EMPTY);

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };

    let sel = PixelSelection {
        item_idx: 0,
        row: 0,
        col: 0,
        width: 2,
        height: 1,
        float_pixels: Some(float),
    };

    commit_floating(&doc, &mut lines, &mut state, &sel);

    let DocLine::Grid(grid) = &lines[1] else {
        panic!("expected grid");
    };
    // Overwrite means the empty float pixels replace filled grid pixels
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_clear());
    assert!(grid.get(0, 2).is_bitmap_filled());
}

#[test]
fn mirror_h_entire_glyph() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n@@@@..\n..@@@@";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };

    let changed =
        handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::MirrorH);
    assert!(changed);

    let DocLine::Grid(grid) = &lines[1] else {
        panic!("expected grid")
    };
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_bitmap_filled());
    assert!(grid.get(0, 2).is_bitmap_filled());
    assert!(grid.get(1, 0).is_bitmap_filled());
    assert!(grid.get(1, 1).is_bitmap_filled());
    assert!(grid.get(1, 2).is_clear());
}

#[test]
fn flip_v_entire_glyph() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n@@@@..\n......";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };

    handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::FlipV);

    let DocLine::Grid(grid) = &lines[1] else {
        panic!("expected grid")
    };
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_clear());
    assert!(grid.get(0, 2).is_clear());
    assert!(grid.get(1, 0).is_bitmap_filled());
    assert!(grid.get(1, 1).is_bitmap_filled());
    assert!(grid.get(1, 2).is_clear());
}

#[test]
fn rotate_180_entire_glyph() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n@@....\n......";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };

    handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::Rotate180);

    let DocLine::Grid(grid) = &lines[1] else {
        panic!("expected grid")
    };
    assert!(grid.get(0, 0).is_clear());
    assert!(grid.get(0, 1).is_clear());
    assert!(grid.get(0, 2).is_clear());
    assert!(grid.get(1, 0).is_clear());
    assert!(grid.get(1, 1).is_clear());
    assert!(grid.get(1, 2).is_bitmap_filled());
}

#[test]
fn rotate_cw_blocked_on_non_square_glyph() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 3 2\n@@@@..\n......";
    let lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };

    assert!(!can_transform(&doc, &state, SelectionTransform::RotateCW));
    assert!(!can_transform(&doc, &state, SelectionTransform::RotateCCW));
    assert!(can_transform(&doc, &state, SelectionTransform::Rotate180));
    assert!(can_transform(&doc, &state, SelectionTransform::MirrorH));
}

#[test]
fn transform_grounded_selection_becomes_floating() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 4 4\n@@@@....\n........\n........\n........";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };
    state.pixel_selection = Some(PixelSelection {
        item_idx: 0,
        row: 0,
        col: 0,
        width: 4,
        height: 1,
        float_pixels: None,
    });

    handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::MirrorH);

    let sel = state.pixel_selection.as_ref().unwrap();
    assert!(sel.is_floating());
    assert_eq!(sel.width, 4);
    assert_eq!(sel.height, 1);
    let float = sel.float_pixels.as_ref().unwrap();
    // Original: filled, filled, empty, empty → mirrored: empty, empty, filled, filled
    assert!(float.get(0, 0).is_clear());
    assert!(float.get(0, 1).is_clear());
    assert!(float.get(0, 2).is_bitmap_filled());
    assert!(float.get(0, 3).is_bitmap_filled());
}

#[test]
fn rotate_cw_selection_changes_dimensions() {
    use crate::document_io::parse_doclines;
    let content = "glyph test 4 4\n@@@@@@..\n........\n........\n........";
    let mut lines = parse_doclines(content);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();

    let mut state = EditorState::new();
    state.mode = EditMode::PixelSelect {
        item_idx: 0,
        backrefs: false,
    };
    state.pixel_selection = Some(PixelSelection {
        item_idx: 0,
        row: 0,
        col: 0,
        width: 3,
        height: 1,
        float_pixels: None,
    });

    // 3x1 selection: fits when rotated (becomes 1x3, which fits in 4x4 grid)
    assert!(can_transform(&doc, &state, SelectionTransform::RotateCW));

    handle_transform_selection(&doc, &mut lines, &mut state, SelectionTransform::RotateCW);

    let sel = state.pixel_selection.as_ref().unwrap();
    assert!(sel.is_floating());
    assert_eq!(sel.width, 1);
    assert_eq!(sel.height, 3);
}

// -----------------------------------------------------------------------
// Scale adjustment tests
// -----------------------------------------------------------------------

#[test]
fn round_half_to_even_cases() {
    assert_eq!(round_half_to_even(0.5), 0);
    assert_eq!(round_half_to_even(1.5), 2);
    assert_eq!(round_half_to_even(2.5), 2);
    assert_eq!(round_half_to_even(3.5), 4);
    assert_eq!(round_half_to_even(-0.5), 0);
    assert_eq!(round_half_to_even(-1.5), -2);
    assert_eq!(round_half_to_even(2.3), 2);
    assert_eq!(round_half_to_even(2.7), 3);
}

fn make_scale_test_doc(source: &str) -> (Document, Vec<DocLine>, EditorState) {
    let lines = crate::document_io::parse_doclines(source);
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();
    let state = EditorState::new();
    (doc, lines, state)
}

#[test]
fn adjust_scale_rescales_grid() {
    let source = "\
glyph foo 2 2
@@..
..@@
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert_eq!(can_adjust_scale(&doc, &lines, &state), Some(1));
    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));

    // Header should now have scale 2
    let header = lines[0].as_text().unwrap();
    assert!(header.contains("scale 2"), "header: {header}");

    // Grid should be 4x4
    let grid = lines[1].as_grid().unwrap();
    assert_eq!((grid.width, grid.height), (4, 4));
    // Top-left 2×2 block should be filled (was one filled pixel)
    assert!(grid.get(0, 0).is_bitmap_filled());
    assert!(grid.get(0, 1).is_bitmap_filled());
    assert!(grid.get(1, 0).is_bitmap_filled());
    assert!(grid.get(1, 1).is_bitmap_filled());
    // Top-right 2×2 block should be empty
    assert!(grid.get(0, 2).is_clear());
}

#[test]
fn adjust_scale_leaves_no_custom_details() {
    // Scale 2 → 3 halves every source cell across a destination cell, so
    // the exact rescale produces regions no shape code can spell. `.unf`
    // has no syntax for those (they serialized as `??`), so the grid the
    // editor writes back must carry plain codes only.
    let source = "\
glyph foo 1 1 scale 2
@@..
....
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert_eq!(can_adjust_scale(&doc, &lines, &state), Some(2));
    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 3));

    let grid = lines[1].as_grid().unwrap();
    assert_eq!((grid.width, grid.height), (3, 3));
    assert!(grid.details.is_empty(), "details left: {:?}", grid.details);
    for row in 0..grid.height {
        for col in 0..grid.width {
            let shape = grid.get(row, col);
            assert_ne!(
                shape.shape_id(),
                crate::pixel::PX_CUSTOM,
                "cell ({row}, {col}) is a custom detail"
            );
            let [c1, c2] = crate::pixel::shape_to_chars(shape);
            assert!(
                (c1, c2) != ('?', '?'),
                "cell ({row}, {col}) serializes as ??"
            );
        }
    }
}

/// Adjust scale must write the hardblanks back out: a `$$` is a claim on
/// the cell, so the cells it grows into (or collapses onto) are claimed
/// too. The exact rescale used to hand a hardblank to the geometry layer,
/// where it reads as the nothing it draws, and the claim was simply gone
/// from the rewritten grid.
#[test]
fn adjust_scale_carries_hardblanks() {
    let source = "\
glyph foo 2 1
@@$$
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));
    let grid = lines[1].as_grid().unwrap();
    assert_eq!((grid.width, grid.height), (4, 2));
    for row in 0..2u16 {
        for col in 2..4u16 {
            assert!(
                grid.get(row, col).is_hardblank(),
                "cell ({row}, {col}) lost its claim: {:?}",
                crate::pixel::shape_to_chars(grid.get(row, col)),
            );
        }
    }

    // And back down again — the claim survives the round trip.
    let (doc, _) = crate::document_io::derive_document(&lines, "test.unf".into()).unwrap();
    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 1));
    let grid = lines[1].as_grid().unwrap();
    assert_eq!((grid.width, grid.height), (2, 1));
    assert!(grid.get(0, 0).is_bitmap_filled());
    assert!(grid.get(0, 1).is_hardblank());
}

#[test]
fn adjust_scale_updates_ref_offsets() {
    let source = "\
glyph foo 3 3
......
......
......
ref bar 2 4
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));

    // ref line should have scaled offsets: 2*2/1=4, 4*2/1=8
    let ref_text = lines[2].as_text().unwrap();
    assert_eq!(ref_text.trim(), "ref bar 4 8");
}

/// `tokenize_tokens` drops the trailing `// …` comment, so a rewrite built
/// from its tokens alone silently erases comments on every line the scale
/// adjustment touches. They are data the user wrote; keep them.
#[test]
fn adjust_scale_keeps_trailing_comments() {
    let source = "\
glyph foo 2 2 // header note
@@..
..@@
ref bar 1 1 // keep me
anchor -a 0 0 // and me
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));

    let header = lines[0].as_text().unwrap();
    assert!(header.contains("scale 2"), "header: {header}");
    assert!(header.ends_with("// header note"), "header: {header}");
    assert_eq!(lines[2].as_text().unwrap().trim(), "ref bar 2 2 // keep me");
    assert_eq!(
        lines[3].as_text().unwrap().trim(),
        "anchor -a 0..1 0..1 // and me"
    );
}

#[test]
fn adjust_scale_updates_anchor_positions() {
    let source = "\
glyph foo 4 4
........
........
........
........
anchor top 1 2
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 3));

    let anchor_text = lines[2].as_text().unwrap();
    // Single cell at scale 1 → 3-cell range at scale 3
    assert_eq!(anchor_text.trim(), "anchor top 3..5 6..8");
}

#[test]
fn adjust_scale_noop_for_same_scale() {
    let source = "\
glyph foo 2 2 scale 2
@@@@
@@@@
@@@@
@@@@
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert_eq!(can_adjust_scale(&doc, &lines, &state), Some(2));
    assert!(!handle_adjust_scale(&doc, &mut lines, &mut state, 2));
}

#[test]
fn adjust_scale_removes_scale_when_1() {
    let source = "\
glyph foo 2 2 scale 2
@@@@
@@@@
@@@@
@@@@
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 1));

    let header = lines[0].as_text().unwrap();
    assert!(!header.contains("scale"), "header: {header}");
    let grid = lines[1].as_grid().unwrap();
    assert_eq!((grid.width, grid.height), (2, 2));
}

#[test]
fn adjust_scale_undo_restores_original() {
    let source = "\
glyph foo 2 2
@@..
..@@
ref bar 1 2
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    let original_lines = lines.clone();
    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 3));

    // Verify it changed
    assert_ne!(lines, original_lines);

    // Undo
    state.undo.undo(&mut lines);
    assert_eq!(lines, original_lines);
}

#[test]
fn adjust_scale_anchor_range() {
    let source = "\
glyph foo 4 4
........
........
........
........
anchor top 1..3 2..3
";
    let (doc, mut lines, mut state) = make_scale_test_doc(source);
    state.cursor = c(0, 0);

    assert!(handle_adjust_scale(&doc, &mut lines, &mut state, 2));

    let anchor_text = lines[2].as_text().unwrap();
    assert_eq!(anchor_text.trim(), "anchor top 2..7 4..7");
}
