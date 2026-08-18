//! Copy, cut and paste over text — including the line-wise forms, which
//! take a glyph header and its grid together.

use super::*;

fn text_doc() -> String {
    "line one\nline two\nline three\n".to_string()
}

// -- paste tests --

#[test]
fn paste_single_line_and_undo() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(0, 5);
    h.paste("ABC");
    assert_eq!(h.text(0), "line ABCone");
    assert_eq!(h.cursor(), Caret::new(0, 8));

    cmd_z(&mut h);
    assert_eq!(h.text(0), "line one");
    assert_eq!(h.cursor(), Caret::new(0, 5));
}

#[test]
fn paste_multiline_undoes_atomically() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(0, 4);
    h.paste("X\nY\nZ");
    assert_eq!(h.text(0), "lineX");
    assert_eq!(h.text(1), "Y");
    assert_eq!(h.text(2), "Z one");
    assert_eq!(h.cursor(), Caret::new(2, 1));

    cmd_z(&mut h);
    assert_eq!(h.text(0), "line one");
}

#[test]
fn paste_multiline_with_crlf_and_undo() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(0, 4);
    h.paste("X\r\nY\r\nZ");
    assert_eq!(h.text(0), "lineX");
    assert_eq!(h.text(1), "Y");
    assert_eq!(h.text(2), "Z one");
    assert_eq!(h.cursor(), Caret::new(2, 1));

    cmd_z(&mut h);
    assert_eq!(h.text(0), "line one");
}

#[test]
fn paste_over_selection_undoes_atomically() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(0, 5);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    assert!(h.state.selection_range().is_some());
    h.paste("ABC");
    assert_eq!(h.text(0), "line ABC");

    cmd_z(&mut h);
    assert_eq!(h.text(0), "line one");
}

#[test]
fn paste_multiline_over_selection_undoes_atomically() {
    let mut h = EditorHarness::new(&text_doc());
    // Select "one\nline two\nline "
    h.click_text(0, 5);
    for _ in 0..18 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    }
    h.paste("X\nY\nZ");
    assert_eq!(h.text(0), "line X");
    assert_eq!(h.text(1), "Y");
    assert_eq!(h.text(2), "Zthree");
    assert_eq!(h.cursor(), Caret::new(2, 1));

    cmd_z(&mut h);
    assert_eq!(h.text(0), "line one");
    assert_eq!(h.text(1), "line two");
    assert_eq!(h.text(2), "line three");
}

#[test]
fn select_all_and_shift_arrow_selection() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 0);

    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    let (lo, hi) = h.state.selection_range().expect("selection");
    assert_eq!((lo, hi), (Caret::new(0, 0), Caret::new(0, 2)));

    // Plain arrow clears the selection.
    h.key(Key::ArrowLeft);
    assert!(h.state.selection_range().is_none());

    h.key_mod(Key::A, Modifiers::COMMAND);
    let (lo, hi) = h.state.selection_range().expect("select all");
    assert_eq!(lo, Caret::new(0, 0));
    assert_eq!(hi.line, 4, "select-all extends to the last doc line");
}

// -- Cmd+C / Cmd+X with no selection (line copy/cut) --

#[test]
fn copy_no_selection_copies_whole_line() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(0, 0);
    assert!(h.state.selection_range().is_none());
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some("line one\n"));
}

#[test]
fn copy_no_selection_mid_line() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(0, 4);
    assert!(h.state.selection_range().is_none());
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some("line one\n"));
}

#[test]
fn copy_no_selection_last_line_no_trailing_newline() {
    let mut h = EditorHarness::new("alpha\nbeta");
    h.click_text(1, 2);
    assert!(h.state.selection_range().is_none());
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some("beta"));
}

#[test]
fn cut_no_selection_removes_line() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(1, 3);
    assert!(h.state.selection_range().is_none());
    h.cut();
    assert_eq!(h.last_copied_text.as_deref(), Some("line two\n"));
    assert_eq!(h.text(0), "line one");
    assert_eq!(h.text(1), "line three");
    assert_eq!(h.cursor(), Caret::new(1, 0));
}

#[test]
fn copy_with_selection_still_copies_selection() {
    let mut h = EditorHarness::new(&text_doc());
    h.click_text(0, 5);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    assert!(h.state.selection_range().is_some());
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some("one"));
}

#[test]
fn copy_paste_preserves_grid_content() {
    // DocLines: 0=header "glyph bar 4 2", 1=Grid(4x2), 2=blank
    let doc = "glyph bar 4 2\n@@......\n......@@\n\n".to_string();
    let mut h = EditorHarness::new(&doc);

    // Select header + grid (lines 0..=1): click start of header, shift-down twice
    h.click_text(0, 0);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    h.key_mod(Key::ArrowDown, Modifiers::SHIFT);
    h.copy();
    let copied = h.last_copied_text.clone().unwrap();
    assert!(
        copied.contains("@@......"),
        "grid row 0 should be in clipboard"
    );
    assert!(
        copied.contains("......@@"),
        "grid row 1 should be in clipboard"
    );

    // Paste into the blank line (line 2)
    h.key(Key::ArrowDown);
    h.click_text(2, 0);
    h.paste(&copied);

    // The pasted content should reconstruct a glyph header + grid
    assert_eq!(h.text(2), "glyph bar 4 2");
    let pasted_grid = h.grid(3);
    assert_eq!(pasted_grid.width, 4);
    assert_eq!(pasted_grid.height, 2);
    assert!(pasted_grid.get(0, 0).is_bitmap_filled());
    assert!(pasted_grid.get(0, 1).is_clear());
    assert!(pasted_grid.get(1, 2).is_clear());
    assert!(pasted_grid.get(1, 3).is_bitmap_filled());

    // Undo should restore the original state
    cmd_z(&mut h);
    assert_eq!(h.text(2), "");
}

/// A multi-line paste whose last line lands on a grid-owning header must not
/// bring a grid of its own along: `parse_doclines` gives every dimensioned
/// header a grid, and that fresh empty one used to be spliced in *between* the
/// header and the real grid — orphaning the pixel art (demoted to raw text)
/// and leaving the glyph with an empty bitmap.
#[test]
fn multiline_paste_at_a_grid_header_keeps_the_existing_grid() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    let original_lines = h.lines.clone();

    h.click_text(0, 0); // start of "glyph foo 4 4"
    h.paste("// x\n");

    assert_eq!(h.text(0), "// x");
    assert_eq!(h.text(1), "glyph foo 4 4");
    let grid = h.grid(2);
    assert_eq!((grid.width, grid.height), (4, 4));
    assert!(grid.get(2, 2).is_bitmap_filled(), "pixel art must survive");
    assert_eq!(h.grid_row_count(2), 4, "and stay a graphical grid");
    assert_eq!(h.text(3), "");
    assert_eq!(h.text(4), "glyph bar 4 2");
    assert_eq!(h.grid_row_count(5), 2);
    assert_eq!(h.cursor(), Caret::new(1, 0));
    assert_view_consistent(&h);

    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}

/// A pasted header that does *not* land on an existing grid still gets one of
/// its own — the rule above drops the synthesized grid only where a real one is
/// waiting for that header.
#[test]
fn multiline_paste_of_a_header_still_creates_its_grid() {
    let mut h = EditorHarness::new("// head\n\nglyph bar 4 2\n@@......\n......@@\n");
    h.click_text(1, 0);
    h.paste("glyph foo 2 1\n// tail");

    assert_eq!(h.text(1), "glyph foo 2 1");
    let grid = h.grid(2);
    assert_eq!((grid.width, grid.height), (2, 1));
    assert_eq!(h.grid_row_count(2), 1);
    assert_eq!(h.text(3), "// tail");
    assert_eq!(h.text(4), "glyph bar 4 2");
    assert_eq!(h.grid_row_count(5), 2);
    assert_view_consistent(&h);
}

/// Nor does it drop a pasted grid that carries pixels: pasting a whole glyph
/// over a header keeps the *pasted* bitmap, rather than re-attaching the old
/// grid to the new header.
#[test]
fn multiline_paste_of_a_glyph_with_pixels_keeps_the_pasted_bitmap() {
    let mut h = EditorHarness::new(&two_glyph_doc());

    h.click_text(0, 0);
    h.key_mod(Key::End, Modifiers::SHIFT); // select the whole header line
    h.paste("glyph foo 4 4\n@@@@@@@@\n@@@@@@@@\n@@@@@@@@\n@@@@@@@@");

    assert_eq!(h.text(0), "glyph foo 4 4");
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (4, 4));
    for r in 0..4 {
        for c in 0..4 {
            assert!(
                grid.get(r, c).is_bitmap_filled(),
                "pasted pixel {r},{c} survives"
            );
        }
    }
    assert_eq!(h.grid_row_count(1), 4);
    assert_view_consistent(&h);
}

/// Cmd+X with no selection cuts the caret's line. On a grid-owning header the
/// line range used to stop *at* the grid line, so the grid was deleted with the
/// header while only the header text reached the clipboard: the pixel art was
/// gone and unpasteable. The whole glyph block goes, pixel rows included.
#[test]
fn line_cut_on_a_grid_header_takes_the_pixels_along() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    let original_lines = h.lines.clone();

    h.click_text(0, 4);
    assert!(h.state.selection_range().is_none());
    h.cut();

    let copied = h.last_copied_text.clone().expect("cut copies something");
    assert_eq!(
        copied, "glyph foo 4 4\n@@......\n..@@....\n....@@..\n......@@\n",
        "the clipboard must carry the header and its pixel rows"
    );
    assert_eq!(h.text(0), "");
    assert_eq!(h.text(1), "glyph bar 4 2");
    assert_eq!(h.grid_row_count(2), 2);
    assert_view_consistent(&h);

    // What was cut pastes back as the same glyph.
    h.paste(&copied);
    assert_eq!(h.text(0), "glyph foo 4 4");
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (4, 4));
    assert!(grid.get(2, 2).is_bitmap_filled());
    assert_eq!(h.grid_row_count(1), 4);
    assert_view_consistent(&h);

    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
}

/// Cmd+C with no selection on a grid-owning header copies the block too, so
/// copy and cut put the same thing on the clipboard.
#[test]
fn line_copy_on_a_grid_header_includes_the_pixels() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    h.click_text(0, 0);
    h.copy();
    assert_eq!(
        h.last_copied_text.as_deref(),
        Some("glyph foo 4 4\n@@......\n..@@....\n....@@..\n......@@\n")
    );
    // Copy changes nothing.
    assert_eq!(h.grid_row_count(1), 4);
    assert_view_consistent(&h);
}

// -- autocomplete -----------------------------------------------------------
