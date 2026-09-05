//! Ctrl/Cmd+/ — commenting lines out and taking the comment back off, with
//! the grid demotion/promotion that goes with it.

use super::*;

fn toggle_comment(h: &mut EditorHarness) {
    h.key_mod(Key::Slash, Modifiers::COMMAND);
}

/// The plain case: one line, commented and uncommented again. Uncommenting is
/// greedy — the indentation goes with the marker.
#[test]
fn toggles_one_line_and_eats_its_indentation() {
    let mut h = EditorHarness::new("    ref foo\nref bar\n");
    h.click_text(0, 8);
    toggle_comment(&mut h);
    assert_eq!(h.text(0), "//     ref foo");
    assert_eq!(h.text(1), "ref bar");
    // The caret rode along with the text it was in.
    assert_eq!(h.cursor(), Caret::new(0, 11));

    toggle_comment(&mut h);
    assert_eq!(h.text(0), "ref foo");
    assert_eq!(h.cursor(), Caret::new(0, 4));
}

/// A trailing `// …` on a directive is not a whole-line comment: the chord
/// comments the line rather than reading the one already there.
#[test]
fn a_trailing_comment_is_not_a_line_comment() {
    let mut h = EditorHarness::new("ref foo // why\n");
    h.click_text(0, 0);
    toggle_comment(&mut h);
    assert_eq!(h.text(0), "// ref foo // why");
    toggle_comment(&mut h);
    assert_eq!(h.text(0), "ref foo // why");
}

/// A blank line takes the bare marker, so nothing gains trailing whitespace,
/// and it comes back blank.
#[test]
fn a_blank_line_takes_the_bare_marker() {
    let mut h = EditorHarness::new("ref foo\n\nref bar\n");
    h.click_text(1, 0);
    toggle_comment(&mut h);
    assert_eq!(h.text(1), "//");
    toggle_comment(&mut h);
    assert_eq!(h.text(1), "");
}

/// A header and the grid under it are one block: commenting the header
/// demotes the grid to its pixel rows and comments those too, and doing it
/// again fuses them back into the same grid.
#[test]
fn commenting_a_header_carries_its_grid_out_and_back() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    let original_lines = h.lines.clone();

    h.click_text(0, 0);
    toggle_comment(&mut h);

    assert_eq!(h.text(0), "// glyph foo 4 4");
    assert_eq!(h.text(1), "// @@......");
    assert_eq!(h.text(2), "// ..@@....");
    assert_eq!(h.text(3), "// ....@@..");
    assert_eq!(h.text(4), "// ......@@");
    assert_eq!(h.text(5), "");
    assert_eq!(h.text(6), "glyph bar 4 2");
    assert_view_consistent(&h);
    // Nothing in the derived document draws foo any more.
    assert!(!h.doc.items.iter().any(|it| matches!(
        it,
        crate::document::DocumentItem::Glyph { name, .. } if name.0 == "foo"
    )));

    // Back again: the header pulls its commented rows with it.
    toggle_comment(&mut h);
    assert_eq!(h.text(0), "glyph foo 4 4");
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (4, 4));
    for r in 0..4u16 {
        for c in 0..4u16 {
            assert_eq!(
                grid.get(r, c).is_bitmap_filled(),
                r == c,
                "pixel {r},{c} survived the round trip"
            );
        }
    }
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}

/// Commenting from the grid line itself reaches the header above it, so the
/// grid is never left orphaned under a live header.
#[test]
fn commenting_a_grid_reaches_its_header() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    h.click_text(0, 0);
    h.key(Key::ArrowDown); // onto the grid line itself
    assert_eq!(h.cursor(), Caret::new(1, 0));
    toggle_comment(&mut h);

    assert_eq!(h.text(0), "// glyph foo 4 4");
    assert_eq!(h.text(4), "// ......@@");
    assert_eq!(h.text(5), "");
    assert_view_consistent(&h);
}

/// One undo takes the whole toggle back, grid and all.
#[test]
fn one_undo_takes_the_whole_toggle_back() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    let original_lines = h.lines.clone();

    h.click_text(0, 0);
    toggle_comment(&mut h);
    assert_eq!(h.text(0), "// glyph foo 4 4");

    cmd_z(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}

/// The whole line range the selection touches is the target, both ends
/// included, and the selection survives so the chord can be pressed twice.
#[test]
fn a_selection_comments_every_line_it_touches() {
    let mut h = EditorHarness::new("ref a\nref b\nref c\nref d\n");
    h.click_text(0, 3);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    toggle_comment(&mut h);

    assert_eq!(h.text(0), "// ref a");
    assert_eq!(h.text(1), "// ref b");
    assert_eq!(h.text(2), "// ref c");
    assert_eq!(h.text(3), "ref d");
    assert!(h.state.selection_anchor.is_some(), "selection survives");

    toggle_comment(&mut h);
    assert_eq!(h.text(0), "ref a");
    assert_eq!(h.text(2), "ref c");
    assert_eq!(h.text(3), "ref d");
}

/// A mixed range is commented, not uncommented: the already-commented lines
/// take a second marker, and one more toggle is the way back.
#[test]
fn a_mixed_range_comments_rather_than_uncomments() {
    let mut h = EditorHarness::new("// ref a\nref b\n");
    h.click_text(0, 0);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    toggle_comment(&mut h);

    assert_eq!(h.text(0), "// // ref a");
    assert_eq!(h.text(1), "// ref b");

    toggle_comment(&mut h);
    assert_eq!(h.text(0), "// ref a");
    assert_eq!(h.text(1), "ref b");
}

/// A blank line inside the range does not stop the range from being read as
/// "all comments".
#[test]
fn a_blank_line_does_not_block_uncommenting() {
    let mut h = EditorHarness::new("// ref a\n\n// ref b\n");
    h.click_text(0, 0);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    toggle_comment(&mut h);

    assert_eq!(h.text(0), "ref a");
    assert_eq!(h.text(1), "");
    assert_eq!(h.text(2), "ref b");
}

/// A header with no pixel rows written under it keeps the empty grid every
/// dimensioned header owns, rather than losing it to the round trip.
#[test]
fn an_empty_glyph_survives_the_round_trip() {
    let mut h = EditorHarness::new("glyph foo 4 4\n\nglyph bar 4 4\n");
    let original_lines = h.lines.clone();

    h.click_text(0, 0);
    toggle_comment(&mut h);
    assert_eq!(h.text(0), "// glyph foo 4 4");

    toggle_comment(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}
