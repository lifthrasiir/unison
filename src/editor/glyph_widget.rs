use crate::pixel::{self, PixelShape};

/// Draw one cell of a grid, dispatching custom detail cells to their exact
/// stored geometry.
pub fn draw_grid_cell_colored(
    painter: &egui::Painter,
    rect: egui::Rect,
    grid: &crate::document::PixelGrid,
    row: u16,
    col: u16,
    color: egui::Color32,
) {
    let shape = grid.get(row, col);
    if shape.shape_id() == pixel::PX_CUSTOM {
        if let Some(region) = grid.details.get(&(row, col)) {
            draw_detail_region(painter, rect, region, color);
        }
        return;
    }
    draw_pixel_cell_colored(painter, rect, shape, color);
}

fn draw_detail_region(
    painter: &egui::Painter,
    rect: egui::Rect,
    region: &crate::detail::DetailRegion,
    color: egui::Color32,
) {
    if region.is_empty() {
        return;
    }
    let o = rect.min;
    let w = rect.width();
    let h = rect.height();
    let den = region.den.max(1) as f32;
    // NOTE: hole rings (regions with interior holes) are filled over; exact
    // even-odd meshing can be added if such details ever occur in practice.
    for ring in &region.rings {
        let pts: Vec<egui::Pos2> = ring
            .iter()
            .map(|&(x, y)| egui::pos2(o.x + x as f32 / den * w, o.y + y as f32 / den * h))
            .collect();
        if pts.len() < 3 {
            continue;
        }
        let sub_polys = split_at_pinch_points(&pts);
        let mut mesh = egui::Mesh::default();
        let white_uv = egui::pos2(0.0, 0.0);
        for poly in &sub_polys {
            for tri in &triangulate(poly) {
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
        }
        if !mesh.indices.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
        }
    }
}

pub fn draw_pixel_cell_colored(
    painter: &egui::Painter,
    rect: egui::Rect,
    shape: PixelShape,
    color: egui::Color32,
) {
    if shape.is_empty() {
        return;
    }

    if shape.is_hardblank() {
        draw_hardblank_cell(painter, rect, color);
        return;
    }

    let shape_id = shape.shape_id();

    if shape_id == pixel::PX_ALMOSTFULL {
        painter.rect_filled(rect.shrink(0.5), 0.0, color);
        return;
    }

    // Disconnected shapes need their parts drawn separately: the generic
    // edge-chaining in `polygon_from_adjacency` closes the ring at the first
    // pinch point and drops whatever comes after it.
    let parts = pixel::shape_parts(shape_id);
    if !parts.is_empty() {
        for &qid in parts {
            let (a, s) = pixel::adjacency(qid);
            let pts = build_shape_polygon(a, s, rect);
            if pts.len() >= 3 {
                let mut mesh = egui::Mesh::default();
                let white_uv = egui::pos2(0.0, 0.0);
                for tri in &triangulate(&pts) {
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
                if !mesh.indices.is_empty() {
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
        }
        return;
    }

    let (adj_bits, segs) = pixel::adjacency(shape_id);
    if adj_bits == 0 && segs.is_empty() {
        return;
    }

    let points = build_shape_polygon(adj_bits, segs, rect);

    if points.len() >= 3 {
        let sub_polys = split_at_pinch_points(&points);
        let mut mesh = egui::Mesh::default();
        let white_uv = egui::pos2(0.0, 0.0);
        for poly in &sub_polys {
            for tri in &triangulate(poly) {
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
        }
        if !mesh.indices.is_empty() {
            painter.add(egui::Shape::mesh(mesh));
        }
    }
}

/// A hardblank cell: diagonal stripes across the whole cell, since the cell
/// holds no geometry of its own to draw (see [`pixel::PX_HARDBLANK`]). The
/// stripes run bottom-left to top-right on the `u + v = c` diagonals of the
/// unit cell, so they tile continuously across a run of hardblanks.
fn draw_hardblank_cell(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    /// Diagonal spacing, as a fraction of the cell — three stripes per cell,
    /// one of them corner to corner.
    const STEP: f32 = 0.5;

    let (w, h) = (rect.width(), rect.height());
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let width = (w.min(h) * 0.08).clamp(0.5, 1.5);
    let stroke = egui::Stroke::new(width, color);
    let at = |u: f32, v: f32| egui::pos2(rect.min.x + u * w, rect.min.y + v * h);

    let mut c = STEP;
    while c < 2.0 {
        // Clip `u + v = c` to the unit square: it enters on the left/bottom
        // edge and leaves on the top/right one.
        let (a, b) = if c <= 1.0 {
            (at(0.0, c), at(c, 0.0))
        } else {
            (at(c - 1.0, 1.0), at(1.0, c - 1.0))
        };
        painter.line_segment([a, b], stroke);
        c += STEP;
    }
}

fn split_at_pinch_points(points: &[egui::Pos2]) -> Vec<Vec<egui::Pos2>> {
    let n = points.len();
    let eps = 0.01;
    for i in 0..n {
        for j in i + 2..n {
            if (points[i].x - points[j].x).abs() < eps && (points[i].y - points[j].y).abs() < eps {
                let sub_a: Vec<_> = points[i..j].to_vec();
                let mut sub_b: Vec<_> = points[j..].to_vec();
                sub_b.extend_from_slice(&points[..i]);
                let mut result = split_at_pinch_points(&sub_a);
                result.extend(split_at_pinch_points(&sub_b));
                return result;
            }
        }
    }
    vec![points.to_vec()]
}

fn build_shape_polygon(
    adj_bits: u8,
    gap_segs: &[(f32, f32, f32, f32)],
    rect: egui::Rect,
) -> Vec<egui::Pos2> {
    let o = rect.min;
    let w = rect.width();
    let h = rect.height();
    pixel::polygon_from_adjacency(adj_bits, gap_segs)
        .into_iter()
        .map(|(x, y)| egui::pos2(o.x + x * w, o.y + y * h))
        .collect()
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

pub fn all_valid_shapes() -> &'static [PixelShape] {
    static SHAPES: std::sync::LazyLock<Vec<PixelShape>> =
        std::sync::LazyLock::new(build_all_valid_shapes);
    &SHAPES
}

fn build_all_valid_shapes() -> Vec<PixelShape> {
    use pixel::*;
    // Row 0 (16): almostfull, dot, hquad, vquad, halves, corners, invcorners
    let mut s = vec![
        PixelShape::new(PX_ALMOSTFULL, true),
        PixelShape::new(PX_DOT, false),
        PixelShape::new(PX_HQUAD, true),
        PixelShape::new(PX_VQUAD, true),
    ];
    for &id in &[PX_HALF3, PX_HALF2, PX_HALF4, PX_HALF1] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_CORNER3, PX_CORNER2, PX_CORNER4, PX_CORNER1] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_INVCORNER3, PX_INVCORNER2, PX_INVCORNER4, PX_INVCORNER1] {
        s.push(PixelShape::new(id, true));
    }

    // Row 1 (16): quads, cones, invquads, invcones (all filled)
    for &id in &[PX_QUAD2, PX_QUAD3, PX_QUAD4, PX_QUAD1] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_CONE2, PX_CONE3, PX_CONE4, PX_CONE1] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_INVQUAD2, PX_INVQUAD3, PX_INVQUAD4, PX_INVQUAD1] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_INVCONE2, PX_INVCONE3, PX_INVCONE4, PX_INVCONE1] {
        s.push(PixelShape::new(id, true));
    }

    // Row 2 (16): halfslant H (filled), slant H (unfilled),
    //             halfslant V (filled), slant V (unfilled)
    for &id in &[
        PX_HALFSLANT3H,
        PX_HALFSLANT2H,
        PX_HALFSLANT4H,
        PX_HALFSLANT1H,
    ] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_SLANT3H, PX_SLANT2H, PX_SLANT4H, PX_SLANT1H] {
        s.push(PixelShape::new(id, false));
    }
    for &id in &[
        PX_HALFSLANT3V,
        PX_HALFSLANT2V,
        PX_HALFSLANT4V,
        PX_HALFSLANT1V,
    ] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_SLANT3V, PX_SLANT2V, PX_SLANT4V, PX_SLANT1V] {
        s.push(PixelShape::new(id, false));
    }

    // Row 3 (12): DOT plus two corners — the diagonals and the houses, then
    // their complements (the two corners that were left out).
    for &id in &[PX_SLASH, PX_BACKSLASH] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_HOUSE2, PX_HOUSE3, PX_HOUSE4, PX_HOUSE1] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_INVSLASH, PX_INVBACKSLASH] {
        s.push(PixelShape::new(id, true));
    }
    for &id in &[PX_INVHOUSE2, PX_INVHOUSE3, PX_INVHOUSE4, PX_INVHOUSE1] {
        s.push(PixelShape::new(id, true));
    }

    s
}

/// The palette is *rotation-invariant*: it lists one representative per
/// 90°-rotation orbit of [`all_valid_shapes`], and the current rotation is
/// remembered next to it (`EditorState::shape_rotation`) rather than being part
/// of the selected shape. Every catalog shape is therefore reached as
/// "representative + rotation", which is why the palette has 18 cells for 60
/// shapes: the orbits have periods 1 (`PX_ALMOSTFULL`, `PX_DOT`), 2
/// (`PX_HQUAD`/`PX_VQUAD`, `PX_SLASH`/`PX_BACKSLASH` and their complements)
/// and 4 (everything else).
///
/// Orbits are derived from [`all_valid_shapes`] rather than spelled out, so the
/// palette follows that list — adding a shape there adds it here, either as a
/// new representative or as a rotation of an existing one.
struct Orbits {
    /// Representatives, in `all_valid_shapes()` order.
    reps: Vec<PixelShape>,
    /// Number of distinct shapes each representative rotates through (1, 2, 4).
    periods: Vec<u32>,
    /// Shape id (the fill bit ignored — rotation never touches it) →
    /// (representative index, clockwise steps from that representative).
    by_id: std::collections::HashMap<u8, (usize, u32)>,
    /// First representative of the palette's second row; see
    /// [`palette_row_break`].
    row_break: usize,
}

fn orbits() -> &'static Orbits {
    static ORBITS: std::sync::LazyLock<Orbits> = std::sync::LazyLock::new(build_orbits);
    &ORBITS
}

fn build_orbits() -> Orbits {
    let mut reps: Vec<PixelShape> = Vec::new();
    let mut periods: Vec<u32> = Vec::new();
    let mut by_id: std::collections::HashMap<u8, (usize, u32)> = std::collections::HashMap::new();

    for &shape in all_valid_shapes() {
        if by_id.contains_key(&shape.shape_id()) {
            continue;
        }
        let idx = reps.len();
        let mut ids: Vec<u8> = Vec::new();
        let mut cur = shape;
        while !ids.contains(&cur.shape_id()) {
            ids.push(cur.shape_id());
            cur = cur.rotate_cw();
        }
        for (step, id) in ids.iter().enumerate() {
            by_id.insert(*id, (idx, step as u32));
        }
        reps.push(shape);
        periods.push(ids.len() as u32);
    }

    let row_break = reps
        .iter()
        .position(|s| s.is_slant_pair())
        .unwrap_or(reps.len());

    Orbits {
        reps,
        periods,
        by_id,
        row_break,
    }
}

/// One shape per rotation orbit: the cells of the shape palette.
pub fn palette_shapes() -> &'static [PixelShape] {
    &orbits().reps
}

/// Which palette cell a shape belongs to, and how many clockwise 90° steps
/// separate it from that cell's representative. `None` for shapes outside the
/// catalog (`PX_CUSTOM`, `PX_EMPTY`).
pub fn shape_orbit(shape: PixelShape) -> Option<(usize, u32)> {
    orbits().by_id.get(&shape.shape_id()).copied()
}

/// How many distinct shapes palette cell `idx` rotates through (1, 2 or 4).
pub fn orbit_period(idx: usize) -> u32 {
    orbits().periods.get(idx).copied().unwrap_or(1)
}

/// `shape` rotated `steps` × 90° clockwise (negative for counter-clockwise).
/// The fill bit rides along untouched.
pub fn rotate_shape(shape: PixelShape, steps: i32) -> PixelShape {
    let mut out = shape;
    for _ in 0..steps.rem_euclid(4) {
        out = out.rotate_cw();
    }
    out
}

/// Adopt the rotation implied by a shape that was chosen by some other route —
/// a keyboard shortcut or the slant re-paint toggle, both of which name an
/// absolute orientation. The rotation only moves when it actually disagrees:
/// for an orbit of period 1 or 2 the shape says nothing about the remaining
/// quarter turns, and those are what the *other* palette cells rotate by.
pub fn sync_rotation(shape: PixelShape, rotation: &mut u32) {
    if let Some((idx, rot)) = shape_orbit(shape) {
        let period = orbit_period(idx);
        if *rotation % period != rot {
            *rotation = rot;
        }
    }
}

/// One wheel notch over the palette or the grid: rotate the whole palette, or
/// — with shift held — step to the neighbouring palette cell, keeping the
/// rotation. Rotation and shape choice are orthogonal; this is the only place
/// that moves either of them together.
pub fn wheel_step_shape(
    selected: &mut PixelShape,
    rotation: &mut u32,
    step: i32,
    select_shape: bool,
) {
    if select_shape {
        let reps = palette_shapes();
        let cur = shape_orbit(*selected).map_or(0, |(idx, _)| idx);
        let next = (cur as i32 + step).clamp(0, reps.len() as i32 - 1) as usize;
        *selected = rotate_shape(reps[next], *rotation as i32);
    } else {
        *rotation = (*rotation as i32 + step).rem_euclid(4) as u32;
        *selected = rotate_shape(*selected, step);
    }
}

/// The palette wraps into two rows, and the break is a *family* boundary
/// rather than a column count: the whole-cell shapes (halves, corners, quads,
/// cones) stay on the first row, and the second starts at the slants.
fn palette_row_break() -> usize {
    orbits().row_break
}

pub fn palette_cols() -> usize {
    let brk = palette_row_break();
    brk.max(palette_shapes().len() - brk)
}

pub fn palette_row_col(idx: usize) -> (usize, usize) {
    let brk = palette_row_break();
    if idx < brk { (0, idx) } else { (1, idx - brk) }
}

pub fn palette_rows() -> usize {
    if palette_shapes().len() > palette_row_break() {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::*;

    #[test]
    fn palette_holds_one_cell_per_rotation_orbit() {
        let reps = palette_shapes();
        assert_eq!(reps.len(), 18, "orbits: {reps:?}");
        // Two rows: the whole-cell shapes, then the slants and the
        // DOT+corner shapes.
        assert_eq!(palette_rows(), 2);
        assert_eq!(palette_cols(), 10);
        assert_eq!(palette_row_col(9), (0, 9));
        assert_eq!(palette_row_col(10), (1, 0));
        assert!(reps[10].is_slant_pair(), "row 1 starts at the slants");
        // Periods: PX_ALMOSTFULL and PX_DOT are fixed; HQUAD/VQUAD and the two
        // diagonal pairs (SLASH/BACKSLASH, INVSLASH/INVBACKSLASH) alternate;
        // the remaining thirteen turn through four orientations.
        let mut by_period = [0usize; 5];
        for idx in 0..reps.len() {
            by_period[orbit_period(idx) as usize] += 1;
        }
        assert_eq!((by_period[1], by_period[2], by_period[4]), (2, 3, 13));
    }

    #[test]
    fn every_catalog_shape_is_a_rotation_of_a_palette_cell() {
        for &shape in all_valid_shapes() {
            let (idx, rot) = shape_orbit(shape).expect("catalog shape has an orbit");
            assert_eq!(
                rotate_shape(palette_shapes()[idx], rot as i32).shape_id(),
                shape.shape_id(),
                "{shape:?} is not cell {idx} rotated {rot}×90°"
            );
        }
        // ... and nothing outside the catalog sneaks in: 18 cells × their
        // periods must be exactly the 60 shapes.
        let reached: std::collections::HashSet<u8> = (0..palette_shapes().len())
            .flat_map(|idx| (0..4).map(move |r| rotate_shape(palette_shapes()[idx], r).shape_id()))
            .collect();
        let catalog: std::collections::HashSet<u8> =
            all_valid_shapes().iter().map(|s| s.shape_id()).collect();
        assert_eq!(reached, catalog);
    }

    #[test]
    fn rotation_survives_a_shape_change_and_vice_versa() {
        let mut shape = PixelShape::new(PX_ALMOSTFULL, true);
        let mut rot = 0;
        // Two notches of plain wheel: rotation only.
        wheel_step_shape(&mut shape, &mut rot, 1, false);
        wheel_step_shape(&mut shape, &mut rot, 1, false);
        assert_eq!(rot, 2);
        // Shift+wheel picks another cell at the same rotation.
        wheel_step_shape(&mut shape, &mut rot, 1, true);
        assert_eq!(rot, 2, "the shape change must not disturb the rotation");
        let (idx, _) = shape_orbit(shape).unwrap();
        assert_eq!(shape, rotate_shape(palette_shapes()[idx], 2));
    }

    #[test]
    fn an_absolute_shape_choice_adopts_its_own_rotation() {
        let mut rot = 3;
        // A period-4 shape pins the rotation...
        let half3 = PixelShape::new(PX_HALF3, true);
        sync_rotation(half3, &mut rot);
        assert_eq!(rot, shape_orbit(half3).unwrap().1);
        // ...while a rotation-invariant one leaves it alone.
        let before = rot;
        sync_rotation(PixelShape::new(PX_ALMOSTFULL, true), &mut rot);
        assert_eq!(rot, before);
    }

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

#[cfg(test)]
mod dot_render_tests {
    use super::*;
    use crate::pixel::{PX_DOT, adjacency};

    /// PX_DOT must render as the same edge-midpoint diamond the font builder
    /// emits (`detail.rs::base_shape_rings`), not as a circle.
    #[test]
    fn dot_renders_as_full_cell_diamond() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0));
        let (adj, segs) = adjacency(PX_DOT);
        let pts = build_shape_polygon(adj, segs, rect);
        assert_eq!(pts.len(), 4, "dot polygon: {pts:?}");
        for want in [(5.0, 0.0), (10.0, 5.0), (5.0, 10.0), (0.0, 5.0)] {
            assert!(
                pts.iter().any(|p| near((p.x, p.y), want)),
                "dot polygon missing {want:?}: {pts:?}"
            );
        }
        let area: f32 = triangulate(&pts)
            .iter()
            .map(|t| cross_2d(t[0], t[1], t[2]).abs() / 2.0)
            .sum();
        assert!((area - 50.0).abs() < 0.01, "dot area: {area}");
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }
}
