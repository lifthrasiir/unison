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
            (SnapKind::Text { text, col_offset }, DocLine::Text(s)) => {
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
