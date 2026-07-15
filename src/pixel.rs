use std::fmt;

pub const PX_SUBPIXEL: u8 = 0x1f;
pub const PX_FULL: u8 = 0x20;

pub const PX_EMPTY: u8 = 0;
pub const PX_ALMOSTFULL: u8 = PX_SUBPIXEL;
pub const PX_HALF1: u8 = 1; //               |\ (b filled, \ unfilled)
pub const PX_HALF2: u8 = 1 ^ PX_SUBPIXEL; // \| (9 filled, \ unfilled)
pub const PX_HALF3: u8 = 2; //               |/ (P filled, / unfilled)
pub const PX_HALF4: u8 = 2 ^ PX_SUBPIXEL; // /| (d filled, / unfilled)
pub const PX_QUAD1: u8 = 3; //               |> () filled, > unfilled)
pub const PX_QUAD2: u8 = 4; //               v  (u filled, v unfilled)
pub const PX_QUAD3: u8 = 5; //               <| (( filled, < unfilled)
pub const PX_QUAD4: u8 = 6; //               ^  (n filled, ^ unfilled)
pub const PX_INVQUAD1: u8 = 3 ^ PX_SUBPIXEL;
pub const PX_INVQUAD2: u8 = 4 ^ PX_SUBPIXEL;
pub const PX_INVQUAD3: u8 = 5 ^ PX_SUBPIXEL;
pub const PX_INVQUAD4: u8 = 6 ^ PX_SUBPIXEL;
pub const PX_SLANT1H: u8 = 7;
pub const PX_SLANT2H: u8 = 8;
pub const PX_SLANT3H: u8 = 9;
pub const PX_SLANT4H: u8 = 10;
pub const PX_SLANT1V: u8 = 11;
pub const PX_SLANT2V: u8 = 12;
pub const PX_SLANT3V: u8 = 13;
pub const PX_SLANT4V: u8 = 14;
pub const PX_HALFSLANT1H: u8 = 8 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT2H: u8 = 7 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT3H: u8 = 10 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT4H: u8 = 9 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT1V: u8 = 12 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT2V: u8 = 11 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT3V: u8 = 14 ^ PX_SUBPIXEL;
pub const PX_HALFSLANT4V: u8 = 13 ^ PX_SUBPIXEL;
pub const PX_DOT: u8 = 15;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PixelShape(pub u8);

impl PixelShape {
    pub const EMPTY: Self = Self(PX_EMPTY);

    pub fn new(shape_id: u8, filled: bool) -> Self {
        debug_assert!(shape_id < 32);
        Self(shape_id | if filled { PX_FULL } else { 0 })
    }

    pub fn shape_id(self) -> u8 {
        self.0 & PX_SUBPIXEL
    }

    pub fn is_filled(self) -> bool {
        self.0 & PX_FULL != 0
    }

    pub fn is_empty(self) -> bool {
        self.shape_id() == PX_EMPTY && !self.is_filled()
    }

}

impl fmt::Debug for PixelShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PixelShape({}, filled={})",
            self.shape_id(),
            self.is_filled()
        )
    }
}

type Seg = (f32, f32, f32, f32);

struct AdjEntry {
    bits: u8,
    segs: &'static [Seg],
}

const ADJACENCY_MAP: &[(u8, u8, &[Seg])] = &[
    //    a   b
    //   +--+--+
    // h |     | c
    //   +     +
    // g |     | d
    //   +--+--+
    //    f   e     abcdefgh
    (PX_EMPTY, 0b00000000, &[]),
    (PX_HALF1, 0b00001111, &[(0.0, 0.0, 1.0, 1.0)]),
    (PX_HALF3, 0b11000011, &[(0.0, 1.0, 1.0, 0.0)]),
    (
        PX_QUAD1,
        0b00000011,
        &[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 0.0, 1.0)],
    ),
    (
        PX_QUAD2,
        0b11000000,
        &[(0.0, 0.0, 0.5, 0.5), (0.5, 0.5, 1.0, 0.0)],
    ),
    (
        PX_QUAD3,
        0b00110000,
        &[(1.0, 0.0, 0.5, 0.5), (0.5, 0.5, 1.0, 1.0)],
    ),
    (
        PX_QUAD4,
        0b00001100,
        &[(0.0, 1.0, 0.5, 0.5), (0.5, 0.5, 1.0, 1.0)],
    ),
    (PX_SLANT1H, 0b00000111, &[(0.0, 0.0, 0.5, 1.0)]),
    (PX_SLANT2H, 0b01110000, &[(0.5, 0.0, 1.0, 1.0)]),
    (PX_SLANT3H, 0b10000011, &[(0.0, 1.0, 0.5, 0.0)]),
    (PX_SLANT4H, 0b00111000, &[(0.5, 1.0, 1.0, 0.0)]),
    (PX_SLANT1V, 0b00001110, &[(0.0, 0.5, 1.0, 1.0)]),
    (PX_SLANT2V, 0b11100000, &[(0.0, 0.0, 1.0, 0.5)]),
    (PX_SLANT3V, 0b11000001, &[(0.0, 0.5, 1.0, 0.0)]),
    (PX_SLANT4V, 0b00011100, &[(0.0, 1.0, 1.0, 0.5)]),
    (
        PX_DOT,
        0b00000000,
        &[
            (0.0, 0.5, 0.5, 0.0),
            (0.5, 0.0, 1.0, 0.5),
            (1.0, 0.5, 0.5, 1.0),
            (0.5, 1.0, 0.0, 0.5),
        ],
    ),
];

static ADJACENCY: std::sync::LazyLock<[AdjEntry; 33]> = std::sync::LazyLock::new(|| {
    let mut table: [AdjEntry; 33] = std::array::from_fn(|_| AdjEntry { bits: 0, segs: &[] });

    for &(shape, bits, segs) in ADJACENCY_MAP {
        table[shape as usize] = AdjEntry { bits, segs };
    }

    for k in 0u8..32 {
        if ADJACENCY_MAP.iter().any(|&(s, _, _)| s == k) {
            continue;
        }
        let complement = k ^ PX_SUBPIXEL;
        if let Some(&(_, bits, segs)) = ADJACENCY_MAP.iter().find(|&&(s, _, _)| s == complement) {
            table[k as usize] = AdjEntry {
                bits: bits ^ 0xFF,
                segs,
            };
        }
    }

    table[32] = AdjEntry {
        bits: table[PX_ALMOSTFULL as usize].bits,
        segs: table[PX_ALMOSTFULL as usize].segs,
    };

    table
});

pub fn adjacency(shape_id: u8) -> (u8, &'static [(f32, f32, f32, f32)]) {
    let entry = &ADJACENCY[shape_id.min(32) as usize];
    (entry.bits, entry.segs)
}

#[derive(Clone, Copy, Debug)]
pub struct EdgeInterval {
    pub start: f32,
    pub end: f32,
}

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
impl EdgeInterval {
    pub const EMPTY: Self = Self {
        start: 0.0,
        end: 0.0,
    };

    pub fn is_empty(self) -> bool {
        self.end <= self.start + 1e-6
    }

    pub fn intersect(self, other: Self) -> Self {
        let s = self.start.max(other.start);
        let e = self.end.min(other.end);
        if e > s + 1e-6 {
            Self { start: s, end: e }
        } else {
            Self::EMPTY
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub struct ShapeEdgeCoverage {
    pub top: EdgeInterval,
    pub bottom: EdgeInterval,
    pub left: EdgeInterval,
    pub right: EdgeInterval,
}

static EDGE_COVERAGE: std::sync::LazyLock<[ShapeEdgeCoverage; 32]> =
    std::sync::LazyLock::new(|| std::array::from_fn(|i| compute_edge_coverage(i as u8)));

#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn edge_coverage(shape_id: u8) -> &'static ShapeEdgeCoverage {
    &EDGE_COVERAGE[shape_id.min(31) as usize]
}

fn compute_edge_coverage(shape_id: u8) -> ShapeEdgeCoverage {
    let polygon = build_unit_polygon(shape_id);
    if polygon.len() < 3 {
        return ShapeEdgeCoverage {
            top: EdgeInterval::EMPTY,
            bottom: EdgeInterval::EMPTY,
            left: EdgeInterval::EMPTY,
            right: EdgeInterval::EMPTY,
        };
    }

    ShapeEdgeCoverage {
        top: coverage_on_edge(&polygon, Edge::Top),
        bottom: coverage_on_edge(&polygon, Edge::Bottom),
        left: coverage_on_edge(&polygon, Edge::Left),
        right: coverage_on_edge(&polygon, Edge::Right),
    }
}

enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

fn coverage_on_edge(polygon: &[(f32, f32)], edge: Edge) -> EdgeInterval {
    let n = polygon.len();
    let mut points_on_edge: Vec<f32> = Vec::new();

    for i in 0..n {
        let (x1, y1) = polygon[i];
        let (x2, y2) = polygon[(i + 1) % n];

        match edge {
            Edge::Top => {
                // y=0 edge, parameter is x
                collect_edge_intersections(y1, y2, x1, x2, 0.0, &mut points_on_edge);
            }
            Edge::Bottom => {
                // y=1 edge, parameter is x
                collect_edge_intersections(y1, y2, x1, x2, 1.0, &mut points_on_edge);
            }
            Edge::Left => {
                // x=0 edge, parameter is y
                collect_edge_intersections(x1, x2, y1, y2, 0.0, &mut points_on_edge);
            }
            Edge::Right => {
                // x=1 edge, parameter is y
                collect_edge_intersections(x1, x2, y1, y2, 1.0, &mut points_on_edge);
            }
        }
    }

    if points_on_edge.is_empty() {
        return EdgeInterval::EMPTY;
    }

    let min = points_on_edge.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = points_on_edge
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);

    if max - min < 1e-6 {
        EdgeInterval::EMPTY
    } else {
        EdgeInterval {
            start: min,
            end: max,
        }
    }
}

fn collect_edge_intersections(
    coord1: f32,
    coord2: f32, // coordinate perpendicular to the edge
    param1: f32,
    param2: f32,     // coordinate along the edge
    edge_coord: f32, // the fixed coordinate value of the edge (0 or 1)
    out: &mut Vec<f32>,
) {
    let eps = 1e-6;
    let on1 = (coord1 - edge_coord).abs() < eps;
    let on2 = (coord2 - edge_coord).abs() < eps;

    if on1 {
        out.push(param1.clamp(0.0, 1.0));
    }
    if on2 {
        out.push(param2.clamp(0.0, 1.0));
    }

    if !on1 && !on2 {
        // Check if the segment crosses the edge
        if (coord1 - edge_coord) * (coord2 - edge_coord) < 0.0 {
            let t = (edge_coord - coord1) / (coord2 - coord1);
            let param = param1 + t * (param2 - param1);
            out.push(param.clamp(0.0, 1.0));
        }
    }
}

fn build_unit_polygon(shape_id: u8) -> Vec<(f32, f32)> {
    let (adj_bits, gap_segs) = adjacency(shape_id);
    if adj_bits == 0 && gap_segs.is_empty() {
        return vec![];
    }
    polygon_from_adjacency(adj_bits, gap_segs)
}

/// Chain a shape's boundary half-edges and gap segments into a single
/// closed polygon in unit-square coordinates.
pub(crate) fn polygon_from_adjacency(
    adj_bits: u8,
    gap_segs: &[(f32, f32, f32, f32)],
) -> Vec<(f32, f32)> {
    if adj_bits == 0xFF {
        return vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
    }

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

    let mut used = vec![false; edges.len()];
    used[0] = true;
    mark_reverse_local(&edges, &mut used, 0);
    let mut polygon = vec![(edges[0][0], edges[0][1])];
    let mut cur = (edges[0][2], edges[0][3]);

    for _ in 0..edges.len() {
        if near_f(cur, polygon[0]) {
            break;
        }
        let mut found = false;
        for (i, e) in edges.iter().enumerate() {
            if !used[i] && near_f((e[0], e[1]), cur) {
                used[i] = true;
                mark_reverse_local(&edges, &mut used, i);
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

    polygon.dedup_by(|a, b| near_f(*a, *b));
    polygon
}

fn near_f(a: (f32, f32), b: (f32, f32)) -> bool {
    (a.0 - b.0).abs() < 0.001 && (a.1 - b.1).abs() < 0.001
}

fn mark_reverse_local(edges: &[[f32; 4]], used: &mut [bool], idx: usize) {
    let e = edges[idx];
    for (j, other) in edges.iter().enumerate() {
        if !used[j]
            && near_f((other[0], other[1]), (e[2], e[3]))
            && near_f((other[2], other[3]), (e[0], e[1]))
        {
            used[j] = true;
            return;
        }
    }
}

const SHAPE_TO_CHARS: [[u8; 2]; 64] = {
    let mut table = [*b"??"; 64];
    // Unfilled shapes (PX_FULL = 0)
    table[PX_EMPTY as usize] = *b"..";
    table[PX_HALF1 as usize] = *b"0\\";
    table[PX_HALF3 as usize] = *b"0/";
    table[PX_QUAD1 as usize] = *b"0>";
    table[PX_QUAD2 as usize] = *b"0P";
    table[PX_QUAD3 as usize] = *b"<0";
    table[PX_QUAD4 as usize] = *b"d0";
    table[PX_SLANT1H as usize] = *b"v.";
    table[PX_SLANT2H as usize] = *b"`v";
    table[PX_SLANT3H as usize] = *b"v'";
    table[PX_SLANT4H as usize] = *b".v";
    table[PX_SLANT1V as usize] = *b"h\\";
    table[PX_SLANT2V as usize] = *b"\\h";
    table[PX_SLANT3V as usize] = *b"h/";
    table[PX_SLANT4V as usize] = *b"/h";
    table[PX_DOT as usize] = *b"<>";
    // 16 is unused, skip
    table[PX_HALFSLANT3V as usize] = *b"h_";
    table[PX_HALFSLANT4V as usize] = *b"~h";
    table[PX_HALFSLANT1V as usize] = *b"h~";
    table[PX_HALFSLANT2V as usize] = *b"_h";
    table[PX_HALFSLANT3H as usize] = *b"v/";
    table[PX_HALFSLANT4H as usize] = *b"/v";
    table[PX_HALFSLANT1H as usize] = *b"v\\";
    table[PX_HALFSLANT2H as usize] = *b"\\v";
    table[PX_INVQUAD4 as usize] = *b"P0";
    table[PX_INVQUAD3 as usize] = *b"0<";
    table[PX_INVQUAD2 as usize] = *b"0d";
    table[PX_INVQUAD1 as usize] = *b">0";
    table[PX_HALF4 as usize] = *b"/0";
    table[PX_HALF2 as usize] = *b"\\0";
    table[PX_ALMOSTFULL as usize] = *b"88"; // unfilled almostfull — rare

    // Filled shapes (PX_FULL = 0x20, offset by 32)
    table[32 + PX_EMPTY as usize] = *b"__"; // filled empty — unlikely but define
    table[32 + PX_HALF1 as usize] = *b"1\\";
    table[32 + PX_HALF3 as usize] = *b"1/";
    table[32 + PX_QUAD1 as usize] = *b"1>";
    table[32 + PX_QUAD2 as usize] = *b"1P";
    table[32 + PX_QUAD3 as usize] = *b"<1";
    table[32 + PX_QUAD4 as usize] = *b"d1";
    // SLANT types don't get PX_FULL in practice, but define for completeness
    table[32 + PX_SLANT1H as usize] = *b"V.";
    table[32 + PX_SLANT2H as usize] = *b"`V";
    table[32 + PX_SLANT3H as usize] = *b"V'";
    table[32 + PX_SLANT4H as usize] = *b".V";
    table[32 + PX_SLANT1V as usize] = *b"H\\";
    table[32 + PX_SLANT2V as usize] = *b"\\H";
    table[32 + PX_SLANT3V as usize] = *b"H/";
    table[32 + PX_SLANT4V as usize] = *b"/H";
    table[32 + PX_DOT as usize] = *b"{}"; // filled dot
                                          // 32+16 unused
    table[32 + PX_HALFSLANT3V as usize] = *b"H_";
    table[32 + PX_HALFSLANT4V as usize] = *b"~H";
    table[32 + PX_HALFSLANT1V as usize] = *b"H~";
    table[32 + PX_HALFSLANT2V as usize] = *b"_H";
    table[32 + PX_HALFSLANT3H as usize] = *b"V/";
    table[32 + PX_HALFSLANT4H as usize] = *b"/V";
    table[32 + PX_HALFSLANT1H as usize] = *b"V\\";
    table[32 + PX_HALFSLANT2H as usize] = *b"\\V";
    table[32 + PX_INVQUAD4 as usize] = *b"P1";
    table[32 + PX_INVQUAD3 as usize] = *b"1<";
    table[32 + PX_INVQUAD2 as usize] = *b"1d";
    table[32 + PX_INVQUAD1 as usize] = *b">1";
    table[32 + PX_HALF4 as usize] = *b"/1";
    table[32 + PX_HALF2 as usize] = *b"\\1";
    table[32 + PX_ALMOSTFULL as usize] = *b"@@"; // the standard filled pixel

    table
};

pub fn shape_to_chars(shape: PixelShape) -> [char; 2] {
    let [c1, c2] = SHAPE_TO_CHARS[shape.0 as usize];
    [c1 as char, c2 as char]
}

pub fn chars_to_shape(c1: char, c2: char) -> Option<PixelShape> {
    PAIR_TO_SHAPE.get(&(c1, c2)).copied()
}

// ---------------------------------------------------------------------------
// Shape combine (union / subtract) table
// ---------------------------------------------------------------------------

const RASTER_N: usize = 10;
const RASTER_BITS: usize = RASTER_N * RASTER_N;
const FULL_RASTER: u128 = (1u128 << RASTER_BITS) - 1;

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
                if ((yi > py) != (yj > py))
                    && (px < (xj - xi) * (py - yi) / (yj - yi) + xi)
                {
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

struct ShapeCombineTable {
    union_id: [[u8; 32]; 32],
    subtract_id: [[u8; 32]; 32],
}

static COMBINE: std::sync::LazyLock<ShapeCombineTable> = std::sync::LazyLock::new(|| {
    let mut rasters = [0u128; 32];
    for s in 0..32u8 {
        rasters[s as usize] = rasterize_polygon(&build_unit_polygon(s));
    }

    let mut raster_to_id = std::collections::HashMap::new();
    for s in 0..32u8 {
        raster_to_id.entry(rasters[s as usize]).or_insert(s);
    }
    raster_to_id.insert(0, PX_EMPTY);
    raster_to_id.insert(FULL_RASTER, PX_ALMOSTFULL);

    let mut union_id = [[PX_DOT; 32]; 32];
    let mut subtract_id = [[PX_DOT; 32]; 32];

    for a in 0..32u8 {
        for b in 0..32u8 {
            let ur = rasters[a as usize] | rasters[b as usize];
            if let Some(&id) = raster_to_id.get(&ur) {
                union_id[a as usize][b as usize] = id;
            }

            let sr = rasters[a as usize] & (!rasters[b as usize] & FULL_RASTER);
            if let Some(&id) = raster_to_id.get(&sr) {
                subtract_id[a as usize][b as usize] = id;
            }
        }
    }

    ShapeCombineTable {
        union_id,
        subtract_id,
    }
});

/// Union two pixel shapes. Returns the combined shape, or a `PX_DOT`
/// fallback if the geometric result doesn't match any known shape.
/// The filled flag is set if either input is filled.
#[cfg_attr(not(feature = "editor"), expect(dead_code))]
pub fn shape_union(a: PixelShape, b: PixelShape) -> PixelShape {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    let result_id = COMBINE.union_id[a.shape_id() as usize][b.shape_id() as usize];
    PixelShape::new(result_id, a.is_filled() || b.is_filled())
}

/// Subtract shape `b`'s area from shape `a`. The filled flag is preserved
/// from `a`. Returns `EMPTY` when nothing remains, or a `PX_DOT` fallback
/// when the geometric result doesn't match any known shape.
pub fn shape_subtract(a: PixelShape, b: PixelShape) -> PixelShape {
    if a.is_empty() || b.is_empty() {
        return a;
    }
    let result_id = COMBINE.subtract_id[a.shape_id() as usize][b.shape_id() as usize];
    if result_id == PX_EMPTY {
        PixelShape::EMPTY
    } else {
        PixelShape::new(result_id, a.is_filled())
    }
}

// ---------------------------------------------------------------------------
// Multi-shape adjacency (union of overlapping subpixels within one pixel)
// ---------------------------------------------------------------------------

/// Compute adjacency bits and gap segments for the union of multiple shapes
/// within a single pixel cell. Returns `(combined_adj_bits, gap_segments)`.
///
/// For a single shape this is equivalent to [`adjacency`]. For multiple shapes,
/// the adjacency bits are OR'd and the gap segments are geometrically clipped
/// so they represent the boundary of the union polygon.
pub fn multi_shape_adjacency(shapes: &[u8]) -> (u8, Vec<Seg>) {
    match shapes.len() {
        0 => return (0, Vec::new()),
        1 => {
            let (bits, segs) = adjacency(shapes[0]);
            return (bits, segs.to_vec());
        }
        _ => {}
    }

    let mut combined_bits = 0u8;
    for &s in shapes {
        combined_bits |= adjacency(s).0;
    }
    if combined_bits == 0xFF {
        return (0xFF, Vec::new());
    }

    let polygons: Vec<Vec<(f32, f32)>> = shapes.iter().map(|&s| build_unit_polygon(s)).collect();

    let mut combined_segs: Vec<Seg> = Vec::new();
    for (i, &s) in shapes.iter().enumerate() {
        let (_, gap_segs) = adjacency(s);
        if gap_segs.is_empty() {
            continue;
        }
        let outside_normal = gap_outside_normal(gap_segs[0], &polygons[i]);

        for &seg in gap_segs {
            let mut intervals = vec![(0.0f32, 1.0f32)];
            for (j, poly_j) in polygons.iter().enumerate() {
                if i == j || poly_j.len() < 3 {
                    continue;
                }
                intervals =
                    subtract_covered_intervals(seg, outside_normal, &intervals, poly_j);
                if intervals.is_empty() {
                    break;
                }
            }
            let (x1, y1, x2, y2) = seg;
            for (t0, t1) in intervals {
                if t1 - t0 < 1e-4 {
                    continue;
                }
                combined_segs.push((
                    x1 + t0 * (x2 - x1),
                    y1 + t0 * (y2 - y1),
                    x1 + t1 * (x2 - x1),
                    y1 + t1 * (y2 - y1),
                ));
            }
        }
    }

    (combined_bits, combined_segs)
}

/// Determine which side of a gap segment is "outside" (empty) for the given polygon.
/// Returns a normal vector pointing toward the empty side.
fn gap_outside_normal(seg: Seg, polygon: &[(f32, f32)]) -> (f32, f32) {
    let (x1, y1, x2, y2) = seg;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let mx = (x1 + x2) * 0.5;
    let my = (y1 + y2) * 0.5;
    let eps = 0.002;
    // Left normal candidate
    let (nx, ny) = (-dy, dx);
    let len = (nx * nx + ny * ny).sqrt().max(1e-9);
    let (nx, ny) = (nx / len, ny / len);
    if !point_in_polygon(mx + eps * nx, my + eps * ny, polygon) {
        (nx, ny)
    } else {
        (-nx, -ny)
    }
}

fn point_in_polygon(x: f32, y: f32, polygon: &[(f32, f32)]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Clip gap segment intervals, removing parts where the "outside" of the
/// source shape is filled by `other_polygon`.
fn subtract_covered_intervals(
    seg: Seg,
    outside_normal: (f32, f32),
    intervals: &[(f32, f32)],
    other_polygon: &[(f32, f32)],
) -> Vec<(f32, f32)> {
    let (x1, y1, x2, y2) = seg;
    let dx = x2 - x1;
    let dy = y2 - y1;

    let n = other_polygon.len();
    let mut crossings: Vec<f32> = Vec::new();
    for i in 0..n {
        let (px1, py1) = other_polygon[i];
        let (px2, py2) = other_polygon[(i + 1) % n];
        if let Some(t) = seg_intersect_t(x1, y1, x2, y2, px1, py1, px2, py2)
            && t > 0.002 && t < 0.998 {
                crossings.push(t);
            }
    }
    crossings.sort_by(|a, b| a.partial_cmp(b).unwrap());
    crossings.dedup_by(|a, b| (*a - *b).abs() < 0.002);

    let eps = 0.002;
    let (nx, ny) = outside_normal;

    let mut result = Vec::new();
    for &(start, end) in intervals {
        let mut splits = vec![start];
        for &t in &crossings {
            if t > start + eps && t < end - eps {
                splits.push(t);
            }
        }
        splits.push(end);

        for k in 0..splits.len() - 1 {
            let s = splits[k];
            let e = splits[k + 1];
            let mid = (s + e) * 0.5;
            let test_x = x1 + mid * dx + eps * nx;
            let test_y = y1 + mid * dy + eps * ny;
            if !point_in_polygon(test_x, test_y, other_polygon) {
                result.push((s, e));
            }
        }
    }
    result
}

/// Parameter `t` along segment A where it intersects segment B.
/// Returns `None` if segments are parallel or don't intersect.
fn seg_intersect_t(
    ax1: f32,
    ay1: f32,
    ax2: f32,
    ay2: f32,
    bx1: f32,
    by1: f32,
    bx2: f32,
    by2: f32,
) -> Option<f32> {
    let dx = ax2 - ax1;
    let dy = ay2 - ay1;
    let ex = bx2 - bx1;
    let ey = by2 - by1;
    let denom = dx * ey - dy * ex;
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = ((bx1 - ax1) * ey - (by1 - ay1) * ex) / denom;
    let u = ((bx1 - ax1) * dy - (by1 - ay1) * dx) / denom;
    if (-0.001..=1.001).contains(&t) && (-0.001..=1.001).contains(&u) {
        Some(t.clamp(0.0, 1.0))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Multi-shape difference adjacency (union of positive shapes minus union of
// negative shapes).  Used by contour tracing for negated ref layers.
// ---------------------------------------------------------------------------

/// Compute adjacency bits and gap segments for the geometric difference of
/// two sets of shapes within a single pixel cell:  union(positive) \ union(negative).
///
/// The result is computed by rasterizing both unions on a fine grid, taking
/// the set difference, and finding the closest known shape whose adjacency
/// (bits + gap segments) is guaranteed to form valid closed contours.
pub fn multi_shape_diff_adjacency(
    positive_shapes: &[u8],
    negative_shapes: &[u8],
) -> (u8, Vec<Seg>) {
    if negative_shapes.is_empty() {
        return multi_shape_adjacency(positive_shapes);
    }
    if positive_shapes.is_empty() {
        return (0, Vec::new());
    }

    let rasters = &DIFF_TABLE.rasters;

    let mut pos_raster = 0u128;
    for &s in positive_shapes {
        pos_raster |= rasters[s as usize];
    }
    let mut neg_raster = 0u128;
    for &s in negative_shapes {
        neg_raster |= rasters[s as usize];
    }

    let result_raster = pos_raster & (!neg_raster & FULL_RASTER);
    if result_raster == 0 {
        return (0, Vec::new());
    }

    let best_id = DIFF_TABLE.closest_shape(result_raster);
    let (bits, segs) = adjacency(best_id);
    (bits, segs.to_vec())
}

struct DiffTable {
    rasters: [u128; 32],
}

impl DiffTable {
    fn closest_shape(&self, target: u128) -> u8 {
        if target == 0 {
            return PX_EMPTY;
        }
        if target == FULL_RASTER {
            return PX_ALMOSTFULL;
        }
        // Exact match first.
        for (i, &r) in self.rasters.iter().enumerate() {
            if r == target {
                return i as u8;
            }
        }
        // Best-fit by minimum Hamming distance.
        let mut best = PX_ALMOSTFULL;
        let mut best_dist = u32::MAX;
        for (i, &r) in self.rasters.iter().enumerate() {
            if r == 0 {
                continue;
            }
            let dist = (target ^ r).count_ones();
            if dist < best_dist {
                best_dist = dist;
                best = i as u8;
            }
        }
        best
    }
}

static DIFF_TABLE: std::sync::LazyLock<DiffTable> = std::sync::LazyLock::new(|| {
    let mut rasters = [0u128; 32];
    for s in 0..32u8 {
        rasters[s as usize] = rasterize_polygon(&build_unit_polygon(s));
    }
    DiffTable { rasters }
});

// ---------------------------------------------------------------------------

static PAIR_TO_SHAPE: std::sync::LazyLock<std::collections::HashMap<(char, char), PixelShape>> =
    std::sync::LazyLock::new(|| {
        let mut map = std::collections::HashMap::new();
        for (i, &[c1, c2]) in SHAPE_TO_CHARS.iter().enumerate() {
            if c1 != b'?' || c2 != b'?' {
                let shape = PixelShape(i as u8);
                let pair = shape_to_chars(shape);
                map.insert((pair[0], pair[1]), shape);
            }
        }
        map
    });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_shape_roundtrip() {
        for raw in 0u8..64 {
            let shape = PixelShape(raw);
            let [c1, c2] = shape_to_chars(shape);
            if c1 == '?' {
                continue;
            }
            let decoded = chars_to_shape(c1, c2).unwrap();
            assert_eq!(shape, decoded, "roundtrip failed for raw={raw}, chars={c1}{c2}");
        }
    }

    #[test]
    fn common_shapes() {
        assert_eq!(shape_to_chars(PixelShape::EMPTY), ['.', '.']);
        assert_eq!(shape_to_chars(PixelShape::new(PX_ALMOSTFULL, true)), ['@', '@']);
        assert_eq!(chars_to_shape('.', '.'), Some(PixelShape::EMPTY));
        assert_eq!(
            chars_to_shape('@', '@'),
            Some(PixelShape::new(PX_ALMOSTFULL, true))
        );
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
        assert_eq!(
            shape_subtract(full, half1),
            PixelShape::new(PX_HALF2, true),
        );
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
        for s in 0..32u8 {
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
        assert_eq!(
            bits,
            adjacency(PX_SLANT1H).0 | adjacency(PX_SLANT3H).0,
        );
        // Should have 2 gap segments meeting at the intersection point
        assert_eq!(segs.len(), 2, "expected 2 clipped gap segments, got {}", segs.len());
        // Both segments should share the intersection point (0.25, 0.5)
        let has_intersection = segs.iter().any(|&(x1, y1, _, _)| {
            (x1 - 0.25).abs() < 0.01 && (y1 - 0.5).abs() < 0.01
        }) || segs.iter().any(|&(_, _, x2, y2)| {
            (x2 - 0.25).abs() < 0.01 && (y2 - 0.5).abs() < 0.01
        });
        assert!(has_intersection, "gap segs should meet at (0.25, 0.5): {segs:?}");
    }
}
