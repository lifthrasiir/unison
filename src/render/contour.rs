//! Pixel shapes → outline contours.
//!
//! Tracing a single grid is [`track_contour`]; a composite is traced from all of
//! its layers at once ([`track_contour_multi`], [`track_contour_multi_diff`]),
//! because a cell two layers both touch has to be their geometric *union*, not
//! two overlapping outlines.
//!
//! **Coordinate space.** The `track_contour_multi*` functions normalize their
//! output to the bounding box of the layers they were given. The production
//! callers want the layers' own space instead, so they use the `_at` variants —
//! a negative `ref` offset is a bearing that has to survive tracing, see
//! [`crate::ref_composite`].
//!
//! This is, with `ttf_builder`, where the bugs are: sub-pixel and on-demand
//! shapes inside composites. Check contour output at *composite* level whenever
//! shape codes, [`crate::detail::DetailRegion`] or on-demand synthesis change,
//! and test the degenerate inputs (empty, 1×1, zero-extent) explicitly.

use std::collections::{BTreeMap, HashMap, HashSet};

/// A cell's outline data as [`pixel::multi_shape_adjacency`] computes it: the
/// adjacency bitmask plus the gap segments interior to the cell.
struct CellEdges {
    adj: u8,
    gaps: Vec<(f32, f32, f32, f32)>,
}

use crate::document::PixelGrid;
use crate::pixel::{self, PX_ALMOSTFULL, PX_EMPTY, PX_SUBPIXEL, PixelShape};

pub fn track_contour(grid: &PixelGrid, mask: u8) -> Vec<Vec<(f32, f32)>> {
    let width = grid.width as usize;
    let height = grid.height as usize;
    let stride = width + 1;

    // Add sentinel rows at top and bottom, sentinel column at right.
    // Data layout: (height+2) rows of stride columns.
    // Row 0 = sentinel (all empty), rows 1..=height = pixel data, row height+1 = sentinel.
    let total = (height + 2) * stride;
    let mut data = vec![PX_EMPTY; total];
    for r in 0..height {
        for c in 0..width {
            data[(r + 1) * stride + c] = grid.get(r as u16, c as u16).shape_id();
        }
    }

    // Precompute custom cell geometry at a common lattice. `lat` doubles as
    // the endpoint key quantization scale, so plain half-lattice geometry
    // and custom detail geometry share exact keys.
    let lat: i64 = if grid.details.is_empty() {
        2
    } else {
        let den = grid.den.max(1) as i64;
        if den % 2 == 0 { den } else { den * 2 }
    };
    let mut custom: HashMap<usize, CustomCell> = HashMap::new();
    for (&(r, c), region) in &grid.details {
        let idx = (r as usize + 1) * stride + c as usize;
        if data[idx] & mask != pixel::PX_CUSTOM {
            continue;
        }
        let cov = region.edge_coverage();
        let scale = lat / cov.den.max(1) as i64;
        let cnv = |list: &[(u8, u8)]| -> Vec<(i64, i64)> {
            list.iter()
                .map(|&(a, b)| (a as i64 * scale, b as i64 * scale))
                .collect()
        };
        custom.insert(
            idx,
            CustomCell {
                cov: [
                    cnv(&cov.top),
                    cnv(&cov.right),
                    cnv(&cov.bottom),
                    cnv(&cov.left),
                ],
                interior: region.interior_segments(),
            },
        );
    }

    let mut paths = Vec::new();
    let mut visited = HashSet::new();

    // Iterate over pixel rows (offset by 1 for top sentinel)
    for row in 0..height {
        let i0 = (row + 1) * stride;
        for i in i0..i0 + width {
            if data[i] == PX_EMPTY {
                continue;
            }
            if visited.contains(&i) {
                continue;
            }

            let mut unsure = vec![i];
            let mut segs: Vec<(f32, f32, f32, f32)> = Vec::new();

            while let Some(i) = unsure.pop() {
                if visited.contains(&i) {
                    continue;
                }
                visited.insert(i);

                // top, right, bottom, left (guarded by the sentinel border).
                let neighbor_idx = [i.wrapping_sub(stride), i + 1, i + stride, i.wrapping_sub(1)];
                let custom_involved = !custom.is_empty()
                    && (custom.contains_key(&i)
                        || neighbor_idx.iter().any(|n| custom.contains_key(n)));

                if custom_involved {
                    trace_custom_pixel(
                        i,
                        stride,
                        &data,
                        mask,
                        &custom,
                        lat,
                        neighbor_idx,
                        &visited,
                        &mut unsure,
                        &mut segs,
                    );
                    continue;
                }

                let (pixel_adj, gap_segs) = pixel::adjacency(data[i] & mask);
                let (top_adj, _) = pixel::adjacency(data[i.wrapping_sub(stride)] & mask);
                let (bottom_adj, _) = pixel::adjacency(data[i + stride] & mask);
                let (left_adj, _) = pixel::adjacency(data[i.wrapping_sub(1)] & mask);
                let (right_adj, _) = pixel::adjacency(data[i + 1] & mask);

                let connected = connected_bits(pixel_adj, top_adj, right_adj, bottom_adj, left_adj);

                if (connected & 0b11000000) != 0 && !visited.contains(&(i - stride)) {
                    unsure.push(i - stride);
                }
                if (connected & 0b00110000) != 0 && !visited.contains(&(i + 1)) {
                    unsure.push(i + 1);
                }
                if (connected & 0b00001100) != 0 && !visited.contains(&(i + stride)) {
                    unsure.push(i + stride);
                }
                if (connected & 0b00000011) != 0 && !visited.contains(&(i - 1)) {
                    unsure.push(i - 1);
                }

                let disconnected = connected ^ 0xFF;
                if disconnected != 0 {
                    let y = (i / stride) as f32 - 1.0;
                    let x = (i % stride) as f32;
                    emit_boundary_segs(x, y, pixel_adj, disconnected, gap_segs, &mut segs);
                }
            }

            // Link segments into closed paths
            trace_closed_paths(&segs, lat as f32, &mut paths);
        }
    }

    // Fix winding directions
    fix_winding(&mut paths);

    paths
}

/// Exact per-side geometry of a custom pixel, at the tracer's common
/// lattice. Side order: top, right, bottom, left.
struct CustomCell {
    cov: [Vec<(i64, i64)>; 4],
    interior: Vec<(f32, f32, f32, f32)>,
}

/// Interval-based connectivity and boundary emission for a pixel that is
/// custom or has a custom neighbor. Equivalent to the half-edge bit logic
/// whenever both sides are plain shapes.
#[expect(clippy::too_many_arguments)]
fn trace_custom_pixel(
    i: usize,
    stride: usize,
    data: &[u8],
    mask: u8,
    custom: &HashMap<usize, CustomCell>,
    lat: i64,
    neighbor_idx: [usize; 4],
    visited: &HashSet<usize>,
    unsure: &mut Vec<usize>,
    segs: &mut Vec<(f32, f32, f32, f32)>,
) {
    let side_cov = |idx: usize, side: usize| -> Vec<(i64, i64)> {
        if let Some(cell) = custom.get(&idx) {
            return cell.cov[side].clone();
        }
        let (bits, _) = pixel::adjacency(data[idx] & mask);
        // Half-edge bits per side, in axis direction:
        //    a   b        top:    a=0x80 [0,½], b=0x40 [½,1]
        //   +--+--+       right:  c=0x20 [0,½], d=0x10 [½,1]
        // h |     | c     bottom: f=0x04 [0,½], e=0x08 [½,1]
        //   +     +       left:   h=0x01 [0,½], g=0x02 [½,1]
        // g |     | d
        //   +--+--+
        //    f   e
        let (b1, b2) = match side {
            0 => (0x80, 0x40),
            1 => (0x20, 0x10),
            2 => (0x04, 0x08),
            _ => (0x01, 0x02),
        };
        let h = lat / 2;
        match (bits & b1 != 0, bits & b2 != 0) {
            (false, false) => vec![],
            (true, false) => vec![(0, h)],
            (false, true) => vec![(h, lat)],
            (true, true) => vec![(0, lat)],
        }
    };

    let y = (i / stride) as f32 - 1.0;
    let x = (i % stride) as f32;
    let latf = lat as f32;

    for (side, &n) in neighbor_idx.iter().enumerate() {
        let cov_s = side_cov(i, side);
        let cov_n = side_cov(n, side ^ 2);
        if intervals_intersect(&cov_s, &cov_n) && !visited.contains(&n) {
            unsure.push(n);
        }
        for (a, b) in intervals_subtract(&cov_s, &cov_n) {
            let (fa, fb) = (a as f32 / latf, b as f32 / latf);
            let seg = match side {
                0 => (x + fa, y, x + fb, y),
                1 => (x + 1.0, y + fa, x + 1.0, y + fb),
                2 => (x + fa, y + 1.0, x + fb, y + 1.0),
                _ => (x, y + fa, x, y + fb),
            };
            segs.push(seg);
        }
    }

    if let Some(cell) = custom.get(&i) {
        for &(x1, y1, x2, y2) in &cell.interior {
            segs.push((x + x1, y + y1, x + x2, y + y2));
        }
    } else {
        let (pixel_adj, gap_segs) = pixel::adjacency(data[i] & mask);
        if pixel_adj != 0xFF {
            for &(x1, y1, x2, y2) in gap_segs {
                segs.push((x + x1, y + y1, x + x2, y + y2));
            }
        }
    }
}

fn intervals_intersect(a: &[(i64, i64)], b: &[(i64, i64)]) -> bool {
    for &(a1, a2) in a {
        for &(b1, b2) in b {
            if a1.max(b1) < a2.min(b2) {
                return true;
            }
        }
    }
    false
}

/// Subtract sorted disjoint interval lists: `a − b`.
fn intervals_subtract(a: &[(i64, i64)], b: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for &(a1, a2) in a {
        let mut cur = a1;
        for &(b1, b2) in b {
            if b2 <= cur || b1 >= a2 {
                continue;
            }
            if b1 > cur {
                out.push((cur, b1));
            }
            cur = cur.max(b2);
            if cur >= a2 {
                break;
            }
        }
        if cur < a2 {
            out.push((cur, a2));
        }
    }
    out
}

/// Combined-connectivity bits between a pixel's adjacency and its four
/// neighbors' adjacency (see `pixel::adjacency` for the bit layout).
fn connected_bits(pixel_adj: u8, top_adj: u8, right_adj: u8, bottom_adj: u8, left_adj: u8) -> u8 {
    (pixel_adj & (top_adj << 5) & 0b10000000)
        | (pixel_adj & (top_adj << 3) & 0b01000000)
        | (pixel_adj & (right_adj << 5) & 0b00100000)
        | (pixel_adj & (right_adj << 3) & 0b00010000)
        | (pixel_adj & (bottom_adj >> 3) & 0b00001000)
        | (pixel_adj & (bottom_adj >> 5) & 0b00000100)
        | (pixel_adj & (left_adj >> 3) & 0b00000010)
        | (pixel_adj & (left_adj >> 5) & 0b00000001)
}

/// Emit the boundary segments of the pixel at `(x, y)` whose sides are not
/// connected to a neighbor. `line_segs` holds the disconnected adjacency
/// bits; `gap_segs` are the pixel's interior gap segments, emitted when any
/// non-adjacent side is disconnected.
fn emit_boundary_segs(
    x: f32,
    y: f32,
    pixel_adj: u8,
    disconnected: u8,
    gap_segs: &[(f32, f32, f32, f32)],
    segs: &mut Vec<(f32, f32, f32, f32)>,
) {
    let line_segs = pixel_adj & disconnected;

    if (line_segs & 0b11000000) == 0b11000000 {
        segs.push((x, y, x + 1.0, y));
    } else {
        if line_segs & 0b10000000 != 0 {
            segs.push((x, y, x + 0.5, y));
        }
        if line_segs & 0b01000000 != 0 {
            segs.push((x + 0.5, y, x + 1.0, y));
        }
    }

    if (line_segs & 0b00110000) == 0b00110000 {
        segs.push((x + 1.0, y, x + 1.0, y + 1.0));
    } else {
        if line_segs & 0b00100000 != 0 {
            segs.push((x + 1.0, y, x + 1.0, y + 0.5));
        }
        if line_segs & 0b00010000 != 0 {
            segs.push((x + 1.0, y + 0.5, x + 1.0, y + 1.0));
        }
    }

    if (line_segs & 0b00001100) == 0b00001100 {
        segs.push((x + 1.0, y + 1.0, x, y + 1.0));
    } else {
        if line_segs & 0b00001000 != 0 {
            segs.push((x + 1.0, y + 1.0, x + 0.5, y + 1.0));
        }
        if line_segs & 0b00000100 != 0 {
            segs.push((x + 0.5, y + 1.0, x, y + 1.0));
        }
    }

    if (line_segs & 0b00000011) == 0b00000011 {
        segs.push((x, y + 1.0, x, y));
    } else {
        if line_segs & 0b00000010 != 0 {
            segs.push((x, y + 1.0, x, y + 0.5));
        }
        if line_segs & 0b00000001 != 0 {
            segs.push((x, y + 0.5, x, y));
        }
    }

    if !pixel_adj & disconnected != 0 {
        for &(x1, y1, x2, y2) in gap_segs {
            segs.push((x + x1, y + y1, x + x2, y + y2));
        }
    }
}

/// Link the unordered boundary segments of one connected component into
/// closed paths and append them to `paths`. Segment endpoints are quantized
/// at `key_scale` subdivisions per pixel so shared endpoints coincide
/// exactly (all emitted coordinates are multiples of `1/key_scale`).
fn trace_closed_paths(
    segs: &[(f32, f32, f32, f32)],
    key_scale: f32,
    paths: &mut Vec<Vec<(f32, f32)>>,
) {
    let to_key = |x: f32, y: f32| -> (i64, i64) {
        (
            (x * key_scale).round() as i64,
            (y * key_scale).round() as i64,
        )
    };
    let from_key = |x: i64, y: i64| -> (f32, f32) { (x as f32 / key_scale, y as f32 / key_scale) };
    let mut px_to_segs: BTreeMap<(i64, i64), Vec<(i64, i64)>> = BTreeMap::new();
    for (x1, y1, x2, y2) in segs {
        let k1 = to_key(*x1, *y1);
        let k2 = to_key(*x2, *y2);
        px_to_segs.entry(k1).or_default().push(k2);
        px_to_segs.entry(k2).or_default().push(k1);
    }

    for list in px_to_segs.values_mut() {
        list.sort();
    }

    while !px_to_segs.is_empty() {
        let (&start_key, _) = px_to_segs.iter().next().unwrap();
        let v = px_to_segs.get_mut(&start_key).unwrap();
        assert!(v.len() >= 2);
        let next_key = v.pop().unwrap();
        if v.is_empty() {
            px_to_segs.remove(&start_key);
        }

        let mut path: Vec<(i64, i64)> = vec![start_key];
        let mut indices: HashMap<(i64, i64), usize> = HashMap::new();
        indices.insert(start_key, 0);

        let mut x0 = start_key;
        let mut x = next_key;
        let mut dx = start_key.0 - next_key.0;
        let mut dy = start_key.1 - next_key.1;

        while let Some(mut list) = px_to_segs.remove(&x) {
            list.retain(|k| *k != x0);
            let nx_list = list;

            if let Some(&k) = indices.get(&x) {
                let mut extracted: Vec<(i64, i64)> = path[k..].to_vec();
                path.truncate(k);
                if extracted.first() != Some(&x) {
                    extracted.insert(0, x);
                }
                paths.push(extracted.iter().map(|&(a, b)| from_key(a, b)).collect());

                if path.is_empty() {
                    if !nx_list.is_empty() {
                        px_to_segs.insert(x, nx_list);
                    }
                    break;
                }

                indices.retain(|_, v| *v < path.len());

                let prev = path[path.len() - 1];
                dx = x.0 - prev.0;
                dy = x.1 - prev.1;
            }

            if nx_list.is_empty() {
                break;
            }

            let xx = nx_list[0];
            if nx_list.len() > 1 {
                px_to_segs.insert(x, nx_list[1..].to_vec());
            }

            indices.insert(x, path.len());
            if dx * (x.1 - xx.1) != dy * (x.0 - xx.0) {
                path.push(x);
                dx = x.0 - xx.0;
                dy = x.1 - xx.1;
            }

            x0 = x;
            x = xx;
        }
    }
}

fn signed_area(path: &[(f32, f32)]) -> f32 {
    let n = path.len();
    if n < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    let (mut x0, mut y0) = path[n - 1];
    for &(x, y) in path {
        area += x0 * y - x * y0;
        x0 = x;
        y0 = y;
    }
    area
}

fn ccw(x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) -> f32 {
    (x2 - x1) * (y3 - y1) - (y2 - y1) * (x3 - x1)
}

fn inside(x1: f32, y1: f32, x: f32, y: f32, x2: f32, y2: f32) -> bool {
    ccw(x1, y1, x, y, x2, y2) == 0.0
        && (x1 <= x && x <= x2 || x1 >= x && x >= x2)
        && (y1 <= y && y <= y2 || y1 >= y && y >= y2)
}

fn winding_number(x: f32, y: f32, path: &[(f32, f32)]) -> i32 {
    let n = path.len();
    let (mut xx0, mut yy0) = path[n - 1];
    let mut wn = 0i32;
    for &(xx, yy) in path {
        if yy0 <= y {
            if yy > y && ccw(xx0, yy0, xx, yy, x, y) > 0.0 {
                wn += 1;
            }
        } else if yy <= y && ccw(xx0, yy0, xx, yy, x, y) < 0.0 {
            wn -= 1;
        }
        xx0 = xx;
        yy0 = yy;
    }
    wn
}

pub fn track_contour_fullpixel(grid: &PixelGrid) -> Vec<Vec<(f32, f32)>> {
    let mut fp_grid = PixelGrid::new(grid.width, grid.height);
    for r in 0..grid.height {
        for c in 0..grid.width {
            if grid.get(r, c).is_filled() {
                fp_grid.set(r, c, PixelShape(PX_ALMOSTFULL));
            }
        }
    }
    track_contour(&fp_grid, PX_SUBPIXEL)
}

/// Bounding box of positioned layers: `(min_r, min_c, width, height)`.
/// The box always includes the origin, matching how composites treat (0, 0)
/// as the own-glyph anchor.
pub(crate) fn layer_bounds<'a>(
    layers: impl IntoIterator<Item = (&'a PixelGrid, i32, i32)>,
) -> (i32, i32, usize, usize) {
    let mut min_r: i32 = 0;
    let mut min_c: i32 = 0;
    let mut max_r: i32 = 0;
    let mut max_c: i32 = 0;
    for (grid, row_off, col_off) in layers {
        min_r = min_r.min(row_off);
        min_c = min_c.min(col_off);
        max_r = max_r.max(row_off + grid.height as i32);
        max_c = max_c.max(col_off + grid.width as i32);
    }
    let width = (max_c - min_c).max(0) as usize;
    let height = (max_r - min_r).max(0) as usize;
    (min_r, min_c, width, height)
}

/// Merge positioned layers into a single grid with exact per-cell geometry:
/// positive layers unioned, negated layers subtracted.
///
/// The bitmask tracers below identify a cell by its shape id alone, so every
/// `PX_CUSTOM` cell looks alike to them (and carries no adjacency at all).
/// Whenever custom detail geometry is in play we therefore combine the cells
/// through [`crate::detail::bool_op`] first and hand the result to
/// [`track_contour`], which does understand detail regions.
fn merge_layers_exact(
    layers: &[(&PixelGrid, i32, i32, bool)],
    mask: u8,
    min_r: i32,
    min_c: i32,
    width: usize,
    height: usize,
) -> PixelGrid {
    use crate::detail::{BoolOp, DetailRegion, bool_op};

    let mut out = PixelGrid::new(width as u16, height as u16);
    for r in 0..height as i32 {
        for c in 0..width as i32 {
            let mut single: Option<PixelShape> = None;
            let mut multiple = false;
            let mut filled = false;
            let mut pos: Option<DetailRegion> = None;
            let mut neg: Option<DetailRegion> = None;

            for &(grid, row_off, col_off, negated) in layers {
                let lr = r + min_r - row_off;
                let lc = c + min_c - col_off;
                if lr < 0 || lc < 0 || lr >= grid.height as i32 || lc >= grid.width as i32 {
                    continue;
                }
                let shape = grid.get(lr as u16, lc as u16);
                if shape.shape_id() & mask == PX_EMPTY {
                    continue;
                }
                let region = grid.region_at(lr as u16, lc as u16);
                if negated {
                    neg = Some(match neg {
                        None => region,
                        Some(acc) => bool_op(&acc, &region, BoolOp::Union),
                    });
                    continue;
                }
                filled |= shape.is_filled();
                if single.is_some() {
                    multiple = true;
                } else {
                    single = Some(shape);
                }
                pos = Some(match pos {
                    None => region,
                    Some(acc) => bool_op(&acc, &region, BoolOp::Union),
                });
            }

            let Some(pos) = pos else { continue };
            // A lone positive catalog cell keeps its own shape id verbatim,
            // which avoids re-classifying geometry that is already exact.
            // Custom cells must go through `set_detail` so their region is
            // carried over into the merged grid.
            let plain_single = single
                .filter(|_| !multiple && neg.is_none())
                .filter(|s| s.shape_id() & mask != pixel::PX_CUSTOM);
            if let Some(shape) = plain_single {
                out.set(r as u16, c as u16, shape);
                continue;
            }
            let region = match neg {
                Some(neg) => bool_op(&pos, &neg, BoolOp::Subtract),
                None => pos.canonical(),
            };
            out.set_detail(r as u16, c as u16, &region, filled);
        }
    }
    out
}

/// Whether any layer carries custom detail geometry, which the shape-id
/// bitmask tracers cannot represent.
fn layers_have_detail(layers: &[(&PixelGrid, i32, i32, bool)]) -> bool {
    layers
        .iter()
        .any(|&(grid, _, _, _)| !grid.details.is_empty())
}

/// One step of the flood fill shared by [`track_contour_multi`] and
/// [`track_contour_multi_diff`]: computes which sides of pixel `i` connect
/// to its neighbors, queues connected unvisited neighbors, and emits
/// boundary segments for the disconnected sides.
#[expect(clippy::too_many_arguments)]
fn expand_pixel(
    i: usize,
    stride: usize,
    pixel_adj: u8,
    gap_segs: &[(f32, f32, f32, f32)],
    adj_data: &[u8],
    visited: &HashSet<usize>,
    unsure: &mut Vec<usize>,
    segs: &mut Vec<(f32, f32, f32, f32)>,
) {
    let top_adj = adj_data[i.wrapping_sub(stride)];
    let bottom_adj = adj_data[i + stride];
    let left_adj = adj_data[i.wrapping_sub(1)];
    let right_adj = adj_data[i + 1];

    let connected = connected_bits(pixel_adj, top_adj, right_adj, bottom_adj, left_adj);

    if (connected & 0b11000000) != 0 && !visited.contains(&(i - stride)) {
        unsure.push(i - stride);
    }
    if (connected & 0b00110000) != 0 && !visited.contains(&(i + 1)) {
        unsure.push(i + 1);
    }
    if (connected & 0b00001100) != 0 && !visited.contains(&(i + stride)) {
        unsure.push(i + stride);
    }
    if (connected & 0b00000011) != 0 && !visited.contains(&(i - 1)) {
        unsure.push(i - 1);
    }

    let disconnected = connected ^ 0xFF;
    if disconnected != 0 {
        let y = (i / stride) as f32 - 1.0;
        let x = (i % stride) as f32;
        emit_boundary_segs(x, y, pixel_adj, disconnected, gap_segs, segs);
    }
}

/// Shift contours traced in bounding-box space back into the coordinate space
/// the layers were positioned in.  [`layer_bounds`] anchors its box at the
/// topmost/leftmost layer, so a layer at a negative offset comes back out at
/// zero; callers that treat a negative offset as a bearing need it preserved.
fn to_layer_space(contours: &mut [Vec<(f32, f32)>], min_r: i32, min_c: i32) {
    if min_r == 0 && min_c == 0 {
        return;
    }
    let (dx, dy) = (min_c as f32, min_r as f32);
    for contour in contours {
        for point in contour.iter_mut() {
            point.0 += dx;
            point.1 += dy;
        }
    }
}

/// [`track_contour_multi`] with the result in the layers' own coordinate
/// space rather than normalized to the bounding box origin.
pub fn track_contour_multi_at(layers: &[(&PixelGrid, i32, i32)], mask: u8) -> Vec<Vec<(f32, f32)>> {
    let (min_r, min_c, _, _) = layer_bounds(layers.iter().copied());
    let mut contours = track_contour_multi(layers, mask);
    to_layer_space(&mut contours, min_r, min_c);
    contours
}

/// [`track_contour_multi_diff`] with the result in the layers' own coordinate
/// space rather than normalized to the bounding box origin.
pub fn track_contour_multi_diff_at(
    layers: &[(&PixelGrid, i32, i32, bool)],
    mask: u8,
) -> Vec<Vec<(f32, f32)>> {
    let (min_r, min_c, _, _) = layer_bounds(layers.iter().map(|&(g, r, c, _)| (g, r, c)));
    let mut contours = track_contour_multi_diff(layers, mask);
    to_layer_space(&mut contours, min_r, min_c);
    contours
}

/// Trace contours from multiple overlapping grids, correctly handling pixels
/// where different layers contribute different subpixel shapes by computing
/// the geometric union.
///
/// Each entry in `layers` is `(grid, row_offset, col_offset)` in the composite
/// coordinate space. The function computes the bounding box from all layers.
///
/// Shape combinations are cached by bitmask so each unique set of overlapping
/// shapes is computed only once.
pub fn track_contour_multi(layers: &[(&PixelGrid, i32, i32)], mask: u8) -> Vec<Vec<(f32, f32)>> {
    if layers.is_empty() {
        return Vec::new();
    }
    if layers.len() == 1 && layers[0].1 == 0 && layers[0].2 == 0 {
        return track_contour(layers[0].0, mask);
    }

    let (min_r, min_c, width, height) = layer_bounds(layers.iter().copied());
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let flagged: Vec<(&PixelGrid, i32, i32, bool)> =
        layers.iter().map(|&(g, r, c)| (g, r, c, false)).collect();
    if layers_have_detail(&flagged) {
        let merged = merge_layers_exact(&flagged, mask, min_r, min_c, width, height);
        return track_contour(&merged, mask);
    }

    let stride = width + 1;
    let total = (height + 2) * stride;

    // Per-pixel shape bitmask (bit i set ↔ shape_id i is present).
    let mut shape_masks: Vec<u128> = vec![0; total];
    // Also track single-shape fast path
    let mut single_shape: Vec<u8> = vec![PX_EMPTY; total];

    for &(grid, row_off, col_off) in layers {
        let off_r = (row_off - min_r) as usize;
        let off_c = (col_off - min_c) as usize;
        for r in 0..grid.height as usize {
            for c in 0..grid.width as usize {
                let sid = grid.get(r as u16, c as u16).shape_id() & mask;
                if sid != PX_EMPTY {
                    let idx = (off_r + r + 1) * stride + (off_c + c);
                    if shape_masks[idx] == 0 {
                        single_shape[idx] = sid;
                    }
                    shape_masks[idx] |= 1u128 << sid;
                }
            }
        }
    }

    // Pre-compute combined adjacency bits per pixel (for neighbor lookups)
    let mut adj_data: Vec<u8> = vec![0; total];
    for i in 0..total {
        if shape_masks[i] != 0 {
            if shape_masks[i].count_ones() == 1 {
                adj_data[i] = pixel::adjacency(single_shape[i]).0;
            } else {
                let shapes = bitmask_to_ids(shape_masks[i]);
                for &s in &shapes {
                    adj_data[i] |= pixel::adjacency(s).0;
                }
            }
        }
    }

    // Cache for multi-shape gap segments, keyed by shape bitmask
    let mut gap_cache: HashMap<u128, CellEdges> = HashMap::new();

    let mut paths = Vec::new();
    let mut visited = HashSet::new();

    for row in 0..height {
        let i0 = (row + 1) * stride;
        for i in i0..i0 + width {
            if shape_masks[i] == 0 || visited.contains(&i) {
                continue;
            }

            let mut unsure = vec![i];
            let mut segs: Vec<(f32, f32, f32, f32)> = Vec::new();

            while let Some(i) = unsure.pop() {
                if visited.contains(&i) {
                    continue;
                }
                visited.insert(i);

                let smask = shape_masks[i];
                let (pixel_adj, gap_segs) = if smask.count_ones() == 1 {
                    let (a, g) = pixel::adjacency(single_shape[i]);
                    (a, std::borrow::Cow::Borrowed(g))
                } else {
                    let entry = gap_cache.entry(smask).or_insert_with(|| {
                        let ids = bitmask_to_ids(smask);
                        let (adj, gaps) = pixel::multi_shape_adjacency(&ids);
                        CellEdges { adj, gaps }
                    });
                    (entry.adj, std::borrow::Cow::Borrowed(entry.gaps.as_slice()))
                };

                expand_pixel(
                    i,
                    stride,
                    pixel_adj,
                    gap_segs.as_ref(),
                    &adj_data,
                    &visited,
                    &mut unsure,
                    &mut segs,
                );
            }

            trace_closed_paths(&segs, MULTI_KEY_SCALE, &mut paths);
        }
    }

    fix_winding(&mut paths);
    paths
}

/// Like [`track_contour_multi`] but supports negative (subtracted) layers.
///
/// Each entry in `layers` is `(grid, row_offset, col_offset, negated)`.
/// Positive layers are unioned; negative layers are subtracted from the result.
/// Per-pixel adjacency is computed via [`pixel::multi_shape_diff_adjacency`]
/// and cached by `(positive_mask, negative_mask)` to avoid redundant work.
pub fn track_contour_multi_diff(
    layers: &[(&PixelGrid, i32, i32, bool)],
    mask: u8,
) -> Vec<Vec<(f32, f32)>> {
    if layers.is_empty() {
        return Vec::new();
    }
    let has_negated = layers.iter().any(|l| l.3);
    if !has_negated {
        let plain: Vec<(&PixelGrid, i32, i32)> =
            layers.iter().map(|&(g, r, c, _)| (g, r, c)).collect();
        return track_contour_multi(&plain, mask);
    }

    let (min_r, min_c, width, height) = layer_bounds(layers.iter().map(|&(g, r, c, _)| (g, r, c)));
    if width == 0 || height == 0 {
        return Vec::new();
    }

    if layers_have_detail(layers) {
        let merged = merge_layers_exact(layers, mask, min_r, min_c, width, height);
        return track_contour(&merged, mask);
    }

    let stride = width + 1;
    let total = (height + 2) * stride;

    let mut pos_masks: Vec<u128> = vec![0; total];
    let mut neg_masks: Vec<u128> = vec![0; total];

    for &(grid, row_off, col_off, negated) in layers {
        let off_r = (row_off - min_r) as usize;
        let off_c = (col_off - min_c) as usize;
        let target = if negated {
            &mut neg_masks
        } else {
            &mut pos_masks
        };
        for r in 0..grid.height as usize {
            for c in 0..grid.width as usize {
                let sid = grid.get(r as u16, c as u16).shape_id() & mask;
                if sid != PX_EMPTY {
                    let idx = (off_r + r + 1) * stride + (off_c + c);
                    target[idx] |= 1u128 << sid;
                }
            }
        }
    }

    // A pixel has content if positive shapes remain after subtracting negatives.
    // For the adjacency pre-computation we use the diff-aware function; however
    // for the quick "is this pixel non-empty" test we conservatively mark any
    // pixel that has positive shapes (we'll skip it during tracing if its diff
    // adjacency turns out to be 0).

    // Pre-compute per-pixel adjacency.
    let mut adj_data: Vec<u8> = vec![0; total];
    let mut diff_cache: HashMap<(u128, u128), CellEdges> = HashMap::new();

    for i in 0..total {
        if pos_masks[i] != 0 {
            let key = (pos_masks[i], neg_masks[i]);
            let entry = diff_cache.entry(key).or_insert_with(|| {
                let pos_ids = bitmask_to_ids(pos_masks[i]);
                let (adj, gaps) = if neg_masks[i] == 0 {
                    pixel::multi_shape_adjacency(&pos_ids)
                } else {
                    let neg_ids = bitmask_to_ids(neg_masks[i]);
                    pixel::multi_shape_diff_adjacency(&pos_ids, &neg_ids)
                };
                CellEdges { adj, gaps }
            });
            adj_data[i] = entry.adj;
        }
    }

    let mut paths = Vec::new();
    let mut visited = HashSet::new();

    for row in 0..height {
        let i0 = (row + 1) * stride;
        for i in i0..i0 + width {
            if pos_masks[i] == 0 || adj_data[i] == 0 || visited.contains(&i) {
                continue;
            }

            let mut unsure = vec![i];
            let mut segs: Vec<(f32, f32, f32, f32)> = Vec::new();

            while let Some(i) = unsure.pop() {
                if visited.contains(&i) {
                    continue;
                }
                if adj_data[i] == 0 {
                    continue;
                }
                visited.insert(i);

                let key = (pos_masks[i], neg_masks[i]);
                let entry = diff_cache.get(&key).unwrap();
                let (pixel_adj, gap_segs) = (entry.adj, entry.gaps.as_slice());

                expand_pixel(
                    i,
                    stride,
                    pixel_adj,
                    gap_segs,
                    &adj_data,
                    &visited,
                    &mut unsure,
                    &mut segs,
                );
            }

            trace_closed_paths(&segs, MULTI_KEY_SCALE, &mut paths);
        }
    }

    fix_winding(&mut paths);
    paths
}

// Clipped gap segments can have coordinates at 1/4, 1/6, 1/8 etc. of a pixel
// (from intersections of diagonal gap segments between half-pixel-aligned endpoints).
// Use 24× resolution so all such coordinates map to distinct integer keys.
const MULTI_KEY_SCALE: f32 = 24.0;

fn bitmask_to_ids(mask: u128) -> Vec<u8> {
    let mut ids = Vec::new();
    let mut m = mask;
    while m != 0 {
        let bit = m.trailing_zeros() as u8;
        ids.push(bit);
        m &= m - 1;
    }
    ids
}

fn fix_winding(paths: &mut [Vec<(f32, f32)>]) {
    let n = paths.len();
    for i in 0..n {
        let mut wn = 0i32;
        for j in 0..n {
            if i == j {
                continue;
            }

            let path_i = &paths[i];
            let path_j = &paths[j];
            let mut found = false;

            for k in 0..path_i.len() {
                let (x1, y1) = path_i[if k == 0 { path_i.len() - 1 } else { k - 1 }];
                let (x2, y2) = path_i[k];
                let x = (x1 + x2 * 1023.0) / 1024.0;
                let y = (y1 + y2 * 1023.0) / 1024.0;

                if path_j.iter().enumerate().all(|(m, &(px, py))| {
                    let (px0, py0) = path_j[if m == 0 { path_j.len() - 1 } else { m - 1 }];
                    !inside(px0, py0, x, y, px, py)
                }) {
                    wn += winding_number(x, y, path_j);
                    found = true;
                    break;
                }
            }

            assert!(found, "could not find non-overlapping point");
        }

        let a = signed_area(&paths[i]);
        if ((wn & 1) == 1) ^ (a < 0.0) {
            paths[i].reverse();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::{PX_ALMOSTFULL, PX_FULL, PX_SUBPIXEL, PixelShape};

    fn make_grid(w: u16, h: u16, pixels: &[u8]) -> PixelGrid {
        PixelGrid {
            width: w,
            height: h,
            pixels: pixels.iter().map(|&p| PixelShape(p)).collect(),
            den: 1,
            details: Default::default(),
        }
    }

    #[test]
    fn single_filled_pixel() {
        let grid = make_grid(1, 1, &[PX_ALMOSTFULL | PX_FULL]);
        let paths = track_contour(&grid, PX_SUBPIXEL);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].len(), 4);
        let path = &paths[0];
        assert!(path.contains(&(0.0, 0.0)));
        assert!(path.contains(&(1.0, 0.0)));
        assert!(path.contains(&(1.0, 1.0)));
        assert!(path.contains(&(0.0, 1.0)));
    }

    #[test]
    fn two_adjacent_pixels() {
        let grid = make_grid(2, 1, &[PX_ALMOSTFULL | PX_FULL, PX_ALMOSTFULL | PX_FULL]);
        let paths = track_contour(&grid, PX_SUBPIXEL);
        assert_eq!(paths.len(), 1);
        let path = &paths[0];
        assert!(path.len() >= 4 && path.len() <= 6);
        assert!(path.contains(&(0.0, 0.0)));
        assert!(path.contains(&(2.0, 0.0)));
        assert!(path.contains(&(2.0, 1.0)));
        assert!(path.contains(&(0.0, 1.0)));
    }

    #[test]
    fn empty_grid() {
        let grid = make_grid(3, 3, &[PX_EMPTY; 9]);
        let paths = track_contour(&grid, PX_SUBPIXEL);
        assert!(paths.is_empty());
    }

    #[test]
    fn multi_layer_complement_halves_give_full_pixel() {
        use crate::pixel::{PX_HALF1, PX_HALF2};
        let grid_a = make_grid(1, 1, &[PX_HALF1 | PX_FULL]);
        let grid_b = make_grid(1, 1, &[PX_HALF2 | PX_FULL]);
        let paths = track_contour_multi(&[(&grid_a, 0, 0), (&grid_b, 0, 0)], PX_SUBPIXEL);
        assert_eq!(paths.len(), 1, "expected one closed contour");
        let path = &paths[0];
        assert!(path.contains(&(0.0, 0.0)));
        assert!(path.contains(&(1.0, 0.0)));
        assert!(path.contains(&(1.0, 1.0)));
        assert!(path.contains(&(0.0, 1.0)));
    }

    #[test]
    fn multi_layer_partial_overlap_produces_valid_contour() {
        use crate::pixel::{PX_SLANT1H, PX_SLANT3H};
        let grid_a = make_grid(1, 1, &[PX_SLANT1H | PX_FULL]);
        let grid_b = make_grid(1, 1, &[PX_SLANT3H | PX_FULL]);
        let paths = track_contour_multi(&[(&grid_a, 0, 0), (&grid_b, 0, 0)], PX_SUBPIXEL);
        assert_eq!(
            paths.len(),
            1,
            "expected one closed contour for overlapping slants"
        );
        // The union covers edges a,h,g,f and has interior gap segments
        let path = &paths[0];
        assert!(
            path.len() >= 4,
            "path should have at least 4 vertices: {:?}",
            path
        );
        // Should contain the corner points of the union shape
        assert!(
            path.contains(&(0.0, 0.0)),
            "should contain (0,0): {:?}",
            path
        );
        assert!(
            path.contains(&(0.0, 1.0)),
            "should contain (0,1): {:?}",
            path
        );
    }

    #[test]
    fn multi_layer_with_offset() {
        // Two full pixels at different positions
        let grid_a = make_grid(1, 1, &[PX_ALMOSTFULL | PX_FULL]);
        let grid_b = make_grid(1, 1, &[PX_ALMOSTFULL | PX_FULL]);
        let paths = track_contour_multi(&[(&grid_a, 0, 0), (&grid_b, 0, 1)], PX_SUBPIXEL);
        assert_eq!(
            paths.len(),
            1,
            "two adjacent full pixels should produce one contour"
        );
        let path = &paths[0];
        assert!(path.contains(&(0.0, 0.0)));
        assert!(path.contains(&(2.0, 0.0)));
        assert!(path.contains(&(2.0, 1.0)));
        assert!(path.contains(&(0.0, 1.0)));
    }

    #[test]
    fn diff_full_minus_half_produces_smooth_contour() {
        use crate::pixel::PX_HALF1;
        // Full pixel minus bottom-left triangle → top-right triangle.
        let full = make_grid(1, 1, &[PX_ALMOSTFULL | PX_FULL]);
        let half = make_grid(1, 1, &[PX_HALF1 | PX_FULL]);
        let paths =
            track_contour_multi_diff(&[(&full, 0, 0, false), (&half, 0, 0, true)], PX_SUBPIXEL);
        assert_eq!(paths.len(), 1, "should produce one contour");
        let path = &paths[0];
        // The result is the top-right triangle: (0,0)→(1,0)→(1,1)→(0,0).
        assert!(path.contains(&(0.0, 0.0)), "missing (0,0): {path:?}");
        assert!(path.contains(&(1.0, 0.0)), "missing (1,0): {path:?}");
        assert!(path.contains(&(1.0, 1.0)), "missing (1,1): {path:?}");
        // Should NOT have grid-like staircase points.
        assert!(path.len() <= 4, "too many vertices (grid-like?): {path:?}");
    }

    #[test]
    fn diff_adjacent_full_minus_half_produces_valid_contours() {
        use crate::pixel::PX_HALF1;
        // Two full pixels side by side; subtract bottom-left half from the right one.
        // Result: full square on left + top-right triangle on right (share only a point,
        // so they form two separate contours).
        let full = make_grid(2, 1, &[PX_ALMOSTFULL | PX_FULL, PX_ALMOSTFULL | PX_FULL]);
        let half = make_grid(1, 1, &[PX_HALF1 | PX_FULL]);
        let paths =
            track_contour_multi_diff(&[(&full, 0, 0, false), (&half, 0, 1, true)], PX_SUBPIXEL);
        assert_eq!(
            paths.len(),
            2,
            "full square + triangle share only a point: {paths:?}"
        );
        // Neither contour should be grid-like (excessive vertices).
        for path in &paths {
            assert!(path.len() <= 5, "contour has too many vertices: {path:?}");
        }
    }

    #[test]
    fn diff_no_negated_delegates_to_multi() {
        // Without any negated layers, should produce the same result as
        // track_contour_multi.
        let grid = make_grid(2, 1, &[PX_ALMOSTFULL | PX_FULL, PX_ALMOSTFULL | PX_FULL]);
        let paths_multi = track_contour_multi(&[(&grid, 0, 0)], PX_SUBPIXEL);
        let paths_diff = track_contour_multi_diff(&[(&grid, 0, 0, false)], PX_SUBPIXEL);
        assert_eq!(paths_multi.len(), paths_diff.len());
        assert_eq!(paths_multi[0].len(), paths_diff[0].len());
    }

    #[test]
    fn fine_key_distinguishes_eighth_fractions() {
        // Gap-segment intersection points can land at 1/8, 1/6, 1/12 etc. of a
        // pixel. These must not collide when quantized into integer keys.
        let pts = [(1.0 / 8.0, 0.0), (1.0 / 6.0, 0.0), (1.0 / 12.0, 0.0)];
        let keys: Vec<(i64, i64)> = pts
            .iter()
            .map(|&(x, y)| {
                (
                    (x * MULTI_KEY_SCALE).round() as i64,
                    (y * MULTI_KEY_SCALE).round() as i64,
                )
            })
            .collect();
        assert_ne!(
            keys[0], keys[1],
            "1/8 and 1/6 must map to distinct keys: {keys:?}"
        );
        assert_ne!(
            keys[0], keys[2],
            "1/8 and 1/12 must map to distinct keys: {keys:?}"
        );
        assert_ne!(
            keys[1], keys[2],
            "1/6 and 1/12 must map to distinct keys: {keys:?}"
        );
    }

    #[test]
    fn fine_key_roundtrips_within_fine_resolution() {
        let tolerance = 1.0 / (2.0 * MULTI_KEY_SCALE) + 1e-4;
        for &(x, y) in &[
            (1.0 / 8.0, 3.0 / 8.0),
            (1.0 / 6.0, 5.0 / 6.0),
            (1.0 / 12.0, 7.0 / 12.0),
        ] {
            let (kx, ky) = (
                (x * MULTI_KEY_SCALE).round() as i64,
                (y * MULTI_KEY_SCALE).round() as i64,
            );
            let (rx, ry) = (kx as f32 / MULTI_KEY_SCALE, ky as f32 / MULTI_KEY_SCALE);
            assert!((rx - x).abs() <= tolerance, "x round-trip off: {x} -> {rx}");
            assert!((ry - y).abs() <= tolerance, "y round-trip off: {y} -> {ry}");
        }
    }

    #[test]
    fn brute_force_multi_shape_degree_parity() {
        // Test all pairs of shapes overlapping in the same pixel.
        // The contour segments must form a graph where every vertex has even degree.
        use std::collections::BTreeMap;

        let shape_ids: Vec<u8> = (1..=30).chain((1..=30).map(|s| s ^ PX_SUBPIXEL)).collect();

        let mut failures: Vec<(u8, u8)> = Vec::new();

        for &s1 in &shape_ids {
            for &s2 in &shape_ids {
                if s1 >= s2 {
                    continue;
                }
                let width = 1usize;
                let height = 1usize;
                let stride = width + 1;
                let total = (height + 2) * stride;

                let mut shape_masks: Vec<u128> = vec![0; total];
                let mut single_shape: Vec<u8> = vec![PX_EMPTY; total];
                let mut adj_data: Vec<u8> = vec![0; total];

                // Layer 1
                let sid1 = s1 & PX_SUBPIXEL;
                let idx = stride; // row 1, column 0
                single_shape[idx] = sid1;
                shape_masks[idx] |= 1u128 << sid1;

                // Layer 2
                let sid2 = s2 & PX_SUBPIXEL;
                if sid2 != sid1 {
                    shape_masks[idx] |= 1u128 << sid2;
                }

                // Compute adj_data the same way as track_contour_multi
                for i in 0..total {
                    if shape_masks[i] != 0 {
                        if shape_masks[i].count_ones() == 1 {
                            adj_data[i] = pixel::adjacency(single_shape[i]).0;
                        } else {
                            let ids = bitmask_to_ids(shape_masks[i]);
                            for &s in &ids {
                                adj_data[i] |= pixel::adjacency(s).0;
                            }
                        }
                    }
                }

                // Compute segments like track_contour_multi does
                let smask = shape_masks[idx];
                if smask == 0 {
                    continue;
                }
                let (pixel_adj, gap_segs) = if smask.count_ones() == 1 {
                    let (a, g) = pixel::adjacency(single_shape[idx]);
                    (a, g.to_vec())
                } else {
                    let ids = bitmask_to_ids(smask);
                    pixel::multi_shape_adjacency(&ids)
                };

                let top_adj = adj_data[idx.wrapping_sub(stride)];
                let bottom_adj = adj_data[idx + stride];
                let left_adj = adj_data[idx.wrapping_sub(1)];
                let right_adj = adj_data[idx + 1];

                let connected = connected_bits(pixel_adj, top_adj, right_adj, bottom_adj, left_adj);
                let disconnected = connected ^ 0xFF;

                if disconnected == 0 {
                    continue;
                }

                let mut segs: Vec<(f32, f32, f32, f32)> = Vec::new();
                emit_boundary_segs(0.0, 0.0, pixel_adj, disconnected, &gap_segs, &mut segs);

                // Check degree parity
                let to_key = |x: f32, y: f32| -> (i64, i64) {
                    (
                        (x * MULTI_KEY_SCALE).round() as i64,
                        (y * MULTI_KEY_SCALE).round() as i64,
                    )
                };
                let mut degree: BTreeMap<(i64, i64), usize> = BTreeMap::new();
                for &(x1, y1, x2, y2) in &segs {
                    let k1 = to_key(x1, y1);
                    let k2 = to_key(x2, y2);
                    *degree.entry(k1).or_default() += 1;
                    *degree.entry(k2).or_default() += 1;
                }

                let has_odd = degree.iter().any(|(_, &d)| d % 2 != 0);
                if has_odd {
                    failures.push((s1, s2));
                }
            }
        }

        assert!(
            failures.is_empty(),
            "Shape pairs with odd-degree vertices: {:?}",
            failures
        );
    }

    /// A layer with custom detail geometry must keep that geometry when it is
    /// merged with other layers: the shape-id tracer sees every custom cell as
    /// one and the same (adjacency-less) id, which used to drop the diagonal
    /// and leave a whole-pixel staircase.
    #[test]
    fn multi_layer_keeps_custom_detail_geometry() {
        use crate::ref_composite::{OnDemandGlyph, make_on_demand_grid, parse_on_demand_glyph};

        let grid_of = |name: &str| {
            let Some(OnDemandGlyph::Rect(rect)) = parse_on_demand_glyph(name) else {
                panic!("{name} must parse");
            };
            make_on_demand_grid(&rect)
        };
        // The three refs of `sextant-13-dr` (U+1FB43), rescaled to the
        // parent's scale 1: a 1:8/3-slope triangle plus two rectangles.
        let tri = grid_of("4x10p2r3-dr").rescale(3, 1);
        assert!(
            !tri.details.is_empty(),
            "the 8:3 slope needs custom details"
        );
        let right = grid_of("4x16");
        let bottom = grid_of("8x-5p1r3").rescale(3, 1);

        let paths = track_contour_multi(
            &[(&tri, 0, 0), (&right, 0, 4), (&bottom, 10, 0)],
            PX_SUBPIXEL,
        );
        assert_eq!(paths.len(), 1, "single outline: {paths:?}");
        // Convex pentagon: the hypotenuse runs from (4, 0) to (0, 32/3).
        let expected = [
            (0.0f32, 32.0 / 3.0),
            (4.0, 0.0),
            (8.0, 0.0),
            (8.0, 16.0),
            (0.0, 16.0),
        ];
        assert_eq!(paths[0].len(), expected.len(), "no staircase: {paths:?}");
        let start = paths[0]
            .iter()
            .position(|p| (p.0 - expected[0].0).abs() < 1e-4 && (p.1 - expected[0].1).abs() < 1e-4)
            .unwrap_or_else(|| panic!("apex missing in {paths:?}"));
        for (i, e) in expected.iter().enumerate() {
            let p = paths[0][(start + i) % expected.len()];
            assert!(
                (p.0 - e.0).abs() < 1e-4 && (p.1 - e.1).abs() < 1e-4,
                "vertex {i} is {p:?}, expected {e:?} in {paths:?}",
            );
        }
    }
}
