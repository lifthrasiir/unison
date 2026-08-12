//! End-to-end GUI behavior tests for the document editor.
//!
//! These drive the real `show_document` frame loop through
//! [`EditorHarness`]: synthetic keyboard/mouse input goes in, and the
//! assertions read both the editor/document state and the rendered layout
//! (visual lines, grid rows, gutter line numbers) captured per frame.

use crate::document::DocLine;
use crate::editor::EditMode;
use crate::editor::caret::Caret;
use crate::editor::harness::{EditorHarness, SnapKind};
use egui::{Key, Modifiers};

/// Assert that the rendered visual lines and the logical `DocLine`s agree:
/// every DocLine is rendered (no line is "eaten"), text visual lines show
/// the actual line content, grid rows sit on grid lines (or on the ref lines
/// of a ref-only composite), and visual order follows logical order.
#[track_caller]
fn assert_view_consistent(h: &EditorHarness) {
    let vlines = &h.snap().vlines;
    let mut covered = vec![false; h.lines.len()];
    let mut prev_doc_line = 0usize;
    for vl in vlines {
        assert!(
            vl.doc_line < h.lines.len(),
            "visual line points past the document: {} >= {}",
            vl.doc_line,
            h.lines.len()
        );
        assert!(
            vl.doc_line >= prev_doc_line,
            "visual order must follow logical order ({} after {})",
            vl.doc_line,
            prev_doc_line
        );
        prev_doc_line = vl.doc_line;
        covered[vl.doc_line] = true;
        match (&vl.kind, &h.lines[vl.doc_line]) {
            (
                SnapKind::Text {
                    text, col_offset, ..
                },
                DocLine::Text(s),
            ) => {
                let seg: String = s
                    .chars()
                    .skip(*col_offset)
                    .take(text.chars().count())
                    .collect();
                assert_eq!(
                    text, &seg,
                    "text visual line differs from DocLine {} content",
                    vl.doc_line
                );
            }
            (SnapKind::Text { .. }, DocLine::Grid(_)) => {
                panic!("text visual line rendered on grid DocLine {}", vl.doc_line);
            }
            (SnapKind::GridRow { .. }, DocLine::Grid(_)) => {}
            (SnapKind::GridRow { .. }, DocLine::Text(s)) => {
                // Ref-only composites render their virtual grid on the
                // first `ref` line; anything else is a misattribution.
                assert!(
                    s.trim_start().starts_with("ref "),
                    "grid rows rendered on non-grid DocLine {} ({s:?})",
                    vl.doc_line
                );
            }
        }
    }
    for (i, seen) in covered.iter().enumerate() {
        assert!(seen, "DocLine {i} ({:?}) has no visual line", h.lines[i]);
    }
}

/// glyph foo 16 16 with a filled diagonal, a blank line, then glyph bar 4 2.
///
/// DocLines: 0 header foo, 1 grid 16x16, 2 blank, 3 header bar, 4 grid 4x2.
/// Source lines: 1, 2..=17, 18, 19, 20..=21.
fn sample_doc() -> String {
    let mut s = String::from("glyph foo 16 16\n");
    for r in 0..16 {
        for c in 0..16 {
            s.push_str(if r == c { "@@" } else { ".." });
        }
        s.push('\n');
    }
    s.push('\n');
    s.push_str("glyph bar 4 2\n@@......\n......@@\n");
    s
}

fn cmd_z(h: &mut EditorHarness) {
    h.key_mod(Key::Z, Modifiers::COMMAND);
}

fn undo_all(h: &mut EditorHarness) {
    let mut guard = 0;
    while h.state.undo.can_undo() {
        cmd_z(h);
        guard += 1;
        assert!(guard < 100, "undo did not converge");
    }
}

#[test]
fn initial_layout_has_expected_gutter_and_grid_rows() {
    let h = EditorHarness::new(&sample_doc());

    assert_eq!(h.grid_row_count(1), 16);
    assert_eq!(h.grid_row_count(4), 2);
    assert_eq!(h.gutter_of(0), Some(1));
    assert_eq!(h.gutter_of(2), Some(18));
    assert_eq!(h.gutter_of(3), Some(19));
    assert_eq!(h.gutter_numbers(), (1..=21).collect::<Vec<_>>());
}

/// A document of `n` comment lines — nothing but line numbers to draw.
fn numbered_doc(n: usize) -> String {
    (1..=n)
        .map(|i| format!("# line {i}\n"))
        .collect::<Vec<_>>()
        .concat()
}

#[test]
fn gutter_width_follows_the_line_numbers_it_could_draw() {
    let width = |n: usize| EditorHarness::new(&numbered_doc(n)).snap().origin_x;

    // One digit's worth of growth per decade, and nothing in between.
    let (w1, w2, w3) = (width(9), width(10), width(100));
    assert!(w2 > w1, "two-digit gutter must be wider than one-digit");
    assert_eq!(width(99), w2, "99 still fits two digits");
    let digit = w2 - w1;
    assert!(
        (w3 - w2 - digit).abs() < 0.5,
        "one more digit, one more char"
    );
}

/// A grid is one `DocLine` but many *source* lines, and every one of its rows
/// gets a number. The width has to be counted in source lines, not in the
/// line buffer's own length.
#[test]
fn gutter_counts_the_source_lines_a_grid_occupies() {
    let mut src = String::new();
    for i in 0..6 {
        src.push_str(&format!("glyph g{i} 8 16\n"));
        for _ in 0..16 {
            src.push_str("@@..............\n");
        }
    }
    let h = EditorHarness::new(&src);
    // 6 × (header + 16 rows) — well past a hundred, and all of it on screen.
    assert_eq!(h.gutter_numbers().iter().copied().max(), Some(102));
    // Glyph blocks fold, so this gutter also reserves a marker column the
    // plain comment file has no use for; the number field is what is compared.
    assert!(h.snap().marker_width > 0.0);
    assert_eq!(
        h.snap().origin_x - h.snap().marker_width,
        EditorHarness::new(&numbered_doc(102)).snap().origin_x,
        "a hundred-and-something needs three digits either way"
    );
}

#[test]
fn gutter_widens_when_scrolled_into_higher_line_numbers() {
    // Far more lines than a page holds: at the top only three-digit numbers
    // can appear, so the fourth digit is reserved only once the view moves.
    let mut h = EditorHarness::new(&numbered_doc(2000));
    let top = h.snap().origin_x;

    h.click_text(0, 0);
    h.key_mod(Key::End, Modifiers::COMMAND);
    let bottom = h.snap().origin_x;
    assert!(
        bottom > top,
        "gutter should grow for four-digit numbers ({top} -> {bottom})"
    );
}

/// Folding removes the lines from the page but not their numbers: a file whose
/// sections are all shut draws a dozen visual lines carrying numbers well past
/// a hundred. Counting the page in rows would reserve two digits for them.
#[test]
fn gutter_keeps_room_for_the_numbers_a_shut_group_hides() {
    let mut src = String::from("# title\n");
    for s in 0..40 {
        src.push_str(&format!("## section {s}\n"));
        for i in 0..29 {
            src.push_str(&format!("// line {s}-{i}\n"));
        }
    }
    let mut h = EditorHarness::new(&src);
    for s in 0..40 {
        h.click_fold_marker(1 + s * 30);
    }
    assert_eq!(
        shown_lines(&h).len(),
        41,
        "the title and one line per shut section"
    );
    assert_eq!(h.gutter_numbers().iter().copied().max(), Some(1172));

    // Widths of a one- and a two-digit gutter give the width of one digit; the
    // folded file's numbers reach four.
    let one = EditorHarness::new(&numbered_doc(9)).snap().origin_x;
    let digit = EditorHarness::new(&numbered_doc(10)).snap().origin_x - one;
    let field = h.snap().origin_x - h.snap().marker_width;
    assert!(
        (field - (one + digit * 3.0)).abs() < 0.5,
        "a hidden thousandth line still needs its fourth digit ({field} vs {})",
        one + digit * 3.0
    );
}

#[test]
fn click_then_type_places_text() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 6);
    assert_eq!(h.cursor(), Caret::new(0, 6));
    h.type_text("X");
    assert_eq!(h.text(0), "glyph Xfoo 16 16");
}

#[test]
fn typing_inserts_text_and_marks_document_dirty() {
    let mut h = EditorHarness::new(&sample_doc());
    assert!(!h.doc.dirty);

    h.click_text(0, 0);
    h.type_text("// ");
    assert_eq!(h.text(0), "// glyph foo 16 16");
    assert!(h.doc.dirty);

    undo_all(&mut h);
    assert_eq!(h.text(0), "glyph foo 16 16");
    assert!(!h.doc.dirty);
}

#[test]
fn delete_key_updates_immediately() {
    let mut h = EditorHarness::new(&sample_doc());
    // Place cursor at start of "glyph foo 16 16"
    h.click_text(0, 0);
    h.key(Key::Delete);
    assert_eq!(h.text(0), "lyph foo 16 16");
    assert!(h.doc.dirty);
}

#[test]
fn click_grid_enters_glyph_edit() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit for item_idx 0"
    );
}

#[test]
fn header_height_edit_resizes_grid_when_caret_leaves_line() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // "glyph foo 16 16" -> "glyph foo 16 8"
    // Select trailing "16" by navigating to end, backspace twice.
    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.text(0), "glyph foo 16 8");

    // Grid hasn't changed yet — resize is deferred while the caret is on the
    // header line.
    assert_eq!(
        h.grid_row_count(1),
        16,
        "grid is still 16 rows while deferred"
    );
    assert_eq!(h.gutter_of(3), Some(19), "gutter hasn't changed yet");

    // Move the caret off the header line — now the grid resizes.
    h.key(Key::ArrowDown);
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (16, 8));
    assert!(!grid.get(5, 5).is_empty(), "surviving pixel kept");
    assert_eq!(h.grid_row_count(1), 8, "grid widget shrank to 8 rows");

    // All following lines moved up by 8 source lines.
    assert_eq!(h.gutter_of(2), Some(10));
    assert_eq!(h.gutter_of(3), Some(11));
    assert_eq!(h.gutter_numbers(), (1..=13).collect::<Vec<_>>());

    // Undo everything: header text, grid size, truncated pixels, gutter.
    undo_all(&mut h);
    assert_eq!(h.lines, original_lines);
    assert_eq!(h.text(0), "glyph foo 16 16");
    assert_eq!(h.grid_row_count(1), 16);
    assert!(
        !h.grid(1).get(12, 12).is_empty(),
        "truncated pixel restored"
    );
    assert_eq!(h.gutter_of(3), Some(19));
    assert_eq!(h.gutter_numbers(), (1..=21).collect::<Vec<_>>());
}

/// The grid resize is a consequence of the header edit, not a separate user
/// action: one undo has to take the text *and* the grid back together. Undoing
/// only the resize would leave a `18 16` header over an 8-wide grid, which the
/// reparse then renders as an empty 18-wide grid.
#[test]
fn header_dimension_edit_undoes_in_one_step() {
    let mut h = EditorHarness::new(&sample_doc());
    let original_lines = h.lines.clone();

    // "glyph foo 16 16" -> "glyph foo 18 16"
    h.click_text(0, 12); // just after the width "16"
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("18");
    assert_eq!(h.text(0), "glyph foo 18 16");

    h.key(Key::ArrowDown);
    assert_eq!((h.grid(1).width, h.grid(1).height), (18, 16));

    cmd_z(&mut h);
    assert_eq!(
        h.text(0),
        "glyph foo 16 16",
        "header text restored by one undo"
    );
    assert_eq!(h.lines, original_lines, "grid restored by the same undo");
    assert_eq!(h.grid_row_count(1), 16);
    assert!(
        !h.state.undo.can_undo(),
        "no leftover undo entry for the resize"
    );

    // Redo has to bring both sides back in one step too.
    h.key_mod(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.text(0), "glyph foo 18 16");
    assert_eq!((h.grid(1).width, h.grid(1).height), (18, 16));
    assert!(
        !h.state.undo.can_redo(),
        "no leftover redo entry for the resize"
    );
}

/// Same as above, but the deferred resize is flushed by the editor losing
/// keyboard focus rather than by caret movement.
#[test]
fn header_height_edit_resizes_grid_on_focus_loss() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.grid_row_count(1), 16, "still deferred while focused");

    h.blur();
    h.frame();
    assert_eq!(h.grid(1).height, 8);
    assert_eq!(h.grid_row_count(1), 8);
    assert_eq!(h.gutter_of(3), Some(11));
}

/// Same as above, but the deferred resize is flushed by clicking straight into
/// the grid: entering a pixel mode leaves the header line just as surely as
/// moving the caret off it does.
#[test]
fn header_height_edit_resizes_grid_on_grid_click() {
    let mut h = EditorHarness::new(&sample_doc());

    h.click_text(0, 15);
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    h.type_text("8");
    assert_eq!(h.grid_row_count(1), 16, "still deferred while editing");

    // Click into the grid without moving the caret off the header line first.
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "expected GlyphEdit for item_idx 0"
    );
    assert_eq!(
        h.grid(1).height,
        8,
        "header edit applied on entering the grid"
    );
    assert_eq!(h.grid_row_count(1), 8);
    assert_eq!(h.gutter_of(3), Some(11));
}

#[test]
fn growing_header_height_expands_grid_and_gutter() {
    let mut h = EditorHarness::new(&sample_doc());

    // "glyph bar 4 2" -> "glyph bar 4 12"
    h.click_text(3, 12); // just before the "2"
    h.type_text("1");
    assert_eq!(h.text(3), "glyph bar 4 12");
    assert_eq!(h.grid_row_count(4), 2, "deferred while editing header");

    h.key(Key::ArrowUp);
    assert_eq!(h.grid(4).height, 12);
    assert_eq!(h.grid_row_count(4), 12);
    // bar's grid rows now cover source lines 20..=31.
    assert_eq!(h.gutter_numbers(), (1..=31).collect::<Vec<_>>());

    undo_all(&mut h);
    assert_eq!(h.grid(4).height, 2);
}

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
    assert!(pasted_grid.get(0, 0).is_filled());
    assert!(pasted_grid.get(0, 1).is_empty());
    assert!(pasted_grid.get(1, 2).is_empty());
    assert!(pasted_grid.get(1, 3).is_filled());

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
    assert!(grid.get(2, 2).is_filled(), "pixel art must survive");
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
            assert!(grid.get(r, c).is_filled(), "pasted pixel {r},{c} survives");
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
    assert!(grid.get(2, 2).is_filled());
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

/// DocLines: 0=glyph alpha header, 1=Grid(2x2), 2=glyph beta header,
/// 3=Grid(2x2), 4=blank, 5="ref "
fn ac_doc() -> String {
    "glyph alpha 2 2\n@@@@\n@@..\n\
     glyph beta 2 2\n..@@\n@@@@\n\
     \n\
     ref "
        .to_string()
}

fn ctrl_j(h: &mut EditorHarness) {
    h.key_mod(Key::J, Modifiers::CTRL);
}

fn ctrl_k(h: &mut EditorHarness) {
    h.key_mod(Key::K, Modifiers::CTRL);
}

#[test]
fn autocomplete_trigger_and_dismiss() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    assert!(h.state.autocomplete.is_none());

    ctrl_j(&mut h);
    assert!(h.state.autocomplete.is_some());
    let ac = h.state.autocomplete.as_ref().unwrap();
    assert!(ac.candidates.len() >= 2);

    h.key(Key::Escape);
    assert!(h.state.autocomplete.is_none());
}

#[test]
fn autocomplete_accept_inserts_text() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_j(&mut h);

    let ac = h.state.autocomplete.as_ref().unwrap();
    let first_label = ac.candidates[0].label.clone();

    h.key(Key::Enter);
    assert!(h.state.autocomplete.is_none());
    assert_eq!(h.text(5), format!("ref {}", first_label));
}

#[test]
fn autocomplete_filters_as_you_type() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_j(&mut h);
    let initial_count = h.state.autocomplete.as_ref().unwrap().candidates.len();

    h.type_text("al");
    if let Some(ac) = &h.state.autocomplete {
        assert!(ac.candidates.len() <= initial_count);
        assert!(ac.candidates.iter().all(|c| c.label.starts_with("al")));
    }
}

#[test]
fn autocomplete_keyword_on_empty_line() {
    // DocLines: 0=header, 1=grid, 2=blank
    let mut h = EditorHarness::new("glyph alpha 2 2\n@@@@\n@@..\n\n");
    h.click_text(2, 0);
    ctrl_j(&mut h);
    if let Some(ac) = &h.state.autocomplete {
        assert!(ac.candidates.iter().any(|c| c.label == "glyph"));
        assert!(ac.candidates.iter().any(|c| c.label == "ref"));
    } else {
        panic!("autocomplete should be active on empty line");
    }
}

#[test]
fn autocomplete_undo_after_accept() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    let original_text = h.text(5).to_string();
    ctrl_j(&mut h);
    h.key(Key::Enter);
    assert_ne!(h.text(5), original_text);

    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(5), original_text);
}

/// Ctrl+J/Ctrl+K walk the open popup like Down/Up. The trigger itself is the
/// first step down from a virtual item before the list, so the popup opens on
/// item 0 and Ctrl+K there stays put rather than closing it — there is nothing
/// above to step back to. Ctrl+K must not reach the code-point popup either.
#[test]
fn autocomplete_ctrl_j_k_navigate() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);

    ctrl_j(&mut h);
    assert!(h.state.autocomplete.as_ref().unwrap().candidates.len() >= 2);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);

    // Nothing above item 0, and the popup survives.
    ctrl_k(&mut h);
    assert!(h.state.autocomplete.is_some());
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);
    assert!(matches!(h.state.popup, crate::editor::PopupState::None));

    ctrl_j(&mut h);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 1);
    ctrl_k(&mut h);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);

    // Arrow keys keep working alongside them.
    h.key(Key::ArrowDown);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 1);
    h.key(Key::ArrowUp);
    assert_eq!(h.state.autocomplete.as_ref().unwrap().selected, 0);
}

/// Ctrl+K with no popup open still starts code-point entry.
#[test]
fn ctrl_k_without_autocomplete_opens_codepoint_entry() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    ctrl_k(&mut h);
    assert!(h.state.autocomplete.is_none());
    assert!(!matches!(h.state.popup, crate::editor::PopupState::None));
}

// -- visual line <-> logical line reconciliation --------------------------

/// glyph foo 4 4 with a filled diagonal, a blank line, then glyph bar 4 2.
/// DocLines: 0 header foo, 1 grid 4x4, 2 blank, 3 header bar, 4 grid 4x2.
fn two_glyph_doc() -> String {
    let mut s = String::from("glyph foo 4 4\n");
    for r in 0..4 {
        for c in 0..4 {
            s.push_str(if r == c { "@@" } else { ".." });
        }
        s.push('\n');
    }
    s.push('\n');
    s.push_str("glyph bar 4 2\n@@......\n......@@\n");
    s
}

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
    assert!(grid.get(2, 2).is_filled(), "pixel art must survive");
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

/// Build a document tall enough to scroll (20 × 16-row glyphs ≈ 5000 px).
fn tall_doc() -> String {
    let mut s = String::new();
    for i in 0..20 {
        use std::fmt::Write;
        writeln!(s, "glyph tall{i} 16 16").unwrap();
        for _ in 0..16 {
            s.push_str("@@..............................\n");
        }
        s.push('\n');
    }
    s
}

#[test]
fn scroll_position_survives_zoom_change_across_documents() {
    let mut h = EditorHarness::new(&tall_doc());

    // Scroll to a line well past the viewport.
    h.state.goto_line(100);
    h.frame();
    h.frame();

    let scroll_y_z1 = h.scroll_y();
    assert!(
        scroll_y_z1 > 100.0,
        "should have scrolled down; y = {scroll_y_z1}"
    );

    // --- simulate switching to another document ---
    let mut stashed = std::mem::replace(&mut h.state, crate::editor::EditorState::new());

    // The "other document" scrolls to the top and we change zoom.
    h.frame();
    h.frame();

    h.zoom = 2;
    h.state.notify_zoom_change(1);
    h.frame();
    h.frame();

    // --- switch back to the original document ---
    std::mem::swap(&mut h.state, &mut stashed);
    h.frame();
    h.frame();

    let scroll_y_z2 = h.scroll_y();

    // At zoom=2, grid rows are 2× taller so the same logical position
    // requires a substantially larger pixel offset.  A naïve raw-pixel
    // restore would keep scroll_y ≈ scroll_y_z1; the correct centre-
    // fraction restore scales it up.
    assert!(
        scroll_y_z2 > scroll_y_z1 * 1.3,
        "scroll was not scaled for the new zoom: z1={scroll_y_z1:.1}, z2={scroll_y_z2:.1}"
    );
}

// -- multiple editors in one context -----------------------------------------

/// Two editors alive in the same `egui::Context` and the same frame must keep
/// their view state to themselves. Everything an editor parks in `ctx.data()`
/// is keyed by its [`crate::editor::EditorId`]; when those keys were bare
/// strings instead, the two panes shared one scroll offset, one caret-anchored
/// popup position and one layout snapshot, so whichever painted last won.
#[test]
fn two_editors_do_not_share_view_state() {
    let mut h = EditorHarness::new(&tall_doc());
    h.split(&tall_doc());

    // Scroll only the first pane, well past its viewport.
    h.state.goto_line(100);
    h.frame();
    h.frame();

    let first_y = h.scroll_y();
    let second_state = &h.second.as_ref().unwrap().state;
    let second_y = h.scroll_y_of(second_state);
    assert!(
        first_y > 100.0,
        "first pane should have scrolled; y = {first_y}"
    );
    assert!(
        second_y < 1.0,
        "second pane must stay at the top; y = {second_y} (first = {first_y})"
    );

    // Each pane published its own layout. The scrolled pane's first line sits
    // far above the viewport; the unscrolled one's sits inside it.
    let first_y = h.snap().vlines.first().expect("first pane vlines").y;
    let second_y = h
        .second_snap()
        .vlines
        .first()
        .expect("second pane vlines")
        .y;
    assert!(
        second_y - first_y > 100.0,
        "panes share a layout snapshot: first line y = {first_y} vs {second_y}"
    );

    // Carets move independently: clicking into the second pane leaves the
    // first pane's caret alone.
    let first_cursor = h.state.cursor;
    let second_pos = {
        let snap = h.second_snap();
        let vl = snap
            .vlines
            .iter()
            .find(|vl| matches!(vl.kind, SnapKind::Text { .. }))
            .expect("second pane text line");
        egui::pos2(snap.origin_x + 2.0, vl.y + vl.height * 0.5)
    };
    h.click_at(second_pos);
    assert_eq!(
        h.state.cursor, first_cursor,
        "click leaked into the first pane"
    );
}

// -- subglyph layer interactions ---------------------------------------------

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

/// Put `item_idx`'s glyph into `LayerMove` on `layer_idx`: click its grid (whose
/// DocLine is `grid_doc_line`) to enter `GlyphEdit`, then Ctrl+wheel over the
/// grid to step onto the wanted layer, exactly as the app's layer palette does.
#[track_caller]
fn enter_layer_move(
    h: &mut EditorHarness,
    grid_doc_line: usize,
    item_idx: usize,
    layer_idx: usize,
) {
    h.click_grid_cell(grid_doc_line, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: i, .. } if i == item_idx),
        "expected GlyphEdit for item_idx {item_idx}, got {:?}",
        h.state.mode
    );

    // Let any scroll-into-view animation settle before computing screen
    // coordinates to click/hover on.
    for _ in 0..10 {
        h.frame();
    }

    let hover = h.grid_cell_pos(grid_doc_line, 0, 0);
    for _ in 0..=layer_idx {
        // Space the ticks past the coarse wheel cooldown, or every tick after
        // the first is debounced away as one physical notch.
        h.advance_time(0.1);
        h.frame_with(
            vec![
                egui::Event::PointerMoved(hover),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -1.0),
                    modifiers: Modifiers::COMMAND,
                },
            ],
            Modifiers::COMMAND,
        );
    }
    assert!(
        matches!(h.state.mode, EditMode::LayerMove { item_idx: i, layer_idx: l } if i == item_idx && l == layer_idx),
        "expected LayerMove on layer {layer_idx}, got {:?}",
        h.state.mode
    );
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
        h.grid(4).get(0, 4).is_filled(),
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
        !h.grid(7).get(0, 4).is_filled(),
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
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2 }),
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
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2 }),
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
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2 }),
        "Backtick right after inlining should enter PixelSelect, got {:?}",
        h.state.mode
    );
}

// ---------------------------------------------------------------------------
// Pixel selection mode tests
// ---------------------------------------------------------------------------

fn make_pixel_select_harness() -> EditorHarness {
    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0); // enter GlyphEdit
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "should be in GlyphEdit"
    );
    h.key(Key::Backtick); // enter PixelSelect
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0 }),
        "should be in PixelSelect"
    );
    h
}

#[test]
fn backtick_enters_pixel_select_and_num1_returns() {
    let mut h = make_pixel_select_harness();
    h.key(Key::Num1);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "Num1 should return to GlyphEdit"
    );
}

#[test]
fn escape_exits_pixel_select() {
    let mut h = make_pixel_select_harness();
    h.key(Key::Escape);
    assert!(
        matches!(h.state.mode, EditMode::Normal),
        "Escape should go to Normal"
    );
}

#[test]
fn drag_creates_grounded_selection() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 2, 2));
    assert!(!sel.is_floating());
}

#[test]
fn move_selection_makes_floating_and_clears_grid() {
    let mut h = make_pixel_select_harness();
    // Select the top-left 2x2 area
    h.drag_grid(1, (0, 0), (1, 1));

    // Now drag from inside the selection to move it
    h.drag_grid(1, (0, 0), (0, 2));

    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert!(sel.is_floating());
    // The original position (0,0)-(1,1) in grid should be cleared
    let grid = h.grid(1);
    assert!(
        grid.get(0, 0).is_empty(),
        "original cell should be empty after move"
    );
    assert!(grid.get(0, 1).is_empty());
}

#[test]
fn undo_move_restores_grid_and_grounded_state() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));
    h.drag_grid(1, (0, 0), (0, 2));

    // Should be floating now
    assert!(h.state.pixel_selection.as_ref().unwrap().is_floating());

    // Undo
    h.key_mod(Key::Z, Modifiers::COMMAND);
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection after undo");
    assert!(!sel.is_floating(), "should be grounded after undo");
    assert_eq!(
        (sel.row, sel.col),
        (0, 0),
        "should be back at original position"
    );

    // Grid should be restored
    let grid = h.grid(1);
    assert!(
        grid.get(0, 0).is_filled(),
        "grid should be restored after undo"
    );
}

#[test]
fn mode_change_commits_floating_selection() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.drag_grid(1, (0, 0), (2, 0)); // move down by 2

    let sel = h.state.pixel_selection.as_ref().unwrap();
    assert!(sel.is_floating());

    // Switch to GlyphEdit (commits)
    h.key(Key::Num1);
    assert!(
        h.state.pixel_selection.is_none(),
        "selection should be cleared"
    );

    // The moved pixels should be merged into the grid at new position
    let grid = h.grid(1);
    assert!(
        grid.get(2, 0).is_filled(),
        "moved pixel should be merged at new position"
    );
    // Original position should be empty
    assert!(
        grid.get(0, 0).is_empty(),
        "original position should be empty"
    );
}

#[test]
fn delete_grounded_fills_empty() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));

    // Delete
    h.key(Key::Delete);
    assert!(h.state.pixel_selection.is_none());

    let grid = h.grid(1);
    assert!(grid.get(0, 0).is_empty());
    assert!(grid.get(0, 1).is_empty());
    // Rest unchanged
    assert!(grid.get(0, 2).is_filled());
}

#[test]
fn delete_floating_discards_no_merge() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.drag_grid(1, (0, 0), (2, 0)); // move down

    h.key(Key::Delete);
    assert!(h.state.pixel_selection.is_none());

    let grid = h.grid(1);
    // Original was cleared during float, and floating was discarded, so both are empty
    assert!(grid.get(0, 0).is_empty());
    assert!(grid.get(0, 1).is_empty());
    assert!(
        grid.get(2, 0).is_empty(),
        "floating pixels should not merge on delete"
    );
}

#[test]
fn undo_after_mode_change_commit_terminates() {
    // Move a selection (floating), then leave PixelSelect: reconcile commits
    // the float. Undoing that commit must not restore a floating selection
    // into a mode that cannot hold one — reconcile would commit it again,
    // pushing a fresh entry for every undo, and the stack would never drain.
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.drag_grid(1, (0, 0), (2, 0)); // move down -> floating
    h.key(Key::Num1); // back to GlyphEdit; reconcile commits the float
    assert!(h.state.pixel_selection.is_none());

    for _ in 0..10 {
        h.key_mod(Key::Z, Modifiers::COMMAND);
    }
    assert!(
        !h.state.undo.can_undo(),
        "undo must drain instead of regenerating entries"
    );

    let grid = h.grid(1);
    assert!(
        grid.get(0, 0).is_filled(),
        "grid should be back to its original state"
    );
    assert!(grid.get(0, 1).is_filled());
    assert!(grid.get(2, 0).is_empty());
}

/// A glyph with pixels, a ref placed by an explicit offset, a ref placed by
/// anchor matching (at `2 1`, so a wrong base position shows up), and the
/// anchor that places it — one of each thing a move-all touches.
///
/// Doc lines: 0/1 `part`, 2..4 `mark`, 5 header, 6 grid, 7/8 refs, 9 anchor.
fn make_move_all_harness() -> EditorHarness {
    let mut h = EditorHarness::new(
        "glyph part 2 2\n@@..\n....\n\
         glyph mark 2 2\n@@..\n....\nanchor -join 0 0\n\
         glyph test 4 3\n@@@@@@..\n..@@@@..\n........\n\
         ref part 1 0\nref mark\nanchor +join 2 1",
    );
    h.click_grid_cell(6, 0, 0);
    h.key(Key::Backtick);
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 2 }),
        "should be in PixelSelect, got {:?}",
        h.state.mode
    );
    h
}

#[test]
fn cmd_drag_outside_selection_moves_all_layers() {
    let mut h = make_move_all_harness();
    // Empty bottom row, so nothing is selected there: Cmd+drag one cell right.
    h.drag_grid_mod(6, (2, 0), (2, 1), Modifiers::COMMAND);

    let grid = h.grid(6);
    assert!(grid.get(0, 0).is_empty(), "pixels should have moved right");
    assert!(grid.get(0, 1).is_filled());
    assert!(grid.get(0, 2).is_filled());
    assert!(grid.get(0, 3).is_filled());
    assert!(grid.get(1, 1).is_empty());
    assert!(grid.get(1, 2).is_filled());

    assert_eq!(
        h.text(7).trim(),
        "ref part 2 0",
        "explicit ref offset shifts"
    );
    assert_eq!(
        h.text(8).trim(),
        "ref mark 3 1",
        "an auto-placed ref moves from where the composite drew it (2 1)"
    );
    assert_eq!(h.text(9).trim(), "anchor +join 3 1", "anchor shifts");

    assert!(
        h.state.pixel_selection.is_none(),
        "a move-all leaves no selection behind"
    );
}

#[test]
fn cmd_drag_moves_all_layers_down() {
    let mut h = make_move_all_harness();
    h.drag_grid_mod(6, (0, 3), (1, 3), Modifiers::COMMAND);

    let grid = h.grid(6);
    assert!(grid.get(0, 0).is_empty(), "top row should be vacated");
    assert!(grid.get(1, 0).is_filled());
    assert!(grid.get(2, 1).is_filled());

    assert_eq!(h.text(7).trim(), "ref part 1 1");
    assert_eq!(h.text(8).trim(), "ref mark 2 2");
    assert_eq!(h.text(9).trim(), "anchor +join 2 2");
}

#[test]
fn cmd_drag_over_selection_still_moves_only_selection() {
    let mut h = make_move_all_harness();
    h.drag_grid(6, (0, 0), (1, 1)); // select the top-left 2x2
    h.drag_grid_mod(6, (0, 0), (0, 2), Modifiers::COMMAND);

    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("selection should survive its own move");
    assert!(sel.is_floating(), "dragging inside the selection moves it");
    assert_eq!((sel.row, sel.col), (0, 2));

    assert_eq!(h.text(7).trim(), "ref part 1 0", "layers must not move");
    assert_eq!(h.text(8).trim(), "ref mark");
    assert_eq!(h.text(9).trim(), "anchor +join 2 1");
}

#[test]
fn undo_move_all_restores_every_layer_at_once() {
    let mut h = make_move_all_harness();
    h.drag_grid_mod(6, (2, 0), (2, 2), Modifiers::COMMAND); // two cells right

    assert_eq!(h.text(7).trim(), "ref part 3 0");
    assert_eq!(h.text(8).trim(), "ref mark 4 1");
    assert_eq!(h.text(9).trim(), "anchor +join 4 1");

    h.key_mod(Key::Z, Modifiers::COMMAND);

    let grid = h.grid(6);
    assert!(grid.get(0, 0).is_filled(), "pixels should be back");
    assert!(grid.get(0, 3).is_empty());
    assert_eq!(h.text(7).trim(), "ref part 1 0");
    assert_eq!(
        h.text(8).trim(),
        "ref mark",
        "undo restores the auto placement"
    );
    assert_eq!(h.text(9).trim(), "anchor +join 2 1");
    assert!(
        !h.state.undo.can_undo(),
        "the whole drag should be a single undo entry"
    );
}

#[test]
fn copy_produces_correct_text() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));

    // Copy (uses Event::Copy, same as Cmd+C)
    h.copy();
    let copied = h
        .last_copied_text
        .as_ref()
        .expect("should have copied text");
    assert_eq!(
        copied, "@@@@\n..@@",
        "copied text should match grid content"
    );
}

#[test]
fn paste_in_pixel_select_creates_floating() {
    let mut h = make_pixel_select_harness();
    h.paste("@@..\n..@@");

    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0 }),
        "should stay in PixelSelect"
    );
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert!(sel.is_floating());
    assert_eq!((sel.width, sel.height), (2, 2));
}

#[test]
fn paste_in_glyph_edit_switches_to_pixel_select() {
    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0); // enter GlyphEdit
    assert!(matches!(h.state.mode, EditMode::GlyphEdit { .. }));

    h.paste("@@\n@@");
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0 }),
        "paste should switch to PixelSelect"
    );
    let sel = h
        .state
        .pixel_selection
        .as_ref()
        .expect("should have selection");
    assert!(sel.is_floating());
}

#[test]
fn cut_copies_and_deletes() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));

    h.cut();
    let copied = h.last_copied_text.as_ref().expect("should have copied");
    assert_eq!(copied, "@@@@");
    assert!(h.state.pixel_selection.is_none());

    let grid = h.grid(1);
    assert!(grid.get(0, 0).is_empty());
    assert!(grid.get(0, 1).is_empty());
}

#[test]
fn paste_via_pixel_selection_function() {
    use crate::document_io::{derive_document, parse_doclines};
    use crate::editor::pixel_selection;

    let mut lines = parse_doclines("glyph test 3 2\n......\n......");
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let mut state = crate::editor::EditorState::new();
    state.mode = EditMode::GlyphEdit {
        item_idx: 0,
        selected_shape: crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
    };

    let ok = pixel_selection::paste_selection(&doc, &mut lines, &mut state, "@@..\n..@@");
    assert!(ok, "paste should succeed");
    assert!(matches!(state.mode, EditMode::PixelSelect { item_idx: 0 }));
    let sel = state.pixel_selection.as_ref().unwrap();
    assert!(sel.is_floating());
    assert_eq!((sel.width, sel.height), (2, 2));
}

#[test]
fn paste_too_small_for_selection_fails() {
    use crate::document_io::{derive_document, parse_doclines};
    use crate::editor::pixel_selection;

    let mut lines = parse_doclines("glyph test 3 2\n......\n......");
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    let mut state = crate::editor::EditorState::new();
    state.mode = EditMode::PixelSelect { item_idx: 0 };
    state.pixel_selection = Some(pixel_selection::PixelSelection {
        item_idx: 0,
        row: 0,
        col: 0,
        width: 3,
        height: 2,
        float_pixels: None,
    });

    let ok = pixel_selection::paste_selection(&doc, &mut lines, &mut state, "@@\n@@");
    assert!(
        !ok,
        "paste should fail when clipboard is smaller than selection"
    );
}

#[test]
fn right_click_cancels_selection() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    assert!(h.state.pixel_selection.is_some());

    let pos = h.grid_cell_pos(1, 0, 0);
    h.right_click_at(pos);
    assert!(
        h.state.pixel_selection.is_none(),
        "right click should cancel selection"
    );
}

#[test]
fn blur_commits_floating() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 0)); // select single cell
    h.drag_grid(1, (0, 0), (2, 0)); // move to row 2

    assert!(h.state.pixel_selection.as_ref().unwrap().is_floating());
    h.blur();
    assert!(
        h.state.pixel_selection.is_none(),
        "blur should commit and clear"
    );

    // Pixel should be merged at new position
    let grid = h.grid(1);
    assert!(grid.get(2, 0).is_filled());
}

#[test]
fn on_demand_triangle_ref_reaches_renderer() {
    // A ref-only glyph pointing at an on-demand triangle must resolve with
    // exact custom details, expose them through the composite layers the
    // grid renderer draws from, and lay out one grid row per pixel row.
    let h = EditorHarness::new("glyph tri\nref 4x16-ul 0 0\n");
    assert_view_consistent(&h);

    // The offsetless form (auto-resolved placement) must reach the
    // renderer just the same.
    let h2 = EditorHarness::new("glyph tri2\nref 4x16-ul\n");
    assert_view_consistent(&h2);
    let resolved2 = h2.named_glyphs.get("tri2").expect("tri2 must resolve");
    assert!(
        !resolved2.grid.details.is_empty(),
        "offsetless triangle ref must resolve with details"
    );

    let resolved = h.named_glyphs.get("tri").expect("tri must resolve");
    assert!(
        !resolved.grid.details.is_empty(),
        "1:4 hypotenuse requires custom details in the resolved grid"
    );

    let rows = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| matches!(vl.kind, SnapKind::GridRow { .. }))
        .count();
    assert_eq!(rows, 16, "one grid row per pixel row");

    // The composite the renderer receives must carry the custom cells and
    // report them as filled (thumbnails and previews use that test).
    let comp = crate::editor::ref_composite::compute_composite(
        match &h.doc.items[0] {
            crate::document::DocumentItem::Glyph { body, .. } => body,
            other => panic!("expected glyph item, got {other:?}"),
        },
        &h.named_glyphs,
        &h.name_parts,
        &h.alt_index,
        &Default::default(),
    )
    .expect("composite for ref-only glyph");
    let layer = &comp.layers[0];
    assert!(
        !layer.grid.details.is_empty(),
        "composite layer must keep the custom details the renderer draws"
    );
    assert!(comp.any_layer_filled_at(0, 0), "right-angle corner filled");
    assert!(!comp.any_layer_filled_at(15, 3), "opposite corner empty");
}

// ---------------------------------------------------------------------------
// Horizontal grid scrolling
// ---------------------------------------------------------------------------

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
    assert!(h.grid(1).get(2, 3).is_empty(), "cell (2, 3) starts empty");

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
        h.grid(1).get(2, 3).is_empty(),
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
        !h.grid(1).get(2, 3).is_empty(),
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

/// `map` spells out the codepoint of a literally written character as a
/// dimmed inline annotation. It is display-only: the document line is
/// untouched, and the caret treats the character plus its annotation as one.
#[test]
fn map_literal_char_renders_codepoint_annotation() {
    let mut h = EditorHarness::new("map 가 = hangul-ga\nglyph hangul-ga 2 2\n....\n....\n");
    assert_view_consistent(&h);

    let vl = &h.snap().vlines[0];
    match &vl.kind {
        SnapKind::Text { text, display, .. } => {
            assert_eq!(text, "map 가 = hangul-ga", "the document line is unchanged");
            assert_eq!(display, "map 가 U+AC00 = hangul-ga");
        }
        other => panic!("expected a text visual line, got {other:?}"),
    }

    // Clicking on either side of the annotated character lands on the
    // document column, not inside the annotation.
    h.click_text(0, 4);
    assert_eq!(h.state.cursor, Caret::new(0, 4));
    h.click_text(0, 5);
    assert_eq!(h.state.cursor, Caret::new(0, 5));

    // Nothing in the span the annotation occupies resolves to a column
    // between the two: the pair is a single caret step.
    let x0 = h.text_pos(0, 4);
    let x1 = h.text_pos(0, 5);
    let steps = 12;
    for i in 1..steps {
        let t = i as f32 / steps as f32;
        h.click_at(egui::pos2(x0.x + (x1.x - x0.x) * t, x0.y));
        let col = h.state.cursor.col;
        assert!(
            col == 4 || col == 5,
            "caret landed inside the annotation at col {col}"
        );
    }

    // Typing still edits the document, annotation and all.
    h.click_text(0, 5);
    h.key(Key::ArrowRight);
    assert_eq!(h.state.cursor, Caret::new(0, 6));
}

/// An annotation too long for one visual line wraps by itself, exactly as the
/// same text written in the document would. It used to be unbreakable, so a
/// long one dragged the character it trails onto the next line and then
/// painted past the right edge anyway.
#[test]
fn a_long_annotation_wraps_across_visual_lines() {
    // 30 characters, so the ` U+XXXX` spelling is far wider than any editor.
    let text = "안녕하세요반갑습니다어서오세요고맙습니다또또오세요건강하세요";
    let line = format!("assert shape {text} : greeting");
    let mut h = EditorHarness::new(&format!("{line}\nglyph greeting 2 2\n....\n....\n"));
    assert_view_consistent(&h);

    let segments: Vec<(String, usize, String)> = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == 0)
        .filter_map(|vl| match &vl.kind {
            SnapKind::Text {
                text,
                col_offset,
                display,
                ..
            } => Some((text.clone(), *col_offset, display.clone())),
            SnapKind::GridRow { .. } => None,
        })
        .collect();
    assert!(
        segments.len() > 1,
        "the annotation must wrap for this test to mean anything"
    );

    // The rendered segments reassemble the rendered line: the annotation is
    // split across them, not dropped, duplicated or held back.
    let joined_display: String = segments.iter().map(|(_, _, d)| d.as_str()).collect();
    let expected: String = format!(
        "assert shape {text}{} : greeting",
        text.chars()
            .map(|c| format!(" U+{:04X}", c as u32))
            .collect::<String>()
    );
    assert_eq!(joined_display, expected);
    let joined_text: String = segments.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(joined_text, line, "the document line is unchanged");

    // At least one segment carries only annotation and no document column of
    // its own — the wrap fell inside the annotation twice over.
    assert!(
        segments
            .iter()
            .any(|(t, _, display)| t.is_empty() && !display.is_empty()),
        "expected an annotation-only segment: {segments:?}"
    );

    // The caret still walks document columns: the columns on either side of
    // the wrapped annotation are reachable and one step apart.
    let after = "assert shape ".chars().count() + text.chars().count();
    h.click_text(0, after);
    assert_eq!(h.state.cursor, Caret::new(0, after));
    h.key(Key::ArrowRight);
    assert_eq!(h.state.cursor, Caret::new(0, after + 1));
}

/// A `map` already written as `U+XXXX` needs no annotation.
#[test]
fn map_explicit_codepoint_is_not_annotated() {
    let h = EditorHarness::new("map U+AC00 = hangul-ga\nglyph hangul-ga 2 2\n....\n....\n");
    match &h.snap().vlines[0].kind {
        SnapKind::Text { text, display, .. } => assert_eq!(display, text),
        other => panic!("expected a text visual line, got {other:?}"),
    }
}

/// A `// …` comment is highlighted as a comment wherever it sits, and only
/// from its `//` on. The pixel rows below it keep their `//` as pixels.
#[test]
fn inline_comment_is_highlighted_from_its_marker() {
    let h = EditorHarness::new("glyph slash 2 1 // a note\n0//1\n");
    assert_view_consistent(&h);
    match &h.snap().vlines[0].kind {
        SnapKind::Text {
            text, comment_col, ..
        } => {
            assert_eq!(*comment_col, Some(text.find("//").unwrap()));
        }
        other => panic!("expected a text visual line, got {other:?}"),
    }
    // The pixel row is still a grid, not a commented-out text line.
    assert!(matches!(h.snap().vlines[1].kind, SnapKind::GridRow { .. }));
    assert_eq!(h.grid(1).width, 2);
}

/// A line without a comment has nothing highlighted, and a quoted `//` is an
/// ordinary token rather than a comment marker.
#[test]
fn quoted_double_slash_is_not_a_comment() {
    let h = EditorHarness::new("map `//` = solidus-double\nglyph solidus-double 2 1\n@@..\n");
    match &h.snap().vlines[0].kind {
        SnapKind::Text { comment_col, .. } => assert_eq!(*comment_col, None),
        other => panic!("expected a text visual line, got {other:?}"),
    }
}

/// Typing a comment onto a glyph header must not cost the header its grid:
/// the comment is not part of the `W H` grammar.
#[test]
fn typing_a_comment_on_a_header_keeps_the_grid() {
    let mut h = EditorHarness::new("glyph foo 4 2\n@@......\n......@@\n");
    h.click_text(0, 13);
    h.type_text(" // a note");
    h.key(Key::ArrowDown);
    assert_eq!(h.text(0), "glyph foo 4 2 // a note");
    assert_view_consistent(&h);
    let grid = h.grid(1);
    assert_eq!((grid.width, grid.height), (4, 2));
    assert!(
        !grid.get(0, 0).is_empty(),
        "pixels survived the header edit"
    );
    match &h.snap().vlines[0].kind {
        SnapKind::Text { comment_col, .. } => assert_eq!(*comment_col, Some(14)),
        other => panic!("expected a text visual line, got {other:?}"),
    }
}

/// Cancelling the F2 rename popup with Escape hands keyboard focus back to
/// the editor canvas, with the caret still where it was — otherwise the user
/// has to click back into the document before typing again.
#[test]
fn rename_popup_escape_restores_editor_focus() {
    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\n");
    h.click_text(0, 8);
    assert!(h.editor_has_focus());
    let caret = h.cursor();

    h.key(Key::F2);
    h.frame();
    assert!(!h.editor_has_focus(), "the popup's text field takes focus");

    h.key(Key::Escape);
    h.frame();
    assert!(h.editor_has_focus(), "focus must return to the editor");
    assert_eq!(h.cursor(), caret);
    assert_eq!(h.text(0), "glyph foo 2 1");
}

/// Confirming the rename popup with Enter likewise returns focus to the
/// editor.
#[test]
fn rename_popup_confirm_restores_editor_focus() {
    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\n");
    h.click_text(0, 8);
    h.key(Key::F2);
    h.frame();
    h.type_text("bar");
    h.key(Key::Enter);
    h.frame();
    assert!(h.editor_has_focus(), "focus must return to the editor");
}

/// F2 on a slice qualifier opens the rename popup for the *slice*, not for
/// whatever the rest of the line names.
#[test]
fn rename_popup_opens_for_a_slice_qualifier() {
    use crate::editor::PopupState;
    use crate::editor::doc_links::RenameKind;

    let mut h = EditorHarness::new("glyph a 2 1\n@@..\nmap narrow : A = a\n");
    h.click_text(2, 6);
    h.key(Key::F2);
    h.frame();
    match &h.state.popup {
        PopupState::Rename {
            original_name,
            kind,
            ..
        } => {
            assert_eq!(original_name, "narrow");
            assert_eq!(*kind, RenameKind::Slice);
        }
        other => panic!("F2 opened no rename popup: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Ctrl+K: the code point popup
// ---------------------------------------------------------------------------

/// Ctrl+K opens the code point popup, whose text field takes focus just like
/// the rename popup's.  Nothing is inserted until it is confirmed.
#[test]
fn codepoint_popup_opens_on_ctrl_k() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    assert!(h.editor_has_focus());

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::Codepoint(_)),
        "Ctrl+K opened no code point popup: {:?}",
        h.state.popup
    );
    assert!(!h.editor_has_focus(), "the popup's text field takes focus");
    assert_eq!(h.text(0), "meta name Test");
}

/// On Windows and Linux the backend sets `command` to the same value as `ctrl`
/// (only macOS keeps them apart), so the chord must be recognized with both
/// flags set.  Rejecting `command` there is what made Ctrl+K a no-op.
#[test]
fn codepoint_popup_opens_on_ctrl_k_with_command_alias() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);

    // What winit reports for Ctrl+K off the Mac.
    let win_ctrl = Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    h.key_mod(Key::K, win_ctrl);
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::Codepoint(_)),
        "Ctrl+K opened no code point popup: {:?}",
        h.state.popup
    );
}

/// Cmd+K on macOS (`mac_cmd` + `command`, no `ctrl`) is not the chord.
#[test]
fn codepoint_popup_ignores_mac_cmd_k() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);

    let mac_cmd = Modifiers {
        mac_cmd: true,
        command: true,
        ..Default::default()
    };
    h.key_mod(Key::K, mac_cmd);
    h.frame();
    assert!(
        matches!(h.state.popup, PopupState::None),
        "Cmd+K must not open the code point popup: {:?}",
        h.state.popup
    );
}

/// The popup is anchored under the caret from the frame it opens, not only
/// once the digits decode to something.  The canvas is unfocused while the
/// popup owns the keyboard, and an empty preedit used to fall back to the
/// start of the line, which put the popup at the left margin until the first
/// valid digit jumped it back to the caret.
#[test]
fn codepoint_popup_anchors_at_the_caret_while_still_empty() {
    use crate::editor::document_view::popups::caret_anchor_pos;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    let caret_x = h.text_pos(0, 14).x;

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(h.state.preedit, "", "no digits typed yet");
    let empty_x = caret_anchor_pos(&h.ctx, &h.state).x;
    assert!(
        (empty_x - caret_x).abs() < 2.0,
        "empty popup anchored at {empty_x}, not at the caret {caret_x}"
    );

    // And it stays there once the digits do decode.
    h.type_text("41");
    h.frame();
    let filled_x = caret_anchor_pos(&h.ctx, &h.state).x;
    assert!(
        (filled_x - caret_x).abs() < 2.0,
        "popup with a preedit anchored at {filled_x}, not at the caret {caret_x}"
    );
}

/// While the hex digits are being typed the decoded character shows as the
/// editor's preedit — the popup drives the same preview an IME would — and
/// the document itself is untouched.
#[test]
fn codepoint_popup_previews_as_preedit() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();

    h.type_text("41");
    assert_eq!(h.state.preedit, "A", "the preview should track the digits");
    assert_eq!(h.text(0), "meta name Test", "nothing committed yet");

    h.type_text("0");
    assert_eq!(
        h.state.preedit, "\u{410}",
        "U+0410 CYRILLIC CAPITAL LETTER A"
    );
    assert_eq!(h.text(0), "meta name Test");
}

/// Enter commits the preedit at the caret, exactly as an IME commit would,
/// and hands focus back to the editor.
#[test]
fn codepoint_popup_enter_commits_the_preedit() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("2603");
    h.key(Key::Enter);
    h.frame();

    assert_eq!(h.text(0), "meta name Test\u{2603}");
    assert_eq!(h.cursor(), Caret { line: 0, col: 15 });
    assert!(
        h.state.preedit.is_empty(),
        "the preedit is consumed by the commit"
    );
    assert!(matches!(h.state.popup, PopupState::None));
    assert!(h.editor_has_focus(), "focus must return to the editor");
}

/// Escape rolls the preedit back: no text, no leftover preview, focus back in
/// the editor with the caret where it was.
#[test]
fn codepoint_popup_escape_rolls_back() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    let caret = h.cursor();
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("2603");
    assert_eq!(h.state.preedit, "\u{2603}");

    h.key(Key::Escape);
    h.frame();
    assert_eq!(h.text(0), "meta name Test");
    assert!(
        h.state.preedit.is_empty(),
        "the preview must not survive a cancel"
    );
    assert!(matches!(h.state.popup, PopupState::None));
    assert_eq!(h.cursor(), caret);
    assert!(h.editor_has_focus());
}

/// A code point that decodes to nothing — a lone surrogate, or no digits at
/// all — previews as nothing and commits nothing.
#[test]
fn codepoint_popup_rejects_a_surrogate() {
    use crate::editor::PopupState;

    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("D800");
    assert_eq!(h.state.preedit, "", "a surrogate is not a character");

    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name Test");
    assert!(matches!(h.state.popup, PopupState::None));
}

/// Non-hex characters never reach the field, so a stray keystroke cannot
/// silently turn `2603` into something else.
#[test]
fn codepoint_popup_keeps_only_hex_digits() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();

    h.type_text("2x6g0 3");
    assert_eq!(h.state.preedit, "\u{2603}");
    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name Test\u{2603}");
}

/// Typed digits replace a selection, like any other insertion.
#[test]
fn codepoint_popup_commit_replaces_the_selection() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 10);
    h.key_mod(Key::End, Modifiers::SHIFT);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("41");
    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name A");
}

/// The status bar reads the popup's label off the editor state: the code
/// point as typed, plus the Unicode name and properties that tell the user
/// they got the one they meant.
#[test]
fn codepoint_popup_reports_the_unicode_name() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+")
    );

    h.type_text("41");
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+0041  LATIN CAPITAL LETTER A {gc=Lu eaw=Na}")
    );

    h.type_text("0000");
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+410000  (not a code point)")
    );
}

/// Every popup after the first opens on the code point *after* the last one
/// committed — and pre-selected, so the first keystroke replaces it instead of
/// appending to it. A commit that jumped elsewhere does not make the next guess
/// jump too.
#[test]
fn codepoint_popup_predicts_the_next_code_point() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);

    // The first popup guesses nothing.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+")
    );
    h.type_text("2600");
    h.key(Key::Enter);
    h.frame();

    // With one code point recorded the guess is the one after it.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2601  CLOUD {gc=So eaw=N}")
    );
    // Typing replaces the pre-selected guess rather than appending to it.
    h.type_text("2604");
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2604  COMET {gc=So eaw=N}")
    );
    h.key(Key::Enter);
    h.frame();

    // The jump from U+2600 to U+2604 is not extrapolated: the next guess is
    // still just one past the last commit.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2605  BLACK STAR {gc=So eaw=A}")
    );
    h.key(Key::Enter);
    h.frame();
    assert_eq!(h.text(0), "meta name Test\u{2600}\u{2604}\u{2605}");
}

/// A cancelled popup records nothing, so the guess it was seeded with is still
/// the guess the next one gets.
#[test]
fn codepoint_popup_cancel_does_not_move_the_prediction() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("2600");
    h.key(Key::Enter);
    h.frame();

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.key(Key::Escape);
    h.frame();

    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+2601  CLOUD {gc=So eaw=N}")
    );
}

/// A prediction that would land outside the code space puts the popup back to
/// guessing nothing at all.
#[test]
fn codepoint_popup_drops_a_prediction_off_the_end() {
    let mut h = EditorHarness::new("meta name Test\n");
    h.click_text(0, 14);
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    h.type_text("10FFFF");
    h.key(Key::Enter);
    h.frame();

    // The next code point would be U+110000, past the last one there is.
    h.key_mod(Key::K, Modifiers::CTRL);
    h.frame();
    assert_eq!(
        h.state
            .codepoint_status(&crate::ucd::CharProps::default())
            .as_deref(),
        Some("U+")
    );
}

/// Clicking elsewhere in the document also cancels the rename popup, and the
/// click keeps its usual effect: the caret moves to where it landed and the
/// editor has focus again.
#[test]
fn rename_popup_click_cancels_and_moves_the_caret() {
    let mut h = EditorHarness::new("glyph foo 2 1\n@@..\nmap A = foo\n");
    h.click_text(2, 9);
    h.key(Key::F2);
    h.frame();
    assert!(!h.editor_has_focus());

    // Press and release inside one frame, so the click is processed while
    // the popup is still open — the path that used to swallow it.
    let pos = h.text_pos(0, 3);
    h.click_at_same_frame(pos);
    assert!(h.editor_has_focus(), "focus must return to the editor");
    assert_eq!(h.cursor(), Caret { line: 0, col: 3 });
    assert_eq!(h.text(2), "map A = foo", "the rename was cancelled");
}

// ---------------------------------------------------------------------------
// Alt + wheel over the editor bumps the number at the caret
// ---------------------------------------------------------------------------

/// `meta` puts one number on a plain text line, so the caret can be placed on
/// it by character column. The trailing comment is what a selection can run
/// into, which is the case `alt_wheel_ignores_a_selection_that_is_not_a_number`
/// needs; `descent 0` on its own line is the lower bound case.
const NUMBER_DOC: &str =
    "meta height 16 // and a comment\nmeta ascent 12\nmeta descent 0\nglyph sp 2 2\n....\n....\n";

/// Alt + wheel bumps the number the caret sits in, and it does so with the
/// *pointer* anywhere over the editor — the gesture is anchored to the caret,
/// not to what is under the mouse.
#[test]
fn alt_wheel_increments_the_number_at_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 13); // between the digits of "16"
    let elsewhere = h.text_pos(1, 2);

    h.alt_wheel_at(elsewhere, true);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
    // The bumped number is left selected, so the next tick repeats on it.
    assert_eq!(h.cursor(), Caret { line: 0, col: 14 });
    assert_eq!(h.state.selection_anchor, Some(Caret { line: 0, col: 12 }));

    h.alt_wheel_at(elsewhere, false);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    h.alt_wheel_at(elsewhere, false);
    assert_eq!(h.text(0), "meta height 15 // and a comment");
}

/// Alt + Up/Down is the keyboard spelling of the same gesture: Up steps the
/// number up, Down steps it down, and the caret stays on the line instead of
/// moving as a bare arrow would.
#[test]
fn alt_arrows_step_the_number_at_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 13); // between the digits of "16"

    h.key_mod(Key::ArrowUp, Modifiers::ALT);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
    assert_eq!(h.cursor(), Caret { line: 0, col: 14 });
    assert_eq!(h.state.selection_anchor, Some(Caret { line: 0, col: 12 }));

    h.key_mod(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    h.key_mod(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(h.text(0), "meta height 15 // and a comment");
    assert_eq!(h.cursor().line, 0, "the caret never left the line");
}

/// With no number to step, Alt + Up/Down keeps the arrow's usual meaning and
/// moves the caret.
#[test]
fn alt_arrows_away_from_a_digit_still_move_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(1, 3); // inside "meta" on the second line
    h.key_mod(Key::ArrowDown, Modifiers::ALT);
    assert_eq!(h.text(1), "meta ascent 12");
    assert_eq!(h.cursor(), Caret { line: 2, col: 3 });
}

/// The caret only has to be *adjacent* to a digit run: at its right edge the
/// preceding digits are what gets bumped.
#[test]
fn alt_wheel_takes_the_digits_before_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 14); // right after "16"
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
}

/// With no digit anywhere around the caret the gesture does nothing at all.
#[test]
fn alt_wheel_away_from_any_digit_does_nothing() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 3); // inside "meta"
    let pos = h.text_pos(0, 3);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    assert_eq!(h.state.selection_anchor, None);
}

/// A selection that is not a bare number is left alone: bumping it would have
/// to guess which of its parts is the number.
#[test]
fn alt_wheel_ignores_a_selection_that_is_not_a_number() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 12);
    for _ in 0..4 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT); // "16 /"
    }
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
}

/// A selection of one number padded with spaces is a number: the digits move
/// and the padding stays.
#[test]
fn alt_wheel_accepts_a_number_selection_with_surrounding_space() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 11); // the space before "16"
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    }
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "meta height 17 // and a comment");
}

/// Numbers are non-negative: wheeling down at zero leaves it at zero.
#[test]
fn alt_wheel_down_at_zero_stays_at_zero() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(2, 13); // the "0" of "descent 0"
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, false);
    assert_eq!(h.text(2), "meta descent 0");
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(2), "meta descent 1");
}

/// A run of ticks is one edit: the numbers scroll past several values, and a
/// single undo takes the whole run back — as typing does within its coalesce
/// window.
#[test]
fn alt_wheel_ticks_coalesce_into_one_undo() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 13);
    let pos = h.text_pos(0, 0);
    for _ in 0..3 {
        h.alt_wheel_at(pos, true);
    }
    assert_eq!(h.text(0), "meta height 19 // and a comment");

    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(0), "meta height 16 // and a comment");
    assert_eq!(
        h.cursor(),
        Caret { line: 0, col: 13 },
        "back to the pre-gesture caret"
    );
    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(
        h.text(0),
        "meta height 16 // and a comment",
        "nothing left to undo"
    );
}

/// The wheel notch belongs to the number, not to the view. egui spreads one
/// discrete notch over several frames, so a gesture that only consumed its own
/// frame left the rest of the notch to scroll the document under the caret.
#[test]
fn alt_wheel_does_not_also_scroll_the_view() {
    let src = format!("meta height 16\n{}", tall_doc());
    let mut h = EditorHarness::new(&src);
    h.click_text(0, 13);
    let pos = h.text_pos(0, 2);
    assert_eq!(h.scroll_y(), 0.0);

    // Wheel *down*: the direction that would scroll away from the top.
    h.alt_wheel_at(pos, false);
    for _ in 0..20 {
        h.frame();
    }
    assert_eq!(h.text(0), "meta height 15");
    assert!(
        h.scroll_y() < 0.01,
        "the view scrolled too: y = {}",
        h.scroll_y()
    );
}

/// Swallowing the notch must not latch: once the gesture's delta has drained,
/// an ordinary wheel scrolls the view as before.
#[test]
fn a_plain_wheel_still_scrolls_after_an_alt_gesture() {
    let src = format!("meta height 16\n{}", tall_doc());
    let mut h = EditorHarness::new(&src);
    h.click_text(0, 13);
    let pos = h.text_pos(0, 2);

    h.alt_wheel_at(pos, false);
    for _ in 0..20 {
        h.frame();
    }
    assert!(h.scroll_y() < 0.01);

    h.frame_with(
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: egui::vec2(0.0, -1.0),
                modifiers: Modifiers::NONE,
            },
        ],
        Modifiers::NONE,
    );
    for _ in 0..20 {
        h.frame();
    }
    assert!(
        h.scroll_y() > 1.0,
        "the view no longer scrolls; y = {}",
        h.scroll_y()
    );
}

// ---------------------------------------------------------------------------
// Following links (the input side of go back / go forward)
// ---------------------------------------------------------------------------

fn link_doc(body: &str) -> String {
    format!("glyph a 2 2\n@@..\n..@@\nglyph b\n{body}\n")
}

/// Doc-line index of the first text line starting with `prefix`.
#[track_caller]
fn text_line_at(h: &EditorHarness, prefix: &str) -> usize {
    h.lines
        .iter()
        .position(|l| matches!(l, DocLine::Text(s) if s.trim_start().starts_with(prefix)))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?}"))
}

/// Ctrl/Cmd+clicking a link reports the jump, and reports it as starting at
/// the *link* — not at the caret, which the click deliberately leaves where it
/// was. Go Back relies on that position to return to the reference.
#[test]
fn following_a_link_reports_the_link_position_not_the_caret() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("ref a 0 0"));
    let ref_line = text_line_at(&h, "ref a");
    let def_line = text_line_at(&h, "glyph a");

    // Park the caret somewhere unrelated, so a `from` taken from the caret
    // would be visibly wrong.
    h.click_text(text_line_at(&h, "glyph b"), 2);
    assert_eq!(h.state.cursor.line, text_line_at(&h, "glyph b"));

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, 4), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    assert_eq!(nav.from, Caret::new(ref_line, 4));
    match nav.target {
        NavTarget::Local { line } => assert_eq!(line, def_line),
        NavTarget::CrossFile(_) | NavTarget::Search(_) => {
            panic!("`a` is defined in this document")
        }
    }
    // The editor carries the local jump out itself.
    assert_eq!(h.state.cursor.line, def_line);
}

/// A link whose target is in another file cannot be resolved by the editor, so
/// it is handed to the host — still carrying the link position to come back to.
#[test]
fn a_link_to_another_file_is_handed_to_the_host() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("ref elsewhere 0 0"));
    let ref_line = text_line_at(&h, "ref elsewhere");

    h.last_nav = None;
    h.click_at_mod(h.text_pos(ref_line, 4), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    assert_eq!(nav.from, Caret::new(ref_line, 4));
    match &nav.target {
        NavTarget::CrossFile(goto) => assert_eq!(goto.name, "elsewhere"),
        NavTarget::Local { .. } | NavTarget::Search(_) => {
            panic!("a reference is not a definition, and `elsewhere` is not in this document")
        }
    }
    // Nothing moved: only the host can follow it.
    assert_ne!(h.state.cursor.line, ref_line + 1);
}

/// A jump the *host* carries out — a cross-file link, a search hit, an issue
/// click — moves the caret while egui's focus still sits wherever the gesture
/// started. The caret only paints while the editor has focus, so a jump that
/// left focus behind moved an invisible caret.
#[test]
fn a_host_jump_takes_focus_so_the_caret_shows() {
    let mut h = EditorHarness::new(&tall_doc());
    h.blur();
    assert!(!h.editor_has_focus(), "precondition: focus is elsewhere");

    h.state.goto_line(100);
    h.frame();

    assert!(
        h.editor_has_focus(),
        "a host-driven jump must take focus back, or its caret is invisible"
    );
}

/// Ctrl/Cmd+clicking the *definition* of a name asks the host to list its
/// appearances. Navigating would land on the line the click was already on, so
/// the gesture means "find references" here rather than "go to definition" —
/// and the editor must not move the caret itself.
#[test]
fn clicking_a_definition_asks_for_a_search() {
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("ref a 0 0"));
    let def_line = text_line_at(&h, "glyph a");

    h.click_text(text_line_at(&h, "glyph b"), 2);
    let parked = h.state.cursor.line;

    h.last_nav = None;
    h.click_at_mod(h.text_pos(def_line, 6), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    match &nav.target {
        NavTarget::Search(goto) => assert_eq!(goto.name, "a"),
        NavTarget::Local { .. } | NavTarget::CrossFile(_) => {
            panic!("a definition has nowhere to go")
        }
    }
    assert_eq!(h.state.cursor.line, parked, "the caret must not move");
}

/// An anchor is matched by name across glyphs and declared nowhere in
/// particular, so a click on one can only ever search — and searches for the
/// bare name, since `+above` and `-above` are two sides of one anchor.
#[test]
fn clicking_an_anchor_searches_for_it_without_its_sign() {
    use crate::editor::doc_links::LinkTargetKind;
    use crate::editor::document_view::NavTarget;

    let mut h = EditorHarness::new(&link_doc("anchor +above 1 0"));
    let anchor_line = text_line_at(&h, "anchor");

    h.last_nav = None;
    h.click_at_mod(h.text_pos(anchor_line, 9), Modifiers::COMMAND);

    let nav = h.last_nav.as_ref().expect("no navigation reported");
    match &nav.target {
        NavTarget::Search(goto) => {
            assert_eq!(goto.name, "above");
            assert_eq!(goto.kind, LinkTargetKind::Anchor);
        }
        NavTarget::Local { .. } | NavTarget::CrossFile(_) => {
            panic!("an anchor has no definition to go to")
        }
    }
}

/// A `SLICE :` qualifier is a link of its own, and it does not swallow the
/// links that follow it: the same line still names a glyph two tokens later.
#[test]
fn clicking_a_slice_qualifier_goes_to_the_slice() {
    use crate::editor::document_view::NavTarget;

    let doc = "slice narrow\nglyph a 2 2\n@@..\n..@@\nglyph b\nmap narrow : A = a\n";
    let mut h = EditorHarness::new(doc);
    let slice_line = text_line_at(&h, "slice narrow");
    let glyph_line = text_line_at(&h, "glyph a");
    let map_line = text_line_at(&h, "map narrow");

    h.last_nav = None;
    h.click_at_mod(h.text_pos(map_line, 6), Modifiers::COMMAND);
    match &h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line } => assert_eq!(*line, slice_line),
        _ => panic!("expected the slice declaration"),
    }

    h.last_nav = None;
    h.click_at_mod(h.text_pos(map_line, 17), Modifiers::COMMAND);
    match &h.last_nav.as_ref().expect("no navigation reported").target {
        NavTarget::Local { line } => assert_eq!(*line, glyph_line),
        _ => panic!("expected the glyph"),
    }
}

/// An ordinary click on a link is just a click — no jump, and nothing recorded.
#[test]
fn clicking_a_link_without_the_modifier_reports_nothing() {
    let mut h = EditorHarness::new(&link_doc("ref a 0 0"));
    let ref_line = text_line_at(&h, "ref a");

    h.last_nav = None;
    h.click_text(ref_line, 4);

    assert!(h.last_nav.is_none());
    assert_eq!(h.state.cursor, Caret::new(ref_line, 4));
}

/// A soft wrap is a drawing decision, not a change to the line: a name split
/// across one still links to the whole name, from either half. Reading the
/// links off the wrapped *segment* used to hand the host whatever half was
/// clicked — and the half that no longer started with `ref` linked nothing.
#[test]
fn a_link_split_by_a_soft_wrap_still_names_the_whole_symbol() {
    use crate::editor::document_view::NavTarget;

    // Long enough to wrap at any plausible editor width.
    let long: String = std::iter::repeat_n("very-long-glyph-name", 12)
        .collect::<Vec<_>>()
        .join("-");
    let mut h = EditorHarness::new(&link_doc(&format!("ref {long} 0 0")));
    let ref_line = text_line_at(&h, "ref ");

    let wrap_col = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == ref_line)
        .filter_map(|vl| match &vl.kind {
            SnapKind::Text { col_offset, .. } => Some(*col_offset),
            SnapKind::GridRow { .. } => None,
        })
        .find(|c| *c > 0)
        .expect("the line must wrap for this test to mean anything");
    let name_start = 4;
    let name_end = name_start + long.chars().count();
    assert!(
        name_start < wrap_col && wrap_col < name_end,
        "the wrap must fall inside the name, not between tokens",
    );

    // Both halves of the name are the same link, and both name it in full.
    for col in [wrap_col - 2, wrap_col + 2] {
        h.last_nav = None;
        h.click_at_mod(h.text_pos(ref_line, col), Modifiers::COMMAND);
        let nav = h
            .last_nav
            .as_ref()
            .unwrap_or_else(|| panic!("no navigation reported for a click at col {col}"));
        match &nav.target {
            NavTarget::CrossFile(goto) => assert_eq!(goto.name, long, "at col {col}"),
            NavTarget::Local { .. } | NavTarget::Search(_) => {
                panic!("`{long}` is not in this document")
            }
        }
    }
}

/// The same goes for a color swatch: a `fill` pushed onto a later segment by a
/// soft wrap is still a `ref` line's fill, and still paints its swatch. Read
/// off the segment alone, the tail no longer starts with `ref` and the token
/// simply vanished.
#[test]
fn a_color_token_pushed_past_a_soft_wrap_still_paints_its_swatch() {
    const FILL: &str = "#00ff00";
    let long: String = std::iter::repeat_n("very-long-glyph-name", 12)
        .collect::<Vec<_>>()
        .join("-");
    let line = format!("ref {long} 0 0 fill {FILL}");
    let mut h = EditorHarness::new(&link_doc(&line));
    let ref_line = text_line_at(&h, "ref ");
    h.frame();

    let wrapped = h
        .snap()
        .vlines
        .iter()
        .filter(|vl| vl.doc_line == ref_line)
        .filter(|vl| matches!(vl.kind, SnapKind::Text { col_offset, .. } if col_offset > 0))
        .count();
    assert!(
        wrapped > 0,
        "the line must wrap for this test to mean anything"
    );

    let col_start = line.chars().count() - FILL.len();
    assert!(
        h.color_backgrounds()
            .contains(&(ref_line, col_start, col_start + FILL.len())),
        "no swatch for the fill at col {col_start}: {:?}",
        h.color_backgrounds(),
    );
}

// ---------------------------------------------------------------------------
// Glyph metrics overlay
// ---------------------------------------------------------------------------

/// A combining mark, its metrics the way `font/comb.unf` writes them:
/// `left -3` pushes the ink three columns left of the origin and `top 14`
/// drops it onto the baseline, while `advance 0` makes it take no width.
const MARK_DOC: &str = "\
meta height 16
meta ascent 14
meta descent 2

glyph dia-below 6 2 mark advance 0 left -3 top 14
..............
@@@@@@@@@@@@..

map \u{0323} = dia-below
";

fn first_grid_line(h: &EditorHarness) -> usize {
    h.lines
        .iter()
        .position(|l| matches!(l, DocLine::Grid(_)))
        .expect("the document has a pixel grid")
}

/// `left`/`top` move the ink, so the em box lands at `-left` / `-top`, and the
/// drawn area grows to hold it: the two rows of ink sit at the *bottom* of a
/// box that reaches fourteen rows above them.
#[test]
fn the_metric_box_is_the_em_box_placed_against_the_ink() {
    let mut h = EditorHarness::new(MARK_DOC);
    let grid_line = first_grid_line(&h);

    let (before, rows_before) = h.metrics_of(grid_line);
    assert!(before.is_none(), "the overlay is off by default");
    assert_eq!(rows_before, vec![0, 1], "only the two ink rows are drawn");

    h.set_show_metrics(true);
    let (m, rows) = h.metrics_of(grid_line);
    let m = m.expect("the overlay is on");

    // `left -3` → origin at column 3; `advance 0` → the box has no width.
    assert_eq!((m.left, m.right), (3, 3));
    // `top 14` → the em box's top is fourteen rows above the ink, and its
    // bottom is `meta height` below that.
    assert_eq!((m.top, m.bottom), (-14, 2));
    // Two rows of ink cannot reach the baseline, so it is left off.
    assert_eq!(m.baseline, None);

    assert_eq!(
        rows.first().copied(),
        Some(-14),
        "the drawn area must reach the top of the metric box"
    );
    assert_eq!(rows.last().copied(), Some(1));
}

/// The baseline/ascent pair is drawn wherever there is room for both, and does
/// *not* wait for the glyph to be mapped: a glyph is normally drawn before it
/// is mapped, and metrics that only appear once a `map` line exists are metrics
/// you cannot design against. Room means the glyph clears the ascent — at
/// `ascent 14`, fifteen rows, since the two below it are the descent.
#[test]
fn the_baseline_needs_room_rather_than_a_mapping() {
    for (height, expected) in [(14u16, None), (15u16, Some(14))] {
        let source = format!(
            "meta height 16\nmeta ascent 14\nmeta descent 2\n\nglyph a 4 {height}\n{}",
            "@@......\n".repeat(height as usize),
        );
        let mut h = EditorHarness::new(&source);
        h.set_show_metrics(true);
        let grid_line = first_grid_line(&h);

        let m = h.metrics_of(grid_line).0.expect("the overlay is on");
        assert_eq!(
            m.baseline, expected,
            "a {height}-row glyph, with nothing mapping it"
        );
    }
}

/// A `scale N` glyph's grid is already in subcells (`document_io` multiplies the
/// declared dimensions), but `left`/`top`/`advance` and everything out of
/// `meta` are logical pixels, so the box has to scale them itself.
#[test]
fn the_metric_box_follows_the_glyph_scale() {
    let source = format!(
        "meta height 16\nmeta ascent 14\nmeta descent 2\n\n\
         glyph big 4 16 scale 2 advance 3 left -1 top 2\n{}",
        "@@..............\n".repeat(32),
    );
    let mut h = EditorHarness::new(&source);
    h.set_show_metrics(true);
    let grid_line = first_grid_line(&h);
    let m = h.metrics_of(grid_line).0.expect("the overlay is on");

    // Every logical figure doubled: origin at subcell 2, advance 6 subcells,
    // box top two logical rows above the ink, em box 32 subcells tall.
    assert_eq!((m.left, m.right), (2, 8));
    assert_eq!((m.top, m.bottom), (-4, 28));
    assert_eq!(m.baseline, Some(24));
}

/// An ordinary glyph with no metric flags: the box is the glyph's own area, so
/// switching the overlay on must not resize anything.
#[test]
fn a_plain_glyph_keeps_its_extent_when_the_overlay_is_on() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph a 4 16
"
    .to_string()
        + &"@@......\n".repeat(16);

    let mut h = EditorHarness::new(&source);
    let grid_line = first_grid_line(&h);
    let (_, rows_off) = h.metrics_of(grid_line);

    h.set_show_metrics(true);
    let (m, rows_on) = h.metrics_of(grid_line);
    let m = m.expect("the overlay is on");

    assert_eq!((m.left, m.right, m.top, m.bottom), (0, 4, 0, 16));
    assert_eq!(m.baseline, Some(14));
    assert_eq!(rows_on, rows_off, "the drawn rows must not move");
}

/// The box never runs past the glyph's own raster, and never past
/// `ascent + descent` either. Tying `bottom` to `meta height` alone showed
/// a one-row glyph sixteen rows tall.
#[test]
fn the_metric_box_is_clamped_to_the_ink_and_to_the_em_box() {
    for (rows, expected_bottom) in [(1u16, 1i16), (18u16, 16i16)] {
        // `height` is deliberately not `ascent + descent` here: the clamp is
        // the latter, and 20 would show through if it were not.
        let source = format!(
            "meta height 20\nmeta ascent 14\nmeta descent 2\n\nglyph a 4 {rows}\n{}",
            "@@......\n".repeat(rows as usize),
        );
        let mut h = EditorHarness::new(&source);
        h.set_show_metrics(true);
        let grid_line = first_grid_line(&h);

        let (m, drawn) = h.metrics_of(grid_line);
        let m = m.expect("the overlay is on");
        assert_eq!((m.top, m.bottom), (0, expected_bottom), "{rows}-row glyph");
        assert_eq!(
            drawn.len(),
            rows.max(expected_bottom as u16) as usize,
            "{rows}-row glyph must not be padded out to the em box"
        );
    }
}

/// The horizontal extent of a glyph's drawn grid rows, as the snapshot has it.
#[track_caller]
fn grid_extent_x(h: &EditorHarness, grid_doc_line: usize) -> (i16, i16) {
    h.snap()
        .vlines
        .iter()
        .find_map(|vl| match vl.kind {
            SnapKind::GridRow { left, right, .. } if vl.doc_line == grid_doc_line => {
                Some((left, right))
            }
            _ => None,
        })
        .expect("a grid row for the glyph")
}

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

fn selected_shape(h: &EditorHarness) -> crate::pixel::PixelShape {
    match h.state.mode {
        EditMode::GlyphEdit { selected_shape, .. } => selected_shape,
        ref other => panic!("not editing pixels: {other:?}"),
    }
}

/// A 4×2 glyph, entered in GlyphEdit mode with the pointer over its grid.
fn palette_harness() -> EditorHarness {
    let mut h = EditorHarness::new("glyph test 4 2\n........\n........\n");
    h.click_grid_cell(1, 0, 0);
    h.frame();
    h
}

#[test]
fn wheel_over_the_grid_rotates_the_selected_shape() {
    use crate::editor::glyph_widget::rotate_shape;

    let mut h = palette_harness();
    // `f` picks HALF1 outright, so the starting orientation is known.
    h.key(Key::F);
    h.frame();
    let start = selected_shape(&h);
    assert_eq!(start.shape_id(), crate::pixel::PX_HALF1);
    let start_rotation = h.state.shape_rotation;

    let pos = h.grid_cell_pos(1, 0, 1);
    h.wheel_at_mod(pos, false, Modifiers::NONE);

    assert_eq!(
        selected_shape(&h),
        rotate_shape(start, 1),
        "a wheel notch should turn the shape a quarter clockwise"
    );
    assert_eq!(h.state.shape_rotation, (start_rotation + 1) % 4);
}

#[test]
fn shift_wheel_picks_another_shape_at_the_same_rotation() {
    use crate::editor::glyph_widget::{palette_shapes, rotate_shape, shape_orbit};

    let mut h = palette_harness();
    h.key(Key::F);
    h.frame();
    let pos = h.grid_cell_pos(1, 0, 1);

    // Rotate twice, then step to the neighbouring palette cell.
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    let rotation = h.state.shape_rotation;
    let before = shape_orbit(selected_shape(&h)).unwrap().0;

    h.wheel_at_mod(pos, false, Modifiers::SHIFT);

    let (cell, _) = shape_orbit(selected_shape(&h)).unwrap();
    assert_eq!(cell, before + 1, "shift+wheel walks the palette");
    assert_eq!(
        h.state.shape_rotation, rotation,
        "the rotation is remembered across a shape change"
    );
    assert_eq!(
        selected_shape(&h),
        rotate_shape(palette_shapes()[cell], rotation as i32),
        "the new shape arrives already rotated"
    );
}

#[test]
fn the_whole_palette_rotates_with_the_wheel() {
    use crate::editor::glyph_widget::{palette_shapes, rotate_shape};

    let mut h = palette_harness();
    let pos = h.grid_cell_pos(1, 0, 1);
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    let rotation = h.state.shape_rotation;
    assert_eq!(rotation, 1);

    // Clicking a palette cell yields that cell *as drawn*, i.e. rotated —
    // which is what makes the rotation visible in every cell, not just the
    // preview under the cursor.
    let cell = 3;
    let click = h.palette_cell_pos(cell);
    h.click_at(click);
    h.frame();
    assert_eq!(
        selected_shape(&h),
        rotate_shape(palette_shapes()[cell], rotation as i32)
    );
}

/// The palette wraps at the slants, so the families added after them are only
/// reachable if the second row is laid out and hit-tested at all.
#[test]
fn a_second_row_palette_cell_can_be_picked() {
    use crate::editor::glyph_widget::{palette_row_col, palette_shapes, shape_orbit};
    use crate::pixel::{PX_HOUSE2, PixelShape};

    let mut h = palette_harness();
    let (cell, _) = shape_orbit(PixelShape::new(PX_HOUSE2, true)).unwrap();
    assert_eq!(palette_row_col(cell).0, 1, "cell {cell} should be on row 1");

    h.click_at(h.palette_cell_pos(cell));
    h.frame();
    assert_eq!(selected_shape(&h), palette_shapes()[cell]);
}

/// Clicking the cell that is already selected is not a no-op: it flips the
/// fill, so the shape and its complement are one click apart.
#[test]
fn clicking_the_selected_palette_cell_toggles_the_fill() {
    use crate::editor::glyph_widget::shape_orbit;

    let mut h = palette_harness();
    let (cell, _) = shape_orbit(selected_shape(&h)).unwrap();
    let click = h.palette_cell_pos(cell);

    let before = selected_shape(&h);
    h.click_at(click);
    h.frame();
    assert_eq!(
        selected_shape(&h),
        before.with_fill_toggled(),
        "re-clicking the selected cell flips the fill"
    );

    h.click_at(click);
    h.frame();
    assert_eq!(selected_shape(&h), before, "and flips it back");
}

/// The selected cell shows the fill the selection actually carries, so the
/// toggling is visible: click the same cell and it alternates between the solid
/// and the dimmed drawing. Every other cell keeps the palette's own fill.
#[test]
fn the_selected_palette_cell_is_drawn_with_the_selections_fill() {
    use crate::editor::glyph_widget::{palette_shapes, shape_orbit};

    let mut h = palette_harness();
    let (cell, _) = shape_orbit(selected_shape(&h)).unwrap();
    let other = if cell == 0 { 2 } else { 0 };
    let other_filled = palette_shapes()[other].is_filled();
    let click = h.palette_cell_pos(cell);

    assert!(h.palette_cell_filled(cell), "starts out filled");

    h.click_at(click);
    h.frame();
    assert!(!selected_shape(&h).is_filled());
    assert!(
        !h.palette_cell_filled(cell),
        "the selected cell follows the selection into unfilled"
    );
    assert_eq!(
        h.palette_cell_filled(other),
        other_filled,
        "an unselected cell keeps the palette's own fill"
    );

    h.click_at(click);
    h.frame();
    assert!(h.palette_cell_filled(cell), "and back to filled");
}

/// Only the *selected* cell toggles; picking a different one still yields that
/// cell's own fill, unchanged from the palette.
#[test]
fn clicking_another_palette_cell_takes_its_own_fill() {
    use crate::editor::glyph_widget::palette_shapes;

    let mut h = palette_harness();
    let cell = 1; // PX_DOT, the one unfilled representative on row 0
    assert!(!palette_shapes()[cell].is_filled());

    h.click_at(h.palette_cell_pos(cell));
    h.frame();
    assert_eq!(selected_shape(&h), palette_shapes()[cell]);
}

#[test]
fn a_shape_shortcut_pulls_the_palette_rotation_with_it() {
    let mut h = palette_harness();
    let pos = h.grid_cell_pos(1, 0, 1);
    h.wheel_at_mod(pos, false, Modifiers::NONE);
    h.wheel_at_mod(pos, false, Modifiers::NONE);

    // `a` names PX_HALF3 absolutely; the palette adopts its orientation
    // instead of leaving the two disagreeing.
    h.key(Key::A);
    h.frame();
    let shape = selected_shape(&h);
    assert_eq!(shape.shape_id(), crate::pixel::PX_HALF3);
    assert_eq!(
        h.state.shape_rotation,
        crate::editor::glyph_widget::shape_orbit(shape).unwrap().1
    );
}

// -- edit border under scroll -------------------------------------------------

/// The edit-mode border spans the whole glyph but used to be drawn only from
/// the grid's *top* row. With that row scrolled above the viewport (culled),
/// the border vanished even though most of the glyph was still on screen.
#[test]
fn edit_border_survives_the_top_grid_row_being_culled() {
    let mut source = String::from("glyph tall 16 300\n");
    for _ in 0..300 {
        source.push_str("@@..............................\n");
    }
    source.push_str("\nglyph small 2 2\n@@@@\n@@@@\n");
    let mut h = EditorHarness::new(&source);
    // Taller than twice the font height, so it opens folded; this test is
    // about the block open.
    h.click_fold_marker(0);
    h.state.mode = EditMode::GlyphEdit {
        item_idx: 0,
        selected_shape: crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
    };
    h.frame();
    let full_border = h
        .edit_border_rect()
        .expect("border must be painted with the whole grid in view");

    // Scroll to the end of the document: the tall grid's top rows are culled,
    // its bottom rows still visible. The caret move drops back to Normal
    // mode, so re-enter GlyphEdit once the scroll has settled.
    h.state.goto_line(6);
    h.frame();
    h.frame();
    assert!(
        h.scroll_y() > 1000.0,
        "expected a scroll; y={}",
        h.scroll_y()
    );
    h.state.mode = EditMode::GlyphEdit {
        item_idx: 0,
        selected_shape: crate::pixel::PixelShape::new(crate::pixel::PX_ALMOSTFULL, true),
    };
    h.frame();
    assert!(
        h.scroll_y() > 1000.0,
        "scroll must survive; y={}",
        h.scroll_y()
    );

    let border = h
        .edit_border_rect()
        .expect("border must still be painted when the grid's top row is culled");
    // The border still describes the whole glyph — its top is above the
    // viewport, and its size is unchanged.
    assert!(
        border.min.y < 0.0,
        "border top should be off-screen: {border:?}"
    );
    assert_eq!(border.size(), full_border.size());
}

/// Clicking a menu-bar item hands egui's keyboard focus to that button, and the
/// editor is left with none: every key after it — Ctrl+V included — goes
/// nowhere until the user clicks back into the grid. An action dispatched from
/// a menu therefore has to hand the focus back, or the document it just changed
/// takes no keyboard input at all.
#[test]
fn a_menu_action_leaves_the_editor_ready_for_keys() {
    use crate::editor::pixel_selection;

    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0);
    assert!(matches!(h.state.mode, EditMode::GlyphEdit { .. }));
    assert!(h.editor_has_focus());

    // Opening the menu takes the focus away.
    h.blur();
    assert!(!h.editor_has_focus());

    // What Edit ▸ Adjust scale does to the document.
    assert!(pixel_selection::handle_adjust_scale(
        &h.doc,
        &mut h.lines,
        &mut h.state,
        2
    ));
    crate::editor::document_view::flush_document_changes(&mut h.lines, &mut h.doc, &mut h.state);
    h.state.refocus();
    h.frame();

    assert!(
        h.editor_has_focus(),
        "the editor should have the keyboard back"
    );
    h.paste("@@\n@@");
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0 }),
        "paste should switch to PixelSelect, mode = {:?}",
        h.state.mode
    );
}

/// A menu-bar click takes the editor's keyboard focus (see
/// [`crate::editor::EditorState::refocus`]) before any of the menu's entries
/// can run, and an unfocused editor commits its floating selection away. The
/// selection is what the Selection menu acts on, so it has to survive the trip
/// into the menu — otherwise every entry there is greyed out by the time the
/// user reaches it. `blur_commits_floating` covers the other side: a focus loss
/// with no menu open still commits.
#[test]
fn a_floating_selection_survives_the_menu_that_acts_on_it() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 0)); // select a single cell
    h.drag_grid(1, (0, 0), (2, 0)); // and float it down to row 2
    assert!(h.state.pixel_selection.as_ref().unwrap().is_floating());

    // Opening a menu over the editor: the focus goes to the menu button.
    h.menu_open = true;
    h.blur();
    assert!(
        h.state
            .pixel_selection
            .as_ref()
            .is_some_and(|s| s.is_floating()),
        "the selection the menu acts on should still be there"
    );

    // Once the menu is gone, an editor that never got the focus back commits
    // it as any other unfocused editor does.
    h.menu_open = false;
    h.frame();
    assert!(h.state.pixel_selection.is_none(), "menu closed: commit");
    assert!(h.grid(1).get(2, 0).is_filled());
}

// ---------------------------------------------------------------------------
// Implicit whole-grid selection, Ctrl+A, and shift-click extension
// ---------------------------------------------------------------------------

const WHOLE_GRID_TEXT: &str = "@@@@@@..\n..@@@@..\n........";

fn make_glyph_edit_harness() -> EditorHarness {
    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0); // enter GlyphEdit
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "should be in GlyphEdit"
    );
    h
}

#[track_caller]
fn assert_grid_empty(h: &EditorHarness) {
    let grid = h.grid(1);
    for r in 0..grid.height {
        for c in 0..grid.width {
            assert!(grid.get(r, c).is_empty(), "cell {r},{c} should be empty");
        }
    }
}

#[test]
fn copy_without_selection_copies_whole_grid_in_pixel_select() {
    let mut h = make_pixel_select_harness();
    assert!(h.state.pixel_selection.is_none());
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some(WHOLE_GRID_TEXT));
    assert!(
        h.state.pixel_selection.is_none(),
        "an implicit copy should not leave a selection behind"
    );
}

#[test]
fn copy_without_selection_copies_whole_grid_in_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some(WHOLE_GRID_TEXT));
}

#[test]
fn cut_without_selection_takes_whole_grid_in_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.cut();
    assert_eq!(h.last_copied_text.as_deref(), Some(WHOLE_GRID_TEXT));
    assert_grid_empty(&h);
}

#[test]
fn delete_without_selection_clears_whole_grid() {
    let mut h = make_pixel_select_harness();
    h.key(Key::Delete);
    assert_grid_empty(&h);

    // ...and it is one undo step.
    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert!(h.grid(1).get(0, 0).is_filled());
}

#[test]
fn delete_without_selection_clears_whole_grid_in_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.key(Key::Delete);
    assert_grid_empty(&h);
}

#[test]
fn copy_with_selection_is_unaffected() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.copy();
    assert_eq!(h.last_copied_text.as_deref(), Some("@@@@"));
}

#[test]
fn ctrl_a_selects_the_whole_grid_from_glyph_edit() {
    let mut h = make_glyph_edit_harness();
    h.key_mod(Key::A, Modifiers::COMMAND);
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0 }),
        "Ctrl+A should enter PixelSelect, got {:?}",
        h.state.mode
    );
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 4, 3));
    assert!(!sel.is_floating());
}

#[test]
fn ctrl_a_selects_the_whole_grid_from_pixel_select() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    h.key_mod(Key::A, Modifiers::COMMAND);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 4, 3));
}

#[test]
fn ctrl_a_does_not_select_text_in_a_pixel_mode() {
    let mut h = make_pixel_select_harness();
    h.key_mod(Key::A, Modifiers::COMMAND);
    assert!(
        h.state.selection_anchor.is_none(),
        "Ctrl+A in a pixel mode must not select the document text"
    );
}

#[test]
fn shift_click_extends_the_selection_outward() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1)); // 2x1 at the top-left
    h.click_grid_cell_mod(1, 2, 3, Modifiers::SHIFT);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!(
        (sel.row, sel.col, sel.width, sel.height),
        (0, 0, 4, 3),
        "shift-click should keep the drag's start point and move the end point"
    );
    assert!(!sel.is_floating(), "extending must not float the selection");
}

#[test]
fn shift_click_inside_the_selection_shrinks_it() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (2, 3)); // the whole grid
    // A plain click here would start a move drag; with Shift the cell becomes
    // the new end point instead.
    h.click_grid_cell_mod(1, 1, 1, Modifiers::SHIFT);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 2, 2));
    assert!(!sel.is_floating());
}

#[test]
fn shift_click_extends_from_the_drag_start_not_the_corner() {
    let mut h = make_pixel_select_harness();
    // Drag upward-left: the rect is (0,0)-(1,1) but the drag *started* at (1,1).
    h.drag_grid(1, (1, 1), (0, 0));
    let sel = h.state.pixel_selection.as_ref().unwrap();
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 2, 2));

    h.click_grid_cell_mod(1, 2, 3, Modifiers::SHIFT);
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!(
        (sel.row, sel.col, sel.width, sel.height),
        (1, 1, 3, 2),
        "the fixed corner should be the drag's start cell (1,1)"
    );
}

#[test]
fn plain_click_inside_the_selection_still_moves_it() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));
    h.drag_grid(1, (0, 0), (0, 2)); // plain drag from inside: move
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert!(sel.is_floating(), "a plain drag from inside still moves");
}

// The Edit menu acts through `apply_edit_action`, so these drive that entry
// point rather than the keys — the point is that both readings agree.

#[test]
fn the_edit_menu_offers_the_pixel_grid_in_a_pixel_mode() {
    let h = make_glyph_edit_harness();
    let caps = h.state.edit_menu_caps(&h.doc);
    assert!(caps.can_edit, "the menu must not be dead in a pixel mode");
    assert!(
        caps.has_selection,
        "the implicit whole-grid selection is what Cut/Copy/Delete would take"
    );

    // A ref-only glyph has no pixels to take.
    let mut h2 = EditorHarness::new("glyph part 2 1\n@@@@\nglyph whole\nref part 0 0");
    h2.click_grid_cell(1, 0, 0);
    h2.state.mode = EditMode::PixelSelect { item_idx: 1 };
    h2.frame();
    assert!(!h2.state.edit_menu_caps(&h2.doc).has_selection);
}

#[test]
fn the_edit_menu_copy_and_delete_take_the_whole_grid() {
    use crate::edit_menu::EditAction;
    let ctx = egui::Context::default();
    let mut h = make_glyph_edit_harness();

    h.state
        .apply_edit_action(EditAction::Copy, &h.doc, &mut h.lines, &ctx);
    assert_eq!(
        ctx.output(|o| o.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })),
        Some(WHOLE_GRID_TEXT.to_string())
    );

    h.state
        .apply_edit_action(EditAction::Delete, &h.doc, &mut h.lines, &ctx);
    h.frame();
    assert_grid_empty(&h);
}

#[test]
fn the_edit_menu_select_all_frames_the_grid() {
    use crate::edit_menu::EditAction;
    let mut h = make_glyph_edit_harness();
    h.state.apply_edit_action(
        EditAction::SelectAll,
        &h.doc,
        &mut h.lines,
        &egui::Context::default(),
    );
    assert!(matches!(
        h.state.mode,
        EditMode::PixelSelect { item_idx: 0 }
    ));
    let sel = h.state.pixel_selection.as_ref().expect("selection");
    assert_eq!((sel.row, sel.col, sel.width, sel.height), (0, 0, 4, 3));
    assert!(
        h.state.selection_anchor.is_none(),
        "it must not select the document text as well"
    );
}

// ---------------------------------------------------------------------------
// Glyph resize mode (F2 over a grid)
// ---------------------------------------------------------------------------

const RESIZE_SRC: &str = "\
glyph dot 2 2
@@..
..@@

glyph user 4 4
ref dot 1 1
";

/// Enter resize mode the way a user does: click into the grid, press F2.
fn resize_harness() -> EditorHarness {
    let mut h = EditorHarness::new(RESIZE_SRC);
    h.click_grid_cell(1, 0, 0);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "clicking the grid edits it: {:?}",
        h.state.mode
    );
    h.key(Key::F2);
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { item_idx: 0 }),
        "F2 over a grid resizes the glyph: {:?}",
        h.state.mode
    );
    h
}

/// An arrow moves the boundary the way it points: `Left` grows the glyph
/// leftwards, and the preview is the document itself, so the ink moves with it.
#[test]
fn resize_arrow_grows_the_edge_it_points_at() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    assert_eq!(h.text(0), "glyph dot 3 2");
    assert_eq!(h.grid(1).width, 3);
    assert!(
        h.grid(1).get(0, 1).is_filled() && !h.grid(1).get(0, 0).is_filled(),
        "the ink moved right with the new column, not into it"
    );
    assert_view_consistent(&h);
}

/// Shift moves the *far* edge, so the boundary still travels the way the key
/// points and the glyph shrinks instead of growing.
#[test]
fn resize_shift_arrow_moves_the_far_edge() {
    let mut h = resize_harness();
    h.key_mod(Key::ArrowUp, Modifiers::SHIFT);
    assert_eq!(h.text(0), "glyph dot 2 1", "the bottom edge came up");
    assert_eq!(h.grid(1).height, 1);
    // The row that survived is the top one: nothing moved, the box shrank.
    assert!(h.grid(1).get(0, 0).is_filled());
}

/// Escape puts the document back exactly as it was, in one step, and leaves
/// the mode it was entered from.
#[test]
fn resize_escape_restores_the_glyph() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowUp);
    assert_eq!(h.text(0), "glyph dot 3 3");
    h.key(Key::Escape);
    assert_eq!(h.text(0), "glyph dot 2 2");
    assert_eq!(h.grid(1).width, 2);
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "cancelling goes back to the mode F2 was pressed in: {:?}",
        h.state.mode
    );
    assert!(
        h.take_resize().is_none(),
        "a cancelled resize asks for nothing"
    );
    assert_view_consistent(&h);
}

/// Enter hands the resize to the host and rolls the preview back: the host is
/// the only thing that can move the `ref`s in the other files, so it redoes
/// the whole edit as the one entry it records.
#[test]
fn resize_enter_hands_the_action_to_the_host() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowLeft);
    h.key(Key::ArrowDown);
    h.key(Key::Enter);
    let action = h.take_resize().expect("the resize was applied");
    assert_eq!(action.glyph_name, "dot");
    assert_eq!(action.deltas.left, 2);
    assert_eq!(action.deltas.bottom, 1);
    assert_eq!(action.deltas.right, 0);
    assert_eq!(action.deltas.top, 0);
    assert_eq!(
        h.text(0),
        "glyph dot 2 2",
        "the editor leaves the document untouched for the host to edit once"
    );
    assert!(matches!(
        h.state.mode,
        EditMode::GlyphEdit { item_idx: 0, .. }
    ));
}

/// The panel beside the grid is Apply and Cancel and nothing else while a
/// resize is live; clicking Cancel is Escape.
#[test]
fn resize_panel_cancel_button_restores_the_glyph() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    assert_eq!(h.text(0), "glyph dot 3 2");
    let pos = h.resize_button_pos(crate::editor::glyph_resize::PanelAction::Cancel);
    h.click_at(pos);
    assert_eq!(h.text(0), "glyph dot 2 2");
    assert!(h.take_resize().is_none());
}

/// ...and clicking Apply is Enter.
#[test]
fn resize_panel_apply_button_hands_over_the_action() {
    let mut h = resize_harness();
    h.key(Key::ArrowRight);
    let pos = h.resize_button_pos(crate::editor::glyph_resize::PanelAction::Apply);
    h.click_at(pos);
    let action = h.take_resize().expect("the resize was applied");
    assert_eq!(action.deltas.right, 1);
    assert_eq!(h.text(0), "glyph dot 2 2");
}

/// Dragging the boundary moves it a whole logical pixel at a time.
#[test]
fn resize_drag_moves_the_grabbed_edge() {
    let mut h = resize_harness();
    let rect = h.edit_border_rect().expect("the boundary is painted");
    let cell = h.snap().grid_cell;
    let grab = egui::pos2(rect.right(), rect.center().y);
    h.press_at(grab);
    h.move_pointer(egui::pos2(grab.x + cell * 2.0, grab.y));
    h.release_at(egui::pos2(grab.x + cell * 2.0, grab.y));
    assert_eq!(
        h.text(0),
        "glyph dot 4 2",
        "the right edge followed the pointer"
    );
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { .. }),
        "a press on the grid grabs an edge rather than painting a pixel",
    );
}

/// The mode is over the moment the editor is not the surface being typed
/// into: a resize nobody can see must not stay half-applied in the buffer.
#[test]
fn resize_is_cancelled_by_losing_the_focus() {
    let mut h = resize_harness();
    h.key(Key::ArrowLeft);
    h.blur();
    assert_eq!(h.text(0), "glyph dot 2 2");
    assert!(matches!(h.state.mode, EditMode::GlyphEdit { .. }));
    assert!(h.take_resize().is_none());
}

/// F2 with the caret merely sitting on a grid line resizes too — no pixel mode
/// needed — and cancelling goes back to that plain caret.
#[test]
fn resize_starts_from_a_caret_on_the_grid_line() {
    let mut h = EditorHarness::new(RESIZE_SRC);
    h.click_text(0, 0);
    h.state.cursor = Caret::new(1, 0);
    h.frame();
    h.key(Key::F2);
    assert!(
        matches!(h.state.mode, EditMode::GlyphResize { item_idx: 0 }),
        "{:?}",
        h.state.mode
    );
    h.key(Key::Escape);
    assert!(
        matches!(h.state.mode, EditMode::Normal),
        "{:?}",
        h.state.mode
    );
}

/// The panel is chrome, not glyph rendering, so it has to follow the theme.
/// The editor's own palette deliberately keeps its panel colours dark in both
/// themes — they sit behind glyph swatches over the dark grid — and reusing
/// them here left dark text on a dark button in light mode.
#[test]
fn resize_buttons_stay_legible_in_both_themes() {
    use crate::editor::glyph_resize::PanelAction;

    /// Perceived brightness, 0..1, of a colour painted over the given theme.
    fn luma(c: egui::Color32) -> f32 {
        (0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32) / 255.0
    }

    let mut h = resize_harness();
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        h.set_theme(theme);
        for action in [PanelAction::Apply, PanelAction::Cancel] {
            let (fill, text) = h.resize_button_colors(action);
            assert!(
                (luma(fill) - luma(text)).abs() > 0.25,
                "{action:?} is unreadable in {theme:?}: text {text:?} on {fill:?}",
            );
        }
    }
}

/// The four handles have to be *inside* the glyph's box, because the grid band
/// is clipped to exactly where the grid starts: a handle centred on the left
/// edge of a glyph at column 0 falls in the half that is clipped away, so it
/// was invisible even though the band around it still grabbed the pointer.
#[test]
fn resize_handles_are_inside_the_drawn_band() {
    let h = resize_harness();
    let rect = h.edit_border_rect().expect("the boundary is painted");
    let strip = &h.snap().strip;
    for (side, handle) in crate::editor::glyph_resize::handle_rects(rect, 1.0) {
        assert!(
            handle.left() >= strip.x && handle.right() <= strip.right(),
            "the {side:?} handle is outside the clipped band: {handle:?} vs {}..{}",
            strip.x,
            strip.right(),
        );
        assert!(
            rect.contains_rect(handle),
            "the {side:?} handle is outside the glyph's own box: {handle:?} vs {rect:?}",
        );
    }
}

/// The overlay has to be painted *over* the glyph, not under it. It used to go
/// out with the first visible grid row, so every row below that one painted
/// over it: with the handles moved inside the box, all that survived was the
/// top border and one cell's worth of the sides.
#[test]
fn resize_overlay_is_painted_over_the_grid() {
    let h = resize_harness();
    let border = h.edit_border_rect().expect("the boundary is painted");
    let color = crate::editor::colors::Palette::dark().pixel_selection;
    let painted = h.painted_rects();
    // The outline and its four handles are all painted in the overlay colour;
    // what must not follow them is anything of the grid.
    let last_overlay = painted
        .iter()
        .rposition(|p| p.fill == color || p.stroke.color == color)
        .expect("the overlay is among the painted rects");
    assert!(
        painted[last_overlay].rect.expand(0.5).intersects(border),
        "the overlay belongs to this glyph's box",
    );
    for later in &painted[last_overlay + 1..] {
        assert!(
            !(later.fill.a() > 0
                && later
                    .rect
                    .intersect(later.clip)
                    .intersects(border.shrink(1.0))),
            "{later:?} is painted over the resize overlay {border:?}",
        );
    }
}

/// A document too short to fill the editor leaves an empty band below its last
/// line. A click there used to land on nothing at all: the canvas was allocated
/// exactly as tall as its content, so the band was outside the response.
fn short_doc() -> String {
    String::from("glyph foo 4 2\n@@......\n......@@\n// trailing note")
}

#[test]
fn click_below_the_last_line_lands_on_it() {
    let mut h = EditorHarness::new(&short_doc());
    let last = h.lines.len() - 1;
    assert_eq!(h.text(last), "// trailing note");

    let on_line = h.text_pos(last, 6);
    let below = egui::pos2(on_line.x, h.content_bottom() + 40.0);
    h.click_at(below);
    assert_eq!(h.cursor(), Caret::new(last, 6));
}

#[test]
fn dragging_into_the_empty_band_selects_to_the_last_line() {
    let mut h = EditorHarness::new(&short_doc());
    let last = h.lines.len() - 1;

    let start = h.text_pos(last, 2);
    let end_x = h.text_pos(last, 9).x;
    let below = egui::pos2(end_x, h.content_bottom() + 40.0);
    h.press_at(start);
    // Straight down first, past egui's drag threshold: the drag starts inside
    // the empty band at the same x, which is what column 2 is anchored on.
    h.move_pointer(egui::pos2(start.x, start.y + 12.0));
    h.move_pointer(below);
    h.release_at(below);
    h.frame();

    assert_eq!(
        h.state.selection_range(),
        Some((Caret::new(last, 2), Caret::new(last, 9)))
    );
}

#[test]
fn right_click_in_the_empty_band_opens_the_edit_menu() {
    let mut h = EditorHarness::new(&short_doc());
    h.click_text(0, 0);
    h.type_text("X");
    assert_eq!(h.text(0), "Xglyph foo 4 2");

    let below = egui::pos2(h.text_pos(0, 0).x, h.content_bottom() + 40.0);
    h.right_click_at(below);
    h.frame();

    // The first item of the edit menu is Undo; egui lays it out just inside
    // the menu frame at the click position.
    let item = below + egui::vec2(24.0, 14.0);
    h.move_pointer(item);
    h.click_at(item);
    h.frame();

    assert_eq!(
        h.text(0),
        "glyph foo 4 2",
        "the menu's Undo should have run"
    );
}

// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------

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

/// Which DocLines the frame actually drew.
fn shown_lines(h: &EditorHarness) -> Vec<usize> {
    let mut seen: Vec<usize> = h.snap().vlines.iter().map(|vl| vl.doc_line).collect();
    seen.dedup();
    seen
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

#[test]
fn ctrl_period_folds_the_group_the_caret_sits_in() {
    let mut h = EditorHarness::new(&fold_doc());
    h.click_text(2, 3);
    h.key_mod(Key::Period, Modifiers::COMMAND);

    assert_eq!(shown_lines(&h), vec![0, 3, 4, 5, 6]);
    assert_eq!(
        h.cursor(),
        Caret::new(0, 3),
        "the caret comes up to the header at the same column"
    );

    h.key_mod(Key::Period, Modifiers::COMMAND);
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

    h.key_mod(Key::Period, Modifiers::COMMAND);
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
    h.key_mod(Key::Period, Modifiers::COMMAND);
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
