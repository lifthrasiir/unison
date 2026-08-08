//! Exact per-pixel subpixel geometry on a common-denominator lattice.
//!
//! A [`DetailRegion`] describes an arbitrary filled region within a single
//! unit pixel cell.  Vertices live on a lattice of `1/den` steps (`den` is
//! bounded by `u8`), coordinates are `(x, y)` numerators over `den` with y
//! pointing down and `(0, 0)` at the pixel's top-left corner.
//!
//! The filled set of a region is the even-odd interior of its rings, which
//! makes storage insensitive to ring orientation.
//!
//! Regions are combined through a y-slab trapezoid sweep carried out in
//! exact integer arithmetic; stitching and collinear simplification also
//! run exactly, so only genuinely off-lattice vertices (crossing points of
//! diagonal edges) are snapped to the output lattice at the very end.
//!
//! "Exact" here is a bounded claim, not a hopeful one: the sweep's input is
//! integers on one lattice, every value it derives is computed in one step
//! from those integers rather than by chaining rational operations, and
//! [`MAX_SWEEP_COORD`] carries the width budget that says an `i128` holds all
//! of it. [`Frac`] explains why a chained fixed-width rational cannot work at
//! any width, which is the bug this shape exists to rule out.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use crate::math::{gcd_i128, gcd_u64, lcm_u64};

use crate::pixel::{
    PX_ALMOSTFULL, PX_BACKSLASH, PX_CONE1, PX_CONE2, PX_CONE3, PX_CONE4, PX_CORNER1, PX_CORNER2,
    PX_CORNER3, PX_CORNER4, PX_DOT, PX_EMPTY, PX_HALF1, PX_HALF3, PX_HOUSE1, PX_HOUSE2, PX_HOUSE3,
    PX_HOUSE4, PX_HQUAD, PX_QUAD1, PX_QUAD2, PX_QUAD3, PX_QUAD4, PX_SLANT1H, PX_SLANT1V,
    PX_SLANT2H, PX_SLANT2V, PX_SLANT3H, PX_SLANT3V, PX_SLANT4H, PX_SLANT4V, PX_SLASH, PX_SUBPIXEL,
};

/// Maximum lattice denominator, kept within `u8`.
pub const MAX_DEN: u16 = 255;

/// A filled region inside one unit pixel (even-odd interior of `rings`).
/// Coordinates are numerators over `den` in `0..=den`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DetailRegion {
    pub den: u8,
    pub rings: Vec<Vec<(u8, u8)>>,
}

/// Result of matching a region back against the encodable pixel catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Classified {
    Empty,
    Full,
    /// Exactly equal to catalog shape `id` (a plain `1..=30` id or a
    /// complement id).
    Shape(u8),
    /// Not representable by a plain pixel code.
    Custom(DetailRegion),
}

/// Boolean operation selector for [`bool_op`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    Subtract,
    #[cfg_attr(not(test), expect(dead_code))]
    Intersect,
}

/// A rational scalar for callers building transforms: `n / d`.
#[derive(Clone, Copy, Debug)]
pub struct Frac64 {
    pub n: i64,
    pub d: i64,
}

impl Frac64 {
    pub fn new(n: i64, d: i64) -> Self {
        debug_assert!(d > 0);
        Self { n, d }
    }
}

// ---------------------------------------------------------------------------
// Base shape catalog as lattice rings (den = 2)
// ---------------------------------------------------------------------------

/// Exact outlines of the 30 base catalog shapes on the half lattice.
/// Multi-part shapes (HQUAD) use one ring per part. Derived from the shape
/// definitions in `pixel.rs` (`ADJACENCY_MAP` bits + gap segments).
const fn base_shape_rings(id: u8) -> &'static [&'static [(u8, u8)]] {
    match id {
        PX_HALF1 => &[&[(0, 0), (2, 2), (0, 2)]],
        PX_HALF3 => &[&[(0, 0), (2, 0), (0, 2)]],
        PX_QUAD1 => &[&[(0, 0), (1, 1), (0, 2)]],
        PX_QUAD2 => &[&[(0, 0), (2, 0), (1, 1)]],
        PX_QUAD3 => &[&[(2, 0), (2, 2), (1, 1)]],
        PX_QUAD4 => &[&[(1, 1), (2, 2), (0, 2)]],
        PX_SLANT1H => &[&[(0, 0), (1, 2), (0, 2)]],
        PX_SLANT2H => &[&[(1, 0), (2, 0), (2, 2)]],
        PX_SLANT3H => &[&[(0, 0), (1, 0), (0, 2)]],
        PX_SLANT4H => &[&[(2, 0), (2, 2), (1, 2)]],
        PX_SLANT1V => &[&[(0, 1), (2, 2), (0, 2)]],
        PX_SLANT2V => &[&[(0, 0), (2, 0), (2, 1)]],
        PX_SLANT3V => &[&[(0, 0), (2, 0), (0, 1)]],
        PX_SLANT4V => &[&[(2, 1), (2, 2), (0, 2)]],
        PX_DOT => &[&[(1, 0), (2, 1), (1, 2), (0, 1)]],
        PX_CONE1 => &[&[(0, 0), (2, 1), (0, 2)]],
        PX_CONE2 => &[&[(0, 0), (2, 0), (1, 2)]],
        PX_CONE3 => &[&[(2, 0), (2, 2), (0, 1)]],
        PX_CONE4 => &[&[(1, 0), (2, 2), (0, 2)]],
        PX_HQUAD => &[&[(0, 0), (1, 1), (0, 2)], &[(2, 0), (2, 2), (1, 1)]],
        PX_CORNER1 => &[&[(0, 1), (1, 2), (0, 2)]],
        PX_CORNER2 => &[&[(1, 0), (2, 0), (2, 1)]],
        PX_CORNER3 => &[&[(0, 0), (1, 0), (0, 1)]],
        PX_CORNER4 => &[&[(2, 1), (2, 2), (1, 2)]],
        PX_SLASH => &[&[(1, 0), (2, 0), (2, 1), (1, 2), (0, 2), (0, 1)]],
        PX_BACKSLASH => &[&[(0, 0), (1, 0), (2, 1), (2, 2), (1, 2), (0, 1)]],
        PX_HOUSE1 => &[&[(0, 0), (1, 0), (2, 1), (1, 2), (0, 2)]],
        PX_HOUSE2 => &[&[(0, 0), (2, 0), (2, 1), (1, 2), (0, 1)]],
        PX_HOUSE3 => &[&[(1, 0), (2, 0), (2, 2), (1, 2), (0, 1)]],
        PX_HOUSE4 => &[&[(1, 0), (2, 1), (2, 2), (0, 2), (0, 1)]],
        _ => &[],
    }
}

/// Probe points per axis for [`DetailRegion::sample_mask`]. 8×8 fits a `u64`
/// and keeps every catalog shape on a mask of its own, which is what makes
/// `nearest_shape` an identity there (`sample_masks_are_distinct`).
///
/// It is deliberately not wider. Quantization costs ±½ probe per row, so a
/// `k/den` cut can round to the wrong side of ½ — a 3/7-covered cell inks
/// where the exact area would not. That is the accepted price of sampling: the
/// result is a suggestion the editor hands a human, not a computed value.
#[cfg(feature = "editor")]
const SAMPLE_K: i64 = 8;

/// What `nearest_shape` needs of a catalog id, precomputed: neither the probe
/// mask nor the edge-direction set depends on the region being matched.
#[cfg(feature = "editor")]
struct SnapCandidate {
    mask: u64,
    dirs: std::collections::BTreeSet<(i8, i8)>,
}

/// The [`SnapCandidate`] of every catalog id, `None` for `PX_EMPTY` and the
/// unused ids, built once.
#[cfg(feature = "editor")]
fn shape_snap_table() -> &'static [Option<SnapCandidate>; 128] {
    static TABLE: std::sync::OnceLock<Box<[Option<SnapCandidate>; 128]>> =
        std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let table: Vec<Option<SnapCandidate>> = shape_region_table()
            .iter()
            .map(|region| {
                (!region.is_empty()).then(|| SnapCandidate {
                    mask: region.sample_mask(),
                    dirs: region.interior_edge_dirs(),
                })
            })
            .collect();
        Box::new(table.try_into().ok().unwrap())
    })
}

/// Canonical regions of all 128 catalog ids, built once.
fn shape_region_table() -> &'static [DetailRegion; 128] {
    static TABLE: std::sync::OnceLock<Box<[DetailRegion; 128]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: Vec<DetailRegion> = (0..128u8)
            .map(|id| {
                if id == PX_EMPTY || id > 30 {
                    DetailRegion::EMPTY // complements filled below
                } else {
                    let rings = base_shape_rings(id);
                    if rings.is_empty() {
                        DetailRegion::EMPTY
                    } else {
                        DetailRegion {
                            den: 2,
                            rings: rings.iter().map(|r| r.to_vec()).collect(),
                        }
                        .canonical()
                    }
                }
            })
            .collect();
        table[PX_ALMOSTFULL as usize] = DetailRegion::full().canonical();
        for id in 97..PX_ALMOSTFULL {
            let base = (id ^ PX_SUBPIXEL) as usize;
            if !table[base].is_empty() {
                table[id as usize] =
                    bool_op(&DetailRegion::full(), &table[base], BoolOp::Subtract).canonical();
            }
        }
        table.try_into().unwrap()
    })
}

/// Reverse lookup: canonical region → catalog id (including ALMOSTFULL).
fn classify_index() -> &'static HashMap<DetailRegion, u8> {
    static INDEX: std::sync::OnceLock<HashMap<DetailRegion, u8>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map = HashMap::new();
        for (id, region) in shape_region_table().iter().enumerate() {
            if !region.is_empty() {
                // Prefer the smallest id when two ids share geometry.
                map.entry(region.clone()).or_insert(id as u8);
            }
        }
        map
    })
}

/// Exact `a − b` for two catalog shape ids, memoized. Negated blits hit
/// this for every overlapping plain pixel pair (e.g. dozens of star glyphs
/// punched out of a field), so the geometry must not be recomputed per
/// pixel.
pub fn catalog_subtract(a_id: u8, b_id: u8) -> Classified {
    let key = (a_id & PX_SUBPIXEL, b_id & PX_SUBPIXEL);
    subtract_classified(
        &shape_region_table()[key.0 as usize],
        &shape_region_table()[key.1 as usize],
    )
}

/// Exact classified `a − b` for arbitrary regions, memoized by region pair.
/// Compositions subtract the same few regions (a negated ref's cells) from
/// the same few backgrounds once per referencing site, so pair identity
/// repeats heavily.
pub fn subtract_classified(a: &DetailRegion, b: &DetailRegion) -> Classified {
    use std::sync::Mutex;
    type PairCache = HashMap<(DetailRegion, DetailRegion), Classified>;
    static CACHE: Mutex<Option<PairCache>> = Mutex::new(None);
    let key = (a.clone(), b.clone());
    if let Some(hit) = CACHE
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .get(&key)
    {
        return hit.clone();
    }
    let result = bool_op(a, b, BoolOp::Subtract).classify();
    let mut cache = CACHE.lock().unwrap();
    let map = cache.get_or_insert_with(HashMap::new);
    if map.len() > 65536 {
        map.clear();
    }
    map.insert(key, result.clone());
    result
}

// ---------------------------------------------------------------------------
// Exact rational scalar (internal)
// ---------------------------------------------------------------------------

/// Exact rational in **sweep units**: the real coordinate is `n / (d * scale)`
/// for the scale [`normalize_input`] chose. Always reduced and `d > 0`, so the
/// derived `PartialEq`/`Hash`/`Ord` are equality and order of the value.
///
/// # Why this type has no arithmetic
///
/// A fixed-width rational overflows as soon as you *chain* operations on it:
/// each `a/b op c/d` multiplies the denominators, so the width grows with the
/// number of operations rather than with the size of the input, and no width
/// is enough — `i64` fell over here the day a curve raised the lattice
/// denominator from 4 to 51, and `i128` would only have moved the day.
///
/// So this type carries no `add`/`sub`/`mul`/`div`. Every derived value the
/// sweep needs — a crossing height, an x at a height — is computed *in one
/// step from the integer input* by [`crossing_height`] and [`x_at`], as a
/// determinant would be. The width is then a function of the input alone, and
/// [`MAX_SWEEP_COORD`] states the bound that makes it fit. The remaining
/// operations are comparison and rounding onto the output lattice, and their
/// widths are bounded in the same breath.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Frac {
    n: i128,
    d: i128,
}

impl Frac {
    fn new(n: i128, d: i128) -> Self {
        debug_assert!(d != 0);
        let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
        if n == 0 {
            return Self { n: 0, d: 1 };
        }
        if d == 1 {
            return Self { n, d };
        }
        let g = gcd_i128(n, d);
        // A 128-bit division is a call into a software routine, and a gcd of
        // 1 is the common case; skipping it there is worth the branch.
        if g == 1 {
            Self { n, d }
        } else {
            Self { n: n / g, d: d / g }
        }
    }

    fn from_int(n: i64) -> Self {
        Self { n: n as i128, d: 1 }
    }

    /// The real value as `n / d`, given the sweep scale.
    fn to_real(self, scale: i64) -> (i128, i128) {
        let d = self.d * scale as i128;
        let g = gcd_i128(self.n, d);
        if g == 1 {
            (self.n, d)
        } else {
            (self.n / g, d / g)
        }
    }

    /// Round the real value to the nearest multiple of `1/den`, halves up.
    fn round_to_den(self, den: i64, scale: i64) -> i64 {
        let (n, d) = self.to_real(scale);
        let num = n * den as i128;
        let q = num.div_euclid(d);
        let r = num.rem_euclid(d);
        let q = if 2 * r >= d { q + 1 } else { q };
        q as i64
    }
}

impl PartialOrd for Frac {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Frac {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.d == other.d {
            return self.n.cmp(&other.n);
        }
        // Bounded by the budget in [`MAX_SWEEP_COORD`]: at most 2^68 * 2^52.
        (self.n * other.d).cmp(&(other.n * self.d))
    }
}

/// Least common multiple of two lattice denominators, or `None` when it
/// exceeds [`MAX_DEN`] (callers should then snap to a chosen denominator).
pub fn lcm_den(a: u8, b: u8) -> Option<u8> {
    match lcm_u64(a.max(1) as u64, b.max(1) as u64) {
        Some(l) if l <= MAX_DEN as u64 => Some(l as u8),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Sweep engine: even-odd parity per operand, y-slab trapezoids
// ---------------------------------------------------------------------------

type FPoint = (Frac, Frac);

/// Bound on a sweep input coordinate, in sweep units.
///
/// # The width proof
///
/// The sweep's input is integer points on one lattice ([`normalize_input`]),
/// with every coordinate at most `C = MAX_SWEEP_COORD` in magnitude. Every
/// value the sweep derives is then bounded by `C` alone — not by how many
/// edges, slabs or operations there are:
///
/// | value | form | bound |
/// | --- | --- | --- |
/// | edge delta `dx`, `dy` | difference | `2C` |
/// | `det` of two deltas | 2x2 | `8C²` |
/// | crossing height `y` | [`crossing_height`] | `24C³ / 8C²` |
/// | `x` on an edge at `y` | [`x_at`] | `16C⁴ / 16C³` |
/// | comparing two such `x` | [`Frac::cmp`] | `256C⁷` |
///
/// With `C = 2^16` the widest of those is `2^120`, which leaves seven bits in
/// an `i128`. That is the whole reason `C` is what it is: it is read off the
/// last row, not measured on a corpus. Raising it costs seven bits per bit.
///
/// The one predicate that would *not* have been bounded this way is testing
/// three derived points for collinearity — a 3x3 determinant over `16C⁴`
/// entries is `C^11`, hopeless at any width. [`Line`] removes the need for it.
const MAX_SWEEP_COORD: i64 = 1 << 16;

/// A non-horizontal edge normalized to point downward, tagged with the
/// operand (0 or 1) it belongs to. Coordinates are integers in sweep units.
#[derive(Clone, Copy, Debug)]
struct SweepEdge {
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
    operand: u8,
}

/// The line a ring segment lies on, as a reduced `a·x + b·y + c = 0` over
/// integers in sweep units, sign-normalized so that equality of the triple is
/// equality of the line.
///
/// This is what replaces a collinearity *test* on derived points. Every
/// segment the sweep emits lies either on an input edge or on a slab boundary,
/// so its line is known exactly, from the integers, before any crossing is
/// computed — and two segments are collinear exactly when their lines are
/// equal. Two collinear input edges (a common border of the two operands, say)
/// normalize to the same triple, so this merges as much as the old 3x3
/// determinant did, at a width bounded by the input rather than by `C^11`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Line {
    a: i128,
    b: i128,
    c: i128,
}

impl Line {
    fn new(a: i128, b: i128, c: i128) -> Self {
        let g = gcd_i128(gcd_i128(a, b), c).max(1);
        let (a, b, c) = (a / g, b / g, c / g);
        // Canonical sign: the first nonzero coefficient is positive.
        let neg = if a != 0 {
            a < 0
        } else if b != 0 {
            b < 0
        } else {
            c < 0
        };
        if neg {
            Self {
                a: -a,
                b: -b,
                c: -c,
            }
        } else {
            Self { a, b, c }
        }
    }

    /// The line an edge lies on. Bounded by `2C` and `4C²`.
    fn of_edge(e: &SweepEdge) -> Self {
        let (a, b) = ((e.y2 - e.y1) as i128, (e.x1 - e.x2) as i128);
        Self::new(a, b, -(a * e.x1 as i128 + b * e.y1 as i128))
    }

    /// The horizontal line `y = h`. Input edges are never horizontal, so this
    /// can never collide with [`Line::of_edge`].
    fn horizontal(h: Frac) -> Self {
        Self::new(0, h.d, -h.n)
    }
}

/// A sweep input point in real units, exact as `n/d` per axis, before
/// [`normalize_input`] puts it on the integer lattice.
#[derive(Clone, Copy, Debug)]
struct RatPoint {
    x: (i64, i64),
    y: (i64, i64),
}

impl RatPoint {
    fn new(x: (i64, i64), y: (i64, i64)) -> Self {
        debug_assert!(x.1 > 0 && y.1 > 0);
        Self { x, y }
    }

    fn lattice(x: i64, y: i64, den: i64) -> Self {
        Self::new((x, den), (y, den))
    }
}

/// Put the input polygons on one integer lattice, so that everything after
/// this point is integer arithmetic with the budget of [`MAX_SWEEP_COORD`].
///
/// The scale is the lcm of the input denominators whenever that is within
/// budget, which is every case the callers in this file actually produce: the
/// widest is `lcm(den_a, den_b) <= 255² = 65025`, just inside `2^16`. When a
/// caller does come in finer than the budget, coordinates are rounded onto the
/// finest lattice that fits instead. That is not a silent loss of exactness:
/// the sweep's *output* is snapped to a denominator of at most
/// [`MAX_DEN`] = 255, so a lattice of at least `2^16/3` is already more than
/// eight bits finer than anything the result can express.
fn normalize_input(input: &SweepInput) -> (i64, Vec<SweepEdge>) {
    let mut scale: u64 = 1;
    let mut extent: i64 = 1;
    for p in &input.pts {
        for (n, d) in [p.x, p.y] {
            scale = lcm_u64(scale, d as u64).unwrap_or(u64::MAX);
            extent = extent.max((n.abs() + d - 1) / d);
        }
    }
    let cap = (MAX_SWEEP_COORD / extent).max(1) as u64;
    let scale = scale.min(cap) as i64;

    let to_lattice = |(n, d): (i64, i64)| -> i64 {
        // The lattice was built as the lcm of these denominators, so unless it
        // had to be capped this is an exact multiply and no division runs.
        if scale % d == 0 {
            return n * (scale / d);
        }
        let num = n as i128 * scale as i128;
        let d = d as i128;
        let q = num.div_euclid(d);
        let r = num.rem_euclid(d);
        (if 2 * r >= d { q + 1 } else { q }) as i64
    };

    let mut edges = Vec::new();
    for &(start, end, operand) in &input.rings {
        let ring = &input.pts[start..end];
        let n = ring.len();
        for i in 0..n {
            let p = ring[i];
            let q = ring[(i + 1) % n];
            let (ax, ay) = (to_lattice(p.x), to_lattice(p.y));
            let (bx, by) = (to_lattice(q.x), to_lattice(q.y));
            if ay == by {
                continue; // horizontal edges carry no parity
            }
            let (p, q) = if ay < by {
                ((ax, ay), (bx, by))
            } else {
                ((bx, by), (ax, ay))
            };
            debug_assert!(
                p.0.abs().max(q.0.abs()).max(q.1.abs()) <= MAX_SWEEP_COORD,
                "sweep input outside the width budget"
            );
            edges.push(SweepEdge {
                x1: p.0,
                y1: p.1,
                x2: q.0,
                y2: q.1,
                operand,
            });
        }
    }
    (scale, edges)
}

/// The rings of a sweep's input, in one flat buffer.
#[derive(Default)]
struct SweepInput {
    pts: Vec<RatPoint>,
    /// `(start, end, operand)` into `pts`, one entry per ring.
    rings: Vec<(usize, usize, u8)>,
}

impl SweepInput {
    fn push_ring(&mut self, operand: u8, pts: impl IntoIterator<Item = RatPoint>) {
        let start = self.pts.len();
        self.pts.extend(pts);
        if self.pts.len() - start >= 3 {
            self.rings.push((start, self.pts.len(), operand));
        } else {
            self.pts.truncate(start);
        }
    }

    /// The rings of a lattice region, tagged with an operand.
    fn push_region(&mut self, region: &DetailRegion, operand: u8) {
        let den = region.den.max(1) as i64;
        for ring in &region.rings {
            self.push_ring(
                operand,
                ring.iter()
                    .map(|&(x, y)| RatPoint::lattice(x as i64, y as i64, den)),
            );
        }
    }

    /// The unit square as the clipping operand of a transforming sweep.
    fn push_unit(&mut self, operand: u8) {
        self.push_region(&DetailRegion::full(), operand);
    }

    /// `region` mapped onto the rectangle `[x0, x0+w] x [y0, y0+h]`.
    ///
    /// The mapped coordinate is written out as a single fraction,
    /// `(x0.n·den·w.d + p·w.n·x0.d) / (x0.d·den·w.d)`, rather than assembled
    /// from rational adds and multiplies. Chaining is what a fixed width
    /// cannot afford (see [`Frac`]); a closed form over the caller's integers,
    /// each at most a `u8` lattice denominator or a scale ratio, is bounded by
    /// the input.
    fn push_transformed(
        &mut self,
        region: &DetailRegion,
        x0: Frac64,
        y0: Frac64,
        w: Frac64,
        h: Frac64,
    ) {
        let den = region.den.max(1) as i64;
        let map = |p: i64, o: Frac64, s: Frac64| -> (i64, i64) {
            (o.n * den * s.d + p * s.n * o.d, o.d * den * s.d)
        };
        for ring in &region.rings {
            self.push_ring(
                0,
                ring.iter()
                    .map(|&(px, py)| RatPoint::new(map(px as i64, x0, w), map(py as i64, y0, h))),
            );
        }
    }
}

/// Height at which two edges cross, or `None` when they are parallel.
///
/// Written as the determinant it is rather than as a chain of rational steps:
/// with `u = b1 - a1`, the crossing is at `t = (u x bd) / (ad x bd)` along `a`,
/// so `y = a1y + t·ady` is one fraction built from the integer input in a
/// single step. Bounded by `24C³ / 8C²`.
fn crossing_height(a: &SweepEdge, b: &SweepEdge) -> Option<Frac> {
    let (adx, ady) = ((a.x2 - a.x1) as i128, (a.y2 - a.y1) as i128);
    let (bdx, bdy) = ((b.x2 - b.x1) as i128, (b.y2 - b.y1) as i128);
    let det = adx * bdy - ady * bdx;
    if det == 0 {
        return None;
    }
    let (ux, uy) = ((b.x1 - a.x1) as i128, (b.y1 - a.y1) as i128);
    let t_num = ux * bdy - uy * bdx;
    Some(Frac::new(a.y1 as i128 * det + t_num * ady, det))
}

/// x coordinate where `e` meets height `y`, which must lie within its span.
///
/// `x = e.x1 + (y - e.y1)·edx/edy`, put over `edy·y.d` in one step so that the
/// result is bounded by `16C⁴ / 16C³` however `y` was obtained.
fn x_at(e: &SweepEdge, y: Frac) -> Frac {
    let (edx, edy) = ((e.x2 - e.x1) as i128, (e.y2 - e.y1) as i128);
    let num = e.x1 as i128 * edy * y.d + (y.n - e.y1 as i128 * y.d) * edx;
    Frac::new(num, edy * y.d)
}

/// One filled x-interval of a y-slab, with exact corner coordinates and the
/// lines its two sides lie on.
#[derive(Clone, Debug)]
struct Trap {
    y_top: Frac,
    y_bot: Frac,
    xl_top: Frac,
    xl_bot: Frac,
    xr_top: Frac,
    xr_bot: Frac,
    left: Line,
    right: Line,
}

/// Sweep the plane and keep the set where `filled(in_a, in_b)` holds, with
/// `in_a`/`in_b` the even-odd interiors of the two operands' edge sets.
fn sweep(edges: &[SweepEdge], filled: &dyn Fn(bool, bool) -> bool) -> Vec<Trap> {
    // Slab cut heights: every endpoint plus every pairwise crossing height.
    let mut ys: Vec<Frac> = Vec::new();
    for e in edges {
        ys.push(Frac::from_int(e.y1));
        ys.push(Frac::from_int(e.y2));
    }
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[i + 1..] {
            let lo = Frac::from_int(a.y1.max(b.y1));
            let hi = Frac::from_int(a.y2.min(b.y2));
            if lo >= hi {
                continue;
            }
            if let Some(y) = crossing_height(a, b)
                && y > lo
                && y < hi
            {
                ys.push(y);
            }
        }
    }
    ys.sort();
    ys.dedup();

    // One line per edge, not one per edge per slab.
    let lines: Vec<Line> = edges.iter().map(Line::of_edge).collect();

    let mut traps: Vec<Trap> = Vec::new();
    for w in ys.windows(2) {
        let (ya, yb) = (w[0], w[1]);
        // Every crossing is a slab boundary, so within a slab the spanning
        // edges keep one left-to-right order and `(x at ya, x at yb)` is it.
        // Ordering by the x at the slab's *midpoint* would read the same, but
        // the midpoint is the one value in this file built from two derived
        // heights at once — that product is what put the arithmetic outside
        // any input-shaped bound, and nothing needs it.
        let mut xs: Vec<(Frac, Frac, u8, Line)> = Vec::new();
        for (e, &line) in edges.iter().zip(&lines) {
            if Frac::from_int(e.y1) <= ya && Frac::from_int(e.y2) >= yb {
                xs.push((x_at(e, ya), x_at(e, yb), e.operand, line));
            }
        }
        xs.sort_by(|p, q| p.0.cmp(&q.0).then_with(|| p.1.cmp(&q.1)));

        let mut in_a = false;
        let mut in_b = false;
        let mut run_start: Option<(Frac, Frac, Line)> = None;
        for &(x_top, x_bot, operand, line) in &xs {
            let was = filled(in_a, in_b);
            if operand == 0 {
                in_a = !in_a;
            } else {
                in_b = !in_b;
            }
            let now = filled(in_a, in_b);
            if !was && now {
                run_start = Some((x_top, x_bot, line));
            } else if was && !now {
                let (xl_top, xl_bot, left) = run_start.take().expect("run must be open");
                if !(xl_top == x_top && xl_bot == x_bot) {
                    traps.push(Trap {
                        y_top: ya,
                        y_bot: yb,
                        xl_top,
                        xl_bot,
                        xr_top: x_top,
                        xr_bot: x_bot,
                        left,
                        right: line,
                    });
                }
            }
        }
        debug_assert!(run_start.is_none(), "unclosed parity run");
    }
    traps
}

// ---------------------------------------------------------------------------
// Trapezoid stitching back into lattice rings
// ---------------------------------------------------------------------------

/// Stitch trapezoids into closed rings, working in exact coordinates.
///
/// Sloped/vertical side edges of vertically stacked runs cancel as exact
/// opposite pairs; horizontal borders are accumulated per height as signed
/// 1D intervals so partial overlaps (staircases) resolve exactly. Collinear
/// vertices (slab subdivision points on straight edges) are removed while
/// still exact. The output lattice denominator is chosen as the lcm of the
/// remaining vertices' denominators, so the result is exact whenever that
/// lcm fits [`MAX_DEN`]; otherwise vertices are snapped to a 255 lattice.
fn traps_to_rings(traps: &[Trap], scale: i64) -> (u8, Vec<Vec<(u8, u8)>>) {
    let mut side_segs: Vec<(FPoint, FPoint, Line)> = Vec::new();
    // y → x → signed breakpoint delta for horizontal borders at that height.
    let mut horiz: BTreeMap<Frac, BTreeMap<Frac, i64>> = BTreeMap::new();
    let mut add_horiz = |y: Frac, xa: Frac, xb: Frac, dir: i64| {
        // Directed horizontal segment xa→xb (dir = +1 means left-to-right).
        let (lo, hi) = if xa < xb { (xa, xb) } else { (xb, xa) };
        if lo == hi {
            return;
        }
        let row = horiz.entry(y).or_default();
        *row.entry(lo).or_insert(0) += dir;
        *row.entry(hi).or_insert(0) -= dir;
    };

    for t in traps {
        let lt = (t.xl_top, t.y_top);
        let rt = (t.xr_top, t.y_top);
        let lb = (t.xl_bot, t.y_bot);
        let rb = (t.xr_bot, t.y_bot);
        // Clockwise on screen (positive shoelace in y-down coordinates):
        // top left→right, right side top→bottom, bottom right→left, left
        // side bottom→top.
        add_horiz(t.y_top, lt.0, rt.0, 1);
        add_horiz(t.y_bot, rb.0, lb.0, -1);
        if rt != rb {
            side_segs.push((rt, rb, t.right));
        }
        if lb != lt {
            side_segs.push((lb, lt, t.left));
        }
    }

    // Resolve horizontal borders into net directed segments.
    let mut segs: Vec<(FPoint, FPoint, Line)> = Vec::new();
    for (y, row) in &horiz {
        let line = Line::horizontal(*y);
        let mut level = 0i64;
        let mut start: Option<(Frac, i64)> = None; // (x, direction sign)
        for (&x, &delta) in row {
            let new_level = level + delta;
            let was = level.signum();
            let now = new_level.signum();
            debug_assert!(
                level.abs() <= 1 && new_level.abs() <= 1,
                "horizontal border overlap deeper than 1"
            );
            if was == 0 && now != 0 {
                start = Some((x, now));
            } else if was != 0 && now != was {
                let (sx, sdir) = start.take().expect("open horizontal run");
                if sdir > 0 {
                    segs.push(((sx, *y), (x, *y), line));
                } else {
                    segs.push(((x, *y), (sx, *y), line));
                }
                if now != 0 {
                    start = Some((x, now));
                }
            }
            level = new_level;
        }
        debug_assert!(start.is_none(), "unclosed horizontal run");
    }
    segs.extend(side_segs);

    let rings: Vec<Vec<FPoint>> = link_rings(segs).into_iter().map(drop_collinear).collect();

    // Pick the smallest lattice that represents every remaining vertex
    // exactly, clamped to MAX_DEN (beyond which vertices get rounded).
    let mut den: u64 = 1;
    for ring in &rings {
        for &(x, y) in ring {
            for v in [x, y] {
                let d = v.to_real(scale).1 as u64;
                den = lcm_u64(den, d).unwrap_or(u64::MAX).min(MAX_DEN as u64);
            }
        }
    }
    let den = den as i64;

    let mut out = Vec::new();
    for ring in rings {
        let snapped: Vec<(i64, i64)> = ring
            .iter()
            .map(|&(x, y)| {
                (
                    x.round_to_den(den, scale).clamp(0, den),
                    y.round_to_den(den, scale).clamp(0, den),
                )
            })
            .collect();
        // Clean up again in lattice space (rounding may introduce new
        // degeneracies when the exact lcm exceeded MAX_DEN).
        let snapped = simplify_lattice(dedup_cyclic(snapped));
        if snapped.len() >= 3 && lattice_ring_area2(&snapped) != 0 {
            out.push(
                snapped
                    .into_iter()
                    .map(|(x, y)| (x as u8, y as u8))
                    .collect(),
            );
        }
    }
    (den as u8, out)
}

/// Link directed segments into closed rings, cancelling exact opposite
/// pairs first (shared borders of adjacent trapezoids). Each ring vertex is
/// returned with the line of the segment *leaving* it, for [`drop_collinear`].
///
/// Vertices are interned to ids first. The ids are handed out in sorted order,
/// so an id comparison is a coordinate comparison and the linking order — which
/// decides where a ring starts, and so the exact ring this returns — is
/// unchanged; what it buys is that the cancellation map hashes an 8-byte pair
/// instead of four 128-bit rationals.
fn link_rings(segs: Vec<(FPoint, FPoint, Line)>) -> Vec<Vec<(FPoint, Line)>> {
    let mut points: Vec<FPoint> = Vec::with_capacity(segs.len() * 2);
    for (a, b, _) in &segs {
        points.push(*a);
        points.push(*b);
    }
    points.sort();
    points.dedup();
    let id = |p: &FPoint| -> u32 { points.binary_search(p).expect("interned point") as u32 };

    let mut counter: HashMap<(u32, u32), (i32, Line)> = HashMap::new();
    for (a, b, line) in &segs {
        let (a, b) = (id(a), id(b));
        if a == b {
            continue;
        }
        if let Some((c, _)) = counter.get_mut(&(b, a)) {
            *c -= 1;
            if *c == 0 {
                counter.remove(&(b, a));
            }
            continue;
        }
        counter.entry((a, b)).or_insert((0, *line)).0 += 1;
    }
    let mut by_start: BTreeMap<u32, Vec<(u32, Line)>> = BTreeMap::new();
    for ((a, b), (c, line)) in counter {
        debug_assert!(c > 0);
        for _ in 0..c {
            by_start.entry(a).or_default().push((b, line));
        }
    }
    for list in by_start.values_mut() {
        list.sort_by_key(|p| p.0);
    }

    let mut rings = Vec::new();
    while let Some((&start, _)) = by_start.first_key_value() {
        let mut ring: Vec<(FPoint, Line)> = Vec::new();
        let mut cur = start;
        loop {
            let Some(nexts) = by_start.get_mut(&cur) else {
                debug_assert!(false, "open chain in ring linking");
                break;
            };
            let (next, line) = nexts.pop().expect("empty successor list");
            if nexts.is_empty() {
                by_start.remove(&cur);
            }
            ring.push((points[cur as usize], line));
            if next == start {
                break;
            }
            cur = next;
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    rings
}

/// Drop vertices whose two incident segments lie on the same line.
///
/// This is the collinearity test, done symbolically: the sweep knows the line
/// of every segment it emits ([`Line`]), so three consecutive vertices are
/// collinear exactly when the two segments meeting at the middle one share a
/// line. The alternative — a cross product of three derived points — is the
/// one predicate in this file whose width is cubic in already-derived values,
/// and no fixed integer width bounds it. See [`MAX_SWEEP_COORD`].
fn drop_collinear(ring: Vec<(FPoint, Line)>) -> Vec<FPoint> {
    let n = ring.len();
    let mut out: Vec<FPoint> = Vec::with_capacity(n);
    for i in 0..n {
        let incoming = ring[(i + n - 1) % n].1;
        let (vertex, outgoing) = ring[i];
        if incoming != outgoing {
            out.push(vertex);
        }
    }
    out
}

fn dedup_cyclic(mut ring: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    ring.dedup();
    while ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    ring
}

fn simplify_lattice(ring: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    let n = ring.len();
    if n < 3 {
        return Vec::new();
    }
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(n);
    for i in 0..n {
        let p = ring[(i + n - 1) % n];
        let q = ring[i];
        let r = ring[(i + 1) % n];
        let cross = (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0);
        if cross != 0 {
            out.push(q);
        }
    }
    if out.len() < 3 { Vec::new() } else { out }
}

fn lattice_ring_area2(ring: &[(i64, i64)]) -> i64 {
    let n = ring.len();
    let mut a = 0i64;
    for i in 0..n {
        let (x1, y1) = ring[i];
        let (x2, y2) = ring[(i + 1) % n];
        a += x1 * y2 - x2 * y1;
    }
    a
}

// ---------------------------------------------------------------------------
// Public region operations
// ---------------------------------------------------------------------------

impl DetailRegion {
    pub const EMPTY: DetailRegion = DetailRegion {
        den: 1,
        rings: Vec::new(),
    };

    pub fn full() -> DetailRegion {
        DetailRegion {
            den: 1,
            rings: vec![vec![(0, 0), (1, 0), (1, 1), (0, 1)]],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rings.is_empty()
    }

    /// The exact region of a catalog pixel shape (any id `0..=127`).
    /// Served from a lazily built table — this is on the hot path of
    /// rescaling and classification.
    pub fn from_shape(shape_id: u8) -> DetailRegion {
        shape_region_table()[(shape_id & PX_SUBPIXEL) as usize].clone()
    }

    /// Exact filled area, counted in `1/(2·den²)` of the unit square — so a
    /// full cell is `2·den²` — for any `den` this region's own lattice divides.
    /// Only meaningful for canonical regions (sweep-derived rings: outer rings
    /// and holes are wound oppositely, so the signed shoelace sum is the
    /// even-odd area).
    ///
    /// The caller picks the lattice, rather than getting a `(num, den)` pair
    /// back, because the only thing anyone wants of area here is a *threshold*:
    /// [`crate::on_demand::BitmapFill`] adds up the subcells of a logical pixel
    /// and asks whether the total clears zero, half or full. Measured on one
    /// shared `den` those areas are plain integers that add, which keeps the
    /// whole path free of fraction arithmetic while staying exact — and exact
    /// is the point, since half a cell is precisely the tie `Round` resolves
    /// and a float would leave it to chance.
    ///
    /// Nothing needs area as a magnitude. [`DetailRegion::nearest_shape`] once
    /// did, and moving it to probe sampling is what shrank this to a threshold.
    ///
    /// Panics in debug if `den` is not a multiple of `self.den`.
    pub fn area_units_on(&self, den: u8) -> i64 {
        debug_assert!(
            den != 0 && den.is_multiple_of(self.den),
            "den {den} is not a multiple of the region's own {}",
            self.den
        );
        let k = (den / self.den) as i64;
        let mut a = 0i64;
        for ring in &self.rings {
            let ring: Vec<(i64, i64)> = ring.iter().map(|&(x, y)| (x as i64, y as i64)).collect();
            a += lattice_ring_area2(&ring);
        }
        a.abs() * k * k
    }

    /// A full cell in the units [`area_units_on`](Self::area_units_on) counts.
    pub fn area_units_full(den: u8) -> i64 {
        2 * den as i64 * den as i64
    }

    /// Twice the filled area in unit-square terms (test/diagnostic helper).
    #[cfg(test)]
    pub fn area2(&self) -> f64 {
        self.area_units_on(self.den) as f64 / (self.den as f64 * self.den as f64)
    }

    /// Re-derive the region through the sweep engine. The output ring set is
    /// a deterministic function of the filled point set, which makes it a
    /// canonical form: two regions describing the same set produce identical
    /// rings.
    fn resweep(&self) -> DetailRegion {
        let mut input = SweepInput::default();
        input.push_region(self, 0);
        let (scale, edges) = normalize_input(&input);
        let traps = sweep(&edges, &|a, _| a);
        let (den, rings) = traps_to_rings(&traps, scale);
        DetailRegion { den, rings }
    }

    /// Snap this region onto the lattice `1/den`. Exact when `den` is a
    /// multiple of the current denominator; otherwise vertices are rounded
    /// and degenerate rings dropped.
    pub fn snap_to_den(&self, den: u8) -> DetailRegion {
        if den == self.den {
            return self.clone();
        }
        let d = den as i64;
        let src = self.den.max(1) as i64;
        // Nearest multiple of 1/d to k/src, halves up.
        let snap = |k: u8| -> i64 { (2 * k as i64 * d + src).div_euclid(2 * src).clamp(0, d) };
        let mut rings = Vec::new();
        for ring in &self.rings {
            let snapped: Vec<(i64, i64)> = ring.iter().map(|&(x, y)| (snap(x), snap(y))).collect();
            let snapped = simplify_lattice(dedup_cyclic(snapped));
            if snapped.len() >= 3 && lattice_ring_area2(&snapped) != 0 {
                rings.push(
                    snapped
                        .into_iter()
                        .map(|(x, y)| (x as u8, y as u8))
                        .collect(),
                );
            }
        }
        DetailRegion { den, rings }
    }

    /// Canonical form: sweep-derived rings, denominator reduced by the gcd
    /// of all numerators, rings rotated to their smallest vertex and sorted.
    pub fn canonical(&self) -> DetailRegion {
        let mut region = self.resweep();
        // Reduce the denominator.
        let mut g = region.den as u64;
        for ring in &region.rings {
            for &(x, y) in ring {
                g = gcd_u64(g, x as u64);
                g = gcd_u64(g, y as u64);
            }
        }
        if g > 1 {
            let g = g as u8;
            region.den /= g;
            for ring in &mut region.rings {
                for p in ring.iter_mut() {
                    p.0 /= g;
                    p.1 /= g;
                }
            }
        }
        if region.rings.is_empty() {
            return DetailRegion::EMPTY;
        }
        for ring in &mut region.rings {
            if let Some(min_idx) = (0..ring.len()).min_by_key(|&i| ring[i]) {
                ring.rotate_left(min_idx);
            }
        }
        region.rings.sort();
        region
    }

    /// Match against the encodable catalog: empty, full, or a plain shape
    /// id; otherwise `Custom` (in canonical form).
    pub fn classify(&self) -> Classified {
        let canon = self.canonical();
        if canon.is_empty() {
            return Classified::Empty;
        }
        if canon.den <= 2
            && let Some(&id) = classify_index().get(&canon)
        {
            return if id == PX_ALMOSTFULL {
                Classified::Full
            } else {
                Classified::Shape(id)
            };
        }
        Classified::Custom(canon)
    }

    /// Directions of the region's boundary edges that run through the cell's
    /// interior, each reduced and sign-normalized. Edges lying along a cell
    /// border are left out: they bound the cell, not the shape.
    #[cfg(feature = "editor")]
    fn interior_edge_dirs(&self) -> std::collections::BTreeSet<(i8, i8)> {
        let den = self.den;
        let mut dirs = std::collections::BTreeSet::new();
        for ring in &self.rings {
            for (i, &(x0, y0)) in ring.iter().enumerate() {
                let (x1, y1) = ring[(i + 1) % ring.len()];
                let on_border = (x0 == 0 && x1 == 0)
                    || (x0 == den && x1 == den)
                    || (y0 == 0 && y1 == 0)
                    || (y0 == den && y1 == den);
                if on_border {
                    continue;
                }
                let (mut dx, mut dy) = (x1 as i32 - x0 as i32, y1 as i32 - y0 as i32);
                let g = gcd_u64(dx.unsigned_abs() as u64, dy.unsigned_abs() as u64).max(1) as i32;
                dx /= g;
                dy /= g;
                // A direction and its reverse are the same line direction.
                if (dx, dy) < (-dx, -dy) {
                    dx = -dx;
                    dy = -dy;
                }
                dirs.insert((dx as i8, dy as i8));
            }
        }
        dirs
    }

    /// Which of the [`SAMPLE_K`]² probe points fall inside the region, as bit
    /// `j * SAMPLE_K + i` for the probe in column `i` of row `j`.
    ///
    /// The probes sit at `((5i+1)/5K, (5j+2)/5K)`: one per subcell, offset so
    /// that a probe can never land *on* a catalog edge. Every catalog shape is
    /// bounded by lines `x = p/2`, `y = p/2` or `x ± y = p/2`, and a probe on
    /// one of those would need `2(5i+1)`, `2(5j+2)`, `2(5i+5j+3)` or
    /// `2(5i−5j−1)` to be a multiple of 5 — none of which is ever ≡ 0 (mod 5).
    /// That matters because a probe exactly on an edge falls to whatever the
    /// crossing test's tie rule says, and with *every* catalog shape bounded by
    /// diagonals the resulting bias would be systematic rather than incidental.
    ///
    /// Probes are counted even-odd, matching how rings define the filled set.
    #[cfg(feature = "editor")]
    fn sample_mask(&self) -> u64 {
        let d = self.den as i64;
        let s = 5 * SAMPLE_K; // probes and rings meet on the `5K·den` lattice
        let mut mask = 0u64;
        for j in 0..SAMPLE_K {
            let py = (5 * j + 2) * d;
            for i in 0..SAMPLE_K {
                let px = (5 * i + 1) * d;
                let mut inside = false;
                for ring in &self.rings {
                    let n = ring.len();
                    for k in 0..n {
                        let (x0, y0) = (ring[k].0 as i64 * s, ring[k].1 as i64 * s);
                        let (x1, y1) = (
                            ring[(k + 1) % n].0 as i64 * s,
                            ring[(k + 1) % n].1 as i64 * s,
                        );
                        if (y0 > py) == (y1 > py) {
                            continue; // the +x ray from the probe misses this edge
                        }
                        // `px < x0 + (py−y0)(x1−x0)/(y1−y0)`, cleared of the
                        // division so the test stays in exact integers.
                        let (lhs, rhs) = ((px - x0) * (y1 - y0), (py - y0) * (x1 - x0));
                        if if y1 > y0 { lhs < rhs } else { lhs > rhs } {
                            inside = !inside;
                        }
                    }
                }
                if inside {
                    mask |= 1 << (j * SAMPLE_K + i);
                }
            }
        }
        mask
    }

    /// The catalog id that best stands in for this region.
    ///
    /// This is a *lossy* fallback and belongs only where an exact region has
    /// nowhere to go: `.unf` stores a cell as one of the plain shape codes,
    /// with no syntax for arbitrary geometry, so an editor operation that
    /// writes a grid back into a document (rescaling a glyph, say) has to land
    /// on the catalog. Resolution and the builder keep the exact region.
    ///
    /// The choice is the shape whose [`sample_mask`](Self::sample_mask) differs
    /// in the fewest probes — a quantized symmetric difference. Sampling rather
    /// than an exact area is deliberate: the answer is a suggestion a human
    /// then edits, and probe masks are precomputable per catalog id, which
    /// turns 128 boolean sweeps per query into 128 `popcount`s.
    ///
    /// Candidates are restricted to shapes that invent no edge direction the
    /// region does not already have. Every catalog shape is bounded by
    /// diagonals, so an axis-aligned region — the half-cell rectangles a scale
    /// change produces — is left to round to empty or full rather than turn a
    /// straight edge into a row of triangles, while diagonal geometry still
    /// lands on the diagonal shape that fits it. Remaining ties go to the
    /// shape covering more probes (so a half-covered cell inks, as
    /// `BitmapFill::Round` has it) and then to the lower id, for determinism.
    #[cfg(feature = "editor")]
    pub fn nearest_shape(&self) -> u8 {
        use std::sync::Mutex;
        static CACHE: Mutex<Option<HashMap<DetailRegion, u8>>> = Mutex::new(None);
        let canon = self.canonical();
        if let Some(&hit) = CACHE
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .get(&canon)
        {
            return hit;
        }

        let mask = canon.sample_mask();
        let dirs = canon.interior_edge_dirs();

        // Seed with `PX_EMPTY`, the fallback available to every region.
        let mut best_id = PX_EMPTY;
        let mut best = (mask.count_ones(), 0u32);
        for (id, shape) in shape_snap_table().iter().enumerate() {
            let Some(shape) = shape else {
                continue; // PX_EMPTY (the seed above) and the unused ids
            };
            if !shape.dirs.is_subset(&dirs) {
                continue;
            }
            let key = ((mask ^ shape.mask).count_ones(), shape.mask.count_ones());
            if key.0 < best.0 || (key.0 == best.0 && key.1 > best.1) {
                best = key;
                best_id = id as u8;
            }
        }

        CACHE
            .lock()
            .unwrap()
            .get_or_insert_with(HashMap::new)
            .insert(canon, best_id);
        best_id
    }

    /// Complement within the unit square.
    #[cfg(any(feature = "editor", test))]
    pub fn complement(&self) -> DetailRegion {
        bool_op(&DetailRegion::full(), self, BoolOp::Subtract)
    }

    #[cfg(feature = "editor")]
    fn map_lattice(&self, f: impl Fn(u8, u8, u8) -> (u8, u8)) -> DetailRegion {
        DetailRegion {
            den: self.den,
            rings: self
                .rings
                .iter()
                .map(|ring| ring.iter().map(|&(x, y)| f(x, y, self.den)).collect())
                .collect(),
        }
        .canonical()
    }

    #[cfg(feature = "editor")]
    pub fn mirror_h(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (d - x, y))
    }

    #[cfg(feature = "editor")]
    pub fn flip_v(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (x, d - y))
    }

    #[cfg(feature = "editor")]
    pub fn rotate_cw(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (d - y, x))
    }

    #[cfg(feature = "editor")]
    pub fn rotate_ccw(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (y, d - x))
    }

    #[cfg(feature = "editor")]
    pub fn rotate_180(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (d - x, d - y))
    }

    /// Map this region into the rectangle `[x0, x0+w] × [y0, y0+h]` of a
    /// destination unit square and clip to that square. The single-piece
    /// form of [`union_disjoint_transformed`], which rescaling now uses to
    /// combine all pieces of a destination pixel in one sweep. The output
    /// lattice is chosen automatically (exact up to [`MAX_DEN`]).
    #[cfg(test)]
    pub fn transform_into(&self, x0: Frac64, y0: Frac64, w: Frac64, h: Frac64) -> DetailRegion {
        let mut input = SweepInput::default();
        input.push_transformed(self, x0, y0, w, h);
        input.push_unit(1);
        let (scale, edges) = normalize_input(&input);
        let traps = sweep(&edges, &|a, b| a && b);
        let (den, rings) = traps_to_rings(&traps, scale);
        DetailRegion { den, rings }
    }
}

/// One piece of a transforming sweep: `region` mapped onto the rectangle
/// `[x0, x0+w] x [y0, y0+h]`, as exact rational input points.
///
/// The mapped coordinate is written out as a single fraction,
/// `(x0.n·den·w.d + p·w.n·x0.d) / (x0.d·den·w.d)`, rather than assembled from
/// rational adds and multiplies. Chaining is what a fixed width cannot afford
/// (see [`Frac`]); a closed form over the caller's integers, each at most a
/// `u8` lattice denominator or a scale ratio, is bounded by the input.
/// Union of transformed pieces with mutually disjoint interiors, clipped to
/// the unit pixel, in one sweep. Each piece is `region` mapped exactly like
/// [`DetailRegion::transform_into`] with origin `(x0, y0)` and scale
/// `(w, h)`. All pieces share one even-odd parity, so their interiors MUST
/// be disjoint (true for images of distinct source cells in a rescale);
/// shared boundaries cancel exactly. Replaces a chain of per-piece
/// `transform_into` + `bool_op` union sweeps, which is quadratic in the
/// piece count.
pub fn union_disjoint_transformed(
    pieces: &[(DetailRegion, Frac64, Frac64, Frac64, Frac64)],
) -> DetailRegion {
    let mut input = SweepInput::default();
    for (region, x0, y0, w, h) in pieces {
        input.push_transformed(region, *x0, *y0, *w, *h);
    }
    input.push_unit(1);
    let (scale, edges) = normalize_input(&input);
    let traps = sweep(&edges, &|a, b| a && b);
    let (den, rings) = traps_to_rings(&traps, scale);
    DetailRegion { den, rings }
}

/// Clip an arbitrary polygon (in pixel-local coordinates, possibly far
/// outside the unit square) to the unit square. Used to cut per-pixel
/// pieces out of large synthetic shapes such as on-demand triangles.
pub fn clip_polygon_to_cell(pts: &[(Frac64, Frac64)]) -> DetailRegion {
    if pts.len() < 3 {
        return DetailRegion::EMPTY;
    }
    let mut input = SweepInput::default();
    input.push_ring(
        0,
        pts.iter()
            .map(|&(x, y)| RatPoint::new((x.n, x.d), (y.n, y.d))),
    );
    input.push_unit(1);
    let (scale, edges) = normalize_input(&input);
    let traps = sweep(&edges, &|a, b| a && b);
    let (den, rings) = traps_to_rings(&traps, scale);
    DetailRegion { den, rings }
}

/// Boolean combination of two regions. The output lattice denominator is
/// chosen automatically as the smallest one representing every output
/// vertex — including crossing points of diagonal edges — exactly; only
/// when that exceeds [`MAX_DEN`] are vertices snapped.
pub fn bool_op(a: &DetailRegion, b: &DetailRegion, op: BoolOp) -> DetailRegion {
    match op {
        BoolOp::Union => {
            if a.is_empty() {
                return b.canonical();
            }
            if b.is_empty() {
                return a.canonical();
            }
        }
        BoolOp::Subtract => {
            if a.is_empty() || b.is_empty() {
                return a.canonical();
            }
        }
        BoolOp::Intersect => {
            if a.is_empty() || b.is_empty() {
                return DetailRegion::EMPTY;
            }
        }
    }

    let filled: fn(bool, bool) -> bool = match op {
        BoolOp::Union => |x, y| x || y,
        BoolOp::Subtract => |x, y| x && !y,
        BoolOp::Intersect => |x, y| x && y,
    };

    let mut input = SweepInput::default();
    input.push_region(a, 0);
    input.push_region(b, 1);
    let (scale, edges) = normalize_input(&input);
    let traps = sweep(&edges, &|x, y| filled(x, y));
    let (den, rings) = traps_to_rings(&traps, scale);
    DetailRegion { den, rings }
}

// ---------------------------------------------------------------------------
// Edge coverage & interior segments (tracer interface)
// ---------------------------------------------------------------------------

/// Coverage intervals of the four pixel edges, as numerator pairs over
/// `den`. Intervals are sorted and disjoint, in the natural axis direction
/// (x for top/bottom, y for left/right).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeCoverageExact {
    pub den: u8,
    pub top: Vec<(u8, u8)>,
    pub right: Vec<(u8, u8)>,
    pub bottom: Vec<(u8, u8)>,
    pub left: Vec<(u8, u8)>,
}

impl DetailRegion {
    /// Exact coverage of the four unit-square edges by the filled set.
    /// Requires a canonical region (sweep-derived rings, which run clockwise
    /// on screen around filled area — positive shoelace with y down — so the
    /// filled side of each boundary edge is determined by its direction).
    pub fn edge_coverage(&self) -> EdgeCoverageExact {
        let d = self.den as i64;
        let mut cov = EdgeCoverageExact {
            den: self.den,
            ..Default::default()
        };
        for ring in &self.rings {
            let n = ring.len();
            for i in 0..n {
                let (x1, y1) = (ring[i].0 as i64, ring[i].1 as i64);
                let (x2, y2) = (ring[(i + 1) % n].0 as i64, ring[(i + 1) % n].1 as i64);
                // Filled side faces inward iff: top runs left→right, bottom
                // runs right→left, right runs top→bottom, left runs
                // bottom→top (for clockwise-on-screen outer rings; hole
                // rings run the other way and never lie on the square edge
                // with fill inward).
                if y1 == 0 && y2 == 0 && x2 > x1 {
                    cov.top.push((x1 as u8, x2 as u8));
                } else if y1 == d && y2 == d && x1 > x2 {
                    cov.bottom.push((x2 as u8, x1 as u8));
                } else if x1 == d && x2 == d && y2 > y1 {
                    cov.right.push((y1 as u8, y2 as u8));
                } else if x1 == 0 && x2 == 0 && y1 > y2 {
                    cov.left.push((y2 as u8, y1 as u8));
                }
            }
        }
        for list in [&mut cov.top, &mut cov.right, &mut cov.bottom, &mut cov.left] {
            merge_intervals(list);
        }
        cov
    }

    /// Boundary segments strictly interior to the unit square, as f32
    /// unit-square segments for the contour tracer.
    pub fn interior_segments(&self) -> Vec<(f32, f32, f32, f32)> {
        let d = self.den as i64;
        let df = self.den as f32;
        let mut segs = Vec::new();
        for ring in &self.rings {
            let n = ring.len();
            for i in 0..n {
                let (x1, y1) = (ring[i].0 as i64, ring[i].1 as i64);
                let (x2, y2) = (ring[(i + 1) % n].0 as i64, ring[(i + 1) % n].1 as i64);
                let on_boundary = (y1 == 0 && y2 == 0)
                    || (y1 == d && y2 == d)
                    || (x1 == 0 && x2 == 0)
                    || (x1 == d && x2 == d);
                if !on_boundary {
                    segs.push((
                        x1 as f32 / df,
                        y1 as f32 / df,
                        x2 as f32 / df,
                        y2 as f32 / df,
                    ));
                }
            }
        }
        segs
    }
}

fn merge_intervals(list: &mut Vec<(u8, u8)>) {
    list.sort();
    let mut out: Vec<(u8, u8)> = Vec::with_capacity(list.len());
    for &(a, b) in list.iter() {
        if let Some(last) = out.last_mut()
            && a <= last.1
        {
            last.1 = last.1.max(b);
            continue;
        }
        out.push((a, b));
    }
    *list = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::PX_HALF2;

    /// The sweep's width proof rests on every input coordinate fitting
    /// [`MAX_SWEEP_COORD`]; the lcm of two `u8` lattices always does, and a
    /// caller that comes in finer is rounded onto the finest lattice that
    /// does rather than being allowed to overflow the budget.
    #[test]
    fn sweep_input_stays_inside_the_width_budget() {
        // The widest pair of lattice denominators two regions can carry.
        let a = DetailRegion {
            den: 255,
            rings: vec![vec![(0, 0), (255, 0), (255, 255)]],
        };
        let b = DetailRegion {
            den: 254,
            rings: vec![vec![(0, 254), (254, 254), (0, 0)]],
        };
        let mut input = SweepInput::default();
        input.push_region(&a, 0);
        input.push_region(&b, 1);
        let (scale, edges) = normalize_input(&input);
        assert_eq!(scale, 255 * 254, "an exact common lattice was available");
        for e in &edges {
            for v in [e.x1, e.y1, e.x2, e.y2] {
                assert!(v.abs() <= MAX_SWEEP_COORD, "{v} outside the budget");
            }
        }

        // A caller finer than the budget: coordinates get rounded onto the
        // finest lattice that fits, and none of them escapes it.
        let far = |n: i64| RatPoint::new((n, 65_521), (n, 65_519));
        let mut input = SweepInput::default();
        input.push_ring(0, [far(0), far(65_521 * 3), far(65_519)]);
        let (scale, edges) = normalize_input(&input);
        assert!(scale < 65_521 * 65_519, "the lcm had to be capped");
        assert!(!edges.is_empty());
        for e in &edges {
            for v in [e.x1, e.y1, e.x2, e.y2] {
                assert!(v.abs() <= MAX_SWEEP_COORD, "{v} outside the budget");
            }
        }
    }

    /// `nearest_shape` picks by probe mask, so two catalog shapes sharing a
    /// mask would be indistinguishable and the lower id would swallow the
    /// other. Nothing guarantees `SAMPLE_K` is fine enough for that but this.
    #[cfg(feature = "editor")]
    #[test]
    fn sample_masks_are_distinct() {
        let mut seen: HashMap<u64, usize> = HashMap::new();
        for (id, region) in shape_region_table().iter().enumerate() {
            if region.is_empty() {
                continue;
            }
            let mask = region.sample_mask();
            assert_ne!(mask, 0, "id {id} covers no probe at all");
            if let Some(prev) = seen.insert(mask, id) {
                panic!("ids {prev} and {id} share a probe mask");
            }
        }
    }

    /// The catalog is a fixed point of the snapping: a cell already spelled by
    /// a shape code must survive a rescale round-trip unchanged.
    #[cfg(feature = "editor")]
    #[test]
    fn nearest_shape_is_identity_on_the_catalog() {
        for (id, region) in shape_region_table().iter().enumerate() {
            if region.is_empty() {
                continue;
            }
            assert_eq!(region.canonical().nearest_shape(), id as u8, "id {id}");
        }
    }

    /// Probes never sit on a catalog edge — the property `sample_mask`'s
    /// `(5i+1, 5j+2)` offsets exist for. On the edge, the crossing test's tie
    /// rule would decide, and since every catalog shape is diagonal-bounded
    /// the bias would be systematic.
    #[cfg(feature = "editor")]
    #[test]
    fn probes_avoid_every_catalog_edge_line() {
        for j in 0..SAMPLE_K {
            for i in 0..SAMPLE_K {
                // Probe (x, y) = ((5i+1)/5K, (5j+2)/5K). A catalog edge lies on
                // some x, y, x+y or x−y = p/2, i.e. 2·numerator = p·5K.
                let (u, v) = (5 * i + 1, 5 * j + 2);
                for n in [u, v, u + v, u - v] {
                    assert_ne!(
                        (2 * n).rem_euclid(5),
                        0,
                        "probe ({i}, {j}) lies on a half-lattice line"
                    );
                }
            }
        }
    }

    #[test]
    fn from_shape_areas() {
        assert_eq!(DetailRegion::from_shape(PX_HALF1).area2(), 1.0);
        assert_eq!(DetailRegion::from_shape(PX_QUAD2).area2(), 0.5);
        assert_eq!(DetailRegion::from_shape(PX_SLANT1H).area2(), 0.5);
        assert_eq!(DetailRegion::from_shape(PX_CORNER1).area2(), 0.25);
        assert_eq!(DetailRegion::from_shape(PX_HQUAD).area2(), 1.0);
        assert_eq!(DetailRegion::from_shape(PX_DOT).area2(), 1.0);
    }

    /// The six DOT+two-corner shapes, with the two corners each is built from
    /// and the two its complement is made of.
    #[expect(clippy::type_complexity)]
    const DOT_CORNER_SHAPES: [(u8, (u8, u8), (u8, u8)); 6] = [
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

    fn union_of(ids: &[u8]) -> DetailRegion {
        ids.iter().fold(DetailRegion::EMPTY, |acc, &id| {
            bool_op(&acc, &DetailRegion::from_shape(id), BoolOp::Union)
        })
    }

    #[test]
    fn dot_plus_two_corners_is_exactly_the_new_shape() {
        for (id, (a, b), (c, d)) in DOT_CORNER_SHAPES {
            assert_eq!(
                union_of(&[PX_DOT, a, b]).classify(),
                Classified::Shape(id),
                "DOT+{a}+{b} should be shape {id}"
            );
            // 1/2 (the diamond) + two 1/8 corners.
            assert_eq!(DetailRegion::from_shape(id).area2(), 1.5);
            // ... and what is left over is exactly the other two corners.
            assert_eq!(
                union_of(&[c, d]).classify(),
                Classified::Shape(id ^ PX_SUBPIXEL),
                "the complement of {id} should be {c}+{d}"
            );
            assert_eq!(DetailRegion::from_shape(id ^ PX_SUBPIXEL).area2(), 0.5);
        }
    }

    #[test]
    fn base_rings_match_legacy_polygons() {
        // The hardcoded catalog rings must enclose the same area as the
        // legacy outline polygons (where those are single-part and sane).
        for id in 1..=24u8 {
            let region = DetailRegion::from_shape(id);
            let poly = crate::pixel::unit_polygon(id);
            if poly.is_empty() || id == PX_HQUAD {
                continue; // multi-part: legacy outline is not a simple ring
            }
            let mut a = 0.0f64;
            let n = poly.len();
            for i in 0..n {
                let (x1, y1) = poly[i];
                let (x2, y2) = poly[(i + 1) % n];
                a += x1 as f64 * y2 as f64 - x2 as f64 * y1 as f64;
            }
            assert!(
                (region.area2() - a.abs()).abs() < 1e-6,
                "id {id}: catalog ring area {} vs legacy polygon area {}",
                region.area2(),
                a.abs()
            );
        }
    }

    #[test]
    fn shape_plus_complement_is_full() {
        for id in 1..PX_SUBPIXEL {
            let a = DetailRegion::from_shape(id);
            let b = DetailRegion::from_shape(id ^ PX_SUBPIXEL);
            if a.is_empty() && b.is_empty() {
                continue;
            }
            let u = bool_op(&a, &b, BoolOp::Union);
            assert_eq!(
                u.classify(),
                Classified::Full,
                "shape {id} ∪ complement != full"
            );
            let i = bool_op(&a, &b, BoolOp::Intersect);
            assert_eq!(
                i.classify(),
                Classified::Empty,
                "shape {id} ∩ complement != empty"
            );
        }
    }

    #[test]
    fn subtract_from_full_gives_complement() {
        let half = DetailRegion::from_shape(PX_HALF1);
        let rest = bool_op(&DetailRegion::full(), &half, BoolOp::Subtract);
        match rest.classify() {
            Classified::Shape(id) => {
                assert_eq!(
                    DetailRegion::from_shape(id),
                    DetailRegion::from_shape(PX_HALF2),
                    "complement of HALF1 must be HALF2's geometry"
                );
            }
            other => panic!("expected a catalog shape, got {other:?}"),
        }
    }

    #[test]
    fn classify_round_trip() {
        for id in 1..PX_SUBPIXEL {
            let region = DetailRegion::from_shape(id);
            if region.is_empty() {
                continue;
            }
            match region.classify() {
                Classified::Shape(found) => {
                    assert_eq!(
                        DetailRegion::from_shape(found),
                        region,
                        "id {id} classified as {found} with different geometry"
                    );
                }
                Classified::Full => panic!("id {id} classified as Full"),
                other => panic!("id {id} classified as {other:?}"),
            }
        }
    }

    #[test]
    fn disjoint_intersection_is_empty() {
        let top = DetailRegion::from_shape(PX_QUAD2);
        let bottom = DetailRegion::from_shape(PX_QUAD4);
        let i = bool_op(&top, &bottom, BoolOp::Intersect);
        assert_eq!(i.classify(), Classified::Empty);
    }

    #[test]
    fn third_lattice_triangle() {
        // Smooth-mosaic building block: a right triangle with legs 1 × 1/3
        // on the den-3 lattice.
        let tri = DetailRegion {
            den: 3,
            rings: vec![vec![(0, 2), (3, 3), (0, 3)]],
        };
        assert!(matches!(tri.classify(), Classified::Custom(_)));
        assert_eq!(tri.canonical().area2(), 1.0 / 3.0);
        let comp = tri.complement();
        assert_eq!(
            bool_op(&tri, &comp, BoolOp::Union).classify(),
            Classified::Full
        );
        assert_eq!(
            bool_op(&tri, &comp, BoolOp::Intersect).classify(),
            Classified::Empty
        );
    }

    #[test]
    fn staircase_union() {
        // Two abutting rectangles of different widths — the shared border
        // partially cancels (staircase case).
        let upper = DetailRegion {
            den: 2,
            rings: vec![vec![(0, 0), (2, 0), (2, 1), (0, 1)]],
        };
        let lower = DetailRegion {
            den: 2,
            rings: vec![vec![(0, 1), (1, 1), (1, 2), (0, 2)]],
        };
        let u = bool_op(&upper, &lower, BoolOp::Union);
        assert_eq!(u.area2(), 2.0 * (0.5 + 0.25));
        // Single L-shaped ring with 6 vertices.
        assert_eq!(u.rings.len(), 1);
        assert_eq!(u.rings[0].len(), 6);
    }

    #[test]
    fn hole_subtraction() {
        // Subtract an interior diamond from the full square → ring + hole.
        let diamond = DetailRegion {
            den: 4,
            rings: vec![vec![(2, 1), (3, 2), (2, 3), (1, 2)]],
        };
        let holed = bool_op(&DetailRegion::full(), &diamond, BoolOp::Subtract);
        assert_eq!(holed.rings.len(), 2);
        assert_eq!(holed.area2(), 2.0 - diamond.canonical().area2());
        // Subtracting back the rest leaves the diamond.
        let back = bool_op(&DetailRegion::full(), &holed, BoolOp::Subtract);
        assert_eq!(back, diamond.canonical());
    }

    #[test]
    fn crossing_diagonals_snap() {
        // Two crossing HALF diagonals — the crossing point is on the
        // lattice here, so the union is exact: a bowtie-free hourglass.
        let a = DetailRegion::from_shape(PX_HALF1); // \ lower-left
        let b = DetailRegion::from_shape(crate::pixel::PX_HALF4 & PX_SUBPIXEL); // / lower-right
        let u = bool_op(&a, &b, BoolOp::Union);
        // Union of the two bottom-corner halves fills everything below both
        // diagonals: area 3/4 (two halves overlapping in the bottom quarter
        // triangle... exact value computed by inclusion-exclusion:
        // 1/2 + 1/2 − overlap). Overlap = bottom quarter = 1/4.
        assert_eq!(u.area2(), 2.0 * (0.5 + 0.5 - 0.25));
    }

    #[test]
    fn transform_into_subcell() {
        // A HALF1 diagonal scaled into the top-left quadrant: area 1/8.
        let half = DetailRegion::from_shape(PX_HALF1);
        let out = half.transform_into(
            Frac64::new(0, 1),
            Frac64::new(0, 1),
            Frac64::new(1, 2),
            Frac64::new(1, 2),
        );
        assert_eq!(out.area2(), 0.25);
    }

    #[test]
    fn transform_clips_to_unit_square() {
        // Source full pixel mapped to a rect hanging off the right edge.
        let out = DetailRegion::full().transform_into(
            Frac64::new(1, 2),
            Frac64::new(0, 1),
            Frac64::new(1, 1),
            Frac64::new(1, 1),
        );
        assert_eq!(out.area2(), 1.0);
        let cov = out.edge_coverage();
        assert_eq!(cov.right, vec![(0, cov.den)]);
    }

    #[test]
    fn edge_coverage_of_half() {
        // HALF1 (\ hypotenuse, bottom-left filled): bottom and left fully
        // covered, top and right empty.
        let cov = DetailRegion::from_shape(PX_HALF1).edge_coverage();
        let den = cov.den;
        assert_eq!(cov.bottom, vec![(0, den)]);
        assert_eq!(cov.left, vec![(0, den)]);
        assert!(cov.top.is_empty());
        assert!(cov.right.is_empty());
    }

    #[test]
    fn edge_coverage_matches_table() {
        // The exact coverage must agree with the legacy interval table for
        // every catalog shape.
        for id in 1..=127u8 {
            let region = DetailRegion::from_shape(id);
            if region.is_empty() {
                continue;
            }
            let exact = region.edge_coverage();
            let table = crate::pixel::edge_coverage(id);
            let den = exact.den as f32;
            let to_f = |list: &[(u8, u8)]| -> Vec<(f32, f32)> {
                list.iter()
                    .map(|&(a, b)| (a as f32 / den, b as f32 / den))
                    .collect()
            };
            let from_iv = |iv: &crate::pixel::EdgeInterval| -> Vec<(f32, f32)> {
                if iv.is_empty() {
                    Vec::new()
                } else {
                    vec![(iv.start, iv.end)]
                }
            };
            assert_eq!(
                to_f(&exact.top),
                from_iv(&table.top),
                "top coverage of id {id}"
            );
            assert_eq!(
                to_f(&exact.bottom),
                from_iv(&table.bottom),
                "bottom coverage of id {id}"
            );
            assert_eq!(
                to_f(&exact.left),
                from_iv(&table.left),
                "left coverage of id {id}"
            );
            assert_eq!(
                to_f(&exact.right),
                from_iv(&table.right),
                "right coverage of id {id}"
            );
        }
    }

    #[test]
    fn interior_segments_of_half() {
        let segs = DetailRegion::from_shape(PX_HALF1).interior_segments();
        assert_eq!(segs.len(), 1);
        let (x1, y1, x2, y2) = segs[0];
        let mut pts = [(x1, y1), (x2, y2)];
        pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(pts, [(0.0, 0.0), (1.0, 1.0)]);
    }
}
