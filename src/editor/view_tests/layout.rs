//! The gutter and the initial layout: how wide the line numbers make it,
//! and what a grid or a shut group does to that.

use super::*;

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
