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

/// `child` is 4x4 with ink in its top-left 2x2 cells; `parent` states no size
/// and only places `refs`.
///
/// DocLines: 0 header child, 1 grid child, 2 blank, 3 "glyph parent", 4.. refs.
/// Item indices: child = 0, blank line = 1, parent = 2.
fn dimensionless_parent_doc(refs: &[&str]) -> String {
    let mut s = String::from("glyph child 4 4\n");
    for r in 0..4 {
        for c in 0..4 {
            s.push_str(if r < 2 && c < 2 { "@@" } else { ".." });
        }
        s.push('\n');
    }
    s.push_str("\nglyph parent\n");
    for gref in refs {
        s.push_str(gref);
        s.push('\n');
    }
    s
}

#[track_caller]
fn assert_grid_clear(grid: &crate::document::PixelGrid) {
    for r in 0..grid.height {
        for c in 0..grid.width {
            assert!(grid.get(r, c).is_clear(), "cell ({r}, {c}) is painted");
        }
    }
}

/// Click into `parent`'s composite, which is drawn on its first ref line
/// (DocLine 4), and check that it is being edited and nothing was written.
#[track_caller]
fn enter_dimensionless_parent(h: &mut EditorHarness, row: i16, col: i16) {
    let before = h.lines.clone();
    h.click_grid_cell(4, row, col);
    h.frame();
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 2, .. }),
        "clicking the composite edits it: {:?}",
        h.state.mode
    );
    assert_eq!(h.lines, before, "entering the grid is not a stroke");
}

/// A header that states no size has no grid of its own, so a stroke on its
/// composite used to have nothing to land in. The first stroke pins the size
/// the composite is drawn at, and is carried out on the grid that gave it.
#[test]
fn the_first_stroke_on_a_dimensionless_glyph_pins_its_size() {
    let mut h = EditorHarness::new(&dimensionless_parent_doc(&[
        "ref child 0 0",
        "ref child 4 0",
    ]));
    let original_lines = h.lines.clone();
    enter_dimensionless_parent(&mut h, 1, 5);

    h.click_grid_cell(4, 3, 6);
    h.frame();
    assert_eq!(h.text(3), "glyph parent 8 4");
    let grid = h.grid(4);
    assert_eq!((grid.width, grid.height), (8, 4));
    assert!(
        grid.get(3, 6).is_bitmap_filled(),
        "the stroke that pinned the size paints"
    );
    assert_eq!(h.text(5), "ref child 0 0");
    assert_eq!(h.text(6), "ref child 4 0");
    assert_view_consistent(&h);

    h.click_grid_cell(4, 0, 0);
    assert!(
        h.grid(4).get(0, 0).is_bitmap_filled(),
        "so does the next one"
    );

    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    h.frame();
    assert_eq!(
        h.lines, original_lines,
        "nothing pins it again but a stroke"
    );
    assert_view_consistent(&h);
}

/// Only the composite's positive part becomes the grid; what a negative ref
/// offset reaches stays outside it. A stroke out there is still a stroke on
/// the glyph, so it pins the size — and then, on the new grid, has no cell to
/// land in.
#[test]
fn a_stroke_at_negative_coordinates_pins_the_size_and_paints_nothing() {
    let mut h = EditorHarness::new(&dimensionless_parent_doc(&["ref child -2 -1"]));
    enter_dimensionless_parent(&mut h, 0, 0);

    h.click_grid_cell(4, -1, -2);
    h.frame();
    assert_eq!(h.text(3), "glyph parent 2 3");
    let grid = h.grid(4);
    assert_eq!((grid.width, grid.height), (2, 3));
    assert_grid_clear(grid);
    assert_eq!(h.text(5), "ref child -2 -1", "the ref stays where it was");
    assert_view_consistent(&h);

    h.click_grid_cell(4, 0, 1);
    assert!(h.grid(4).get(0, 1).is_bitmap_filled());
    assert_eq!(h.text(3), "glyph parent 2 3");
}

/// A composite drawn entirely at negative coordinates has no positive part to
/// make a grid of, and a zero-sized grid is not one; the header is left alone.
#[test]
fn a_dimensionless_glyph_with_no_positive_part_is_left_alone() {
    let mut h = EditorHarness::new(&dimensionless_parent_doc(&["ref child -4 0"]));
    let original_lines = h.lines.clone();
    enter_dimensionless_parent(&mut h, 0, -4);

    h.click_grid_cell(4, 0, -3);
    h.frame();
    assert_eq!(h.lines, original_lines);
    assert_view_consistent(&h);
}

/// Clicking into the grid on the way to a layer is not drawing.
#[test]
fn passing_through_the_grid_of_a_dimensionless_glyph_writes_nothing() {
    let mut h = EditorHarness::new(&dimensionless_parent_doc(&[
        "ref child 0 0",
        "ref child 4 0",
    ]));
    let original_lines = h.lines.clone();
    enter_dimensionless_parent(&mut h, 1, 1);

    h.key(Key::Num2);
    h.frame();
    assert!(
        matches!(
            h.state.mode,
            EditMode::LayerMove {
                item_idx: 2,
                layer_idx: 0
            }
        ),
        "{:?}",
        h.state.mode
    );
    assert_eq!(h.lines, original_lines);
}

/// A plain right-click erases, and a glyph with no grid has nothing to erase:
/// that is no reason to give it a size.
#[test]
fn erasing_on_a_dimensionless_glyph_writes_nothing() {
    let mut h = EditorHarness::new(&dimensionless_parent_doc(&[
        "ref child 0 0",
        "ref child 4 0",
    ]));
    let original_lines = h.lines.clone();
    enter_dimensionless_parent(&mut h, 1, 1);

    h.right_click_grid_cell_mod(4, 0, 0, Modifiers::NONE);
    h.frame();
    assert_eq!(h.lines, original_lines);
}

// -- scroll persistence across zoom changes ----------------------------------

/// The view as drawn, one line per visual line: where it sits and what it is.
fn rendered(h: &EditorHarness) -> Vec<String> {
    h.snap()
        .vlines
        .iter()
        .map(|vl| {
            format!(
                "{} {} {} {:?} {:?}",
                vl.doc_line, vl.y, vl.height, vl.gutter, vl.kind
            )
        })
        .collect()
}

/// Typing on a line whose reparse is deferred — a `ref` line, a heading —
/// changes the text and nothing the document was parsed into, so the view lays
/// out that line's segment again and splices it into the view it had rather
/// than laying out the whole file. What comes out must be the view a build from
/// scratch lays out, a folded block later in the file included.
#[test]
fn a_patched_view_is_the_view_a_rebuild_lays_out() {
    let mut h = EditorHarness::new(
        "# section\n// a comment\nglyph foo 2 2\n@@@@\n..@@\n\nglyph bar\nref foo 0 0\nref nope 1 0\n\
         // between\n\nglyph baz 2 2\n@@..\n..@@\n\n## tail\nx\n",
    );
    let line_of = |h: &EditorHarness, text: &str| {
        (0..h.lines.len())
            .find(|&i| h.lines[i].as_text() == Some(text))
            .unwrap_or_else(|| panic!("no line {text:?}"))
    };
    h.click_fold_marker(line_of(&h, "glyph baz 2 2"));
    h.frame();

    let rebuilt = |h: &mut EditorHarness| {
        h.state.view_cache = None;
        h.state.stale_view = None;
        h.frame();
        rendered(h)
    };
    let check = |h: &mut EditorHarness, what: &str, edit: &dyn Fn(&mut EditorHarness)| {
        let patches = h.state.view_patches;
        edit(h);
        h.frame();
        assert!(
            h.state.view_patches > patches,
            "{what}: the view was rebuilt, not patched"
        );
        let patched = rendered(h);
        assert_eq!(patched, rebuilt(h), "{what}");
    };

    // Undefined, so the line carries an error span, which only the glyph block
    // it belongs to knows to give it.
    let r = line_of(&h, "ref nope 1 0");
    h.click_text(r, "ref nope 1 0".len());
    check(&mut h, "a digit on a ref line", &|h| h.type_text("1"));
    check(&mut h, "two backspaces", &|h| {
        h.key(Key::Backspace);
        h.key(Key::Backspace);
    });
    assert_eq!(h.text(r), "ref nope 1 ");

    // Folded, so the segment laid out again has a line the fold must hide.
    // From here the document reparses on every keystroke. The resolution is
    // held, as the app's is until the rebuild behind the edit lands, so the
    // parse is the only thing that moved.
    h.hold_resolution = true;
    let comment = line_of(&h, "// between");
    h.click_text(comment, "// between".len());
    check(&mut h, "a character in a comment", &|h| h.type_text("!"));
    assert_eq!(h.text(comment), "// between!");

    // Leaving the edited `ref` line reparses its block, whose composite moves
    // with the offset: the block is composed again, and nothing else is.
    h.click_text(r, "ref nope 1 ".len());
    h.type_text("0");
    let foo = line_of(&h, "ref foo 0 0");
    h.click_text(foo, "ref foo ".len());
    h.key(Key::Delete);
    h.type_text("3");
    check(&mut h, "the caret leaving an edited ref line", &|h| {
        h.key(Key::ArrowUp)
    });
    assert_eq!(h.text(foo), "ref foo 3 0");

    h.hold_resolution = false;
    h.frame();
    let heading = line_of(&h, "## tail");
    h.click_fold_marker(heading);
    h.frame();
    assert!(
        !rendered(&h).iter().any(|l| l.contains("\"x\"")),
        "the section is folded: {:#?}",
        rendered(&h)
    );
    h.click_text(heading, 0);
    check(&mut h, "a heading one level deeper", &|h| h.type_text("#"));
    assert_eq!(h.text(heading), "### tail");
    assert!(
        !rendered(&h).iter().any(|l| l.contains("\"x\"")),
        "the section is still folded"
    );
}
