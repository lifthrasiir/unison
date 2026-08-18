//! Folding: glyph blocks, heading sections, the marker columns and what a
//! fold does to the caret and the selection.

use super::*;

/// Three glyph blocks, each a header plus a grid, with a `ref` line on the
/// first.
///
/// DocLines: 0 header a, 1 grid, 2 `ref b`, 3 header b, 4 grid, 5 header c,
/// 6 grid. Groups: a = 0..3, b = 3..5, c = 5..7.
fn fold_doc() -> String {
    String::from(
        "glyph a 2 2\n....\n....\nref b\nglyph b 2 2\n@@..\n....\nglyph c 2 2\n....\n..@@\n",
    )
}

#[test]
fn a_glyph_block_folds_down_to_its_header() {
    let mut h = EditorHarness::new(&fold_doc());
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3, 4, 5, 6]);

    h.click_fold_marker(0);
    assert_eq!(
        shown_lines(&h),
        vec![0, 3, 4, 5, 6],
        "the header stays, its grid and ref lines go"
    );

    h.click_fold_marker(0);
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3, 4, 5, 6]);
    assert_view_consistent(&h);
}

/// A `scale N` grid draws one cell per subcell, so a `2 2 scale 32` block is
/// 64 rows where the font is 16 pixels tall. The editor opens with it shut.
#[test]
fn a_glyph_taller_than_twice_the_font_height_opens_folded() {
    let mut src = String::from("glyph a 2 2\n....\n....\nglyph big 2 2 scale 32\n");
    for _ in 0..64 {
        src.push_str(&".".repeat(128));
        src.push('\n');
    }
    let mut h = EditorHarness::new(&src);
    assert_eq!(
        shown_lines(&h),
        vec![0, 1, 2],
        "the ordinary block is whole; the tall one is its header alone"
    );
    let shut: Vec<usize> = h
        .fold_markers()
        .iter()
        .filter(|(.., shut)| *shut)
        .map(|(l, ..)| *l)
        .collect();
    assert_eq!(shut, vec![2]);

    // Opened by hand, it stays open: the initial fold is a one-shot.
    h.click_fold_marker(2);
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3]);
    h.frame();
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3]);
    assert_view_consistent(&h);
}

#[test]
fn every_foldable_block_gets_a_marker_and_the_shut_one_turns_its_triangle() {
    let mut h = EditorHarness::new(&fold_doc());
    let headers: Vec<usize> = h.fold_markers().iter().map(|(l, ..)| *l).collect();
    assert_eq!(headers, vec![0, 3, 5]);
    assert!(h.fold_markers().iter().all(|(.., shut)| !*shut));

    h.click_fold_marker(3);
    let shut: Vec<usize> = h
        .fold_markers()
        .iter()
        .filter(|(.., shut)| *shut)
        .map(|(l, ..)| *l)
        .collect();
    assert_eq!(shut, vec![3]);
}

/// The marker column is reserved for the page that could show one, not for
/// every page — a file with no foldable line spends no width on it.
#[test]
fn only_a_page_with_a_foldable_line_reserves_the_marker_column() {
    assert_eq!(
        EditorHarness::new(&numbered_doc(20)).snap().marker_width,
        0.0
    );
    assert!(EditorHarness::new(&fold_doc()).snap().marker_width > 0.0);
}

#[test]
fn a_shut_marker_is_only_as_tall_as_the_header_it_leaves() {
    let mut h = EditorHarness::new(&fold_doc());
    let open = h.fold_markers()[0].1.height();
    h.click_fold_marker(0);
    let shut = h.fold_markers()[0].1.height();
    assert!(
        shut < open,
        "a shut group shows one row, not the block ({shut} vs {open})"
    );
    assert!(shut <= h.snap().vlines[0].height);
}

/// A click in the gutter belongs to the marker; it must not also drop the
/// caret onto the line beside it.
#[test]
fn clicking_a_marker_does_not_move_the_caret() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_text(3, 2);
    assert_eq!(h.cursor(), Caret::new(3, 2));
    h.click_fold_marker(5);
    assert_eq!(h.cursor(), Caret::new(3, 2));
}

#[test]
fn arrows_step_over_a_shut_group_instead_of_into_it() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_fold_marker(0);

    h.click_text(0, 0);
    h.key(Key::ArrowDown);
    assert_eq!(
        h.cursor().line,
        3,
        "down off a shut header clears the group"
    );

    h.key(Key::ArrowUp);
    assert_eq!(h.cursor().line, 0, "and up comes back to the header");

    h.key_mod(Key::ArrowRight, Modifiers::COMMAND);
    let header_end = h.cursor();
    h.key(Key::ArrowRight);
    assert_eq!(
        h.cursor(),
        Caret::new(3, 0),
        "right off the end of the header opens onto the next visible line"
    );
    h.key(Key::ArrowLeft);
    assert_eq!(h.cursor(), header_end, "and left returns to where it was");
}

/// A tall group's bar runs on past the bottom of the editor's viewport, where
/// the app has its preview panel. Painting clips there and a click never
/// reaches the gutter, so the hover must not either.
#[test]
fn a_marker_below_the_viewport_does_not_shade_under_the_pointer() {
    let mut src = String::from("## section\n");
    for i in 0..20 {
        src.push_str(&format!("glyph g{i} 2 2\n....\n....\n"));
    }
    let mut h = EditorHarness::new(&src);
    h.viewport_height = Some(120.0);
    h.frame();
    h.focus();
    // The viewport shrank under a saved scroll fraction, which lands the view
    // near the end; walking the caret back to the top brings it along.
    h.key(Key::ArrowDown);
    h.key_mod(Key::Home, Modifiers::COMMAND);
    h.frame();
    assert!(h.scroll_y() < 1.0, "scrolled to the top ({})", h.scroll_y());

    let cell = h
        .fold_markers()
        .into_iter()
        .find(|(header, ..)| *header == 0)
        .expect("the heading has a marker")
        .1;
    // The bar is painted clipped to the viewport; below that edge is the
    // editor's outside.
    let bottom = h
        .painted_rects()
        .into_iter()
        .find(|r| r.rect.x_range() == cell.x_range())
        .expect("the marker was painted")
        .clip
        .max
        .y;
    assert!(
        cell.max.y > bottom,
        "the bar has to run past the viewport for this to test anything ({cell:?} vs {bottom})"
    );

    h.move_pointer(egui::pos2(cell.center().x, bottom + 8.0));
    assert_eq!(
        h.hovered_fold_marker(),
        None,
        "the pointer is below the editor entirely"
    );

    h.move_pointer(egui::pos2(cell.center().x, cell.min.y + 4.0));
    assert_eq!(
        h.hovered_fold_marker(),
        Some(0),
        "and inside the viewport it still shades"
    );
}

#[test]
fn ctrl_semicolon_folds_the_group_the_caret_sits_in() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_text(2, 3);
    h.key_mod(Key::Semicolon, Modifiers::COMMAND);

    assert_eq!(shown_lines(&h), vec![0, 3, 4, 5, 6]);
    assert_eq!(
        h.cursor(),
        Caret::new(0, 3),
        "the caret comes up to the header at the same column"
    );

    h.key_mod(Key::Semicolon, Modifiers::COMMAND);
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3, 4, 5, 6]);
}

/// A selection may *span* a shut group — only its two ends have to be
/// somewhere the user can see.
#[test]
fn a_selection_across_a_shut_group_still_covers_what_it_hides() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_fold_marker(0);

    h.click_text(0, 0);
    h.click_at_mod(h.text_pos(3, 11), Modifiers::SHIFT);
    h.copy();
    let copied = h.last_copied_text.clone().expect("nothing copied");
    assert!(
        copied.contains("ref b"),
        "the hidden lines are inside the selection: {copied:?}"
    );
}

#[test]
fn folding_over_an_end_of_the_selection_drops_it() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_text(2, 0);
    h.click_at_mod(h.text_pos(2, 5), Modifiers::SHIFT);
    assert!(h.state.selection_range().is_some());

    h.key_mod(Key::Semicolon, Modifiers::COMMAND);
    assert!(
        h.state.selection_range().is_none(),
        "an endpoint about to be hidden cancels the selection"
    );
    assert_eq!(h.cursor().line, 0);
}

#[test]
fn select_all_then_fold_keeps_the_selection() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_text(0, 0);
    h.key_mod(Key::A, Modifiers::COMMAND);
    let before = h.state.selection_range();
    assert!(before.is_some());

    h.click_fold_marker(0);
    assert_eq!(
        h.state.selection_range(),
        before,
        "neither end was inside the group, so nothing changes"
    );
}

/// Closing a group whose header has scrolled away brings the header to the top
/// of the page, rather than leaving the fold to happen out of sight.
#[test]
fn shutting_a_group_from_below_brings_its_header_to_the_top() {
    let mut src = String::from("glyph tall 2 300\n");
    for _ in 0..300 {
        src.push_str("....\n");
    }
    src.push_str("ref x\nglyph x 2 2\n....\n....\n");
    let mut h = EditorHarness::new(&src);
    // Taller than twice the font height, so it opens folded; this test is
    // about the block open.
    h.click_fold_marker(0);

    h.click_text(0, 0);
    h.key_mod(Key::End, Modifiers::COMMAND);
    assert!(h.scroll_y() > 0.0, "the header should be off the page now");

    h.click_text(2, 0);
    h.key_mod(Key::Semicolon, Modifiers::COMMAND);
    assert_eq!(h.cursor().line, 0);
    assert!(
        h.scroll_y() <= 1.0,
        "the header should have come to the top ({})",
        h.scroll_y()
    );
}

/// Opening a group adds rows *below* the header, so the page must not move.
#[test]
fn opening_a_group_leaves_the_page_where_it_was() {
    let mut src = String::from("glyph pad 2 300\n");
    for _ in 0..300 {
        src.push_str("....\n");
    }
    src.push_str("glyph a 2 2\n....\n....\nref b\nglyph b 2 2\n@@..\n....\n");
    let mut h = EditorHarness::new(&src);

    // Down to the bottom of the file, where the second block is.
    h.click_text(0, 0);
    h.key_mod(Key::End, Modifiers::COMMAND);
    for _ in 0..10 {
        h.frame();
    }

    h.toggle_fold(4);
    let shut = h.scroll_y();
    h.toggle_fold(2);
    assert_eq!(
        h.scroll_y(),
        shut,
        "the rows come back below the header, so the page must not move"
    );
}

#[test]
fn jumping_to_a_hidden_line_opens_the_group_holding_it() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_fold_marker(0);
    assert_eq!(shown_lines(&h), vec![0, 3, 4, 5, 6]);

    h.state.goto_line(2);
    h.frame();
    h.frame();
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(h.cursor().line, 2);
}

/// A fold is remembered by the header it was made on, so an edit that shifts
/// every line below it carries the fold along instead of moving it to whatever
/// glyph inherited the old line number.
#[test]
fn an_edit_above_a_shut_group_carries_the_fold_with_it() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_fold_marker(3);
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3, 5, 6]);

    h.click_text(0, 0);
    h.key(Key::Enter);
    assert_eq!(
        shown_lines(&h),
        vec![0, 1, 2, 3, 4, 6, 7],
        "glyph b is still the shut one, one line further down"
    );
}

/// Typing over a folded header keeps the fold while the caret is on the line —
/// the document is not re-derived under a live edit — but the key that leaves
/// the line has to see the grouping the edit left behind.
#[test]
fn breaking_a_folded_header_lands_the_caret_on_what_it_was_hiding() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_fold_marker(0);

    h.click_text(0, 1);
    h.key(Key::Backspace);
    assert_eq!(h.text(0), "lyph a 2 2");
    assert_eq!(
        shown_lines(&h),
        vec![0, 3, 4, 5, 6],
        "still folded while the caret is on the header"
    );

    h.key(Key::ArrowDown);
    assert_eq!(
        h.cursor().line,
        1,
        "the group is gone, so down lands on the line it used to hide"
    );
    assert!(shown_lines(&h).contains(&1));
}

/// An undo puts the caret back where the edit was, which is a jump like a
/// followed link: a group standing in front of it opens. The fold itself is
/// not on the undo stack — only the caret it has to make room for is.
#[test]
fn undo_opens_the_group_holding_the_line_it_returns_to() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_text(2, 5);
    h.type_text("x");
    assert_eq!(h.text(2), "ref bx");

    h.toggle_fold(2);
    assert_eq!(shown_lines(&h), vec![0, 3, 4, 5, 6]);

    cmd_z(&mut h);
    assert_eq!(h.text(2), "ref b");
    assert_eq!(h.cursor().line, 2);
    assert!(
        shown_lines(&h).contains(&2),
        "the group opened to show where the undo landed"
    );

    // And a redo the same way.
    h.toggle_fold(0);
    assert_eq!(shown_lines(&h), vec![0, 3, 4, 5, 6]);
    h.key_mod(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.text(2), "ref bx");
    assert!(shown_lines(&h).contains(&2));
}

/// A grid taller than the page leaves no header line on screen at all, but the
/// group's bar still runs the whole way through it — so the gutter has to keep
/// its marker column.
#[test]
fn a_page_that_is_all_grid_still_carries_the_fold_bar() {
    let mut src = String::from("glyph tall 2 1000\n");
    for _ in 0..1000 {
        src.push_str("....\n");
    }
    src.push_str("glyph z 2 2\n....\n....\n");
    let mut h = EditorHarness::new(&src);
    // Taller than twice the font height, so it opens folded; this test is
    // about the block open.
    h.click_fold_marker(0);
    let at_top = h.snap().marker_width;
    assert!(at_top > 0.0);

    h.click_text(0, 0);
    h.key(Key::PageDown);
    h.key(Key::PageDown);
    let header_y = h
        .snap()
        .vlines
        .iter()
        .find(|vl| vl.doc_line == 0)
        .expect("no header line")
        .y;
    assert!(
        header_y < 0.0,
        "the header should have scrolled off the top ({header_y})"
    );

    assert_eq!(
        h.snap().marker_width,
        at_top,
        "the column must not collapse"
    );
    assert!(
        h.fold_markers().iter().any(|(header, ..)| *header == 0),
        "the bar of the group this page is inside is still painted"
    );
}

// ---------------------------------------------------------------------------
// Headings
// ---------------------------------------------------------------------------

/// A `#`/`##`/`###` file with a glyph block in the deepest section.
///
/// DocLines: 0 `# title`, 1 `## alpha`, 2 `map a = a`, 3 `### deep`,
/// 4 header a, 5 grid, 6 `## beta`, 7 `map b = b`, 8 `# second`, 9 `map c = c`.
fn heading_doc() -> String {
    String::from(
        "# title\n## alpha\nmap a = a\n### deep\nglyph a 2 2\n....\n....\n\
         ## beta\nmap b = b\n# second\nmap c = c\n",
    )
}

/// Height of the first visual line of `doc_line`.
fn line_height(h: &EditorHarness, doc_line: usize) -> f32 {
    h.snap()
        .vlines
        .iter()
        .find(|vl| vl.doc_line == doc_line)
        .unwrap_or_else(|| panic!("no visual line for {doc_line}"))
        .height
}

/// A heading draws two zoom steps above the body text for `#` and one for
/// `##`, and its row grows with it — `###` is body size.
#[test]
fn a_heading_row_is_as_tall_as_the_type_it_draws_at() {
    let h = EditorHarness::new(&heading_doc());
    let body = line_height(&h, 2);
    assert!(line_height(&h, 0) > line_height(&h, 1));
    assert!(line_height(&h, 1) > body);
    assert_eq!(line_height(&h, 3), body, "### is body size");
    assert_eq!(line_height(&h, 9), body);
    // 16px body, so 48/32/16 — measured as row heights, which scale with them.
    let ratio = |line: usize| line_height(&h, line) / body;
    assert!((ratio(0) - 3.0).abs() < 0.35, "# is 48/16: {}", ratio(0));
    assert!((ratio(1) - 2.0).abs() < 0.35, "## is 32/16: {}", ratio(1));
    assert_view_consistent(&h);
}

/// Folding a section hides everything under it, up to the next heading of its
/// own level or shallower.
#[test]
fn a_heading_section_folds_down_to_its_heading() {
    let mut h = EditorHarness::new(&heading_doc());
    assert_eq!(shown_lines(&h), (0..10).collect::<Vec<_>>());

    h.click_fold_marker(1);
    assert_eq!(
        shown_lines(&h),
        vec![0, 1, 6, 7, 8, 9],
        "`## alpha` swallows the `###` section inside it but stops at `## beta`"
    );

    h.click_fold_marker(1);
    assert_eq!(shown_lines(&h), (0..10).collect::<Vec<_>>());
    assert_view_consistent(&h);
}

/// The one `#` of a file is its title, not a section: nothing folds it, while
/// the second one turns both into sections.
#[test]
fn a_lone_title_has_no_marker_but_a_second_heading_gives_it_one() {
    let h = EditorHarness::new("# title\nmap a = a\nmap b = b\n");
    assert!(h.fold_markers().is_empty());
    assert_eq!(h.snap().marker_width, 0.0, "and no column is reserved");

    let h = EditorHarness::new("# title\nmap a = a\n# second\nmap b = b\n");
    let headers: Vec<usize> = h.fold_markers().iter().map(|(l, ..)| *l).collect();
    assert_eq!(headers, vec![0, 2]);
}

/// The gutter stacks a marker per level of nesting, outermost against the line
/// numbers and each nested one to its left — so the same kind of block sits in
/// different columns depending on what encloses it.
#[test]
fn nested_groups_stack_their_markers_leftwards_from_the_line_numbers() {
    let h = EditorHarness::new(&heading_doc());
    let x_of = |header: usize| -> f32 {
        h.fold_markers()
            .into_iter()
            .find(|(l, ..)| *l == header)
            .unwrap_or_else(|| panic!("no marker for line {header}"))
            .1
            .min
            .x
    };
    // `# title` ⊃ `## alpha` ⊃ `### deep` ⊃ the glyph block.
    assert!(x_of(0) > x_of(1));
    assert!(x_of(1) > x_of(3));
    assert!(x_of(3) > x_of(4));
    // Four columns of markers, and the text starts past all of them.
    assert!(h.snap().marker_width >= 4.0 * (x_of(0) - x_of(1)) - 0.5);

    // The same glyph block, with no section around it, sits in the *outermost*
    // column — the one against the line numbers — rather than three columns to
    // its left. Measured from the text origin, which is what both files share.
    let flat = EditorHarness::new(&fold_doc());
    let inset = |h: &EditorHarness, x: f32| h.snap().origin_x - x;
    assert!(
        inset(&flat, flat.fold_markers()[0].1.min.x) < inset(&h, x_of(4)),
        "a glyph block nested three deep is pushed further from the text"
    );
}

/// Folding a group must not move the markers of the groups around it: the
/// column count is the document's nesting, not the page's, so a second click
/// lands where the first one did.
#[test]
fn a_fold_leaves_every_marker_where_it_was() {
    let mut h = EditorHarness::new(&heading_doc());
    let xs = |h: &EditorHarness| -> Vec<(usize, f32)> {
        let mut m = h.fold_markers();
        m.sort_by_key(|(l, ..)| *l);
        m.iter().map(|(l, r, _)| (*l, r.min.x)).collect()
    };
    let before = xs(&h);
    assert_eq!(
        before.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
        vec![0, 1, 3, 4, 6, 8]
    );
    h.click_fold_marker(3);
    let after = xs(&h);
    assert_eq!(
        after.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
        vec![0, 1, 3, 6, 8],
        "the glyph block inside the shut section is gone with it"
    );
    for (line, x) in after {
        let was = before.iter().find(|(l, _)| *l == line).unwrap().1;
        assert_eq!(x, was, "marker of line {line} moved");
    }
}

/// The marker column's width is an input to the wrapping that decides whether
/// the page shows a group at all: reserve the column and the text narrows,
/// which wraps one more line, which pushes the only group on the page off it,
/// which un-reserves the column — a two-frame cycle the view can never settle.
/// A page of heavily wrapped lines used to sit in that cycle, flickering the
/// gutter on every frame while no marker was even on screen.
#[test]
fn the_marker_column_does_not_flicker_on_a_page_of_wrapped_lines() {
    // Long comment lines of many lengths, so some of them wrap differently once
    // a marker column is taken out of the text area, and one foldable glyph
    // block at the end for the page to lose and regain.
    let mut src = String::new();
    for i in 0..80 {
        src.push_str("// ");
        src.push_str(&"x".repeat(80 + i % 31));
        src.push('\n');
    }
    src.push_str("glyph a 2 2\n@@..\n....\n");

    let mut h = EditorHarness::new(&src);
    h.viewport_height = Some(300.0);
    h.frame();
    // The cycle only shows where a group's edge lands near the page's, so the
    // whole document is swept rather than one hand-picked offset.
    let mut y = 0.0f32;
    while y < 3200.0 {
        h.scroll_to(y);
        let widths: Vec<f32> = (0..4)
            .map(|_| {
                h.frame();
                h.snap().marker_width
            })
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "gutter marker column flickers at scroll {y}: {widths:?}"
        );
        y += 4.0;
    }
}
