//! TrueType hinting: grid-snap hints that make diagonals rasterize as clean
//! staircases at the bitmap PPEM.

use super::contours::{contour_signed_area, gcd};
use super::*;

/// Add midpoints on diagonal contour edges and generate TrueType instructions
/// that snap those midpoints to the pixel grid at `hint_ppem`.
///
/// At other PPEMs the midpoints are collinear with their neighbors (invisible).
/// At `hint_ppem` the SHPIX deltas fire, turning each diagonal into an
/// axis-aligned staircase so the glyph rasterizes as a clean bitmap.
pub(super) fn generate_grid_snap_hints(
    contours: &mut [Vec<(i16, i16)>],
    hint_ppem: u16,
) -> Vec<u8> {
    let scale = UNITS_PER_EM as i32 / hint_ppem as i32;
    let half_scale = scale / 2;
    if half_scale == 0 {
        return Vec::new();
    }

    // (point_index, dx, dy) — dx/dy are in font units, which at this specific
    // PPEM happen to equal F26Dot6 values (1024/16 * 16/1024 * 64 = 64 → 1:1).
    let mut deltas: Vec<(u16, i16, i16)> = Vec::new();
    let mut point_offset = 0u16;

    for contour in contours.iter_mut() {
        let area = contour_signed_area(contour);
        let old = std::mem::take(contour);
        let n = old.len();

        for i in 0..n {
            let (ax, ay) = (old[i].0 as i32, old[i].1 as i32);
            let (bx, by) = (old[(i + 1) % n].0 as i32, old[(i + 1) % n].1 as i32);
            contour.push(old[i]);

            let dx = bx - ax;
            let dy = by - ay;
            if dx == 0 || dy == 0 {
                continue;
            }

            // Only edges on the half-scale lattice split exactly; anything
            // else (custom sub-pixel detail coordinates) would get truncated
            // split points and midpoints *off* the segment, bending the
            // outline at every PPEM. Leave such edges alone — the hints are
            // for pixel-art diagonals.
            if ax % half_scale != 0
                || ay % half_scale != 0
                || dx % half_scale != 0
                || dy % half_scale != 0
            {
                continue;
            }

            // Work in half-scale units to find grid-aligned intermediate points.
            // Grid points sit at multiples of `scale` (= even half-scale units).
            let h_dx = dx / half_scale;
            let h_dy = dy / half_scale;
            if h_dx == 0 || h_dy == 0 {
                continue;
            }
            let g = gcd(h_dx.abs(), h_dy.abs());
            let d1 = h_dx / g;
            let d2 = h_dy / g;

            // Collect segment start points (grid-aligned splits of the diagonal)
            let mut seg_starts: Vec<(i32, i32)> = vec![(ax, ay)];
            for k in 1..g {
                let hx = ax / half_scale + k * d1;
                let hy = ay / half_scale + k * d2;
                if hx % 2 == 0 && hy % 2 == 0 {
                    seg_starts.push((hx * half_scale, hy * half_scale));
                }
            }

            let seg_count = seg_starts.len();
            for si in 0..seg_count {
                let seg_a = seg_starts[si];
                let seg_b = if si + 1 < seg_count {
                    seg_starts[si + 1]
                } else {
                    (bx, by)
                };

                let sdx = seg_b.0 - seg_a.0;
                let sdy = seg_b.1 - seg_a.1;

                let mx = (seg_a.0 + seg_b.0) / 2;
                let my = (seg_a.1 + seg_b.1) / 2;

                if (mx == seg_a.0 && my == seg_a.1) || (mx == seg_b.0 && my == seg_b.1) {
                    if si + 1 < seg_count {
                        contour.push((seg_starts[si + 1].0 as i16, seg_starts[si + 1].1 as i16));
                    }
                    continue;
                }

                // Snap direction: C1 = (seg_a.x, seg_b.y), C2 = (seg_b.x, seg_a.y).
                // CW outer contour (area < 0): filled region is to the RIGHT.
                // use C1 when dxdy and area have different signs.
                let dxdy = sdx as i64 * sdy as i64;
                let use_c1 = (dxdy < 0) != (area < 0);
                let (tx, ty) = if use_c1 {
                    (seg_a.0, seg_b.1)
                } else {
                    (seg_b.0, seg_a.1)
                };

                let delta_x = (tx - mx) as i16;
                let delta_y = (ty - my) as i16;

                let point_idx = point_offset + contour.len() as u16;
                contour.push((mx as i16, my as i16));

                if delta_x != 0 || delta_y != 0 {
                    deltas.push((point_idx, delta_x, delta_y));
                }

                if si + 1 < seg_count {
                    contour.push((seg_starts[si + 1].0 as i16, seg_starts[si + 1].1 as i16));
                }
            }
        }

        point_offset += contour.len() as u16;
    }

    if deltas.is_empty() {
        return Vec::new();
    }

    encode_grid_snap_instructions(&deltas, hint_ppem)
}

fn encode_grid_snap_instructions(deltas: &[(u16, i16, i16)], hint_ppem: u16) -> Vec<u8> {
    let mut code = Vec::new();

    // PUSHB hint_ppem; MPPEM; EQ; IF
    tt_push(&mut code, hint_ppem as i32);
    code.push(0x4D); // MPPEM
    code.push(0x54); // EQ
    code.push(0x58); // IF

    // X-axis deltas
    let x_deltas: Vec<_> = deltas.iter().filter(|d| d.1 != 0).collect();
    if !x_deltas.is_empty() {
        code.push(0x01); // SVTCA[1] — freedom/projection to X
        for &&(pt, dx, _) in &x_deltas {
            tt_push(&mut code, dx as i32);
            tt_push(&mut code, pt as i32);
            code.push(0x38); // SHPIX
        }
    }

    // Y-axis deltas
    let y_deltas: Vec<_> = deltas.iter().filter(|d| d.2 != 0).collect();
    if !y_deltas.is_empty() {
        code.push(0x00); // SVTCA[0] — freedom/projection to Y
        for &&(pt, _, dy) in &y_deltas {
            tt_push(&mut code, dy as i32);
            tt_push(&mut code, pt as i32);
            code.push(0x38); // SHPIX
        }
    }

    code.push(0x59); // EIF
    code
}

fn tt_push(code: &mut Vec<u8>, value: i32) {
    if (0..=255).contains(&value) {
        code.push(0xB0); // PUSHB[0]
        code.push(value as u8);
    } else {
        code.push(0xB8); // PUSHW[0]
        let v = value as i16;
        code.push((v >> 8) as u8);
        code.push(v as u8);
    }
}
