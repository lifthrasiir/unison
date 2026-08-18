//! The metrics overlay: which rectangle the box is, and what moves it.

use super::*;

/// A stated `extent` height is the box's height. Left unstated the box is the
/// em box placed at the glyph's origin — a one-row glyph is not padded out to
/// sixteen — but a source that says how tall its box is has said something the
/// overlay must not overrule.
#[test]
fn the_metric_box_takes_a_stated_extent_height() {
    let source = |flags: &str| {
        format!(
            "\
meta height 16
meta ascent 14
meta descent 2

glyph tall 2 2 {flags}
@@@@
@@@@

map A = tall
"
        )
    };
    let box_of = |flags: &str| {
        let mut h = EditorHarness::new(&source(flags));
        h.set_show_metrics(true);
        let grid_line = first_grid_line(&h);
        let m = h.metrics_of(grid_line).0.expect("the overlay is on");
        (m.top, m.bottom)
    };

    // Unstated: the em box, clamped to what the glyph actually draws.
    assert_eq!(box_of("advance 2"), (0, 2));
    // Stated: the box is as tall as it says, ink or no ink.
    assert_eq!(box_of("extent 2 16"), (0, 16));
    assert_eq!(box_of("extent 2 1"), (0, 1));
}

/// The overlay reads the *declared box*, whichever flag states it: `extent`
/// says the same thing `advance` does about the width, so a glyph written with
/// one must not be drawn with the other's answer — a mark spelled `extent 0 16`
/// used to get the box its raster happened to need.
#[test]
fn the_metric_box_reads_extent_as_well_as_advance() {
    let source = "\
meta height 16
meta ascent 14
meta descent 2

glyph dia-below 6 2 mark extent 0 16 origin 3 -14
..............
@@@@@@@@@@@@..

map \u{0323} = dia-below
";
    let mut h = EditorHarness::new(source);
    h.set_show_metrics(true);
    let grid_line = first_grid_line(&h);
    let m = h.metrics_of(grid_line).0.expect("the overlay is on");
    assert_eq!(
        (m.left, m.right),
        (3, 3),
        "`extent 0 …` is a box with no width, exactly as `advance 0` is"
    );
}

/// The box sits at the glyph's `origin`, and the drawn area grows to hold it:
/// the two rows of ink sit at the *bottom* of a box that reaches fourteen rows
/// above them.
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
/// declared dimensions), but `origin`/`advance` and everything out of `meta`
/// are logical pixels, so the box has to scale them itself.
#[test]
fn the_metric_box_follows_the_glyph_scale() {
    let source = format!(
        "meta height 16\nmeta ascent 14\nmeta descent 2\n\n\
         glyph big 4 16 scale 2 advance 3 origin 1 -2\n{}",
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
