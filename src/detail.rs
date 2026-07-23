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
//! exact rational arithmetic; stitching and collinear simplification also
//! run exactly, so only genuinely off-lattice vertices (crossing points of
//! diagonal edges) are snapped to the output lattice at the very end.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use crate::pixel::{
    PX_ALMOSTFULL, PX_CONE1, PX_CONE2, PX_CONE3, PX_CONE4, PX_CORNER1, PX_CORNER2, PX_CORNER3,
    PX_CORNER4, PX_DOT, PX_EMPTY, PX_HALF1, PX_HALF3, PX_HQUAD, PX_QUAD1, PX_QUAD2, PX_QUAD3,
    PX_QUAD4, PX_SLANT1H, PX_SLANT1V, PX_SLANT2H, PX_SLANT2V, PX_SLANT3H, PX_SLANT3V, PX_SLANT4H,
    PX_SLANT4V, PX_SUBPIXEL,
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
    /// Exactly equal to catalog shape `id` (a plain `1..=24` id or a
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

/// Exact outlines of the 24 base catalog shapes on the half lattice.
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
        _ => &[],
    }
}

/// Canonical regions of all 128 catalog ids, built once.
fn shape_region_table() -> &'static [DetailRegion; 128] {
    static TABLE: std::sync::OnceLock<Box<[DetailRegion; 128]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: Vec<DetailRegion> = (0..128u8)
            .map(|id| {
                if id == PX_EMPTY || id > 24 {
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
        for id in 103..PX_ALMOSTFULL {
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

// ---------------------------------------------------------------------------
// Exact rational scalar (internal)
// ---------------------------------------------------------------------------

/// Rational number `n / d`, `d > 0`, always kept reduced (so the derived
/// `PartialEq`/`Hash` are structural equality of the value).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Frac {
    n: i64,
    d: i64,
}

impl Frac {
    fn new(n: i64, d: i64) -> Self {
        debug_assert!(d != 0);
        let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
        let g = gcd(n.unsigned_abs(), d as u64).max(1) as i64;
        Self { n: n / g, d: d / g }
    }

    fn from_int(n: i64) -> Self {
        Self { n, d: 1 }
    }

    fn add(self, o: Self) -> Self {
        Self::new(self.n * o.d + o.n * self.d, self.d * o.d)
    }

    fn sub(self, o: Self) -> Self {
        Self::new(self.n * o.d - o.n * self.d, self.d * o.d)
    }

    fn mul(self, o: Self) -> Self {
        Self::new(self.n * o.n, self.d * o.d)
    }

    fn div(self, o: Self) -> Self {
        debug_assert!(o.n != 0);
        Self::new(self.n * o.d, self.d * o.n)
    }

    fn mid(self, o: Self) -> Self {
        self.add(o).div(Frac::from_int(2))
    }

    fn is_zero(self) -> bool {
        self.n == 0
    }

    /// Round to the nearest multiple of `1/den`.
    fn round_to_den(self, den: i64) -> i64 {
        let num = self.n * den;
        let q = num.div_euclid(self.d);
        let r = num.rem_euclid(self.d);
        if 2 * r >= self.d { q + 1 } else { q }
    }
}

impl PartialOrd for Frac {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Frac {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.n * other.d).cmp(&(other.n * self.d))
    }
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Least common multiple of two lattice denominators, or `None` when it
/// exceeds [`MAX_DEN`] (callers should then snap to a chosen denominator).
pub fn lcm_den(a: u8, b: u8) -> Option<u8> {
    let (a, b) = (a.max(1) as u64, b.max(1) as u64);
    let l = a / gcd(a, b) * b;
    if l <= MAX_DEN as u64 { Some(l as u8) } else { None }
}

// ---------------------------------------------------------------------------
// Sweep engine: even-odd parity per operand, y-slab trapezoids
// ---------------------------------------------------------------------------

type FPoint = (Frac, Frac);

/// A non-horizontal edge normalized to point downward, tagged with the
/// operand (0 or 1) it belongs to.
#[derive(Clone, Copy, Debug)]
struct SweepEdge {
    x1: Frac,
    y1: Frac,
    x2: Frac,
    y2: Frac,
    operand: u8,
}

impl SweepEdge {
    fn new(a: FPoint, b: FPoint, operand: u8) -> Option<Self> {
        if a.1 == b.1 {
            return None;
        }
        let (p, q) = if a.1 < b.1 { (a, b) } else { (b, a) };
        Some(Self { x1: p.0, y1: p.1, x2: q.0, y2: q.1, operand })
    }

    /// x coordinate at height `y` (`y1 <= y <= y2`).
    fn x_at(&self, y: Frac) -> Frac {
        self.x1
            .add(y.sub(self.y1).mul(self.x2.sub(self.x1)).div(self.y2.sub(self.y1)))
    }
}

fn ring_sweep_edges(ring: &[(u8, u8)], den: i64, operand: u8, out: &mut Vec<SweepEdge>) {
    let n = ring.len();
    for i in 0..n {
        let (x1, y1) = ring[i];
        let (x2, y2) = ring[(i + 1) % n];
        let a = (Frac::new(x1 as i64, den), Frac::new(y1 as i64, den));
        let b = (Frac::new(x2 as i64, den), Frac::new(y2 as i64, den));
        if let Some(e) = SweepEdge::new(a, b, operand) {
            out.push(e);
        }
    }
}

/// One filled x-interval of a y-slab, with exact corner coordinates.
#[derive(Clone, Debug)]
struct Trap {
    y_top: Frac,
    y_bot: Frac,
    xl_top: Frac,
    xl_bot: Frac,
    xr_top: Frac,
    xr_bot: Frac,
}

/// Sweep the plane and keep the set where `filled(in_a, in_b)` holds, with
/// `in_a`/`in_b` the even-odd interiors of the two operands' edge sets.
fn sweep(edges: &[SweepEdge], filled: &dyn Fn(bool, bool) -> bool) -> Vec<Trap> {
    // Slab cut heights: every endpoint plus every pairwise crossing height.
    let mut ys: Vec<Frac> = Vec::new();
    for e in edges {
        ys.push(e.y1);
        ys.push(e.y2);
    }
    for (i, a) in edges.iter().enumerate() {
        for b in &edges[i + 1..] {
            let lo = a.y1.max(b.y1);
            let hi = a.y2.min(b.y2);
            if lo >= hi {
                continue;
            }
            // Solve x_a(y) = x_b(y); both are linear in y.
            let adx = a.x2.sub(a.x1);
            let ady = a.y2.sub(a.y1);
            let bdx = b.x2.sub(b.x1);
            let bdy = b.y2.sub(b.y1);
            let denom = adx.mul(bdy).sub(bdx.mul(ady));
            if denom.is_zero() {
                continue; // parallel
            }
            // From a.x1 + (y - a.y1)·adx/ady = b.x1 + (y - b.y1)·bdx/bdy,
            // multiplied through by ady·bdy:
            let lhs = b
                .x1
                .sub(a.x1)
                .mul(ady)
                .mul(bdy)
                .add(a.y1.mul(adx).mul(bdy))
                .sub(b.y1.mul(bdx).mul(ady));
            let y = lhs.div(denom);
            if y > lo && y < hi {
                ys.push(y);
            }
        }
    }
    ys.sort();
    ys.dedup();

    let mut traps: Vec<Trap> = Vec::new();
    for w in ys.windows(2) {
        let (ya, yb) = (w[0], w[1]);
        let ym = ya.mid(yb);
        // (x at mid, x at top, x at bottom, operand) for edges spanning the slab.
        let mut xs: Vec<(Frac, Frac, Frac, u8)> = Vec::new();
        for e in edges {
            if e.y1 <= ya && e.y2 >= yb {
                xs.push((e.x_at(ym), e.x_at(ya), e.x_at(yb), e.operand));
            }
        }
        xs.sort_by(|p, q| p.0.cmp(&q.0));

        let mut in_a = false;
        let mut in_b = false;
        let mut run_start: Option<(Frac, Frac)> = None;
        for &(_, x_top, x_bot, operand) in &xs {
            let was = filled(in_a, in_b);
            if operand == 0 {
                in_a = !in_a;
            } else {
                in_b = !in_b;
            }
            let now = filled(in_a, in_b);
            if !was && now {
                run_start = Some((x_top, x_bot));
            } else if was && !now {
                let (xl_top, xl_bot) = run_start.take().expect("run must be open");
                if !(xl_top == x_top && xl_bot == x_bot) {
                    traps.push(Trap {
                        y_top: ya,
                        y_bot: yb,
                        xl_top,
                        xl_bot,
                        xr_top: x_top,
                        xr_bot: x_bot,
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
fn traps_to_rings(traps: &[Trap]) -> (u8, Vec<Vec<(u8, u8)>>) {
    let mut side_segs: Vec<(FPoint, FPoint)> = Vec::new();
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
            side_segs.push((rt, rb));
        }
        if lb != lt {
            side_segs.push((lb, lt));
        }
    }

    // Resolve horizontal borders into net directed segments.
    let mut segs: Vec<(FPoint, FPoint)> = Vec::new();
    for (y, row) in &horiz {
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
                    segs.push(((sx, *y), (x, *y)));
                } else {
                    segs.push(((x, *y), (sx, *y)));
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

    let rings: Vec<Vec<FPoint>> = link_rings(segs).into_iter().map(simplify_exact).collect();

    // Pick the smallest lattice that represents every remaining vertex
    // exactly, clamped to MAX_DEN (beyond which vertices get rounded).
    let mut den: u64 = 1;
    for ring in &rings {
        for &(x, y) in ring {
            for d in [x.d as u64, y.d as u64] {
                den = den / gcd(den, d) * d;
                if den > MAX_DEN as u64 {
                    den = MAX_DEN as u64;
                }
            }
        }
    }
    let den = den as i64;

    let mut out = Vec::new();
    for ring in rings {
        let snapped: Vec<(i64, i64)> = ring
            .iter()
            .map(|&(x, y)| (x.round_to_den(den).clamp(0, den), y.round_to_den(den).clamp(0, den)))
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
/// pairs first (shared borders of adjacent trapezoids).
fn link_rings(segs: Vec<(FPoint, FPoint)>) -> Vec<Vec<FPoint>> {
    let mut counter: HashMap<(FPoint, FPoint), i32> = HashMap::new();
    for (a, b) in segs {
        if a == b {
            continue;
        }
        if let Some(c) = counter.get_mut(&(b, a)) {
            *c -= 1;
            if *c == 0 {
                counter.remove(&(b, a));
            }
            continue;
        }
        *counter.entry((a, b)).or_insert(0) += 1;
    }
    let mut by_start: BTreeMap<FPoint, Vec<FPoint>> = BTreeMap::new();
    for ((a, b), c) in counter {
        debug_assert!(c > 0);
        for _ in 0..c {
            by_start.entry(a).or_default().push(b);
        }
    }
    for list in by_start.values_mut() {
        list.sort();
    }

    let mut rings = Vec::new();
    while let Some((&start, _)) = by_start.first_key_value() {
        let mut ring = vec![start];
        let mut cur = start;
        loop {
            let Some(nexts) = by_start.get_mut(&cur) else {
                debug_assert!(false, "open chain in ring linking");
                break;
            };
            let next = nexts.pop().expect("empty successor list");
            if nexts.is_empty() {
                by_start.remove(&cur);
            }
            if next == start {
                break;
            }
            ring.push(next);
            cur = next;
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    rings
}

/// Remove exactly-collinear intermediate vertices (exact arithmetic).
fn simplify_exact(ring: Vec<FPoint>) -> Vec<FPoint> {
    let n = ring.len();
    let mut out: Vec<FPoint> = Vec::with_capacity(n);
    for i in 0..n {
        let p = ring[(i + n - 1) % n];
        let q = ring[i];
        let r = ring[(i + 1) % n];
        // cross = (q - p) × (r - p), exact.
        let cross = q
            .0
            .sub(p.0)
            .mul(r.1.sub(p.1))
            .sub(q.1.sub(p.1).mul(r.0.sub(p.0)));
        if !cross.is_zero() {
            out.push(q);
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
    pub const EMPTY: DetailRegion = DetailRegion { den: 1, rings: Vec::new() };

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

    /// Twice the filled area in unit-square terms. Only meaningful for
    /// canonical regions (test/diagnostic helper) (sweep-derived rings: outer rings and holes are
    /// wound oppositely, so the signed shoelace sum is the even-odd area).
    #[cfg_attr(not(test), expect(dead_code))]
    pub fn area2(&self) -> f64 {
        let mut a = 0i64;
        for ring in &self.rings {
            let ring: Vec<(i64, i64)> =
                ring.iter().map(|&(x, y)| (x as i64, y as i64)).collect();
            a += lattice_ring_area2(&ring);
        }
        a.abs() as f64 / (self.den as f64 * self.den as f64)
    }

    /// Re-derive the region through the sweep engine. The output ring set is
    /// a deterministic function of the filled point set, which makes it a
    /// canonical form: two regions describing the same set produce identical
    /// rings.
    fn resweep(&self) -> DetailRegion {
        let mut edges = Vec::new();
        for ring in &self.rings {
            ring_sweep_edges(ring, self.den as i64, 0, &mut edges);
        }
        let traps = sweep(&edges, &|a, _| a);
        let (den, rings) = traps_to_rings(&traps);
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
        let mut rings = Vec::new();
        for ring in &self.rings {
            let snapped: Vec<(i64, i64)> = ring
                .iter()
                .map(|&(x, y)| {
                    (
                        Frac::new(x as i64, self.den as i64).round_to_den(d).clamp(0, d),
                        Frac::new(y as i64, self.den as i64).round_to_den(d).clamp(0, d),
                    )
                })
                .collect();
            let snapped = simplify_lattice(dedup_cyclic(snapped));
            if snapped.len() >= 3 && lattice_ring_area2(&snapped) != 0 {
                rings.push(snapped.into_iter().map(|(x, y)| (x as u8, y as u8)).collect());
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
                g = gcd(g, x as u64);
                g = gcd(g, y as u64);
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
        if canon.den <= 2 {
            if let Some(&id) = classify_index().get(&canon) {
                return if id == PX_ALMOSTFULL {
                    Classified::Full
                } else {
                    Classified::Shape(id)
                };
            }
        }
        Classified::Custom(canon)
    }

    /// Complement within the unit square.
    pub fn complement(&self) -> DetailRegion {
        bool_op(&DetailRegion::full(), self, BoolOp::Subtract)
    }

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

    pub fn mirror_h(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (d - x, y))
    }

    pub fn flip_v(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (x, d - y))
    }

    pub fn rotate_cw(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (d - y, x))
    }

    pub fn rotate_ccw(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (y, d - x))
    }

    pub fn rotate_180(&self) -> DetailRegion {
        self.map_lattice(|x, y, d| (d - x, d - y))
    }

    /// Map this region into the rectangle `[x0, x0+w] × [y0, y0+h]` of a
    /// destination unit square and clip to that square. Used by exact
    /// rescaling: a source pixel occupies a rational sub-rectangle of each
    /// destination pixel it overlaps. The output lattice is chosen
    /// automatically (exact up to [`MAX_DEN`]).
    pub fn transform_into(&self, x0: Frac64, y0: Frac64, w: Frac64, h: Frac64) -> DetailRegion {
        let den = self.den as i64;
        let fx0 = Frac::new(x0.n, x0.d);
        let fy0 = Frac::new(y0.n, y0.d);
        let fw = Frac::new(w.n, w.d);
        let fh = Frac::new(h.n, h.d);
        let mut edges = Vec::new();
        for ring in &self.rings {
            let n = ring.len();
            for i in 0..n {
                let map = |p: (u8, u8)| {
                    (
                        fx0.add(Frac::new(p.0 as i64, den).mul(fw)),
                        fy0.add(Frac::new(p.1 as i64, den).mul(fh)),
                    )
                };
                if let Some(e) = SweepEdge::new(map(ring[i]), map(ring[(i + 1) % n]), 0) {
                    edges.push(e);
                }
            }
        }
        let unit = DetailRegion::full();
        for ring in &unit.rings {
            ring_sweep_edges(ring, unit.den as i64, 1, &mut edges);
        }
        let traps = sweep(&edges, &|a, b| a && b);
        let (den, rings) = traps_to_rings(&traps);
        DetailRegion { den, rings }
    }
}

/// Clip an arbitrary polygon (in pixel-local coordinates, possibly far
/// outside the unit square) to the unit square. Used to cut per-pixel
/// pieces out of large synthetic shapes such as on-demand triangles.
pub fn clip_polygon_to_cell(pts: &[(Frac64, Frac64)]) -> DetailRegion {
    if pts.len() < 3 {
        return DetailRegion::EMPTY;
    }
    let mut edges = Vec::new();
    let n = pts.len();
    for i in 0..n {
        let a = (Frac::new(pts[i].0.n, pts[i].0.d), Frac::new(pts[i].1.n, pts[i].1.d));
        let b = (
            Frac::new(pts[(i + 1) % n].0.n, pts[(i + 1) % n].0.d),
            Frac::new(pts[(i + 1) % n].1.n, pts[(i + 1) % n].1.d),
        );
        if let Some(e) = SweepEdge::new(a, b, 0) {
            edges.push(e);
        }
    }
    let unit = DetailRegion::full();
    for ring in &unit.rings {
        ring_sweep_edges(ring, unit.den as i64, 1, &mut edges);
    }
    let traps = sweep(&edges, &|a, b| a && b);
    let (den, rings) = traps_to_rings(&traps);
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

    let mut edges = Vec::new();
    for ring in &a.rings {
        ring_sweep_edges(ring, a.den as i64, 0, &mut edges);
    }
    for ring in &b.rings {
        ring_sweep_edges(ring, b.den as i64, 1, &mut edges);
    }
    let traps = sweep(&edges, &|x, y| filled(x, y));
    let (den, rings) = traps_to_rings(&traps);
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
        let mut cov = EdgeCoverageExact { den: self.den, ..Default::default() };
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
                    segs.push((x1 as f32 / df, y1 as f32 / df, x2 as f32 / df, y2 as f32 / df));
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

    #[test]
    fn from_shape_areas() {
        assert_eq!(DetailRegion::from_shape(PX_HALF1).area2(), 1.0);
        assert_eq!(DetailRegion::from_shape(PX_QUAD2).area2(), 0.5);
        assert_eq!(DetailRegion::from_shape(PX_SLANT1H).area2(), 0.5);
        assert_eq!(DetailRegion::from_shape(PX_CORNER1).area2(), 0.25);
        assert_eq!(DetailRegion::from_shape(PX_HQUAD).area2(), 1.0);
        assert_eq!(DetailRegion::from_shape(PX_DOT).area2(), 1.0);
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
            assert_eq!(u.classify(), Classified::Full, "shape {id} ∪ complement != full");
            let i = bool_op(&a, &b, BoolOp::Intersect);
            assert_eq!(i.classify(), Classified::Empty, "shape {id} ∩ complement != empty");
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
        assert_eq!(bool_op(&tri, &comp, BoolOp::Union).classify(), Classified::Full);
        assert_eq!(bool_op(&tri, &comp, BoolOp::Intersect).classify(), Classified::Empty);
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
                list.iter().map(|&(a, b)| (a as f32 / den, b as f32 / den)).collect()
            };
            let from_iv = |iv: &crate::pixel::EdgeInterval| -> Vec<(f32, f32)> {
                if iv.is_empty() { Vec::new() } else { vec![(iv.start, iv.end)] }
            };
            assert_eq!(to_f(&exact.top), from_iv(&table.top), "top coverage of id {id}");
            assert_eq!(to_f(&exact.bottom), from_iv(&table.bottom), "bottom coverage of id {id}");
            assert_eq!(to_f(&exact.left), from_iv(&table.left), "left coverage of id {id}");
            assert_eq!(to_f(&exact.right), from_iv(&table.right), "right coverage of id {id}");
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
