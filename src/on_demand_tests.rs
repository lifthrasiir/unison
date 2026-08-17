//! Tests for [`super::on_demand`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items while keeping the source at a readable size.

use super::*;

/// Ink flag of every *logical* pixel of an on-demand glyph's grid, as a
/// `lh × lw` table. The flag is uniform across a logical pixel's subcells
/// by construction, so reading the top-left subcell is enough; the helper
/// asserts that uniformity rather than trusting it.
fn logical_fill(name: &str) -> Vec<Vec<bool>> {
    let Some(OnDemandGlyph::Shape(rect)) = parse_on_demand_glyph(name) else {
        panic!("{name} must parse as an on-demand shape");
    };
    let grid = make_on_demand_grid(&rect);
    let s = rect.scale.max(1) as u16;
    (0..grid.height / s)
        .map(|lr| {
            (0..grid.width / s)
                .map(|lc| {
                    let want = grid.get(lr * s, lc * s).is_bitmap_filled();
                    for dr in 0..s {
                        for dc in 0..s {
                            assert_eq!(
                                grid.get(lr * s + dr, lc * s + dc).is_bitmap_filled(),
                                want,
                                "{name}: ink flag not uniform across logical pixel ({lr},{lc})"
                            );
                        }
                    }
                    want
                })
                .collect()
        })
        .collect()
}

fn count_filled(name: &str) -> usize {
    logical_fill(name).iter().flatten().filter(|f| **f).count()
}

#[test]
fn on_demand_bitmap_fill_rounds_coverage_half_up() {
    // 4x5p1r3 is 5⅓ tall: logical row 5 is covered ⅓ and stays dark.
    let fill = logical_fill("4x5p1r3");
    assert_eq!(fill.len(), 6);
    for (lr, row) in fill.iter().enumerate() {
        let want = lr < 5;
        assert!(
            row.iter().all(|f| *f == want),
            "row {lr}: {row:?}, expected all {want}"
        );
    }

    // 4x-0p2r3 is a single ⅔-covered row: ⅔ ≥ ½, so it lights up.
    assert_eq!(logical_fill("4x-0p2r3"), vec![vec![true; 4]]);

    // Exactly ½ rounds up.
    assert_eq!(logical_fill("1p1r2x1"), vec![vec![true, true]]);
    // Just under ½ does not.
    assert_eq!(logical_fill("1p2r5x1"), vec![vec![true, false]]);
}

#[test]
fn on_demand_bitmap_fill_leaves_integer_rects_alone() {
    // Whole-pixel shapes have coverage 1 everywhere, so no rounding rule
    // can change them — this is what keeps the plain `WxH` names stable.
    assert_eq!(logical_fill("3x5"), vec![vec![true; 3]; 5]);
    assert_eq!(count_filled("1x1"), 1);
}

#[test]
fn on_demand_bitmap_fill_rounds_triangle_edge_cells() {
    // 8x16-ul: 56 pixels lie fully inside and 16 straddle the hypotenuse,
    // every one of them covered exactly ½ — all of which round up.
    assert_eq!(count_filled("8x16-ul"), 64);
    // 9x6-ul: the 2:3 slope leaves edge cells on both sides of ½.
    assert_eq!(count_filled("9x6-ul"), 27);
    // 2x2-ul: one full pixel plus two half pixels.
    assert_eq!(
        logical_fill("2x2-ul"),
        vec![vec![true, true], vec![true, false]]
    );
}

#[test]
fn on_demand_bitmap_fill_flags_pick_the_rounding_rule() {
    // 4x5p1r3's logical row 5 is covered ⅓ — the one row the rules split on.
    assert!(logical_fill("4x5p1r3:ceil")[5].iter().all(|f| *f));
    assert!(logical_fill("4x5p1r3:floor")[5].iter().all(|f| !*f));
    assert!(logical_fill("4x5p1r3:floor")[4].iter().all(|f| *f));
    assert_eq!(count_filled("4x5p1r3:zero"), 0);
    // :zero darkens even whole pixels, which is what separates it from :floor.
    assert_eq!(count_filled("3x5:zero"), 0);
    assert_eq!(count_filled("3x5:floor"), 15);
    // Triangles: ties go up by default, vanish under :floor.
    assert_eq!(count_filled("8x16-ul:ceil"), 72);
    assert_eq!(count_filled("8x16-ul:floor"), 56);
}

#[test]
fn on_demand_bitmap_fill_survives_rescale_to_any_parent_scale() {
    // The decision is only worth making per logical pixel if it reaches
    // the parent intact: `rescale` ORs the ink flags it merges, so a
    // per-subcell decision would silently come back out as `:ceil`.
    let Some(OnDemandGlyph::Shape(rect)) = parse_on_demand_glyph("4x5p1r3") else {
        panic!("must parse");
    };
    let grid = make_on_demand_grid(&rect);
    for parent_scale in [1u8, 2, 3, 4, 6] {
        let out = grid.rescale(3, parent_scale);
        let s = parent_scale.max(1) as u16;
        assert_eq!(out.height / s, 6, "scale {parent_scale}: logical height");
        for lr in 0..6u16 {
            for lc in 0..out.width / s {
                for dr in 0..s {
                    for dc in 0..s {
                        assert_eq!(
                            out.get(lr * s + dr, lc * s + dc).is_bitmap_filled(),
                            lr < 5,
                            "scale {parent_scale}: logical pixel ({lr},{lc}) subcell ({dr},{dc})"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn on_demand_bitmap_fill_leaves_geometry_alone() {
    // The flag is an ink-flag rule, so the vector build must not notice it.
    let base = make_on_demand_grid(&match parse_on_demand_glyph("9x6-ul").unwrap() {
        OnDemandGlyph::Shape(r) => r,
        _ => unreachable!(),
    });
    for flag in [":ceil", ":floor", ":zero"] {
        let name = format!("9x6-ul{flag}");
        let Some(OnDemandGlyph::Shape(rect)) = parse_on_demand_glyph(&name) else {
            panic!("{name} must parse");
        };
        let grid = make_on_demand_grid(&rect);
        assert_eq!(
            (grid.width, grid.height),
            (base.width, base.height),
            "{name}"
        );
        for r in 0..grid.height {
            for c in 0..grid.width {
                assert_eq!(
                    grid.get(r, c).shape_id(),
                    base.get(r, c).shape_id(),
                    "{name}: geometry changed at ({r},{c})"
                );
                assert_eq!(
                    grid.region_at(r, c),
                    base.region_at(r, c),
                    "{name}: detail region changed at ({r},{c})"
                );
            }
        }
    }
}

#[test]
fn parse_on_demand_bitmap_fill_suffix() {
    let fill_of = |n: &str| match parse_on_demand_glyph(n) {
        Some(OnDemandGlyph::Shape(r)) => Some(r.fill),
        _ => None,
    };
    assert_eq!(fill_of("3x5"), Some(BitmapFill::Round));
    assert_eq!(fill_of("3x5:ceil"), Some(BitmapFill::Ceil));
    assert_eq!(fill_of("1p2r3x4:floor"), Some(BitmapFill::Floor));
    // The rule suffix follows the corner suffix.
    match parse_on_demand_glyph("8x16-ul:zero") {
        Some(OnDemandGlyph::Shape(r)) => {
            assert_eq!(
                (r.shape, r.fill),
                (OnDemandShape::Tri(TriCorner::Ul), BitmapFill::Zero)
            );
        }
        other => panic!("8x16-ul:zero parsed as {other:?}"),
    }
    // Unknown or stacked suffixes are not on-demand names at all, which is
    // what keeps ordinary colon-bearing glyph names out of this path.
    assert_eq!(parse_on_demand_glyph("3x5:bogus"), None);
    assert_eq!(parse_on_demand_glyph("3x5:ceil:floor"), None);
    assert_eq!(parse_on_demand_glyph("3x5:mono"), None);
    assert_eq!(parse_on_demand_glyph("b-inner:compressed"), None);
}

fn simple_rect(w: u8, h: u8) -> OnDemandGlyph {
    OnDemandGlyph::Shape(OnDemandBox {
        w,
        h,
        w_frac: 0,
        h_frac: 0,
        scale: 1,
        align_w: BoxAlign::Near,
        align_h: BoxAlign::Near,
        shape: OnDemandShape::Rect,
        fill: BitmapFill::Round,
    })
}

fn frac_rect(w: u8, h: u8, wf: u8, hf: u8, s: u8, aw: BoxAlign, ah: BoxAlign) -> OnDemandGlyph {
    OnDemandGlyph::Shape(OnDemandBox {
        w,
        h,
        w_frac: wf,
        h_frac: hf,
        scale: s,
        align_w: aw,
        align_h: ah,
        shape: OnDemandShape::Rect,
        fill: BitmapFill::Round,
    })
}

#[test]
fn parse_on_demand_glyph_valid() {
    assert_eq!(parse_on_demand_glyph("3x5"), Some(simple_rect(3, 5)));
    assert_eq!(parse_on_demand_glyph("12x34"), Some(simple_rect(12, 34)));
    assert_eq!(parse_on_demand_glyph("1x1"), Some(simple_rect(1, 1)));
}

#[test]
fn parse_on_demand_triangle_names() {
    match parse_on_demand_glyph("4x8-ul") {
        Some(OnDemandGlyph::Shape(r)) => {
            assert_eq!(r.shape, OnDemandShape::Tri(TriCorner::Ul))
        }
        other => panic!("4x8-ul parsed as {other:?}"),
    }
    match parse_on_demand_glyph("1p2r3x4-dr") {
        Some(OnDemandGlyph::Shape(r)) => {
            assert_eq!(r.shape, OnDemandShape::Tri(TriCorner::Dr));
            assert_eq!((r.w, r.w_frac, r.scale), (1, 2, 3));
        }
        other => panic!("1p2r3x4-dr parsed as {other:?}"),
    }
    assert_eq!(parse_on_demand_glyph("4x-ul"), None);
    assert_eq!(parse_on_demand_glyph("x8-dl"), None);
}

#[test]
fn on_demand_triangle_catalog_slope_uses_plain_codes() {
    // 4x8-ul: the hypotenuse (from (4,0) to (0,8)) has the catalog 1:2
    // slope, so every pixel re-encodes as a plain shape code.
    let Some(OnDemandGlyph::Shape(rect)) = parse_on_demand_glyph("4x8-ul") else {
        panic!("4x8-ul must parse");
    };
    let grid = make_on_demand_grid(&rect);
    assert_eq!((grid.width, grid.height), (4, 8));
    assert!(
        grid.details.is_empty(),
        "1:2 slope must use plain slants, got details {:?}",
        grid.details
    );
    // The right angle corner is filled, the opposite corner empty.
    assert_eq!(grid.get(0, 0).shape_id(), crate::pixel::PX_ALMOSTFULL);
    assert!(grid.get(7, 3).is_clear());
    // Area check: sum of per-pixel region areas must equal W·H/2.
    let mut area2 = 0.0f64;
    for r in 0..8 {
        for c in 0..4 {
            area2 += grid.region_at(r, c).canonical().area2();
        }
    }
    assert!((area2 - 4.0 * 8.0).abs() < 1e-9, "area2 {area2}");
}

#[test]
fn on_demand_triangle_third_slope_traces_cleanly() {
    // 3x1-dr: slope 1:3 (the smooth-mosaic case) — needs custom
    // details, and the contour must come out as one clean triangle.
    let Some(OnDemandGlyph::Shape(rect)) = parse_on_demand_glyph("3x1-dr") else {
        panic!("3x1-dr must parse");
    };
    let grid = make_on_demand_grid(&rect);
    assert_eq!((grid.width, grid.height), (3, 1));
    assert!(
        !grid.details.is_empty(),
        "1:3 slope requires custom details"
    );

    let paths = crate::render::contour::track_contour(&grid, crate::pixel::PX_SUBPIXEL);
    assert_eq!(paths.len(), 1, "single outline, got {paths:?}");
    let mut pts = paths[0].clone();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let expected = [(0.0f32, 1.0f32), (3.0, 0.0), (3.0, 1.0)];
    assert_eq!(pts.len(), 3, "triangle has 3 vertices: {pts:?}");
    for (p, e) in pts.iter().zip(expected.iter()) {
        assert!(
            (p.0 - e.0).abs() < 1e-5 && (p.1 - e.1).abs() < 1e-5,
            "vertex {p:?} != {e:?} in {pts:?}"
        );
    }
}

#[test]
fn parse_on_demand_glyph_rejects_invalid() {
    assert_eq!(parse_on_demand_glyph("0x5"), None);
    assert_eq!(parse_on_demand_glyph("3x0"), None);
    assert_eq!(parse_on_demand_glyph("03x5"), None);
    assert_eq!(parse_on_demand_glyph("3x05"), None);
    assert_eq!(parse_on_demand_glyph("abc"), None);
    assert_eq!(parse_on_demand_glyph("3"), None);
    assert_eq!(parse_on_demand_glyph("x5"), None);
    assert_eq!(parse_on_demand_glyph("3x"), None);
}

#[test]
fn parse_on_demand_glyph_fractional() {
    assert_eq!(
        parse_on_demand_glyph("1p2r3x4p0r3"),
        Some(frac_rect(1, 4, 2, 0, 3, BoxAlign::Near, BoxAlign::Near)),
    );
    assert_eq!(
        parse_on_demand_glyph("1p2r3x4"),
        Some(frac_rect(1, 4, 2, 0, 3, BoxAlign::Near, BoxAlign::Near)),
    );
    assert_eq!(
        parse_on_demand_glyph("3x1p1r2"),
        Some(frac_rect(3, 1, 0, 1, 2, BoxAlign::Near, BoxAlign::Near)),
    );
    assert_eq!(
        parse_on_demand_glyph("-1p2r3x-4p1r3"),
        Some(frac_rect(1, 4, 2, 1, 3, BoxAlign::Far, BoxAlign::Far)),
    );
    assert_eq!(
        parse_on_demand_glyph("_1p2r3x-4p1r3"),
        Some(frac_rect(1, 4, 2, 1, 3, BoxAlign::Center, BoxAlign::Far)),
    );
    assert_eq!(
        parse_on_demand_glyph("1p2r3x_4p1r3"),
        Some(frac_rect(1, 4, 2, 1, 3, BoxAlign::Near, BoxAlign::Center)),
    );
    assert_eq!(
        parse_on_demand_glyph("0p1r3x1p0r3"),
        Some(frac_rect(0, 1, 1, 0, 3, BoxAlign::Near, BoxAlign::Near)),
    );
}

#[test]
fn parse_on_demand_glyph_fractional_rejects_invalid() {
    // R mismatch
    assert_eq!(parse_on_demand_glyph("1p1r2x1p1r3"), None);
    // R < 2
    assert_eq!(parse_on_demand_glyph("1p0r1x1p0r1"), None);
    // B >= R
    assert_eq!(parse_on_demand_glyph("1p3r3x1p0r3"), None);
    // D >= R
    assert_eq!(parse_on_demand_glyph("1p0r3x1p3r3"), None);
    // both zero: 0p0r3
    assert_eq!(parse_on_demand_glyph("0p0r3x1p0r3"), None);
    // neg without frac (simple format)
    assert_eq!(parse_on_demand_glyph("-3x5"), None);
    // ditto for the centering sign
    assert_eq!(parse_on_demand_glyph("_3x5"), None);
    assert_eq!(parse_on_demand_glyph("3x_5"), None);
}

#[test]
fn on_demand_centered_axis_splits_the_leftover() {
    // 1½ cells wide on a quarter lattice: the box is 6 subcells inside an
    // 8-subcell extent, so centering puts one leftover subcell on each side.
    // The ink flags are decided per *logical* pixel, so the empty subcells are
    // told apart by their geometry rather than by `is_empty`.
    let grid = make_on_demand_grid(&shape_of("_1p2r4x_1p2r4"));
    let clear = |r: u16, c: u16| grid.get(r, c).shape_id() == crate::pixel::PX_EMPTY;
    assert_eq!((grid.width, grid.height), (8, 8));
    for i in 0..8u16 {
        assert!(clear(0, i), "row 0 col {i} must be clear");
        assert!(clear(7, i), "row 7 col {i} must be clear");
        assert!(clear(i, 0), "col 0 row {i} must be clear");
        assert!(clear(i, 7), "col 7 row {i} must be clear");
    }
    for r in 1..7u16 {
        for c in 1..7u16 {
            assert_eq!(
                grid.get(r, c).shape_id(),
                crate::pixel::PX_ALMOSTFULL,
                "({r},{c}) must be inked"
            );
        }
    }
}

#[test]
fn on_demand_centered_axis_mixes_with_the_other_signs() {
    // The first inked subcell of each sign, on a 3-subcell leftover. `_` sits
    // between the two ends; the half-subcell it starts inside still counts.
    let off_of = |name: &str| {
        let grid = make_on_demand_grid(&shape_of(name));
        (0..grid.width)
            .position(|c| grid.get(0, c).shape_id() != crate::pixel::PX_EMPTY)
            .unwrap()
    };
    assert_eq!(off_of("3p1r4x1"), 0);
    assert_eq!(off_of("-3p1r4x1"), 3);
    assert_eq!(off_of("_3p1r4x1"), 1);
}

/// `_` must land exactly halfway between where no sign and `-` land, and an
/// odd leftover is no excuse: the box then starts on a half-subcell, which the
/// two boundary cells carry as sub-pixel geometry.
#[test]
fn on_demand_centered_axis_halves_an_odd_leftover() {
    let full = crate::detail::DetailRegion::full().canonical().area2();
    let row0 = |name: &str| {
        let grid = make_on_demand_grid(&shape_of(name));
        (0..grid.width)
            .map(|c| grid.region_at(0, c).canonical().area2() / full)
            .collect::<Vec<_>>()
    };
    // 3¼ × 1 on a quarter lattice: 13 subcells of ink in a 16-subcell extent,
    // so the 3-subcell leftover splits into 1½ subcells at each end.
    let mut near = vec![1.0; 16];
    near[13..].fill(0.0);
    let mut far = vec![1.0; 16];
    far[..3].fill(0.0);
    let mut center = vec![1.0; 16];
    center[0] = 0.0;
    center[1] = 0.5;
    center[14] = 0.5;
    center[15] = 0.0;
    assert_eq!(row0("3p1r4x1"), near);
    assert_eq!(row0("-3p1r4x1"), far);
    assert_eq!(row0("_3p1r4x1"), center);

    // A 1-subcell leftover is the case that used to collapse onto no sign.
    let grid_of = |name: &str| make_on_demand_grid(&shape_of(name));
    assert_ne!(grid_of("_2p2r3x_2p2r3"), grid_of("2p2r3x2p2r3"));
    assert_ne!(grid_of("_2p2r3x_2p2r3"), grid_of("-2p2r3x-2p2r3"));
}

// ---------------------------------------------------------------------------
// Circles and polygons
// ---------------------------------------------------------------------------

fn shape_of(name: &str) -> OnDemandBox {
    match parse_on_demand_glyph(name) {
        Some(OnDemandGlyph::Shape(spec)) => spec,
        other => panic!("{name} parsed as {other:?}"),
    }
}

fn poly_of(name: &str) -> PolySpec {
    match shape_of(name).shape {
        OnDemandShape::Poly(spec) => spec,
        other => panic!("{name} is not a polygon: {other:?}"),
    }
}

/// Twice the exact inked area of a synthesized grid, in *subcells* — which is
/// the same as pixels only when the name carries no fractional dimension.
fn area2_of(name: &str) -> f64 {
    let grid = make_on_demand_grid(&shape_of(name));
    let mut area2 = 0.0f64;
    for r in 0..grid.height {
        for c in 0..grid.width {
            area2 += grid.region_at(r, c).canonical().area2();
        }
    }
    area2
}

#[test]
fn parse_circle_and_poly_names() {
    assert_eq!(shape_of("8x8-circle").shape, OnDemandShape::Circle);
    assert_eq!(shape_of("2x1-circle").shape, OnDemandShape::Circle);
    assert_eq!(shape_of("-3p1r2x4-circle").fill, BitmapFill::Round);
    assert_eq!(shape_of("8x8-circle:floor").fill, BitmapFill::Floor);
    assert_eq!(
        poly_of("8x8-poly5"),
        PolySpec {
            n: 5,
            inset: PolyInset::None,
            rot_num: 0,
            rot_den: 1
        }
    );
    assert_eq!(poly_of("8x8-poly5.528").inset, PolyInset::Milli(528));
    assert_eq!(poly_of("8x8-poly5r2").inset, PolyInset::Star(2));
    // A one-digit fraction pads on the right; a leading zero does not.
    assert_eq!(poly_of("8x8-poly5.5").inset, PolyInset::Milli(500));
    assert_eq!(poly_of("8x8-poly5.05").inset, PolyInset::Milli(50));
    // N need not be coprime with K.
    assert_eq!(poly_of("8x8-poly6r2").inset, PolyInset::Star(2));
}

#[test]
fn poly_names_that_mean_the_same_shape_normalize_together() {
    // Every spelling of "no inset".
    let plain = poly_of("8x8-poly6");
    assert_eq!(poly_of("8x8-poly6.000"), plain);
    assert_eq!(poly_of("8x8-poly6.0"), plain);
    assert_eq!(poly_of("8x8-poly6r1"), plain);
    // A rotation by the full symmetry period, either way round, is none.
    assert_eq!(poly_of("8x8-poly6-cw0"), plain);
    assert_eq!(poly_of("8x8-poly6-cw60"), plain);
    assert_eq!(poly_of("8x8-poly6-ccw60"), plain);
    assert_eq!(poly_of("8x8-poly6-cw300"), plain);

    // Half a period is its own mirror, so cw and ccw agree there and nowhere
    // else: a square's period is 90°.
    assert_eq!(poly_of("8x8-poly4-cw45"), poly_of("8x8-poly4-ccw45"));
    assert_ne!(poly_of("8x8-poly4-cw30"), poly_of("8x8-poly4-ccw30"));
    assert_eq!(poly_of("8x8-poly4-cw30"), poly_of("8x8-poly4-ccw60"));

    // The folded angle of a 7-gon is no whole number of degrees, which is why
    // the rotation is kept as an exact fraction of a turn rather than degrees.
    let p = poly_of("8x8-poly7-cw100");
    assert_eq!((p.rot_num, p.rot_den), (17, 126));
    assert!((p.rot_num as f64 / p.rot_den as f64 * 360.0 - 48.571428).abs() < 1e-5);
    // …and the same shape reached the other way round normalizes to it.
    assert_eq!(poly_of("8x8-poly7-cw100.000"), p);

    // `rK` is an irrational inner radius, so it stays distinct from the
    // decimal that merely rounds to it.
    assert_ne!(poly_of("8x8-poly5r2"), poly_of("8x8-poly5.528"));
    let star = poly_of("8x8-poly5r2").inner_ratio();
    let milli = poly_of("8x8-poly5.528").inner_ratio();
    assert!((star - milli).abs() < 1e-3, "{star} vs {milli}");
}

#[test]
fn poly_inner_radius_matches_the_definition() {
    // .000 leaves the inner points on the edges of the regular N-gon, so
    // they are already nearer the center than the outer ones.
    for n in [3u8, 4, 5, 6, 8, 12] {
        let plain = poly_of(&format!("8x8-poly{n}"));
        assert!(
            (plain.inner_ratio() - (std::f64::consts::PI / n as f64).cos()).abs() < 1e-12,
            "poly{n}"
        );
    }
    // The pentagram's inner radius is 1/phi².
    let phi = (1.0 + 5f64.sqrt()) / 2.0;
    assert!((poly_of("8x8-poly5r2").inner_ratio() - 1.0 / (phi * phi)).abs() < 1e-12);
    // .999 nearly reaches the center; the ratio scales linearly in between.
    let half = poly_of("8x8-poly5.500").inner_ratio();
    assert!((half - poly_of("8x8-poly5").inner_ratio() * 0.5).abs() < 1e-12);
}

#[test]
fn parse_rejects_malformed_shape_suffixes() {
    // The grammar allows exactly one shape word, and the old corner suffixes
    // do not stack with the new ones.
    assert_eq!(parse_on_demand_glyph("8x8-circle-ul"), None);
    assert_eq!(parse_on_demand_glyph("8x8-ul-circle"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5-ul"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5-dr:ceil"), None);
    // A circle has no orientation to turn.
    assert_eq!(parse_on_demand_glyph("8x8-circle-cw45"), None);
    assert_eq!(parse_on_demand_glyph("8x8-circle-ccw45"), None);
    // Degenerate or out-of-range parameters.
    assert_eq!(parse_on_demand_glyph("8x8-poly2"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly0"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5r3"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5r0"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5.1234"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5."), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5-cw360"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5-cw"), None);
    assert_eq!(parse_on_demand_glyph("8x8-poly5-cw45.1234"), None);
    // Ordinary glyph names must not be mistaken for shapes.
    assert_eq!(parse_on_demand_glyph("8x8-circlet"), None);
    assert_eq!(parse_on_demand_glyph("a-circle"), None);
    assert_eq!(parse_on_demand_glyph("8x8-polygon"), None);
}

#[test]
fn curved_shapes_keep_the_declared_box_as_their_grid() {
    for (name, want) in [
        ("8x8-circle", (8u16, 8u16)),
        ("2x1-circle", (2, 1)),
        ("16x9-poly5", (16, 9)),
        ("1x1-circle", (1, 1)),
        // Fractional dimensions round the *grid* up but not the shape.
        ("5p1r2x3-circle", (12, 6)),
        ("-5p1r2x3-poly6", (12, 6)),
    ] {
        let grid = make_on_demand_grid(&shape_of(name));
        assert_eq!((grid.width, grid.height), want, "{name}");
    }
}

/// The area a shape covers is the one arithmetic fact a polyline
/// approximation must not get wrong; everything else about the outline is a
/// question of how fine the approximation is.
#[test]
fn curved_shapes_cover_the_area_their_definition_says() {
    let check = |name: &str, want: f64| {
        let got = area2_of(name) / 2.0;
        assert!(
            (got - want).abs() < want * 0.01,
            "{name}: area {got}, expected {want}"
        );
    };
    // A circle inscribed in the box.
    check("8x8-circle", std::f64::consts::PI * 16.0);
    check("16x16-circle", std::f64::consts::PI * 64.0);
    // …and the ellipse the affine transform makes of it.
    check("16x8-circle", std::f64::consts::PI * 8.0 * 4.0);
    check("2x1-circle", std::f64::consts::PI * 0.5);
    // A fractional box is an ellipse of the fractional size, not of the grid.
    check("5p1r2x8-circle", std::f64::consts::PI * 5.5 * 8.0);

    // A regular N-gon inscribed in a circle of radius r has area
    // (N/2)·r²·sin(2pi/N); the affine transform scales it by (H/W) for a
    // W-wide box, which for a square box is 1.
    for n in [3u8, 4, 5, 6, 8, 12] {
        let r = 8.0;
        let want = 0.5 * n as f64 * r * r * (std::f64::consts::TAU / n as f64).sin();
        check(&format!("16x16-poly{n}"), want);
        // Rotating cannot change the area.
        check(&format!("16x16-poly{n}-cw17"), want);
        // Stretching to half the height halves it.
        check(&format!("16x8-poly{n}"), want / 2.0);
    }

    // A star with inner radius rho has area N·r·rho·sin(pi/N).
    for (name, n, rho) in [
        (
            "16x16-poly5r2",
            5.0,
            1.0 / ((1.0 + 5f64.sqrt()) / 2.0).powi(2),
        ),
        (
            "16x16-poly6.500",
            6.0,
            (std::f64::consts::PI / 6.0).cos() * 0.5,
        ),
    ] {
        let r = 8.0;
        let want = n * r * (r * rho) * (std::f64::consts::PI / n).sin();
        check(name, want);
    }
}

#[test]
fn a_polygon_sits_where_its_default_angle_and_rotation_put_it() {
    // 8x8-poly4 puts a point at the top: a diamond with vertices at the edge
    // midpoints, so the box corners are bare and the center is inked.
    let diamond = make_on_demand_grid(&shape_of("8x8-poly4"));
    assert!(diamond.get(0, 0).is_clear(), "corner of the diamond");
    assert!(diamond.get(7, 7).is_clear(), "corner of the diamond");
    assert!(diamond.get(0, 3).is_bitmap_filled(), "the point at the top");
    assert!(diamond.get(4, 4).is_bitmap_filled(), "the middle");

    // Turned 45°, the same square becomes axis-aligned and inscribed in the
    // circle, so it reaches neither the box corners nor the edge midpoints.
    let square = make_on_demand_grid(&shape_of("8x8-poly4-cw45"));
    assert!(square.get(0, 0).is_clear(), "corner of the box");
    assert!(square.get(0, 3).is_clear(), "the top edge is clear now");
    assert!(square.get(2, 2).is_bitmap_filled(), "inside the square");
    assert!(square.get(4, 4).is_bitmap_filled(), "the middle");
}

#[test]
fn a_circle_reaches_every_edge_of_its_box_and_no_corner() {
    let grid = make_on_demand_grid(&shape_of("16x16-circle"));
    // Tangent to all four edges at the midpoints.
    for (r, c) in [(0u16, 8u16), (15, 8), (8, 0), (8, 15)] {
        assert!(
            !grid.get(r, c).is_clear(),
            "({r},{c}) should hold the curve"
        );
    }
    // The corners are outside.
    for (r, c) in [(0u16, 0u16), (0, 15), (15, 0), (15, 15)] {
        assert!(grid.get(r, c).is_clear(), "({r},{c}) should be bare");
    }
}

#[test]
fn a_negative_dimension_anchors_a_curved_shape_like_a_rectangle() {
    // 3p1r2 is 3½ wide in a 4-cell grid: the empty half-cell falls at the far
    // end by default and at the near end with the minus sign.
    let plain = make_on_demand_grid(&shape_of("3p1r2x4-circle"));
    let flipped = make_on_demand_grid(&shape_of("-3p1r2x4-circle"));
    assert_eq!((plain.width, plain.height), (8, 8));
    assert_eq!((flipped.width, flipped.height), (8, 8));
    // The last subcolumn is bare in one and the first in the other.
    assert!((0..8).all(|r| plain.get(r, 7).is_clear()));
    assert!((0..8).all(|r| flipped.get(r, 0).is_clear()));
    // One is the mirror image of the other, to within the lattice the vertices
    // snap to. Only this part needs `DetailRegion::mirror_h`, which belongs to
    // the editor's shape palette, so only this part is gated — the anchoring
    // the test is named for is core and stays in the headless build.
    #[cfg(feature = "editor")]
    for r in 0..8 {
        for c in 0..8 {
            let a = plain.region_at(r, c).mirror_h().canonical().area2();
            let b = flipped.region_at(r, 7 - c).canonical().area2();
            assert!((a - b).abs() < 0.1, "({r},{c}): {a} vs {b}");
        }
    }
}

#[test]
fn bitmap_fill_rules_reach_curved_shapes() {
    // The corner pixels of a circle's box are untouched under every rule, and
    // the middle is lit under every rule but :zero.
    for suffix in ["", ":ceil", ":floor", ":zero"] {
        let grid = make_on_demand_grid(&shape_of(&format!("8x8-circle{suffix}")));
        assert!(
            !grid.get(0, 0).is_bitmap_filled(),
            "corner under '{suffix}'"
        );
        assert_eq!(
            grid.get(4, 4).is_bitmap_filled(),
            suffix != ":zero",
            "middle under '{suffix}'"
        );
    }
    // The edge pixels a circle only grazes are what the rules disagree about.
    let lit = |name: &str| {
        let grid = make_on_demand_grid(&shape_of(name));
        (0..grid.height)
            .flat_map(|r| (0..grid.width).map(move |c| (r, c)))
            .filter(|&(r, c)| grid.get(r, c).is_bitmap_filled())
            .count()
    };
    assert!(lit("16x16-circle:ceil") > lit("16x16-circle"));
    assert!(lit("16x16-circle") > lit("16x16-circle:floor"));
    assert_eq!(lit("16x16-circle:zero"), 0);
    // …and none of it moves the outline.
    let base = make_on_demand_grid(&shape_of("16x16-circle"));
    for suffix in [":ceil", ":floor", ":zero"] {
        let grid = make_on_demand_grid(&shape_of(&format!("16x16-circle{suffix}")));
        for r in 0..grid.height {
            for c in 0..grid.width {
                assert_eq!(
                    grid.region_at(r, c),
                    base.region_at(r, c),
                    "'{suffix}' moved the geometry at ({r},{c})"
                );
            }
        }
    }
}

#[test]
fn a_degenerate_star_encloses_nothing() {
    // rK with 2K = N sends every inner point to the center, so the spikes
    // have no width at all.
    for name in ["16x16-poly4r2", "16x16-poly6r3", "16x16-poly8r4"] {
        assert_eq!(area2_of(name), 0.0, "{name}");
    }
}

#[test]
fn a_circle_traces_as_one_closed_outline() {
    let grid = make_on_demand_grid(&shape_of("16x16-circle"));
    let paths = crate::render::contour::track_contour(&grid, crate::pixel::PX_SUBPIXEL);
    assert_eq!(paths.len(), 1, "one outline, got {}", paths.len());
    // Every vertex sits on the ellipse, to within the lattice the shape is
    // cut on.
    for &(x, y) in &paths[0] {
        let (dx, dy) = (x as f64 - 8.0, y as f64 - 8.0);
        assert!(
            (dx * dx + dy * dy).sqrt() - 8.0 < 0.05,
            "({x},{y}) is off the circle"
        );
    }
}

/// Exercises the whole size range at every scale, which is where a lattice
/// this arithmetic runs on would overflow: a debug build panics on that, so
/// the assertions here are mostly an excuse to run the geometry.
#[test]
fn curved_shapes_stay_sane_across_sizes_and_scales() {
    for w in [1u8, 2, 3, 5, 9, 16, 24] {
        for h in [1u8, 2, 7, 16] {
            for shape in ["circle", "poly3", "poly5r2", "poly8.750-cw13"] {
                for dims in [format!("{w}x{h}"), format!("{w}p1r3x-{h}p2r3")] {
                    let name = format!("{dims}-{shape}");
                    let spec = shape_of(&name);
                    let grid = make_on_demand_grid(&spec);
                    let s = spec.scale as u16;
                    assert_eq!(grid.width % s, 0, "{name}");
                    assert_eq!(grid.height % s, 0, "{name}");
                    let area = area2_of(&name) / 2.0;
                    assert!(
                        area >= 0.0 && area <= (grid.width * grid.height) as f64,
                        "{name}: area {area} out of its box"
                    );
                }
            }
        }
    }
}

/// Neighbouring cells have to agree about the point where the outline crosses
/// the border between them, or the tracer stitches nothing and the glyph comes
/// out as a heap of fragments. That agreement is what [`REGION_DEN`] is chosen
/// for, so pin it across the shapes and sizes it has to hold for.
#[test]
fn a_curved_outline_stitches_across_cell_borders() {
    for name in [
        "16x16-circle",
        "9x16-circle",
        "24x24-circle",
        "3x3-circle",
        "16x16-poly3",
        "16x16-poly8",
        "16x16-poly6-cw13",
        "16x9-poly5",
        "5p1r2x8-circle",
    ] {
        let grid = make_on_demand_grid(&shape_of(name));
        let paths = crate::render::contour::track_contour(&grid, crate::pixel::PX_SUBPIXEL);
        assert_eq!(paths.len(), 1, "{name}: {} outlines", paths.len());
    }
    // A star is one outline too, however many points it has.
    for name in ["16x16-poly5r2", "16x16-poly6.500", "24x24-poly8.800"] {
        let grid = make_on_demand_grid(&shape_of(name));
        let paths = crate::render::contour::track_contour(&grid, crate::pixel::PX_SUBPIXEL);
        assert_eq!(paths.len(), 1, "{name}: {} outlines", paths.len());
    }
}

/// Boolean ops over the sub-pixel geometry a curve cuts must stay exact.
///
/// A curve cell sits on the `1/51` lattice of `REGION_DEN`, far finer than the
/// 2 or 4 of a catalog shape, and cutting two of them against each other is
/// what first pushed the sweep's arithmetic out of range: the intermediates
/// wrapped, `Frac`'s ordering stopped being an ordering, and the next sort
/// died with "comparison function does not correctly implement a total order".
/// The sweep is bounded by construction now (`detail::MAX_SWEEP_COORD`); this
/// is the case that has to keep passing for that to mean anything.
#[test]
fn curve_regions_survive_boolean_ops() {
    use crate::detail::{BoolOp, DetailRegion};

    let mut regions: Vec<DetailRegion> = Vec::new();
    for name in ["9x16-circle", "5p1r2x8-circle", "16x16-poly5r2"] {
        let grid = make_on_demand_grid(&shape_of(name));
        // Every fifth outline cell: enough shapes to cross-cut each other in
        // every direction without the pair loop below turning quadratic on a
        // whole outline.
        regions.extend(grid.details.values().step_by(5).cloned());
    }

    for a in &regions {
        for b in &regions {
            for op in [BoolOp::Union, BoolOp::Intersect, BoolOp::Subtract] {
                let out = crate::detail::bool_op(a, b, op);
                assert!(out.area_units_on(out.den) >= 0);
            }
        }
    }
}
