//! Tests for making the two builds' outlines point-compatible.
//!
//! Two properties carry everything, and every test here is one of them:
//!
//! 1. **Compatible** — the two sides have the same contour count and the same
//!    point count per contour.
//! 2. **Unchanged** — each side still draws exactly what it drew. That is
//!    checked by simplifying away the padding (repeated points, and the
//!    collinear points a repeat can leave behind) and comparing to the input,
//!    which is the only honest way to say "the drawing survived".

use super::*;
use crate::render::ttf_builder::masters::{MasterPair, compatible_masters};

/// A contour reduced to what a rasterizer would actually fill: repeated points
/// dropped, then collinear ones, then rotated to a canonical start. A zero-area
/// contour reduces to nothing at all, which is what makes a degenerate stand-in
/// invisible to this comparison.
fn drawn(c: &[(i16, i16)]) -> Vec<(i16, i16)> {
    let mut pts: Vec<(i16, i16)> = Vec::new();
    for &p in c {
        if pts.last() != Some(&p) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    if pts.len() < 3 {
        return Vec::new();
    }
    canonicalize_contour(&pts)
}

fn drawn_set(cs: &[Vec<(i16, i16)>]) -> Vec<Vec<(i16, i16)>> {
    let mut out: Vec<Vec<(i16, i16)>> = cs.iter().map(|c| drawn(c)).collect();
    out.retain(|c| !c.is_empty());
    out.sort();
    out
}

/// The two properties, asserted together. Every test calls this; what differs
/// is the pair of drawings handed in.
#[track_caller]
fn check(v: &[Vec<(i16, i16)>], b: &[Vec<(i16, i16)>]) -> MasterPair {
    let pair = compatible_masters(v, b);
    assert!(
        pair.is_compatible(),
        "masters are not point-compatible: {:?} vs {:?}",
        pair.vector.iter().map(Vec::len).collect::<Vec<_>>(),
        pair.bitmap.iter().map(Vec::len).collect::<Vec<_>>(),
    );
    assert_eq!(
        drawn_set(&pair.vector),
        drawn_set(v),
        "the vector master's drawing changed",
    );
    assert_eq!(
        drawn_set(&pair.bitmap),
        drawn_set(b),
        "the bitmap master's drawing changed",
    );
    pair
}

/// The case the whole design exists for: a 45° diagonal and the staircase that
/// rounds it. The staircase has far more corners, so the diagonal is the side
/// that gets padded — and it must come out still a diagonal.
#[test]
fn a_diagonal_and_its_staircase_become_compatible() {
    let diagonal = vec![vec![(0, 0), (400, 400), (0, 400)]];
    let staircase = vec![vec![
        (0, 0),
        (100, 0),
        (100, 100),
        (200, 100),
        (200, 200),
        (300, 200),
        (300, 300),
        (400, 300),
        (400, 400),
        (0, 400),
    ]];
    let pair = check(&diagonal, &staircase);
    assert_eq!(pair.vector.len(), 1);
    assert!(
        pair.vector[0].len() >= staircase[0].len(),
        "the padded diagonal must reach at least the staircase's point count",
    );
}

/// A contour one master has and the other does not: a thin stroke the rounding
/// dropped. The vector side keeps it, the bitmap side gets it collapsed, and a
/// collapsed contour draws nothing.
#[test]
fn a_contour_only_one_master_has_is_collapsed_on_the_other() {
    let v = vec![
        vec![(0, 0), (400, 0), (400, 400), (0, 400)],
        vec![(10, 10), (20, 10), (20, 300), (10, 300)],
    ];
    let b = vec![vec![(0, 0), (400, 0), (400, 400), (0, 400)]];
    let pair = check(&v, &b);
    assert_eq!(pair.bitmap.len(), 2, "the bitmap side gains a stand-in");
    let stand_in = pair
        .bitmap
        .iter()
        .find(|c| c.iter().all(|p| *p == c[0]))
        .expect("one bitmap contour should be fully collapsed");
    assert_eq!(stand_in.len(), 4, "it matches its counterpart's point count");
}

/// The same in the other direction — the rounding joined two pieces into one,
/// so the vector master is the side that gains the stand-in.
#[test]
fn an_extra_bitmap_contour_is_collapsed_on_the_vector_side() {
    let v = vec![vec![(0, 0), (100, 0), (100, 100), (0, 100)]];
    let b = vec![
        vec![(0, 0), (100, 0), (100, 100), (0, 100)],
        vec![(200, 200), (300, 200), (300, 300), (200, 300)],
    ];
    let pair = check(&v, &b);
    assert_eq!(pair.vector.len(), 2);
}

/// Winding is a hard constraint: an outer contour must never be paired with a
/// hole, or the glyph would turn inside out partway between the masters. Here
/// each master has one of each, and the pairing has to keep them apart.
#[test]
fn an_outer_contour_is_never_paired_with_a_hole() {
    let outer_v = vec![(0, 0), (400, 0), (400, 400), (0, 400)];
    let hole_v = vec![(100, 100), (100, 300), (300, 300), (300, 100)];
    let outer_b = vec![(0, 0), (500, 0), (500, 500), (0, 500)];
    let hole_b = vec![(100, 100), (100, 400), (400, 400), (400, 100)];
    assert!(
        (signed_area_of(&outer_v) < 0) != (signed_area_of(&hole_v) < 0),
        "fixture is wrong: the two contours should wind opposite ways",
    );
    let pair = check(
        &[outer_v.clone(), hole_v.clone()],
        &[outer_b.clone(), hole_b.clone()],
    );
    for (v, b) in pair.vector.iter().zip(&pair.bitmap) {
        assert_eq!(
            signed_area_of(v) < 0,
            signed_area_of(b) < 0,
            "a paired contour changed winding",
        );
    }
}

fn signed_area_of(c: &[(i16, i16)]) -> i64 {
    let n = c.len();
    (0..n)
        .map(|i| {
            let (x0, y0) = c[i];
            let (x1, y1) = c[(i + 1) % n];
            x0 as i64 * y1 as i64 - x1 as i64 * y0 as i64
        })
        .sum()
}

/// Identical drawings are the common case — a glyph with no sub-pixel detail
/// rounds to itself — and must cost nothing: no padding, no reordering.
#[test]
fn two_identical_drawings_are_left_alone() {
    let c = vec![vec![(0, 0), (400, 0), (400, 400), (0, 400)]];
    let pair = check(&c, &c);
    assert_eq!(pair.vector, c);
    assert_eq!(pair.bitmap, c);
}

/// The degenerate inputs this module is most likely to be handed: an empty
/// glyph, and one where a whole drawing is missing.
#[test]
fn empty_and_one_sided_inputs_are_handled() {
    check(&[], &[]);
    let c = vec![vec![(0, 0), (100, 0), (100, 100)]];
    let only_vector = check(&c, &[]);
    assert_eq!(only_vector.bitmap.len(), 1);
    assert!(only_vector.bitmap[0].iter().all(|p| *p == only_vector.bitmap[0][0]));
    let only_bitmap = check(&[], &c);
    assert_eq!(only_bitmap.vector.len(), 1);
}
