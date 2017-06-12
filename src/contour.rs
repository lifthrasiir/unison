use std::collections::{HashSet, BTreeMap, BTreeSet};
use font::*;

type Adjacency = (u32, &'static [(u8, u8, u8, u8)]);

const ADJACENCY_SUBPIXEL: [Adjacency; 0x10] = [
    //    a   b
    //   +--+--+
    // h |     | c
    //   +     +   <---+
    // g |     | d     |
    //   +--+--+  (adjacency) (the line segment corresponding to the gap)
    //    f   e       abcdefgh;  [(start x*2, y*2, end x*2, y*2), ...]
    /*PX_EMPTY*/    (0b00000000, &[]),
    /*PX_HALF1*/    (0b00001111, &[(0, 0, 2, 2)]),
    /*PX_HALF3*/    (0b11000011, &[(0, 2, 2, 0)]),
    /*PX_QUAD1*/    (0b00000011, &[(0, 0, 1, 1), (1, 1, 0, 2)]),
    /*PX_QUAD2*/    (0b11000000, &[(0, 0, 1, 1), (1, 1, 2, 0)]),
    /*PX_QUAD3*/    (0b00110000, &[(2, 0, 1, 1), (1, 1, 2, 2)]),
    /*PX_QUAD4*/    (0b00001100, &[(0, 2, 1, 1), (1, 1, 2, 2)]),
    /*PX_SLANT1H*/  (0b00000111, &[(0, 0, 1, 2)]),
    /*PX_SLANT2H*/  (0b01110000, &[(1, 0, 2, 2)]),
    /*PX_SLANT3H*/  (0b10000011, &[(0, 2, 1, 0)]),
    /*PX_SLANT4H*/  (0b00111000, &[(1, 2, 2, 0)]),
    /*PX_SLANT1V*/  (0b00001110, &[(0, 1, 2, 2)]),
    /*PX_SLANT2V*/  (0b11100000, &[(0, 0, 2, 1)]),
    /*PX_SLANT3V*/  (0b11000001, &[(0, 1, 2, 0)]),
    /*PX_SLANT4V*/  (0b00011100, &[(0, 2, 2, 1)]),
    /*PX_DOT*/      (0b00000000, &[(0, 1, 1, 0), (1, 0, 2, 1), (2, 1, 1, 2), (1, 2, 0, 1)]),
];

fn subpixel_adjacency(px: u8) -> Adjacency {
    let (px, adjmask) = if (px & 0x10) != 0 {
        (px ^ 0x1f, 0b11111111)
    } else {
        (px, 0)
    };
    let (adj, segs) = ADJACENCY_SUBPIXEL[(px & 0x0f) as usize];
    (adj ^ adjmask, segs)
}

fn fullpixel_adjacency(px: u8) -> Adjacency {
    const EMPTY_SEGS: &'static [(u8, u8, u8, u8)] = &[];
    if (px & PX_FULL) != 0 {
        (0b11111111, EMPTY_SEGS)
    } else {
        (0b00000000, EMPTY_SEGS)
    }
}

fn signed_area(path: &[(i32, i32)]) -> i32 {
    let &(mut x0, mut y0) = path.last().expect("path should be non-empty");
    let mut area = 0;
    for &(x, y) in path {
        area += x0 * y - x * y0;
        x0 = x;
        y0 = y;
    }
    area
}

// 3-----2   2-----3
//       |   |         1--2--3
//       1   1
// ccw > 0   ccw < 0   ccw = 0
fn ccw(x1: i32, y1: i32, x2: i32, y2: i32, x3: i32, y3: i32) -> i32 {
    (x2 - x1) * (y3 - y1) - (y2 - y1) * (x3 - x1)
}

fn inside(x1: i32, y1: i32, x: i32, y: i32, x2: i32, y2: i32) -> bool {
    // i.e. colinear and (x,y) is inside a rectangle formed by (x1,y1) and (x2,y2)
    ccw(x1, y1, x, y, x2, y2) == 0 && ((x1 <= x && x <= x2) || (x1 >= x && x >= x2))
                                   && ((y1 <= y && y <= y2) || (y1 >= y && y >= y2))
}

// http://geomalgorithms.com/a03-_inclusion.html
fn winding_number(x: i32, y: i32, path: &[(i32, i32)], pshift: usize) -> i32 {
    let &(mut xx0, mut yy0) = path.last().expect("path should be non-empty");
    xx0 <<= pshift;
    yy0 <<= pshift;

    let mut wn = 0;
    for &(xx, yy) in path {
        let xx = xx << pshift;
        let yy = yy << pshift;
        if yy0 <= y {
            if yy > y && ccw(xx0, yy0, xx, yy, x, y) > 0 { wn += 1; }
        } else {
            if yy <= y && ccw(xx0, yy0, xx, yy, x, y) < 0 { wn -= 1; }
        }
        xx0 = xx;
        yy0 = yy;
    }
    wn
}

fn track_contour(height: u32, width: u32, stride: usize, data: &[u8],
                 adjacency: fn(u8) -> Adjacency, shift: usize) -> Vec<Vec<(i32, i32)>> {
    assert!(shift >= 1, "shift should be nonzero due to the presence of half-integer coordinates");

    let height = height as usize;
    let width = width as usize;

    let mut paths = Vec::new();
    let mut visited = HashSet::new();
    for y in 0..height {
        for x in 0..width {
            let i = y * stride + x;

            // find the first non-empty pixel unvisited
            if data[i] == PX_EMPTY {
                continue;
            }
            if visited.contains(&i) {
                continue;
            }

            let mut unsure = BTreeSet::new();
            unsure.insert((i, y, x));
            let mut segs = Vec::new();
            while let Some(&(i, y, x)) = unsure.iter().next() {
                unsure.remove(&(i, y, x));
                visited.insert(i);

                let (pixel, gapsegs) = adjacency(data[i]);
                let top    = if y > 0        { adjacency(data[i-stride]).0 } else { 0 };
                let bottom = if y < height-1 { adjacency(data[i+stride]).0 } else { 0 };
                let left   = if x > 0        { adjacency(data[i-1]).0      } else { 0 };
                let right  = if x < width-1  { adjacency(data[i+1]).0      } else { 0 };

                let connected = (pixel & (top    << 5) & 0b10000000) |
                                (pixel & (top    << 3) & 0b01000000) |
                                (pixel & (right  << 5) & 0b00100000) |
                                (pixel & (right  << 3) & 0b00010000) |
                                (pixel & (bottom >> 3) & 0b00001000) |
                                (pixel & (bottom >> 5) & 0b00000100) |
                                (pixel & (left   >> 3) & 0b00000010) |
                                (pixel & (left   >> 5) & 0b00000001);

                if (connected & 0b11000000) != 0 && !visited.contains(&(i-stride)) {
                    unsure.insert((i-stride, y-1, x));
                }
                if (connected & 0b00110000) != 0 && !visited.contains(&(i+1)) {
                    unsure.insert((i+1, y, x+1));
                }
                if (connected & 0b00001100) != 0 && !visited.contains(&(i+stride)) {
                    unsure.insert((i+stride, y+1, x));
                }
                if (connected & 0b00000011) != 0 && !visited.contains(&(i-1)) {
                    unsure.insert((i-1, y, x-1));
                }

                let x = (x as i32) << shift;
                let y = (y as i32) << shift;
                let one = 1 << shift;
                let half = 1 << (shift-1);
                let disconnected = connected ^ 0b11111111;
                if disconnected != 0 {
                    let linesegs = pixel & disconnected;
                    if (linesegs & 0b11000000) == 0b11000000 {
                        segs.push((x, y, x + one, y))
                    } else {
                        if linesegs & 0b10000000 != 0 {
                            segs.push((x, y, x + half, y));
                        }
                        if linesegs & 0b01000000 != 0 {
                            segs.push((x + half, y, x + one, y));
                        }
                    }
                    if (linesegs & 0b00110000) == 0b00110000 {
                        segs.push((x + one, y, x + one, y + one));
                    } else {
                        if linesegs & 0b00100000 != 0 {
                            segs.push((x + one, y, x + one, y + half));
                        }
                        if linesegs & 0b00010000 != 0 {
                            segs.push((x + one, y + half, x + one, y + one));
                        }
                    }
                    if (linesegs & 0b00001100) == 0b00001100 {
                        segs.push((x + one, y + one, x, y + one));
                    } else {
                        if linesegs & 0b00001000 != 0 {
                            segs.push((x + one, y + one, x + half, y + one));
                        }
                        if linesegs & 0b00000100 != 0 {
                            segs.push((x + half, y + one, x, y + one));
                        }
                    }
                    if (linesegs & 0b00000011) == 0b00000011 {
                        segs.push((x, y + one, x, y));
                    } else {
                        if linesegs & 0b00000010 != 0 {
                            segs.push((x, y + one, x, y + half));
                        }
                        if linesegs & 0b00000001 != 0 {
                            segs.push((x, y + half, x, y));
                        }
                    }

                    if !pixel & disconnected != 0 {
                        for &(x1, y1, x2, y2) in gapsegs {
                            segs.push((
                                x + ((x1 as i32) << (shift-1)),
                                y + ((y1 as i32) << (shift-1)),
                                x + ((x2 as i32) << (shift-1)),
                                y + ((y2 as i32) << (shift-1)),
                            ));
                        }
                    }
                }
            }

            let mut pxtosegs = BTreeMap::new();
            for (x1, y1, x2, y2) in segs {
                pxtosegs.entry((x1, y1)).or_insert(Vec::new()).push((x2, y2));
                pxtosegs.entry((x2, y2)).or_insert(Vec::new()).push((x1, y1));
            }

            while let Some(&(mut x0, mut y0)) = pxtosegs.keys().next() {
                let (mut x, mut y) = {
                    let v = pxtosegs.get_mut(&(x0, y0)).expect("???");
                    assert!(v.len() >= 2);
                    v.pop().unwrap()
                };

                let xorg = x0;
                let yorg = y0;
                let mut dx = x0 - x;
                let mut dy = y0 - y;
                let mut path = vec![(xorg, yorg)];
                let mut indices = vec![((xorg, yorg), 0)];
                loop {
                    let mut nx = pxtosegs.remove(&(x, y)).expect("odd number of neighboring edges");
                    let first = nx.iter().position(|e| *e == (x0, y0)).expect("no matching vertex");
                    nx.remove(first);

                    // check for the cycle...
                    if let Some(&(_, k)) = indices.iter().find(|e| e.0 == (x, y)) {
                        // then extract that cycle.
                        // should re-add the first point if it were elided previously.
                        let mut extracted: Vec<_> = path.drain(k..).collect();
                        if extracted[0] != (x, y) {
                            extracted.insert(0, (x, y));
                        }
                        paths.push(extracted);

                        // if nothing remains, we've reached the initial point.
                        if path.is_empty() {
                            if !nx.is_empty() {
                                pxtosegs.insert((x, y), nx);
                            }
                            break;
                        }

                        indices.retain(|&(_, v)| v < path.len());

                        // simulate the state right after (x, y) was originally added
                        let &(px, py) = path.last().unwrap();
                        dx = x - px;
                        dy = y - py;
                    }

                    let (xx, yy) = nx.remove(0);
                    if !nx.is_empty() {
                        pxtosegs.insert((x, y), nx);
                    }

                    // flush the previous segment if the current segment has a different slope
                    indices.push(((x, y), path.len()));
                    if dx * (y - yy) != dy * (x - xx) {
                        path.push((x, y));
                        dx = x - xx;
                        dy = y - yy;
                    }

                    x0 = x;
                    y0 = y;
                    x = xx;
                    y = yy;
                }
            }
        }
    }

    // lg(the scaling factor), explained later
    const LGMICRO: usize = 10;

    // fix the winding directions of resulting contours
    for i in 0..paths.len() {
        let mut wn = 0;
        {
            // pick any point in the path, and sum the winding numbers of that point in other paths.
            // the point should be not in other paths being compared.
            let path = &paths[i];
            for (j, p) in paths.iter().enumerate() {
                if i == j { continue; } // "any" point would always coincide

                let mut xy = None;
                let np = path.len();
                for k in 0..np {
                    // it is possible that every point in `path` overlaps with `p`
                    // while they don't share any common points themselves.
                    // thus we cheat here and try to pick a point that is not in `path`
                    // while it *does* overlap with `path` and is very unlikely to overlap with `p`.
                    // this is ensured by using, eh, a very small amount of interpolation.
                    // of course, we can do better, but who cares?
                    let (x1, y1) = path[if k > 0 { k-1 } else { np-1 }];
                    let (x2, y2) = path[k];
                    let x = x1 + x2 * ((1 << LGMICRO) - 1);
                    let y = y1 + y2 * ((1 << LGMICRO) - 1);

                    // even if (x, y) is not in the path itself, it may coincide with some segment
                    let coincide = |i| {
                        let (x1, y1) = p[if i > 0 { i-1 } else { p.len()-1 }];
                        let (x2, y2) = p[i];
                        inside(x1 << LGMICRO, y1 << LGMICRO, x, y, x2 << LGMICRO, y2 << LGMICRO)
                    };
                    if (0..p.len()).all(|i| !coincide(i)) {
                        xy = Some((x, y));
                        break;
                    }
                }

                let (xx, yy) = xy.expect("can't find a good point to calculate winding number");
                wn += winding_number(xx, yy, p, LGMICRO);
            }
        }

        // if a sign of the signed area mismatches with the winding number, reverse.
        // (the signed area only works with the simple polygon, which is the case)
        let a = signed_area(&paths[i]);
        if ((wn & 1) == 1) ^ (a < 0) {
            paths[i].reverse();
        }
    }

    paths
}

pub fn track_subpixel_contour(height: u32, width: u32, stride: usize, data: &[u8],
                              shift: usize) -> Vec<Vec<(i32, i32)>> {
    track_contour(height, width, stride, data, subpixel_adjacency, shift)
}

pub fn track_fullpixel_contour(height: u32, width: u32, stride: usize, data: &[u8],
                               shift: usize) -> Vec<Vec<(i32, i32)>> {
    track_contour(height, width, stride, data, fullpixel_adjacency, shift)
}

