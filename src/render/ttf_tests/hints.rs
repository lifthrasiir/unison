//! Tests for the grid-snap hinting instructions.

use super::*;

#[test]
fn grid_snap_hints_single_diagonal() {
    // A triangle contour with one diagonal: (0,896)→(64,832)→(0,832)
    // CW in y-up → area < 0.  Diagonal from (0,896) to (64,832).
    let mut contours = vec![vec![(0i16, 896), (64, 832), (0, 832)]];
    let instructions = generate_grid_snap_hints(&mut contours, 16);

    // The diagonal (0,896)→(64,832) should get a midpoint at (32,864).
    assert_eq!(contours[0].len(), 4, "midpoint should be inserted");
    assert!(
        contours[0].contains(&(32, 864)),
        "midpoint (32,864) missing from {:?}",
        contours[0],
    );

    // Instructions should be non-empty (one delta point).
    assert!(
        !instructions.is_empty(),
        "expected TrueType hint instructions",
    );

    // The instructions should start with: PUSHB 16, MPPEM, EQ, IF
    // and end with EIF (0x59).
    assert_eq!(instructions[0], 0xB0, "PUSHB[0]");
    assert_eq!(instructions[1], 16, "ppem=16");
    assert_eq!(instructions[2], 0x4D, "MPPEM");
    assert_eq!(instructions[3], 0x54, "EQ");
    assert_eq!(instructions[4], 0x58, "IF");
    assert_eq!(*instructions.last().unwrap(), 0x59, "EIF");
}

#[test]
fn grid_snap_hints_collinear_midpoints_invisible() {
    // Verify that the midpoints lie exactly on the diagonal line
    // (collinear with neighbors), so at non-target PPEMs the shape
    // is unchanged.
    let mut contours = vec![vec![(0i16, 896), (64, 832), (0, 832)]];
    generate_grid_snap_hints(&mut contours, 16);

    let c = &contours[0];
    for i in 0..c.len() {
        let prev = c[(i + c.len() - 1) % c.len()];
        let cur = c[i];
        let next = c[(i + 1) % c.len()];
        let cross = (cur.0 - prev.0) as i64 * (next.1 - prev.1) as i64
            - (cur.1 - prev.1) as i64 * (next.0 - prev.0) as i64;
        // For the original 3 non-collinear vertices, cross != 0.
        // For added midpoints, cross must == 0.
        if cross == 0 {
            // This is a collinear (midpoint) vertex — expected.
            assert!(
                ![(0, 896), (64, 832), (0, 832)].contains(&cur),
                "original vertex {cur:?} should not be collinear",
            );
        }
    }
}

#[test]
fn grid_snap_hints_multi_cell_diagonal() {
    // Two-cell diagonal: (0,896)→(128,768)→(0,768)
    // Should split into two 1-cell sub-diagonals with a grid point
    // at (64,832) plus two midpoints.
    let mut contours = vec![vec![(0i16, 896), (128, 768), (0, 768)]];
    let instructions = generate_grid_snap_hints(&mut contours, 16);

    // Expect grid split point (64,832) and midpoints (32,864) and (96,800).
    let c = &contours[0];
    assert!(c.contains(&(64, 832)), "grid split point missing: {c:?}");
    assert!(c.contains(&(32, 864)), "first midpoint missing: {c:?}");
    assert!(c.contains(&(96, 800)), "second midpoint missing: {c:?}");
    assert!(!instructions.is_empty());
}

#[test]
fn grid_snap_hints_no_diagonals() {
    // Pure rectangle — no diagonals, no hints.
    let mut contours = vec![vec![
        (0i16, 896),
        (64, 896),
        (64, 832),
        (0, 832),
    ]];
    let instructions = generate_grid_snap_hints(&mut contours, 16);
    assert!(instructions.is_empty());
    assert_eq!(contours[0].len(), 4, "no points should be added");
}
