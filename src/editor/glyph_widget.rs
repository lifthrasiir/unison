use crate::pixel::{self, PixelShape};

pub fn draw_pixel_cell_colored(
    painter: &egui::Painter,
    rect: egui::Rect,
    shape: PixelShape,
    color_override: Option<egui::Color32>,
) {
    if shape.is_empty() {
        return;
    }

    let color = color_override.unwrap_or(if shape.is_filled() {
        egui::Color32::from_rgb(210, 215, 230)
    } else {
        egui::Color32::from_rgba_unmultiplied(210, 215, 230, 89)
    });

    let shape_id = shape.shape_id();

    if shape_id == pixel::PX_ALMOSTFULL {
        painter.rect_filled(rect.shrink(0.5), 0.0, color);
        return;
    }

    if shape_id == pixel::PX_DOT {
        painter.circle_filled(rect.center(), rect.width() * 0.2, color);
        return;
    }

    let (adj_bits, segs) = pixel::adjacency(shape_id);
    if adj_bits == 0 && segs.is_empty() {
        return;
    }

    let points = build_shape_polygon(adj_bits, segs, rect);

    if points.len() >= 3 {
        let triangles = triangulate(&points);
        if !triangles.is_empty() {
            let mut mesh = egui::Mesh::default();
            let white_uv = egui::pos2(0.0, 0.0);
            for tri in &triangles {
                let base = mesh.vertices.len() as u32;
                for &p in tri {
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: p,
                        uv: white_uv,
                        color,
                    });
                }
                mesh.indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
            painter.add(egui::Shape::mesh(mesh));
        }
    }
}

fn build_shape_polygon(
    adj_bits: u8,
    gap_segs: &[(f32, f32, f32, f32)],
    rect: egui::Rect,
) -> Vec<egui::Pos2> {
    let o = rect.min;
    let w = rect.width();
    let h = rect.height();

    if adj_bits == 0xFF {
        return vec![
            egui::pos2(o.x, o.y),
            egui::pos2(o.x + w, o.y),
            egui::pos2(o.x + w, o.y + h),
            egui::pos2(o.x, o.y + h),
        ];
    }

    //   TL --(a)-- TM --(b)-- TR
    //  (h)                    (c)
    //   LM                    RM
    //  (g)                    (d)
    //   BL --(f)-- BM --(e)-- BR
    // Bits: a=7, b=6, c=5, d=4, e=3, f=2, g=1, h=0

    // Directed boundary segments for each half-edge (clockwise)
    let boundary: [(u8, [f32; 4]); 8] = [
        (7, [0.0, 0.0, 0.5, 0.0]), // a: TL→TM
        (6, [0.5, 0.0, 1.0, 0.0]), // b: TM→TR
        (5, [1.0, 0.0, 1.0, 0.5]), // c: TR→RM
        (4, [1.0, 0.5, 1.0, 1.0]), // d: RM→BR
        (3, [1.0, 1.0, 0.5, 1.0]), // e: BR→BM
        (2, [0.5, 1.0, 0.0, 1.0]), // f: BM→BL
        (1, [0.0, 1.0, 0.0, 0.5]), // g: BL→LM
        (0, [0.0, 0.5, 0.0, 0.0]), // h: LM→TL
    ];

    // Collect all directed edges: set boundary + gap segments (both directions)
    let mut edges: Vec<[f32; 4]> = Vec::new();

    for &(bit, seg) in &boundary {
        if adj_bits & (1 << bit) != 0 {
            edges.push(seg);
        }
    }

    for &(x1, y1, x2, y2) in gap_segs {
        edges.push([x1, y1, x2, y2]);
        edges.push([x2, y2, x1, y1]);
    }

    if edges.is_empty() {
        return vec![];
    }

    // Chain edges into a closed polygon
    let mut used = vec![false; edges.len()];

    // Start from the first boundary segment
    used[0] = true;
    mark_reverse(&edges, &mut used, 0);
    let mut polygon = vec![(edges[0][0], edges[0][1])];
    let mut cur = (edges[0][2], edges[0][3]);

    for _ in 0..edges.len() {
        if near(cur, polygon[0]) {
            break;
        }
        let mut found = false;
        for (i, e) in edges.iter().enumerate() {
            if !used[i] && near((e[0], e[1]), cur) {
                used[i] = true;
                mark_reverse(&edges, &mut used, i);
                polygon.push((e[0], e[1]));
                cur = (e[2], e[3]);
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }

    // Remove consecutive duplicates
    polygon.dedup_by(|a, b| near(*a, *b));

    polygon
        .into_iter()
        .map(|(x, y)| egui::pos2(o.x + x * w, o.y + y * h))
        .collect()
}

fn near(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.001 && (a.1 - b.1).abs() < 0.001
}

fn mark_reverse(edges: &[[f32; 4]], used: &mut [bool], idx: usize) {
    let e = edges[idx];
    for (j, other) in edges.iter().enumerate() {
        if !used[j]
            && near((other[0], other[1]), (e[2], e[3]))
            && near((other[2], other[3]), (e[0], e[1]))
        {
            used[j] = true;
            return;
        }
    }
}

fn triangulate(points: &[egui::Pos2]) -> Vec<[egui::Pos2; 3]> {
    let n = points.len();
    if n < 3 {
        return vec![];
    }
    if n == 3 {
        return vec![[points[0], points[1], points[2]]];
    }

    let mut result = Vec::new();
    let mut remaining: Vec<usize> = (0..n).collect();

    // Determine winding: positive signed area = CCW in screen coords (Y-down = CW visually)
    let area = signed_area_2d(points);
    let expect_positive = area > 0.0;

    for _ in 0..n * 2 {
        if remaining.len() <= 3 {
            break;
        }
        let rn = remaining.len();
        let mut found = false;
        for i in 0..rn {
            let pi = remaining[(i + rn - 1) % rn];
            let ci = remaining[i];
            let ni = remaining[(i + 1) % rn];

            let cross = cross_2d(points[pi], points[ci], points[ni]);
            let is_convex = if expect_positive {
                cross > 0.0
            } else {
                cross < 0.0
            };
            if !is_convex {
                continue;
            }

            let mut is_ear = true;
            for &vi in remaining.iter().take(rn) {
                if vi == pi || vi == ci || vi == ni {
                    continue;
                }
                if point_in_tri(points[vi], points[pi], points[ci], points[ni]) {
                    is_ear = false;
                    break;
                }
            }

            if is_ear {
                result.push([points[pi], points[ci], points[ni]]);
                remaining.remove(i);
                found = true;
                break;
            }
        }
        if !found {
            break;
        }
    }

    if remaining.len() == 3 {
        result.push([
            points[remaining[0]],
            points[remaining[1]],
            points[remaining[2]],
        ]);
    }
    result
}

fn signed_area_2d(pts: &[egui::Pos2]) -> f32 {
    let n = pts.len();
    let mut area = 0.0f32;
    for i in 0..n {
        let j = (i + 1) % n;
        area += pts[i].x * pts[j].y - pts[j].x * pts[i].y;
    }
    area
}

fn cross_2d(a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn point_in_tri(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2, c: egui::Pos2) -> bool {
    let d1 = cross_2d(a, b, p);
    let d2 = cross_2d(b, c, p);
    let d3 = cross_2d(c, a, p);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

pub fn all_valid_shapes() -> Vec<PixelShape> {
    let mut shapes = Vec::new();

    // Filled shapes first (more commonly used)
    shapes.push(PixelShape::new(pixel::PX_ALMOSTFULL, true)); // @
    shapes.push(PixelShape::EMPTY); // .

    // Halves - filled
    for &id in &[
        pixel::PX_HALF1,
        pixel::PX_HALF2,
        pixel::PX_HALF3,
        pixel::PX_HALF4,
    ] {
        shapes.push(PixelShape::new(id, true));
    }
    // Halves - unfilled
    for &id in &[
        pixel::PX_HALF1,
        pixel::PX_HALF2,
        pixel::PX_HALF3,
        pixel::PX_HALF4,
    ] {
        shapes.push(PixelShape::new(id, false));
    }

    // Quads - filled
    for &id in &[
        pixel::PX_QUAD1,
        pixel::PX_QUAD2,
        pixel::PX_QUAD3,
        pixel::PX_QUAD4,
    ] {
        shapes.push(PixelShape::new(id, true));
    }
    // Quads - unfilled
    for &id in &[
        pixel::PX_QUAD1,
        pixel::PX_QUAD2,
        pixel::PX_QUAD3,
        pixel::PX_QUAD4,
    ] {
        shapes.push(PixelShape::new(id, false));
    }

    // InvQuads - filled
    for &id in &[
        pixel::PX_INVQUAD1,
        pixel::PX_INVQUAD2,
        pixel::PX_INVQUAD3,
        pixel::PX_INVQUAD4,
    ] {
        shapes.push(PixelShape::new(id, true));
    }
    // InvQuads - unfilled
    for &id in &[
        pixel::PX_INVQUAD1,
        pixel::PX_INVQUAD2,
        pixel::PX_INVQUAD3,
        pixel::PX_INVQUAD4,
    ] {
        shapes.push(PixelShape::new(id, false));
    }

    // Slants (unfilled only)
    for &id in &[
        pixel::PX_SLANT1H,
        pixel::PX_SLANT2H,
        pixel::PX_SLANT3H,
        pixel::PX_SLANT4H,
        pixel::PX_SLANT1V,
        pixel::PX_SLANT2V,
        pixel::PX_SLANT3V,
        pixel::PX_SLANT4V,
    ] {
        shapes.push(PixelShape::new(id, false));
    }

    // Halfslants (filled only)
    for &id in &[
        pixel::PX_HALFSLANT1H,
        pixel::PX_HALFSLANT2H,
        pixel::PX_HALFSLANT3H,
        pixel::PX_HALFSLANT4H,
        pixel::PX_HALFSLANT1V,
        pixel::PX_HALFSLANT2V,
        pixel::PX_HALFSLANT3V,
        pixel::PX_HALFSLANT4V,
    ] {
        shapes.push(PixelShape::new(id, true));
    }

    // Dot
    shapes.push(PixelShape::new(pixel::PX_DOT, false));

    shapes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::*;

    fn poly(adj: u8, segs: &[(f32, f32, f32, f32)]) -> Vec<(f32, f32)> {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0));
        build_shape_polygon(adj, segs, rect)
            .into_iter()
            .map(|p| (p.x, p.y))
            .collect()
    }

    #[test]
    fn half1_triangle() {
        // PX_HALF1: efgh=1, abcd=0, seg (0,0)→(1,1)
        // Should be bottom-left triangle: contains (0,0), (0,1), (1,1)
        let (adj, segs) = adjacency(PX_HALF1);
        let p = poly(adj, segs);
        assert!(p.len() >= 3, "half1 got {} points: {:?}", p.len(), p);
        assert!(
            p.iter().any(|v| near(*v, (0.0, 0.0))),
            "missing TL: {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (0.0, 1.0))),
            "missing BL: {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (1.0, 1.0))),
            "missing BR: {:?}",
            p
        );
    }

    #[test]
    fn half2_triangle() {
        // PX_HALF2 (complement of HALF1): abcd=1, efgh=0
        // Should be top-right triangle: contains (0,0), (1,0), (1,1)
        let (adj, segs) = adjacency(PX_HALF2);
        let p = poly(adj, segs);
        assert!(p.len() >= 3, "half2 got {} points: {:?}", p.len(), p);
        assert!(
            p.iter().any(|v| near(*v, (0.0, 0.0))),
            "missing TL: {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (1.0, 0.0))),
            "missing TR: {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (1.0, 1.0))),
            "missing BR: {:?}",
            p
        );
    }

    #[test]
    fn quad1_shape() {
        // PX_QUAD1: gh=1, seg (0,0)→(0.5,0.5) and (0.5,0.5)→(0,1)
        let (adj, segs) = adjacency(PX_QUAD1);
        let p = poly(adj, segs);
        assert!(p.len() >= 3, "quad1 got {} points: {:?}", p.len(), p);
        assert!(
            p.iter().any(|v| near(*v, (0.0, 0.0))),
            "missing (0,0): {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (0.5, 0.5))),
            "missing center: {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (0.0, 1.0))),
            "missing (0,1): {:?}",
            p
        );
    }

    #[test]
    fn slant1h_shape() {
        // PX_SLANT1H: bits=0b00000111, seg (0,0)→(0.5,1)
        // gfh=1, abcde=0. Triangle: (0,0), (0.5,1), (0,1), (0,0.5)
        let (adj, segs) = adjacency(PX_SLANT1H);
        let p = poly(adj, segs);
        assert!(p.len() >= 3, "slant1h got {} points: {:?}", p.len(), p);
        assert!(
            p.iter().any(|v| near(*v, (0.0, 0.0))),
            "missing (0,0): {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (0.5, 1.0))),
            "missing (0.5,1): {:?}",
            p
        );
        assert!(
            p.iter().any(|v| near(*v, (0.0, 1.0))),
            "missing (0,1): {:?}",
            p
        );
    }

    #[test]
    fn almostfull_square() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0));
        let p = build_shape_polygon(0xFF, &[], rect);
        assert_eq!(p.len(), 4);
    }

    #[test]
    fn invquad1_concave() {
        // InvQuad1 (28): full square minus left quarter-circle notch
        // Should have (0.5,0.5) as concave vertex, NOT be a full square
        let (adj, segs) = adjacency(PX_INVQUAD1);
        let p = poly(adj, segs);
        assert!(p.len() >= 5, "invquad1 should have >=5 points: {:?}", p);
        assert!(
            p.iter().any(|v| near(*v, (0.5, 0.5))),
            "missing center notch: {:?}",
            p
        );
        // Should NOT contain (0,0.5) as a filled boundary point
        // (since g and h bits are unset)

        // Triangulation should work
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0));
        let pts = build_shape_polygon(adj, segs, rect);
        let tris = triangulate(&pts);
        assert!(!tris.is_empty(), "triangulation failed for invquad1");
    }

    #[test]
    fn invquad2_concave() {
        let (adj, segs) = adjacency(PX_INVQUAD2);
        let p = poly(adj, segs);
        assert!(p.len() >= 5, "invquad2 should have >=5 points: {:?}", p);
        assert!(
            p.iter().any(|v| near(*v, (0.5, 0.5))),
            "missing center notch: {:?}",
            p
        );

        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1.0, 1.0));
        let pts = build_shape_polygon(adj, segs, rect);
        let tris = triangulate(&pts);
        assert!(!tris.is_empty(), "triangulation failed for invquad2");
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }
}
