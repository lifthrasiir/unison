use std::collections::{BTreeMap, HashMap, HashSet};

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

                let (pixel_adj, gap_segs) = pixel::adjacency(data[i] & mask);
                let (top_adj, _) = pixel::adjacency(data[i.wrapping_sub(stride)] & mask);
                let (bottom_adj, _) = pixel::adjacency(data[i + stride] & mask);
                let (left_adj, _) = pixel::adjacency(data[i.wrapping_sub(1)] & mask);
                let (right_adj, _) = pixel::adjacency(data[i + 1] & mask);

                let connected = (pixel_adj & (top_adj << 5) & 0b10000000)
                    | (pixel_adj & (top_adj << 3) & 0b01000000)
                    | (pixel_adj & (right_adj << 5) & 0b00100000)
                    | (pixel_adj & (right_adj << 3) & 0b00010000)
                    | (pixel_adj & (bottom_adj >> 3) & 0b00001000)
                    | (pixel_adj & (bottom_adj >> 5) & 0b00000100)
                    | (pixel_adj & (left_adj >> 3) & 0b00000010)
                    | (pixel_adj & (left_adj >> 5) & 0b00000001);

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
                    let y = (i / stride) as f32 - 1.0; // subtract sentinel offset
                    let x = (i % stride) as f32;

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
            }

            // Build adjacency map from segments
            let mut px_to_segs: BTreeMap<(i64, i64), Vec<(i64, i64)>> = BTreeMap::new();
            for (x1, y1, x2, y2) in &segs {
                let k1 = to_key(*x1, *y1);
                let k2 = to_key(*x2, *y2);
                px_to_segs.entry(k1).or_default().push(k2);
                px_to_segs.entry(k2).or_default().push(k1);
            }

            for list in px_to_segs.values_mut() {
                list.sort();
            }

            // Trace closed paths
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
    }

    // Fix winding directions
    fix_winding(&mut paths);

    paths
}

fn to_key(x: f32, y: f32) -> (i64, i64) {
    ((x * 2.0) as i64, (y * 2.0) as i64)
}

fn from_key(x: i64, y: i64) -> (f32, f32) {
    (x as f32 / 2.0, y as f32 / 2.0)
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

/// Trace contours from multiple overlapping grids, correctly handling pixels
/// where different layers contribute different subpixel shapes by computing
/// the geometric union.
///
/// Each entry in `layers` is `(grid, row_offset, col_offset)` in the composite
/// coordinate space. The function computes the bounding box from all layers.
///
/// Shape combinations are cached by bitmask so each unique set of overlapping
/// shapes is computed only once.
pub fn track_contour_multi(
    layers: &[(&PixelGrid, i32, i32)],
    mask: u8,
) -> Vec<Vec<(f32, f32)>> {
    if layers.is_empty() {
        return Vec::new();
    }
    if layers.len() == 1 && layers[0].1 == 0 && layers[0].2 == 0 {
        return track_contour(layers[0].0, mask);
    }

    // Compute bounding box
    let mut min_r: i32 = 0;
    let mut min_c: i32 = 0;
    let mut max_r: i32 = 0;
    let mut max_c: i32 = 0;
    for &(grid, row_off, col_off) in layers {
        min_r = min_r.min(row_off);
        min_c = min_c.min(col_off);
        max_r = max_r.max(row_off + grid.height as i32);
        max_c = max_c.max(col_off + grid.width as i32);
    }
    let width = (max_c - min_c).max(0) as usize;
    let height = (max_r - min_r).max(0) as usize;
    if width == 0 || height == 0 {
        return Vec::new();
    }

    let stride = width + 1;
    let total = (height + 2) * stride;

    // Per-pixel shape bitmask (bit i set ↔ shape_id i is present).
    let mut shape_masks: Vec<u32> = vec![0; total];
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
                    let bit = 1u32 << sid;
                    if shape_masks[idx] == 0 {
                        single_shape[idx] = sid;
                    }
                    shape_masks[idx] |= bit;
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
    let mut gap_cache: HashMap<u32, (u8, Vec<(f32, f32, f32, f32)>)> = HashMap::new();

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
                        pixel::multi_shape_adjacency(&ids)
                    });
                    (entry.0, std::borrow::Cow::Borrowed(entry.1.as_slice()))
                };

                let top_adj = adj_data[i.wrapping_sub(stride)];
                let bottom_adj = adj_data[i + stride];
                let left_adj = adj_data[i.wrapping_sub(1)];
                let right_adj = adj_data[i + 1];

                let connected = (pixel_adj & (top_adj << 5) & 0b10000000)
                    | (pixel_adj & (top_adj << 3) & 0b01000000)
                    | (pixel_adj & (right_adj << 5) & 0b00100000)
                    | (pixel_adj & (right_adj << 3) & 0b00010000)
                    | (pixel_adj & (bottom_adj >> 3) & 0b00001000)
                    | (pixel_adj & (bottom_adj >> 5) & 0b00000100)
                    | (pixel_adj & (left_adj >> 3) & 0b00000010)
                    | (pixel_adj & (left_adj >> 5) & 0b00000001);

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
                        for &(x1, y1, x2, y2) in gap_segs.as_ref() {
                            segs.push((x + x1, y + y1, x + x2, y + y2));
                        }
                    }
                }
            }

            // Build adjacency map from segments (using fine-resolution keys
            // to handle clipped gap segment coordinates correctly)
            let mut px_to_segs: BTreeMap<(i64, i64), Vec<(i64, i64)>> = BTreeMap::new();
            for (x1, y1, x2, y2) in &segs {
                let k1 = to_key_fine(*x1, *y1);
                let k2 = to_key_fine(*x2, *y2);
                px_to_segs.entry(k1).or_default().push(k2);
                px_to_segs.entry(k2).or_default().push(k1);
            }

            for list in px_to_segs.values_mut() {
                list.sort();
            }

            // Trace closed paths
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
                        paths.push(extracted.iter().map(|&(a, b)| from_key_fine(a, b)).collect());

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
    }

    fix_winding(&mut paths);
    paths
}

// Clipped gap segments can have coordinates at 1/4, 1/6, 1/8 etc. of a pixel
// (from intersections of diagonal gap segments between half-pixel-aligned endpoints).
// Use 24× resolution so all such coordinates map to distinct integer keys.
const MULTI_KEY_SCALE: f32 = 24.0;

fn to_key_fine(x: f32, y: f32) -> (i64, i64) {
    ((x * MULTI_KEY_SCALE).round() as i64, (y * MULTI_KEY_SCALE).round() as i64)
}

fn from_key_fine(x: i64, y: i64) -> (f32, f32) {
    (x as f32 / MULTI_KEY_SCALE, y as f32 / MULTI_KEY_SCALE)
}

fn bitmask_to_ids(mask: u32) -> Vec<u8> {
    let mut ids = Vec::new();
    for i in 0..32u8 {
        if mask & (1u32 << i) != 0 {
            ids.push(i);
        }
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

                if path_j
                    .iter()
                    .enumerate()
                    .all(|(m, &(px, py))| {
                        let (px0, py0) =
                            path_j[if m == 0 { path_j.len() - 1 } else { m - 1 }];
                        !inside(px0, py0, x, y, px, py)
                    })
                {
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
        assert_eq!(paths.len(), 1, "expected one closed contour for overlapping slants");
        // The union covers edges a,h,g,f and has interior gap segments
        let path = &paths[0];
        assert!(path.len() >= 4, "path should have at least 4 vertices: {:?}", path);
        // Should contain the corner points of the union shape
        assert!(path.contains(&(0.0, 0.0)), "should contain (0,0): {:?}", path);
        assert!(path.contains(&(0.0, 1.0)), "should contain (0,1): {:?}", path);
    }

    #[test]
    fn multi_layer_with_offset() {
        // Two full pixels at different positions
        let grid_a = make_grid(1, 1, &[PX_ALMOSTFULL | PX_FULL]);
        let grid_b = make_grid(1, 1, &[PX_ALMOSTFULL | PX_FULL]);
        let paths = track_contour_multi(
            &[(&grid_a, 0, 0), (&grid_b, 0, 1)],
            PX_SUBPIXEL,
        );
        assert_eq!(paths.len(), 1, "two adjacent full pixels should produce one contour");
        let path = &paths[0];
        assert!(path.contains(&(0.0, 0.0)));
        assert!(path.contains(&(2.0, 0.0)));
        assert!(path.contains(&(2.0, 1.0)));
        assert!(path.contains(&(0.0, 1.0)));
    }
}
