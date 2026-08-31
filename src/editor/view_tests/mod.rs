//! End-to-end GUI behavior tests for the document editor.
//!
//! These drive the real `show_document` frame loop through
//! [`EditorHarness`]: synthetic keyboard/mouse input goes in, and the
//! assertions read both the editor/document state and the rendered layout
//! (visual lines, grid rows, gutter line numbers) captured per frame.

mod anchor_shadow;
mod annotations;
mod autocomplete;
mod backref_shadow;
mod clipboard;
mod editing;
mod empty_band;
mod folding;
mod grid_band;
mod grid_clipboard;
mod layers;
mod layout;
mod links;
mod metrics;
mod number_nudge;
mod palette;
mod pixel_select;
mod popups;
mod resize;
mod samples;
mod scroll_zoom;
mod structure;

use crate::document::DocLine;
use crate::editor::caret::Caret;
use crate::editor::harness::{EditorHarness, SnapKind};
use crate::editor::{EditMode, ScrollIntent};
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

const RESIZE_SRC: &str = "\
glyph dot 2 2
@@..
..@@

glyph user 4 4
ref dot 1 1
";

/// A combining mark, its metrics the way `font/comb.unf` writes them:
/// `origin 3 -14` puts the box's corner three columns right of the ink and
/// fourteen rows below it, while `advance 0` makes it take no width.
const MARK_DOC: &str = "\
meta height 16
meta ascent 14
meta descent 2

glyph dia-below 6 2 mark advance 0 origin 3 -14
..............
@@@@@@@@@@@@..

map \u{0323} = dia-below
";

const WHOLE_GRID_TEXT: &str = "@@@@@@..\n..@@@@..\n........";

/// `meta` puts one number on a plain text line, so the caret can be placed on
/// it by character column. The trailing comment is what a selection can run
/// into, which is the case `alt_wheel_ignores_a_selection_that_is_not_a_number`
/// needs; `descent 0` on its own line is the lower bound case.
const NUMBER_DOC: &str =
    "meta height 16 // and a comment\nmeta ascent 12\nmeta descent 0\nglyph sp 2 2\n....\n....\n";

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

/// Which DocLines the frame actually drew.
fn shown_lines(h: &EditorHarness) -> Vec<usize> {
    let mut seen: Vec<usize> = h.snap().vlines.iter().map(|vl| vl.doc_line).collect();
    seen.dedup();
    seen
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

/// A document of `n` comment lines — nothing but line numbers to draw.
fn numbered_doc(n: usize) -> String {
    (1..=n)
        .map(|i| format!("# line {i}\n"))
        .collect::<Vec<_>>()
        .concat()
}

fn first_grid_line(h: &EditorHarness) -> usize {
    h.lines
        .iter()
        .position(|l| matches!(l, DocLine::Grid(_)))
        .expect("the document has a pixel grid")
}

fn make_pixel_select_harness() -> EditorHarness {
    let mut h = EditorHarness::new("glyph test 4 3\n@@@@@@..\n..@@@@..\n........");
    h.click_grid_cell(1, 0, 0); // enter GlyphEdit
    assert!(
        matches!(h.state.mode, EditMode::GlyphEdit { item_idx: 0, .. }),
        "should be in GlyphEdit"
    );
    h.key(Key::Backtick); // enter PixelSelect
    assert!(
        matches!(h.state.mode, EditMode::PixelSelect { item_idx: 0, .. }),
        "should be in PixelSelect"
    );
    h
}

/// Build a document tall enough to scroll (20 × 16-row glyphs ≈ 5000 px).
/// How far below the viewport's top a document line is drawn, as the frame the
/// harness last painted has it.
#[track_caller]
fn view_offset_of(h: &EditorHarness, line: usize) -> f32 {
    let snap = h.snap();
    let content_top = snap.vlines.first().expect("an empty view").y;
    let vl = snap
        .vlines
        .iter()
        .find(|vl| vl.doc_line == line)
        .unwrap_or_else(|| panic!("line {line} is not laid out"));
    // The first visual line sits at the content's top, which is the viewport's
    // top less however far the view has scrolled.
    vl.y - (content_top + h.scroll_y())
}

/// Doc-line index of the first text line starting with `prefix`.
#[track_caller]
fn text_line_at(h: &EditorHarness, prefix: &str) -> usize {
    h.lines
        .iter()
        .position(|l| matches!(l, DocLine::Text(s) if s.trim_start().starts_with(prefix)))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?}"))
}

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
