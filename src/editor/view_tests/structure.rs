//! Edits that change the document *structure*: a header gaining or losing
//! its grid, a ref line reparsed, and the view cache behind it.

use super::*;

/// Enter at the end of a grid-owning header must open the new line *below*
/// the grid, leaving the header/grid pair and everything after it intact.
/// It used to split the pair, which replaced the grid with a fresh empty one
/// and demoted the pixel art to raw text, shifting the following glyph.
#[test]
fn enter_at_end_of_grid_header_opens_line_below_grid() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    let original_lines = h.lines.clone();

    h.click_text(0, 13); // end of "glyph foo 4 4"
    h.key(Key::Enter);

    // Local effect only: one new blank line after foo's grid.
    assert_eq!(h.text(0), "glyph foo 4 4");
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (4, 4));
    assert!(grid.get(2, 2).is_bitmap_filled(), "pixel art must survive");
    assert_eq!(h.text(2), "");
    assert_eq!(h.cursor(), Caret::new(2, 0));
    assert_eq!(h.text(3), "");
    assert_eq!(h.text(4), "glyph bar 4 2");
    assert_eq!(h.grid(5).height, 2);

    // The rendered view is immediately consistent: foo's grid rows on line 1,
    // bar's header and grid where they belong, no line eaten.
    assert_eq!(h.grid_row_count(1), 4);
    assert_eq!(h.grid_row_count(5), 2);
    assert_view_consistent(&h);

    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}

/// Enter in the middle of a grid-owning header detaches the grid; the grid
/// demotes to text rows immediately and locally — the following glyph keeps
/// its own grid, and no visual line is misattributed while the change is
/// pending.
#[test]
fn enter_mid_header_demotes_grid_immediately_and_locally() {
    let mut h = EditorHarness::new(&two_glyph_doc());
    let original_lines = h.lines.clone();

    h.click_text(0, 9); // "glyph foo| 4 4"
    h.key(Key::Enter);

    assert_eq!(h.text(0), "glyph foo");
    assert_eq!(h.text(1), " 4 4");
    assert_eq!(h.cursor(), Caret::new(1, 0));
    // The orphaned grid demoted to its four pixel-text rows.
    assert_eq!(h.text(2), "@@......");
    assert_eq!(h.text(5), "......@@");
    // The following glyph is untouched.
    assert_eq!(h.text(7), "glyph bar 4 2");
    assert_eq!(h.grid(8).height, 2);
    assert_eq!(h.grid_row_count(8), 2);
    assert_view_consistent(&h);

    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}

/// Deleting the dimensions off a `glyph foo 16 16` whose body is only a `ref`
/// must not conjure pixel rows: the grid the parser attached to the header was
/// empty and never text in the file, so it goes away with the dimensions.
#[test]
fn deleting_dims_of_ref_only_glyph_drops_the_empty_grid() {
    let mut h =
        EditorHarness::new("glyph foo 16 16\nref bar\n\nglyph bar 4 2\n@@......\n......@@\n");
    let original_lines = h.lines.clone();
    assert!(matches!(h.lines[1], DocLine::Grid(_)));

    h.click_text(0, 15); // end of "glyph foo 16 16"
    for _ in 0..6 {
        h.key(Key::Backspace);
    }
    // The caret leaves the header, so the deferred reconcile runs.
    h.click_text(2, 7);

    assert_eq!(h.text(0), "glyph foo");
    assert_eq!(h.text(1), "ref bar");
    assert_eq!(h.text(2), "");
    assert_eq!(h.text(3), "glyph bar 4 2");
    assert_eq!(h.grid(4).height, 2);
    assert_view_consistent(&h);

    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}

/// A header with a valued flag before the dimensions (`advance 0 4 3`) must
/// produce a 4x3 grid that the document model accepts. Reconciliation and
/// derivation used to parse the dimensions differently (0x4 vs 4x3), leaving
/// the editor in a permanently inconsistent state where the visual grid
/// swallowed the following line.
#[test]
fn valued_flag_before_dims_creates_matching_grid() {
    let mut h = EditorHarness::new("// top\n\n\nglyph bar 4 2\n@@......\n......@@\n");

    h.click_text(1, 0);
    h.type_text("glyph baz advance 0 4 3");
    h.key(Key::ArrowDown); // leave the header line -> flush

    assert_eq!(h.text(1), "glyph baz advance 0 4 3");
    let grid = h.grid(2);
    assert_eq!((grid.width, grid.height), (4, 3));
    assert_eq!(h.grid_row_count(2), 3);
    // The blank line and the following glyph survive unshifted.
    assert_eq!(h.text(3), "");
    assert_eq!(h.text(4), "glyph bar 4 2");
    assert_eq!(h.grid(5).height, 2);
    assert_eq!(h.grid_row_count(5), 2);
    assert_view_consistent(&h);
}

/// A ref-only glyph draws its composite as a grid on the first `ref` line. That
/// grid must survive editing the ref name: the first keystroke used to reparse
/// straight away, and a half-typed name resolves to nothing, so the graphical
/// grid collapsed to text rows. Worse, the deferral that follows kept it
/// collapsed even after the name was typed back in full.
#[test]
fn editing_a_ref_line_keeps_the_composite_grid() {
    // DocLines: 0 header base, 1 grid 4x4, 2 blank, 3 "glyph comp", 4 "ref base"
    let src = "glyph base 4 4\n@@......\n..@@....\n....@@..\n......@@\n\nglyph comp\nref base\n";
    let mut h = EditorHarness::new(src);
    assert_eq!(h.grid_row_count(4), 4, "the composite renders as a grid");

    h.click_text(4, 8); // end of "ref base"
    h.key(Key::Backspace); // "ref bas" — resolves to nothing
    assert_eq!(
        h.grid_row_count(4),
        4,
        "a half-typed ref name must not collapse the composite grid"
    );

    h.type_text("e"); // back to "ref base"
    assert_eq!(h.text(4), "ref base");
    assert_eq!(h.grid_row_count(4), 4);

    h.key(Key::ArrowUp); // leave the line -> flush
    assert_eq!(h.grid_row_count(4), 4);
    assert_view_consistent(&h);
}

/// Leaving a genuinely undefined ref behind still takes effect — the deferral
/// holds the last good rendering, it does not freeze the view.
#[test]
fn leaving_a_broken_ref_line_updates_the_composite() {
    let src = "glyph base 4 4\n@@......\n..@@....\n....@@..\n......@@\n\nglyph comp\nref base\n";
    let mut h = EditorHarness::new(src);

    h.click_text(4, 8);
    h.type_text("x"); // "ref basex"
    h.key(Key::ArrowUp);

    assert_eq!(h.text(4), "ref basex");
    assert!(
        h.grid_row_count(4) < 4,
        "an undefined ref has no composite to draw"
    );
    assert_view_consistent(&h);
}

/// An unterminated quote anywhere in the file used to abort `derive_document`
/// wholesale, leaving the view built from the *previous* item structure over the
/// new lines: grid rows were painted onto text lines and the real grid line got
/// no visual line at all. A line the grammar cannot read is one opaque text
/// item, so the structure keeps matching the buffer.
#[test]
fn an_unparseable_line_does_not_misattribute_the_view() {
    let mut h = EditorHarness::new(&two_glyph_doc());

    h.click_text(2, 0); // the blank line between the two glyphs
    h.paste("`oops\nmore");

    assert_eq!(h.text(2), "`oops");
    assert_eq!(h.text(3), "more");
    assert_eq!(h.text(4), "glyph bar 4 2");
    // Both glyphs still render as grids, on their own lines.
    assert_eq!(h.grid_row_count(1), 4);
    assert_eq!(h.grid_row_count(5), 2);
    assert_view_consistent(&h);

    // And the document still holds both glyphs.
    let glyphs = h
        .doc
        .items
        .iter()
        .filter(|i| matches!(i, crate::document::DocumentItem::Glyph { .. }))
        .count();
    assert_eq!(glyphs, 2);
}

/// Typing a quote character into a header, on the way to a quoted name, is one
/// unparseable line while it is open. It must not shift the view around either.
#[test]
fn an_unparseable_header_line_does_not_misattribute_the_view() {
    let mut h = EditorHarness::new(&two_glyph_doc());

    h.click_text(3, 0); // "glyph bar 4 2"
    h.type_text("`");
    h.key(Key::ArrowUp); // leave the line -> flush

    assert_eq!(h.text(3), "`glyph bar 4 2");
    assert_eq!(h.grid_row_count(1), 4, "the other glyph is untouched");
    assert_view_consistent(&h);
}

#[test]
fn view_cache_reused_when_idle_and_rebuilt_on_edit() {
    let mut h = EditorHarness::new(&sample_doc());

    let ptr_before = h.state.view_cache.as_ref().expect("cache built").data_ptr();
    h.frame();
    h.frame();
    let ptr_idle = h.state.view_cache.as_ref().expect("cache kept").data_ptr();
    assert_eq!(
        ptr_before, ptr_idle,
        "idle frames must reuse the cached view"
    );

    h.click_text(0, 6);
    h.type_text("X");
    h.frame();
    assert_eq!(h.text(0), "glyph Xfoo 16 16");
    // The rendered view (not just the DocLines) must reflect the edit; a
    // pointer comparison would be flaky since a rebuilt Arc can be
    // reallocated at the freed cache's address.
    let rendered: Vec<&str> = h
        .snap()
        .vlines
        .iter()
        .filter_map(|vl| match &vl.kind {
            crate::editor::harness::SnapKind::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        rendered.contains(&"glyph Xfoo 16 16"),
        "edited text must appear in the rebuilt view: {rendered:?}"
    );
}

// -- scroll persistence across zoom changes ----------------------------------
