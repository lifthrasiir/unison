//! Reference chart strips: which lines get a row, what it costs the layout,
//! and what happens with no strips on hand. See
//! [`crate::editor::ref_images`].

use super::*;
use crate::editor::ref_images::{REF_IMAGE_HEIGHT, REF_IMAGE_ROW, RefImages};

/// Two glyphs of one character, a `ref` to it, and a comment naming it.
fn source() -> String {
    [
        "// han-4e00 is the one to compare against",
        "glyph han-4e00 2 2",
        "..@@",
        "@@..",
        "",
        "glyph han-4e00:alt 2 2",
        "@@..",
        "..@@",
        "",
        "glyph other 2 2",
        "..@@",
        "@@..",
        "  ref han-4e00",
        "",
    ]
    .join("\n")
}

fn harness_with_strips(cps: &[u32]) -> EditorHarness {
    let mut h = EditorHarness::new(&source());
    h.ref_images = Some(RefImages::for_test(cps.iter().copied().collect()));
    h.frame();
    h
}

fn strip_rows(h: &EditorHarness) -> Vec<(usize, u32)> {
    h.snap()
        .vlines
        .iter()
        .filter_map(|vl| match &vl.kind {
            SnapKind::RefImage { codepoint } => Some((vl.doc_line, *codepoint)),
            _ => None,
        })
        .collect()
}

/// The row goes above the *first* `glyph` line naming the code point, once
/// per file — not above the comment, the `ref` or the second drawing.
#[test]
fn one_strip_above_the_first_glyph_line_that_names_it() {
    let h = harness_with_strips(&[0x4e00]);
    assert_eq!(strip_rows(&h), vec![(1, 0x4e00)]);
    assert_view_consistent(&h);
}

/// A code point the directory has no strip for gets no row, which is also
/// what the whole file looks like before the one directory scan lands.
#[test]
fn a_code_point_with_no_strip_gets_no_row() {
    let h = harness_with_strips(&[0x4e01]);
    assert!(strip_rows(&h).is_empty());
    let h = EditorHarness::new(&source());
    assert!(strip_rows(&h).is_empty());
}

/// The strip's row is its own fixed height and it pushes the line it
/// introduces down by exactly that much — the text below it is untouched.
#[test]
fn a_strip_row_is_one_fixed_height_the_lines_below_it_keep() {
    let plain = EditorHarness::new(&source());
    let with_strip = harness_with_strips(&[0x4e00]);
    let y_of = |h: &EditorHarness, line: usize| {
        h.snap()
            .vlines
            .iter()
            .find(|vl| vl.doc_line == line && matches!(vl.kind, SnapKind::Text { .. }))
            .map(|vl| vl.y)
            .expect("the line is drawn")
    };
    let row = with_strip
        .snap()
        .vlines
        .iter()
        .find(|vl| matches!(vl.kind, SnapKind::RefImage { .. }))
        .expect("a strip row");
    assert_eq!(row.height, REF_IMAGE_ROW);
    assert_eq!(row.gutter, None, "a strip is no source line of its own");
    assert_eq!(
        y_of(&with_strip, 1) - y_of(&plain, 1),
        REF_IMAGE_ROW,
        "the `glyph` line is pushed down by the strip's row"
    );
    assert_eq!(
        y_of(&with_strip, 7) - y_of(&plain, 7),
        REF_IMAGE_ROW,
        "and so is everything below it, by the same one row"
    );
}

/// A strip rides on the line it introduces: folding the glyph's block leaves
/// its header on screen, so the strip stays above it. Nothing has to know
/// about strips for that to hold — they are attributed to a document line like
/// every other visual line.
#[test]
fn a_collapsed_glyph_keeps_the_strip_over_its_header() {
    let mut h = harness_with_strips(&[0x4e00]);
    h.click_fold_marker(1);
    h.frame();
    assert_eq!(strip_rows(&h), vec![(1, 0x4e00)]);
    // The grid it hid is gone, so this is a collapsed block and not a no-op.
    assert!(
        !h.snap()
            .vlines
            .iter()
            .any(|vl| vl.doc_line == 2 && matches!(vl.kind, SnapKind::GridRow { .. }))
    );
}

/// A strip that has been read draws at one image pixel to the point, whatever
/// the editor's zoom, and is clipped to the band rather than spilling past it.
#[test]
fn a_strip_draws_at_its_own_size_whatever_the_zoom() {
    let mut h = harness_with_strips(&[0x4e00]);
    let dark = h.ctx.style().visuals.dark_mode;
    let store = h.ref_images.clone().expect("a store");
    store.preload_for_test(0x4e00, [2000, 90], dark);
    h.frame();
    let (drawn, clip) = *h.painted_images().first().expect("the strip is drawn");
    assert_eq!(drawn.height(), 90.0);
    assert_eq!(drawn.width(), 2000.0);
    assert!(
        clip.width() < drawn.width(),
        "a strip wider than the band has to be clipped to it"
    );

    // Zooming the editor magnifies the drawing, not the chart photograph.
    h.zoom = 2;
    h.frame();
    let (zoomed, _) = *h.painted_images().first().expect("the strip is drawn");
    assert_eq!(zoomed.size(), drawn.size());
}

/// A strip taller than its row is scaled down whole rather than cropped — the
/// case for a strip from something other than the extraction script.
#[test]
fn a_strip_taller_than_its_row_is_scaled_to_fit() {
    let mut h = harness_with_strips(&[0x4e00]);
    let dark = h.ctx.style().visuals.dark_mode;
    h.ref_images.clone().expect("a store").preload_for_test(
        0x4e00,
        [400, (REF_IMAGE_HEIGHT as usize) * 2],
        dark,
    );
    h.frame();
    let (drawn, _) = *h.painted_images().first().expect("the strip is drawn");
    assert_eq!(drawn.height(), REF_IMAGE_HEIGHT);
    assert_eq!(drawn.width(), 200.0, "scaled, not cropped");
}

/// Dragging a strip scrolls it sideways, and only as far as it overflows.
#[test]
fn dragging_a_strip_scrolls_it_within_its_overflow() {
    let mut h = harness_with_strips(&[0x4e00]);
    let dark = h.ctx.style().visuals.dark_mode;
    h.ref_images
        .clone()
        .expect("a store")
        .preload_for_test(0x4e00, [2000, 90], dark);
    h.frame();
    let left = |h: &EditorHarness| h.painted_images().first().expect("the strip").0.left();
    let before = left(&h);
    let row = h
        .snap()
        .vlines
        .iter()
        .find(|vl| matches!(vl.kind, SnapKind::RefImage { .. }))
        .map(|vl| vl.y + vl.height / 2.0)
        .expect("a strip row");
    let x = h.snap().strip.x + 40.0;
    h.press_at(egui::pos2(x, row));
    h.move_pointer(egui::pos2(x - 120.0, row));
    h.frame();
    assert_eq!(left(&h), before - 120.0, "the strip follows the pointer");
    // Past its own overflow it stops, rather than sliding off the band.
    h.move_pointer(egui::pos2(x - 100_000.0, row));
    h.frame();
    let overflow = 2000.0 - h.snap().strip.w;
    assert_eq!(left(&h), before - overflow);
    h.release_at(egui::pos2(x - 100_000.0, row));
    h.frame();
    // The caret never moved: the drag was the strip's, not a text selection.
    assert_eq!(h.state.cursor, Caret::new(0, 0));
}
