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
}
