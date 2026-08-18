//! Tests for [`crate::pixel`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items.

use super::*;

#[test]
fn pixel_shape_roundtrip() {
    for raw in 0u16..256 {
        let shape = PixelShape(raw as u8);
        let [c1, c2] = shape_to_chars(shape);
        if c1 == '?' {
            continue;
        }
        let decoded = chars_to_shape(c1, c2).unwrap();
        assert_eq!(
            shape, decoded,
            "roundtrip failed for raw={raw}, chars={c1}{c2}"
        );
    }
}

#[test]
fn common_shapes() {
    assert_eq!(shape_to_chars(PixelShape::EMPTY), ['.', '.']);
    assert_eq!(
        shape_to_chars(PixelShape::new(PX_ALMOSTFULL, true)),
        ['@', '@']
    );
    assert_eq!(chars_to_shape('.', '.'), Some(PixelShape::EMPTY));
    assert_eq!(
        chars_to_shape('@', '@'),
        Some(PixelShape::new(PX_ALMOSTFULL, true))
    );
}

/// A hardblank is written `$$`, is occupied but not ink, and carries no
/// geometry at all — every table answers for it as it does for an empty
/// cell, without a case of its own.
#[test]
fn hardblank_is_a_blank_that_is_not_the_empty_cell() {
    let hb = PixelShape::new(PX_HARDBLANK, false);
    assert_eq!(shape_to_chars(hb), ['$', '$']);
    assert_eq!(chars_to_shape('$', '$'), Some(hb));

    assert!(hb.is_hardblank());
    assert!(hb.is_contour_empty());
    assert!(!hb.is_clear(), "a hardblank occupies its cell");
    assert!(!hb.is_bitmap_filled(), "a hardblank is not ink");
    assert_eq!(hb.catalog_shape_id(), PX_EMPTY);

    assert_eq!(adjacency(PX_HARDBLANK).0, 0);
    assert!(adjacency(PX_HARDBLANK).1.is_empty());
    assert_eq!(SHAPE_RASTERS[PX_HARDBLANK as usize], 0);
    let cov = edge_coverage(PX_HARDBLANK);
    for side in [cov.top, cov.right, cov.bottom, cov.left] {
        assert!(side.is_empty(), "a hardblank covers no cell edge");
    }
    assert!(crate::detail::DetailRegion::from_shape(PX_HARDBLANK).is_empty());
}

/// Every transform has to land back on a shape that can be written: a
/// hardblank has no complement id and no filled form to invert into.
///
/// Gated like the transforms themselves — the headless binary never turns a
/// shape over.
#[cfg(feature = "editor")]
#[test]
fn hardblank_transforms_to_itself() {
    let hb = PixelShape::new(PX_HARDBLANK, false);
    for got in [
        hb.mirror_h(),
        hb.flip_v(),
        hb.rotate_cw(),
        hb.rotate_ccw(),
        hb.rotate_180(),
        hb.with_fill_toggled(),
        hb.opposite(),
        hb.opposite_bitmap(),
    ] {
        assert_eq!(got, hb, "a hardblank transforms to itself");
    }
}

/// Combining is by geometry, and a hardblank has none: it yields to
/// anything drawn over it, and outlives only the empty cell.
#[test]
fn hardblank_combines_as_the_blank_it_is() {
    let hb = PixelShape::new(PX_HARDBLANK, false);
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    assert_eq!(shape_union(hb, full), full);
    assert_eq!(shape_union(full, hb), full);
    assert_eq!(shape_union(hb, PixelShape::EMPTY), hb);
    assert_eq!(shape_union(PixelShape::EMPTY, hb), hb);
    assert_eq!(shape_subtract(hb, full), hb);
    assert_eq!(shape_subtract(full, hb), full);
}

#[test]
fn adjacency_table_correct() {
    let (bits, _) = adjacency(PX_EMPTY);
    assert_eq!(bits, 0);

    let (bits, _) = adjacency(PX_ALMOSTFULL);
    assert_eq!(bits, 0xFF);

    let (bits, segs) = adjacency(PX_HALF1);
    assert_eq!(bits, 0b00001111);
    assert_eq!(segs.len(), 1);

    let (bits, _) = adjacency(PX_HALF2);
    assert_eq!(bits, 0b11110000);
}

#[test]
fn edge_coverage_slant1h() {
    // Slant1H: triangle (0,0)→(0.5,1)→(0,1) — covers left half of bottom
    let cov = edge_coverage(PX_SLANT1H);
    assert!(!cov.bottom.is_empty());
    assert!(
        (cov.bottom.start - 0.0).abs() < 0.01,
        "bottom.start={}",
        cov.bottom.start
    );
    assert!(
        (cov.bottom.end - 0.5).abs() < 0.01,
        "bottom.end={}",
        cov.bottom.end
    );
    // top: single point (0,0) — should be empty interval
    assert!(cov.top.is_empty(), "top should be empty: {:?}", cov.top);
}

#[test]
fn edge_coverage_halfslant1h() {
    // HalfSlant1H (complement of Slant2H): covers left half of top only
    let cov = edge_coverage(PX_HALFSLANT1H);
    assert!(!cov.top.is_empty());
    assert!(
        (cov.top.start - 0.0).abs() < 0.01,
        "top.start={}",
        cov.top.start
    );
    assert!((cov.top.end - 0.5).abs() < 0.01, "top.end={}", cov.top.end);
}

#[test]
fn edge_coverage_slant1h_above_halfslant1h() {
    // When Slant1H is above and HalfSlant1H is below:
    // overlap should be [0, 0.5] (left half only)
    let above_bottom = edge_coverage(PX_SLANT1H).bottom;
    let below_top = edge_coverage(PX_HALFSLANT1H).top;
    let overlap = above_bottom.intersect(below_top);
    assert!(!overlap.is_empty(), "should overlap");
    assert!((overlap.start - 0.0).abs() < 0.01);
    assert!(
        (overlap.end - 0.5).abs() < 0.01,
        "overlap.end={}",
        overlap.end
    );
}

#[test]
fn union_empty_identity() {
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    let slant = PixelShape(PX_SLANT1H);
    assert_eq!(shape_union(PixelShape::EMPTY, slant), slant);
    assert_eq!(shape_union(slant, PixelShape::EMPTY), slant);
    assert_eq!(shape_union(PixelShape::EMPTY, full), full);
}

#[test]
fn union_complement_gives_full() {
    // Two unfilled complements → unfilled almostfull
    let half1 = PixelShape(PX_HALF1);
    let half2 = PixelShape(PX_HALF2);
    assert_eq!(shape_union(half1, half2), PixelShape(PX_ALMOSTFULL));
    // One filled → result is filled
    let half1f = PixelShape::new(PX_HALF1, true);
    assert_eq!(
        shape_union(half1f, half2),
        PixelShape::new(PX_ALMOSTFULL, true),
    );
}

#[test]
fn union_slant_with_complement() {
    // SLANT1H + HALFSLANT2H (its complement) → full
    assert_eq!(
        shape_union(PixelShape(PX_SLANT1H), PixelShape(PX_HALFSLANT2H)),
        PixelShape(PX_ALMOSTFULL),
    );
}

#[test]
fn subtract_self_gives_empty() {
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    assert_eq!(shape_subtract(full, full), PixelShape::EMPTY);
    let half1 = PixelShape(PX_HALF1);
    assert_eq!(shape_subtract(half1, half1), PixelShape::EMPTY);
}

#[test]
fn subtract_from_full() {
    let full = PixelShape::new(PX_ALMOSTFULL, true);
    let half1 = PixelShape(PX_HALF1);
    // Subtracting unfilled half1 from filled full → filled half2
    assert_eq!(shape_subtract(full, half1), PixelShape::new(PX_HALF2, true),);
}

#[test]
fn edge_coverage_almostfull() {
    let cov = edge_coverage(PX_ALMOSTFULL);
    assert!((cov.top.start - 0.0).abs() < 0.01);
    assert!((cov.top.end - 1.0).abs() < 0.01);
    assert!((cov.bottom.start - 0.0).abs() < 0.01);
    assert!((cov.bottom.end - 1.0).abs() < 0.01);
    assert!((cov.left.start - 0.0).abs() < 0.01);
    assert!((cov.left.end - 1.0).abs() < 0.01);
    assert!((cov.right.start - 0.0).abs() < 0.01);
    assert!((cov.right.end - 1.0).abs() < 0.01);
}

#[test]
fn multi_shape_single_same_as_adjacency() {
    for &s in &valid_shape_ids() {
        let (bits, segs) = adjacency(s);
        let (mbits, msegs) = multi_shape_adjacency(&[s]);
        assert_eq!(bits, mbits, "bits mismatch for shape {s}");
        assert_eq!(segs.len(), msegs.len(), "segs len mismatch for shape {s}");
    }
}

#[test]
fn multi_shape_complements_fill_pixel() {
    // HALF1 + HALF2 = full pixel, no gap segs
    let (bits, segs) = multi_shape_adjacency(&[PX_HALF1, PX_HALF2]);
    assert_eq!(bits, 0xFF);
    assert!(segs.is_empty());
}

#[test]
fn multi_shape_slant_union_gap_segments() {
    // SLANT1H (bottom-left triangle) + SLANT3H (upper-left triangle)
    // Union covers: a, h, g, f edges; gap goes via (0.25,0.5)
    let (bits, segs) = multi_shape_adjacency(&[PX_SLANT1H, PX_SLANT3H]);
    assert_eq!(bits, adjacency(PX_SLANT1H).0 | adjacency(PX_SLANT3H).0,);
    // Should have 2 gap segments meeting at the intersection point
    assert_eq!(
        segs.len(),
        2,
        "expected 2 clipped gap segments, got {}",
        segs.len()
    );
    // Both segments should share the intersection point (0.25, 0.5)
    let has_intersection = segs
        .iter()
        .any(|&(x1, y1, _, _)| (x1 - 0.25).abs() < 0.01 && (y1 - 0.5).abs() < 0.01)
        || segs
            .iter()
            .any(|&(_, _, x2, y2)| (x2 - 0.25).abs() < 0.01 && (y2 - 0.5).abs() < 0.01);
    assert!(
        has_intersection,
        "gap segs should meet at (0.25, 0.5): {segs:?}"
    );
}

#[test]
fn cone_adjacency() {
    let (bits, segs) = adjacency(PX_CONE1);
    assert_eq!(bits, 0b00000011, "CONE1 bits");
    assert_eq!(segs.len(), 2, "CONE1 segs");

    let (bits, segs) = adjacency(PX_INVCONE1);
    assert_eq!(bits, 0b11111100, "INVCONE1 bits");
    assert_eq!(segs.len(), 2, "INVCONE1 segs");

    let poly = polygon_from_adjacency(bits, segs);
    assert!(
        poly.len() >= 5,
        "INVCONE1 polygon should have >= 5 vertices, got {}",
        poly.len()
    );
}

#[test]
fn dot_polygon_is_the_edge_midpoint_diamond() {
    // The editor draws PX_DOT through the generic polygon path, so this
    // must match the outline the font builder emits (`detail.rs`).
    let (bits, segs) = adjacency(PX_DOT);
    let poly = polygon_from_adjacency(bits, segs);
    assert_eq!(poly.len(), 4, "DOT polygon vertices: {poly:?}");
    for v in [(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)] {
        assert!(
            poly.iter().any(|&p| near_f(p, v)),
            "DOT polygon missing {v:?}: {poly:?}"
        );
    }
}

#[test]
fn cone_complement_union_gives_full() {
    assert_eq!(
        shape_union(
            PixelShape::new(PX_CONE1, false),
            PixelShape::new(PX_INVCONE1, false),
        ),
        PixelShape::new(PX_ALMOSTFULL, false),
    );
}

#[test]
fn invcone3_polygon_has_both_triangles() {
    let (bits, segs) = adjacency(PX_INVCONE3);
    let poly = polygon_from_adjacency(bits, segs);
    assert!(
        poly.len() >= 7,
        "INVCONE3 should have >= 7 vertices, got {}",
        poly.len()
    );
    let has_top_right = poly
        .iter()
        .any(|&(x, y)| (x - 1.0).abs() < 0.01 && y.abs() < 0.01);
    let has_bottom_right = poly
        .iter()
        .any(|&(x, y)| (x - 1.0).abs() < 0.01 && (y - 1.0).abs() < 0.01);
    assert!(has_top_right, "missing top-right corner (1,0)");
    assert!(has_bottom_right, "missing bottom-right corner (1,1)");
}

// -----------------------------------------------------------------------
// Verification: recompute all precomputed tables from geometry and compare
// -----------------------------------------------------------------------

fn valid_shape_ids() -> Vec<u8> {
    let mut ids: Vec<u8> = Vec::new();
    for &(shape, _, _) in ADJACENCY_MAP {
        ids.push(shape);
        let complement = shape ^ PX_SUBPIXEL;
        if complement != shape && !ids.contains(&complement) {
            ids.push(complement);
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn rasterize_polygon(polygon: &[(f32, f32)]) -> u128 {
    if polygon.len() < 3 {
        return 0;
    }
    let mut bits = 0u128;
    let n = polygon.len();
    for r in 0..RASTER_N {
        for c in 0..RASTER_N {
            let px = (c as f32 + 0.5) / RASTER_N as f32;
            let py = (r as f32 + 0.3) / RASTER_N as f32;
            let mut inside = false;
            let mut j = n - 1;
            for i in 0..n {
                let (xi, yi) = polygon[i];
                let (xj, yj) = polygon[j];
                if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
                    inside = !inside;
                }
                j = i;
            }
            if inside {
                bits |= 1u128 << (r * RASTER_N + c);
            }
        }
    }
    bits
}

/// Derive edge coverage from the adjacency half-edge bits, which are
/// the tracer's ground truth. Every catalog shape covers each square
/// edge in a single run whose endpoints are on the half lattice, so the
/// two bits per edge determine the interval exactly. (The former
/// polygon-based derivation inherited broken outlines for multi-part
/// shapes like HQUAD, whose chained "single polygon" is not a faithful
/// boundary.)
fn compute_edge_coverage(shape_id: u8) -> ShapeEdgeCoverage {
    let bits = ADJACENCY_BITS[shape_id.min(128) as usize];
    let iv = |first: bool, second: bool| -> EdgeInterval {
        match (first, second) {
            (false, false) => EdgeInterval::EMPTY,
            (true, false) => EdgeInterval {
                start: 0.0,
                end: 0.5,
            },
            (false, true) => EdgeInterval {
                start: 0.5,
                end: 1.0,
            },
            (true, true) => EdgeInterval {
                start: 0.0,
                end: 1.0,
            },
        }
    };
    //    a   b
    //   +--+--+
    // h |     | c
    //   +     +
    // g |     | d
    //   +--+--+
    //    f   e
    ShapeEdgeCoverage {
        top: iv(bits & 0x80 != 0, bits & 0x40 != 0),
        right: iv(bits & 0x20 != 0, bits & 0x10 != 0),
        bottom: iv(bits & 0x04 != 0, bits & 0x08 != 0),
        left: iv(bits & 0x01 != 0, bits & 0x02 != 0),
    }
}

#[test]
fn verify_adjacency_bits() {
    for &(shape, bits, _) in ADJACENCY_MAP {
        assert_eq!(
            ADJACENCY_BITS[shape as usize], bits,
            "ADJACENCY_BITS mismatch for base shape {shape}"
        );
        let compl = shape ^ PX_SUBPIXEL;
        assert_eq!(
            ADJACENCY_BITS[compl as usize],
            bits ^ 0xFF,
            "ADJACENCY_BITS mismatch for complement shape {compl}"
        );
    }
    assert_eq!(ADJACENCY_BITS[128], ADJACENCY_BITS[PX_ALMOSTFULL as usize]);
}

#[test]
fn verify_adjacency_segs() {
    for &(shape, _, expected_segs) in ADJACENCY_MAP {
        let (_, segs) = adjacency(shape);
        assert_eq!(
            segs, expected_segs,
            "adjacency segs mismatch for base shape {shape}"
        );
        let compl = shape ^ PX_SUBPIXEL;
        let (_, csegs) = adjacency(compl);
        assert_eq!(
            csegs, expected_segs,
            "adjacency segs mismatch for complement shape {compl}"
        );
    }
}

#[test]
fn verify_edge_coverage() {
    for i in 0u8..128 {
        let expected = compute_edge_coverage(i);
        let actual = &EDGE_COVERAGE_TABLE[i as usize];
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(
            close(actual.top.start, expected.top.start)
                && close(actual.top.end, expected.top.end)
                && close(actual.bottom.start, expected.bottom.start)
                && close(actual.bottom.end, expected.bottom.end)
                && close(actual.left.start, expected.left.start)
                && close(actual.left.end, expected.left.end)
                && close(actual.right.start, expected.right.start)
                && close(actual.right.end, expected.right.end),
            "EDGE_COVERAGE mismatch for shape {i}: \
                expected ({},{},{},{},{},{},{},{}) got ({},{},{},{},{},{},{},{})",
            expected.top.start,
            expected.top.end,
            expected.bottom.start,
            expected.bottom.end,
            expected.left.start,
            expected.left.end,
            expected.right.start,
            expected.right.end,
            actual.top.start,
            actual.top.end,
            actual.bottom.start,
            actual.bottom.end,
            actual.left.start,
            actual.left.end,
            actual.right.start,
            actual.right.end,
        );
    }
}

/// The raster a shape's geometry demands. Complements are the bitwise
/// complement of their base rather than a rasterized outline, because
/// [`polygon_from_adjacency`] can only chain a *connected* boundary: the
/// two-corner complements (INVSLASH and friends) would lose a triangle.
/// PX_VQUAD and the inverse dot are the two whose stored raster predates
/// that rule: their parts meet only at points, and each holds the
/// (unfaithful) outline the chainer produced — as PX_HQUAD does.
fn expected_raster(s: u8) -> u128 {
    if s >= 97 && s != PX_VQUAD && s != PX_DOT ^ PX_SUBPIXEL {
        FULL_RASTER ^ rasterize_polygon(&build_unit_polygon(s ^ PX_SUBPIXEL))
    } else {
        rasterize_polygon(&build_unit_polygon(s))
    }
}

#[test]
fn verify_rasters() {
    let valid = valid_shape_ids();
    for &s in &valid {
        assert_eq!(
            SHAPE_RASTERS[s as usize],
            expected_raster(s),
            "SHAPE_RASTERS mismatch for shape {s}"
        );
    }
}

#[test]
fn verify_union_exhaustive() {
    let valid = valid_shape_ids();
    let mut computed_rasters = [0u128; 128];
    for &s in &valid {
        computed_rasters[s as usize] = expected_raster(s);
    }
    let mut raster_to_id = std::collections::HashMap::new();
    for &s in &valid {
        raster_to_id
            .entry(computed_rasters[s as usize])
            .or_insert(s);
    }
    raster_to_id.insert(0, PX_EMPTY);
    raster_to_id.insert(FULL_RASTER, PX_ALMOSTFULL);

    for &a in &valid {
        for &b in &valid {
            let ur = computed_rasters[a as usize] | computed_rasters[b as usize];
            let expected = raster_to_id.get(&ur).copied().unwrap_or(PX_DOT);
            let sa = PixelShape(a);
            let sb = PixelShape(b);
            if sa.is_clear() {
                assert_eq!(shape_union(sa, sb), sb, "union({a},{b}) identity");
            } else if sb.is_clear() {
                assert_eq!(shape_union(sa, sb), sa, "union({a},{b}) identity");
            } else {
                assert_eq!(
                    shape_union(sa, sb).shape_id(),
                    expected,
                    "union({a},{b}) mismatch"
                );
            }
        }
    }
}

#[test]
fn verify_subtract_exhaustive() {
    let valid = valid_shape_ids();
    let mut computed_rasters = [0u128; 128];
    for &s in &valid {
        computed_rasters[s as usize] = expected_raster(s);
    }
    let mut raster_to_id = std::collections::HashMap::new();
    for &s in &valid {
        raster_to_id
            .entry(computed_rasters[s as usize])
            .or_insert(s);
    }
    raster_to_id.insert(0, PX_EMPTY);
    raster_to_id.insert(FULL_RASTER, PX_ALMOSTFULL);

    for &a in &valid {
        for &b in &valid {
            let sr = computed_rasters[a as usize] & (!computed_rasters[b as usize] & FULL_RASTER);
            let expected = raster_to_id.get(&sr).copied().unwrap_or(PX_DOT);
            let sa = PixelShape(a);
            let sb = PixelShape(b);
            if sa.is_clear() || sb.is_clear() {
                continue; // early-return paths tested separately
            }
            let result = shape_subtract(sa, sb);
            let result_id = if result.is_clear() {
                PX_EMPTY
            } else {
                result.shape_id()
            };
            assert_eq!(
                result_id, expected,
                "subtract({a},{b}) mismatch: got {result_id}, expected {expected}"
            );
        }
    }
}

#[test]
fn transform_tables_consistent_with_adjacency() {
    // Verify transforms using adjacency bits (8 half-edges around the cell).
    // Mirror H swaps: a↔b, c↔h, d↔g, e↔f
    // Flip V: new = (f,e,d,c,b,a,h,g) from original (a,b,c,d,e,f,g,h)
    // Rotate CW: new = (g,h,a,b,c,d,e,f) (shift right by 2)
    fn adj(id: u8) -> u8 {
        ADJACENCY_BITS[id.min(128) as usize]
    }
    fn mirror_adj(bits: u8) -> u8 {
        let a = (bits >> 7) & 1;
        let b = (bits >> 6) & 1;
        let c = (bits >> 5) & 1;
        let d = (bits >> 4) & 1;
        let e = (bits >> 3) & 1;
        let f = (bits >> 2) & 1;
        let g = (bits >> 1) & 1;
        let h = bits & 1;
        (b << 7) | (a << 6) | (h << 5) | (g << 4) | (f << 3) | (e << 2) | (d << 1) | c
    }
    fn flip_adj(bits: u8) -> u8 {
        let a = (bits >> 7) & 1;
        let b = (bits >> 6) & 1;
        let c = (bits >> 5) & 1;
        let d = (bits >> 4) & 1;
        let e = (bits >> 3) & 1;
        let f = (bits >> 2) & 1;
        let g = (bits >> 1) & 1;
        let h = bits & 1;
        (f << 7) | (e << 6) | (d << 5) | (c << 4) | (b << 3) | (a << 2) | (h << 1) | g
    }
    fn rotate_cw_adj(bits: u8) -> u8 {
        let a = (bits >> 7) & 1;
        let b = (bits >> 6) & 1;
        let c = (bits >> 5) & 1;
        let d = (bits >> 4) & 1;
        let e = (bits >> 3) & 1;
        let f = (bits >> 2) & 1;
        let g = (bits >> 1) & 1;
        let h = bits & 1;
        (g << 7) | (h << 6) | (a << 5) | (b << 4) | (c << 3) | (d << 2) | (e << 1) | f
    }

    // Every catalog id except PX_CUSTOM (31) and its unused complement (96).
    let used_ids: Vec<u8> = (0..31).chain(97..128).collect();
    for &id in &used_ids {
        let bits = adj(id);
        let shape = PixelShape(id);

        let m_id = shape.mirror_h().shape_id();
        assert_eq!(
            adj(m_id),
            mirror_adj(bits),
            "mirror_h adjacency mismatch for id={id}: got id={m_id} adj={:#010b}, expected adj={:#010b}",
            adj(m_id),
            mirror_adj(bits)
        );

        let f_id = shape.flip_v().shape_id();
        assert_eq!(
            adj(f_id),
            flip_adj(bits),
            "flip_v adjacency mismatch for id={id}: got id={f_id} adj={:#010b}, expected adj={:#010b}",
            adj(f_id),
            flip_adj(bits)
        );

        let r_id = shape.rotate_cw().shape_id();
        assert_eq!(
            adj(r_id),
            rotate_cw_adj(bits),
            "rotate_cw adjacency mismatch for id={id}: got id={r_id} adj={:#010b}, expected adj={:#010b}",
            adj(r_id),
            rotate_cw_adj(bits)
        );
    }
}

#[test]
fn transform_inverse_properties() {
    // Every catalog id except PX_CUSTOM (31) and its unused complement (96).
    let used_ids: Vec<u8> = (0..31).chain(97..128).collect();
    for &id in &used_ids {
        let shape = PixelShape(id | PX_FULL);

        // mirror_h is self-inverse
        assert_eq!(
            shape.mirror_h().mirror_h(),
            shape,
            "mirror_h not involutory for id={id}"
        );

        // flip_v is self-inverse
        assert_eq!(
            shape.flip_v().flip_v(),
            shape,
            "flip_v not involutory for id={id}"
        );

        // rotate_180 is self-inverse
        assert_eq!(
            shape.rotate_180().rotate_180(),
            shape,
            "rotate_180 not involutory for id={id}"
        );

        // rotate_cw and rotate_ccw are inverses
        assert_eq!(
            shape.rotate_cw().rotate_ccw(),
            shape,
            "cw/ccw not inverse for id={id}"
        );
        assert_eq!(
            shape.rotate_ccw().rotate_cw(),
            shape,
            "ccw/cw not inverse for id={id}"
        );

        // 4x rotate_cw = identity
        assert_eq!(
            shape.rotate_cw().rotate_cw().rotate_cw().rotate_cw(),
            shape,
            "4x cw not identity for id={id}"
        );

        // rotate_180 = rotate_cw twice
        assert_eq!(
            shape.rotate_cw().rotate_cw(),
            shape.rotate_180(),
            "2x cw != 180 for id={id}"
        );
    }
}

/// Two corners make the complement, and the dot on top of it makes the
/// shape: both unions have to land on the catalog id rather than fall
/// back to PX_DOT. (A *single* corner over the dot still has no catalog
/// id, so it does not survive a union — build them the other way round.)
#[test]
fn dot_plus_two_corners_unions_to_the_new_shape() {
    let cases = [
        (PX_SLASH, (PX_CORNER1, PX_CORNER2), (PX_CORNER3, PX_CORNER4)),
        (
            PX_BACKSLASH,
            (PX_CORNER3, PX_CORNER4),
            (PX_CORNER1, PX_CORNER2),
        ),
        (
            PX_HOUSE1,
            (PX_CORNER3, PX_CORNER1),
            (PX_CORNER2, PX_CORNER4),
        ),
        (
            PX_HOUSE2,
            (PX_CORNER3, PX_CORNER2),
            (PX_CORNER1, PX_CORNER4),
        ),
        (
            PX_HOUSE3,
            (PX_CORNER2, PX_CORNER4),
            (PX_CORNER3, PX_CORNER1),
        ),
        (
            PX_HOUSE4,
            (PX_CORNER1, PX_CORNER4),
            (PX_CORNER3, PX_CORNER2),
        ),
    ];
    let lit = |s| PixelShape::new(s, true);
    for (id, (a, b), (c, d)) in cases {
        let pair = shape_union(lit(a), lit(b));
        let grown = shape_union(lit(PX_DOT), pair);
        assert_eq!(grown, lit(id), "DOT+{a}+{b}");
        // The two corners left over are the complement, and putting them
        // back fills the cell.
        let rest = shape_union(lit(c), lit(d));
        assert_eq!(rest.shape_id(), id ^ PX_SUBPIXEL, "complement of {id}");
        assert_eq!(shape_union(grown, rest), lit(PX_ALMOSTFULL));
        assert_eq!(shape_subtract(lit(PX_ALMOSTFULL), rest), grown);
    }
}

#[test]
fn multi_shape_adjacency_hquad_dot() {
    let (bits, segs) = multi_shape_adjacency(&[PX_HQUAD, PX_DOT]);
    assert_eq!(bits, 0b00110011);
    // Gap segments + boundary edges must form closed contours.
    // Collect all edges (gap + boundary) and verify even degree.
    let mut all_segs = segs.clone();
    let boundary: [(u8, [f32; 4]); 8] = [
        (7, [0.0, 0.0, 0.5, 0.0]),
        (6, [0.5, 0.0, 1.0, 0.0]),
        (5, [1.0, 0.0, 1.0, 0.5]),
        (4, [1.0, 0.5, 1.0, 1.0]),
        (3, [1.0, 1.0, 0.5, 1.0]),
        (2, [0.5, 1.0, 0.0, 1.0]),
        (1, [0.0, 1.0, 0.0, 0.5]),
        (0, [0.0, 0.5, 0.0, 0.0]),
    ];
    for &(bit, seg) in &boundary {
        if bits & (1 << bit) != 0 {
            all_segs.push((seg[0], seg[1], seg[2], seg[3]));
        }
    }
    let mut degree: std::collections::HashMap<(i32, i32), u32> = std::collections::HashMap::new();
    let quantize = |v: f32| (v * 1200.0).round() as i32;
    for &(x1, y1, x2, y2) in &all_segs {
        *degree.entry((quantize(x1), quantize(y1))).or_default() += 1;
        *degree.entry((quantize(x2), quantize(y2))).or_default() += 1;
    }
    for (&k, &d) in &degree {
        assert!(d % 2 == 0, "odd degree {d} at ({}, {})", k.0, k.1);
    }
}
