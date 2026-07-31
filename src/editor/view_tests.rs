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
            (SnapKind::Text { text, col_offset, .. }, DocLine::Text(s)) => {
                let seg: String = s.chars().skip(*col_offset).take(text.chars().count()).collect();
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
    assert_eq!(h.grid_row_count(1), 16, "grid is still 16 rows while deferred");
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
    assert!(!h.grid(1).get(12, 12).is_empty(), "truncated pixel restored");
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
    assert_eq!(h.text(0), "glyph foo 16 16", "header text restored by one undo");
    assert_eq!(h.lines, original_lines, "grid restored by the same undo");
    assert_eq!(h.grid_row_count(1), 16);
    assert!(!h.state.undo.can_undo(), "no leftover undo entry for the resize");

    // Redo has to bring both sides back in one step too.
    h.key_mod(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.text(0), "glyph foo 18 16");
    assert_eq!((h.grid(1).width, h.grid(1).height), (18, 16));
    assert!(!h.state.undo.can_redo(), "no leftover redo entry for the resize");
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
    assert_eq!(h.grid(1).height, 8, "header edit applied on entering the grid");
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
    assert!(copied.contains("@@......"), "grid row 0 should be in clipboard");
    assert!(copied.contains("......@@"), "grid row 1 should be in clipboard");

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

fn ctrl_space(h: &mut EditorHarness) {
    h.key_mod(Key::Space, Modifiers::CTRL);
}

#[test]
fn autocomplete_trigger_and_dismiss() {
    let mut h = EditorHarness::new(&ac_doc());
    h.click_text(5, 4);
    assert!(h.state.autocomplete.is_none());

    ctrl_space(&mut h);
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
    ctrl_space(&mut h);

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
    ctrl_space(&mut h);
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
    ctrl_space(&mut h);
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
    ctrl_space(&mut h);
    h.key(Key::Enter);
    assert_ne!(h.text(5), original_text);

    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(5), original_text);
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

#[test]
fn view_cache_reused_when_idle_and_rebuilt_on_edit() {
    let mut h = EditorHarness::new(&sample_doc());

    let ptr_before = h.state.view_cache.as_ref().expect("cache built").data_ptr();
    h.frame();
    h.frame();
    let ptr_idle = h.state.view_cache.as_ref().expect("cache kept").data_ptr();
    assert_eq!(ptr_before, ptr_idle, "idle frames must reuse the cached view");

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
    assert!(scroll_y_z1 > 100.0, "should have scrolled down; y = {scroll_y_z1}");

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
    assert!(first_y > 100.0, "first pane should have scrolled; y = {first_y}");
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
    assert_eq!(h.state.cursor, first_cursor, "click leaked into the first pane");
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
fn enter_layer_move(h: &mut EditorHarness, grid_doc_line: usize, item_idx: usize, layer_idx: usize) {
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
            EditMode::LayerMove { item_idx: 2, layer_idx: 0 }
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
            EditMode::LayerMove { item_idx: 2, layer_idx: 0 }
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

/// The subglyph menu ("Inline to pixels") used to be reachable only by
/// right-clicking the ref thumbnail in the inline tools panel. Right-clicking
/// the grid while that ref layer is the selected one must offer it too.
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
    let sel = h.state.pixel_selection.as_ref().expect("should have selection");
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

    let sel = h.state.pixel_selection.as_ref().expect("should have selection");
    assert!(sel.is_floating());
    // The original position (0,0)-(1,1) in grid should be cleared
    let grid = h.grid(1);
    assert!(grid.get(0, 0).is_empty(), "original cell should be empty after move");
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
    let sel = h.state.pixel_selection.as_ref().expect("should have selection after undo");
    assert!(!sel.is_floating(), "should be grounded after undo");
    assert_eq!((sel.row, sel.col), (0, 0), "should be back at original position");

    // Grid should be restored
    let grid = h.grid(1);
    assert!(grid.get(0, 0).is_filled(), "grid should be restored after undo");
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
    assert!(h.state.pixel_selection.is_none(), "selection should be cleared");

    // The moved pixels should be merged into the grid at new position
    let grid = h.grid(1);
    assert!(grid.get(2, 0).is_filled(), "moved pixel should be merged at new position");
    // Original position should be empty
    assert!(grid.get(0, 0).is_empty(), "original position should be empty");
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
    assert!(grid.get(2, 0).is_empty(), "floating pixels should not merge on delete");
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
    assert!(grid.get(0, 0).is_filled(), "grid should be back to its original state");
    assert!(grid.get(0, 1).is_filled());
    assert!(grid.get(2, 0).is_empty());
}

#[test]
fn copy_produces_correct_text() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (1, 1));

    // Copy (uses Event::Copy, same as Cmd+C)
    h.copy();
    let copied = h.last_copied_text.as_ref().expect("should have copied text");
    assert_eq!(copied, "@@@@\n..@@", "copied text should match grid content");
}


#[test]
fn paste_in_pixel_select_creates_floating() {
    let mut h = make_pixel_select_harness();
    h.paste("@@..\n..@@");

    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0 }),
        "should stay in PixelSelect"
    );
    let sel = h.state.pixel_selection.as_ref().expect("should have selection");
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
    let sel = h.state.pixel_selection.as_ref().expect("should have selection");
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
    assert!(!ok, "paste should fail when clipboard is smaller than selection");
}

#[test]
fn right_click_cancels_selection() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 1));
    assert!(h.state.pixel_selection.is_some());

    let pos = h.grid_cell_pos(1, 0, 0);
    h.right_click_at(pos);
    assert!(h.state.pixel_selection.is_none(), "right click should cancel selection");
}

#[test]
fn blur_commits_floating() {
    let mut h = make_pixel_select_harness();
    h.drag_grid(1, (0, 0), (0, 0)); // select single cell
    h.drag_grid(1, (0, 0), (2, 0)); // move to row 2

    assert!(h.state.pixel_selection.as_ref().unwrap().is_floating());
    h.blur();
    assert!(h.state.pixel_selection.is_none(), "blur should commit and clear");

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
    let panel_w = crate::editor::glyph_widget::PALETTE_COLS as f32
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

    assert!(block_bottom > 2400.0, "the grid should overflow the viewport");
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
        SnapKind::Text { text, comment_col, .. } => {
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
    assert!(!grid.get(0, 0).is_empty(), "pixels survived the header edit");
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

/// `font-meta` puts several numbers on one plain text line, so the caret can
/// be placed on them by character column.
const NUMBER_DOC: &str = "font-meta height 16 ascent 12 descent 0\nglyph sp 2 2\n....\n....\n";

/// Alt + wheel bumps the number the caret sits in, and it does so with the
/// *pointer* anywhere over the editor — the gesture is anchored to the caret,
/// not to what is under the mouse.
#[test]
fn alt_wheel_increments_the_number_at_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 18); // between the digits of "16"
    let elsewhere = h.text_pos(1, 2);

    h.alt_wheel_at(elsewhere, true);
    assert_eq!(h.text(0), "font-meta height 17 ascent 12 descent 0");
    // The bumped number is left selected, so the next tick repeats on it.
    assert_eq!(h.cursor(), Caret { line: 0, col: 19 });
    assert_eq!(h.state.selection_anchor, Some(Caret { line: 0, col: 17 }));

    h.alt_wheel_at(elsewhere, false);
    assert_eq!(h.text(0), "font-meta height 16 ascent 12 descent 0");
    h.alt_wheel_at(elsewhere, false);
    assert_eq!(h.text(0), "font-meta height 15 ascent 12 descent 0");
}

/// The caret only has to be *adjacent* to a digit run: at its right edge the
/// preceding digits are what gets bumped.
#[test]
fn alt_wheel_takes_the_digits_before_the_caret() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 19); // right after "16"
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "font-meta height 17 ascent 12 descent 0");
}

/// With no digit anywhere around the caret the gesture does nothing at all.
#[test]
fn alt_wheel_away_from_any_digit_does_nothing() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 3); // inside "font-meta"
    let pos = h.text_pos(0, 3);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "font-meta height 16 ascent 12 descent 0");
    assert_eq!(h.state.selection_anchor, None);
}

/// A selection that is not a bare number is left alone: bumping it would have
/// to guess which of its parts is the number.
#[test]
fn alt_wheel_ignores_a_selection_that_is_not_a_number() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 17);
    for _ in 0..4 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT); // "16 a"
    }
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "font-meta height 16 ascent 12 descent 0");
}

/// A selection of one number padded with spaces is a number: the digits move
/// and the padding stays.
#[test]
fn alt_wheel_accepts_a_number_selection_with_surrounding_space() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 16); // the space before "16"
    for _ in 0..3 {
        h.key_mod(Key::ArrowRight, Modifiers::SHIFT);
    }
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "font-meta height 17 ascent 12 descent 0");
}

/// Numbers are non-negative: wheeling down at zero leaves it at zero.
#[test]
fn alt_wheel_down_at_zero_stays_at_zero() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 38); // the "0" of "descent 0"
    let pos = h.text_pos(0, 0);
    h.alt_wheel_at(pos, false);
    assert_eq!(h.text(0), "font-meta height 16 ascent 12 descent 0");
    h.alt_wheel_at(pos, true);
    assert_eq!(h.text(0), "font-meta height 16 ascent 12 descent 1");
}

/// A run of ticks is one edit: the numbers scroll past several values, and a
/// single undo takes the whole run back — as typing does within its coalesce
/// window.
#[test]
fn alt_wheel_ticks_coalesce_into_one_undo() {
    let mut h = EditorHarness::new(NUMBER_DOC);
    h.click_text(0, 18);
    let pos = h.text_pos(0, 0);
    for _ in 0..3 {
        h.alt_wheel_at(pos, true);
    }
    assert_eq!(h.text(0), "font-meta height 19 ascent 12 descent 0");

    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(0), "font-meta height 16 ascent 12 descent 0");
    assert_eq!(h.cursor(), Caret { line: 0, col: 18 }, "back to the pre-gesture caret");
    h.key_mod(Key::Z, Modifiers::COMMAND);
    assert_eq!(h.text(0), "font-meta height 16 ascent 12 descent 0", "nothing left to undo");
}

/// The wheel notch belongs to the number, not to the view. egui spreads one
/// discrete notch over several frames, so a gesture that only consumed its own
/// frame left the rest of the notch to scroll the document under the caret.
#[test]
fn alt_wheel_does_not_also_scroll_the_view() {
    let src = format!("font-meta height 16 ascent 12 descent 0\n{}", tall_doc());
    let mut h = EditorHarness::new(&src);
    h.click_text(0, 18);
    let pos = h.text_pos(0, 2);
    assert_eq!(h.scroll_y(), 0.0);

    // Wheel *down*: the direction that would scroll away from the top.
    h.alt_wheel_at(pos, false);
    for _ in 0..20 {
        h.frame();
    }
    assert_eq!(h.text(0), "font-meta height 15 ascent 12 descent 0");
    assert!(h.scroll_y() < 0.01, "the view scrolled too: y = {}", h.scroll_y());
}

/// Swallowing the notch must not latch: once the gesture's delta has drained,
/// an ordinary wheel scrolls the view as before.
#[test]
fn a_plain_wheel_still_scrolls_after_an_alt_gesture() {
    let src = format!("font-meta height 16 ascent 12 descent 0\n{}", tall_doc());
    let mut h = EditorHarness::new(&src);
    h.click_text(0, 18);
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
    assert!(h.scroll_y() > 1.0, "the view no longer scrolls; y = {}", h.scroll_y());
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

// ---------------------------------------------------------------------------
// Glyph metrics overlay
// ---------------------------------------------------------------------------

/// A combining mark, its metrics the way `font/comb.unf` writes them:
/// `left -3` pushes the ink three columns left of the origin and `top 14`
/// drops it onto the baseline, while `advance 0` makes it take no width.
const MARK_DOC: &str = "\
font-meta height 16 ascent 14 descent 2

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
    // bottom is `font-meta height` below that.
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
            "font-meta height 16 ascent 14 descent 2\n\nglyph a 4 {height}\n{}",
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
/// `font-meta` are logical pixels, so the box has to scale them itself.
#[test]
fn the_metric_box_follows_the_glyph_scale() {
    let source = format!(
        "font-meta height 16 ascent 14 descent 2\n\n\
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
font-meta height 16 ascent 14 descent 2

glyph a 4 16
".to_string()
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
/// `ascent + descent` either. Tying `bottom` to `font-meta height` alone showed
/// a one-row glyph sixteen rows tall.
#[test]
fn the_metric_box_is_clamped_to_the_ink_and_to_the_em_box() {
    for (rows, expected_bottom) in [(1u16, 1i16), (18u16, 16i16)] {
        // `height` is deliberately not `ascent + descent` here: the clamp is
        // the latter, and 20 would show through if it were not.
        let source = format!(
            "font-meta height 20 ascent 14 descent 2\n\nglyph a 4 {rows}\n{}",
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
font-meta height 16 ascent 14 descent 2

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
    let grid_line = 3;
    assert_eq!(grid_extent_x(&h, grid_line), (0, 2), "the mark's own extent");

    // The mark has no refs, so layer 0 is its `-above` anchor.
    enter_layer_move(&mut h, grid_line, 2, 0);
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
font-meta height 16 ascent 14 descent 2

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
    let grid_line = 3;
    enter_layer_move(&mut h, grid_line, 2, 0);
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
font-meta height 16 ascent 14 descent 2

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
    let comp_grid_line = 11;
    assert_eq!(grid_extent_x(&h, comp_grid_line), (0, 4), "comp's own extent");

    // comp has one ref and no declared points, so layer 1 is the inherited
    // `+above`.
    enter_layer_move(&mut h, comp_grid_line, 6, 1);
    assert!(
        matches!(h.state.mode, EditMode::LayerMove { item_idx: 6, layer_idx: 1 }),
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
font-meta height 16 ascent 14 descent 2

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
    let comp_grid_line = 7;
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
