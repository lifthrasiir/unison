//! Making the two builds' outlines *point-compatible*, so one glyph can carry
//! both drawings.
//!
//! The bitmap and the vector build draw the same glyph differently (see this
//! crate's module docs). To ship them as one variable glyph rather than two
//! fonts, the two outlines have to agree on their **shape**: the same number of
//! contours, and the same number of points in each, so that point *i* of one is
//! point *i* of the other and a `gvar` delta can carry one into the other.
//!
//! Neither drawing may change in the process. Two padding moves are enough, and
//! both are shape-preserving by construction:
//!
//! - **A duplicated point.** Repeating a point in place adds a zero-length
//!   segment, which no rasterizer fills. It is how a contour of *n* points is
//!   stretched to the *L* steps of an alignment.
//! - **A degenerate contour.** A contour whose every point sits at one place
//!   has zero area and is drawn by nobody, so it stands in for a contour the
//!   other master has and this one does not.
//!
//! # Why the alignment is a walk and not arc length
//!
//! The obvious correspondence — normalized arc length — is wrong for exactly
//! the case that matters. A 45° diagonal and the staircase that rounds it have
//! the same endpoints but different lengths (`√2` against `2` per cell), so
//! equal arc-length fractions name different corners and every delta comes out
//! skewed. What actually corresponds is *position*: a staircase corner belongs
//! with the part of the diagonal it rounds.
//!
//! So the correspondence is a monotone alignment minimizing the distance
//! between paired points — the same shape as dynamic time warping, and monotone
//! for the same reason: an alignment that goes backwards would fold the outline
//! through itself partway between the two masters.
//!
//! # What the alignment is *not* asked to be
//!
//! Only the two ends are ever rendered. The axis carrying these masters is a
//! switch whose tent is narrow enough that no intermediate is reachable, so a
//! mediocre correspondence costs delta *size* and never correctness. That is
//! why a cheap greedy pairing below is enough, and why nothing here tries to
//! keep the interpolation free of self-intersection.
//!
//! # What it costs
//!
//! Over `font/` as it stands: 12,270 simple glyphs, 537,822 vector points and
//! 665,533 bitmap ones, compatible at 784,703 — 1.46× the vector master, and
//! 15% above the `max(n, m)` a perfect alignment would reach. Nearly all of
//! that 15% is real: penalizing a stall to push the walk toward diagonal steps
//! recovers 1.4% of it and no more, so the rest is places where the two
//! drawings genuinely disagree about where their corners are. The whole pass
//! takes ~180 ms for the font, against a ~1.7 s build.

/// A contour's signed area, doubled — sign is the winding direction.
fn signed_area2(c: &[(i16, i16)]) -> i64 {
    let n = c.len();
    if n < 3 {
        return 0;
    }
    (0..n)
        .map(|i| {
            let (x0, y0) = c[i];
            let (x1, y1) = c[(i + 1) % n];
            x0 as i64 * y1 as i64 - x1 as i64 * y0 as i64
        })
        .sum()
}

fn dist2(a: (i16, i16), b: (i16, i16)) -> i64 {
    let dx = a.0 as i64 - b.0 as i64;
    let dy = a.1 as i64 - b.1 as i64;
    dx * dx + dy * dy
}

/// The point every collapsed copy of `c` sits at: its centroid, rounded. Any
/// place would be correct — the contour has no area either way — but the
/// centroid keeps the deltas that carry it there as short as they can be.
fn collapse_point(c: &[(i16, i16)]) -> (i16, i16) {
    if c.is_empty() {
        return (0, 0);
    }
    let n = c.len() as i64;
    let sx: i64 = c.iter().map(|p| p.0 as i64).sum();
    let sy: i64 = c.iter().map(|p| p.1 as i64).sum();
    // Round half away from zero, so a centroid does not drift toward the
    // origin as a side effect of integer division.
    let round = |s: i64| -> i16 {
        let q = if s >= 0 {
            (s * 2 + n) / (2 * n)
        } else {
            (s * 2 - n) / (2 * n)
        };
        q.clamp(i16::MIN as i64, i16::MAX as i64) as i16
    };
    (round(sx), round(sy))
}

/// Where the two contours are to be walked from: the closest pair of vertices.
///
/// A cyclic alignment has to start somewhere, and starting at two points that
/// genuinely correspond is what keeps the rest of the walk honest. The two
/// masters share exact vertices wherever an edge was already on the pixel grid,
/// so this usually finds distance zero.
fn best_start(p: &[(i16, i16)], q: &[(i16, i16)]) -> (usize, usize) {
    let mut best = (0usize, 0usize);
    let mut best_d = i64::MAX;
    for (i, &a) in p.iter().enumerate() {
        for (j, &b) in q.iter().enumerate() {
            let d = dist2(a, b);
            if d < best_d {
                best_d = d;
                best = (i, j);
                if d == 0 {
                    return best;
                }
            }
        }
    }
    best
}

/// Pad `p` and `q` to one common length by repeating points, so that index `k`
/// of each names the same place on the glyph.
///
/// The returned pair always has equal lengths, and each side simplifies back to
/// what it was: only whole points are repeated, never moved or dropped.
fn align_pair(p: &[(i16, i16)], q: &[(i16, i16)]) -> (Vec<(i16, i16)>, Vec<(i16, i16)>) {
    let (n, m) = (p.len(), q.len());
    if n == 0 || m == 0 {
        // Nothing to align against: the empty side becomes a collapsed copy of
        // the other, which is the degenerate-contour rule one contour down.
        let at = collapse_point(if n == 0 { q } else { p });
        return if n == 0 {
            (vec![at; m], q.to_vec())
        } else {
            (p.to_vec(), vec![at; n])
        };
    }

    let (si, sj) = best_start(p, q);
    let pr: Vec<(i16, i16)> = (0..n).map(|k| p[(si + k) % n]).collect();
    let qr: Vec<(i16, i16)> = (0..m).map(|k| q[(sj + k) % m]).collect();

    // Monotone alignment, cost = squared distance between the paired points.
    // `(0, 0)` and `(n-1, m-1)` are pinned: the walk starts at the chosen pair
    // and closes back onto it.
    let idx = |a: usize, b: usize| a * m + b;
    let mut cost = vec![i64::MAX; n * m];
    let mut from = vec![0u8; n * m]; // 0 = diagonal, 1 = p advanced, 2 = q advanced
    cost[0] = dist2(pr[0], qr[0]);
    for a in 0..n {
        for b in 0..m {
            if a == 0 && b == 0 {
                continue;
            }
            let d = dist2(pr[a], qr[b]);
            let mut best = i64::MAX;
            let mut step = 0u8;
            let consider = |prev: i64, s: u8, best: &mut i64, step: &mut u8| {
                if prev != i64::MAX && prev < *best {
                    *best = prev;
                    *step = s;
                }
            };
            if a > 0 && b > 0 {
                consider(cost[idx(a - 1, b - 1)], 0, &mut best, &mut step);
            }
            if a > 0 {
                consider(cost[idx(a - 1, b)], 1, &mut best, &mut step);
            }
            if b > 0 {
                consider(cost[idx(a, b - 1)], 2, &mut best, &mut step);
            }
            cost[idx(a, b)] = best.saturating_add(d);
            from[idx(a, b)] = step;
        }
    }

    let (mut a, mut b) = (n - 1, m - 1);
    let mut out_p = Vec::new();
    let mut out_q = Vec::new();
    loop {
        out_p.push(pr[a]);
        out_q.push(qr[b]);
        if a == 0 && b == 0 {
            break;
        }
        match from[idx(a, b)] {
            0 => {
                a -= 1;
                b -= 1;
            }
            1 => a -= 1,
            _ => b -= 1,
        }
    }
    out_p.reverse();
    out_q.reverse();
    (out_p, out_q)
}

/// One glyph's two drawings, made point-compatible.
///
/// `vector[i][k]` and `bitmap[i][k]` are the same point of the same contour in
/// the two masters, which is the whole contract: `gvar` needs nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MasterPair {
    pub(super) vector: Vec<Vec<(i16, i16)>>,
    pub(super) bitmap: Vec<Vec<(i16, i16)>>,
}

impl MasterPair {
    /// True when the two sides really do have the same shape. The invariant the
    /// rest of the pipeline may assume, asserted rather than trusted.
    #[cfg_attr(not(test), expect(dead_code))]
    pub(super) fn is_compatible(&self) -> bool {
        self.vector.len() == self.bitmap.len()
            && self
                .vector
                .iter()
                .zip(&self.bitmap)
                .all(|(v, b)| v.len() == b.len())
    }
}

/// Which contour of one master stands for which contour of the other.
///
/// Greedy, and deliberately so — see the module docs on what the alignment is
/// not asked to be. Two constraints do the real work:
///
/// - **Winding must agree.** An outer contour paired with a hole would turn one
///   into the other partway across, and the two masters are the same drawing
///   rounded differently, so a piece that is solid in one is solid in both.
/// - **Shared vertices come first.** Wherever an edge already sat on the pixel
///   grid the rounding left it alone, so the two contours literally share those
///   points; counting them finds the intended pairing far more reliably than
///   any distance does.
fn pair_contours(v: &[Vec<(i16, i16)>], b: &[Vec<(i16, i16)>]) -> Vec<(Option<usize>, Option<usize>)> {
    let shared = |x: &[(i16, i16)], y: &[(i16, i16)]| -> usize {
        let set: std::collections::HashSet<(i16, i16)> = x.iter().copied().collect();
        y.iter().filter(|p| set.contains(p)).count()
    };
    let centroid = |c: &[(i16, i16)]| collapse_point(c);

    let mut candidates: Vec<(usize, i64, usize, usize)> = Vec::new();
    for (i, vc) in v.iter().enumerate() {
        for (j, bc) in b.iter().enumerate() {
            if (signed_area2(vc) < 0) != (signed_area2(bc) < 0) {
                continue;
            }
            candidates.push((shared(vc, bc), dist2(centroid(vc), centroid(bc)), i, j));
        }
    }
    // Most shared points first, then closest centroids, then source order so
    // the result does not depend on how the candidates happened to be built.
    candidates.sort_by(|x, y| {
        y.0.cmp(&x.0)
            .then(x.1.cmp(&y.1))
            .then(x.2.cmp(&y.2))
            .then(x.3.cmp(&y.3))
    });

    let mut v_taken = vec![false; v.len()];
    let mut b_taken = vec![false; b.len()];
    let mut pairs: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    for &(_, _, i, j) in &candidates {
        if !v_taken[i] && !b_taken[j] {
            v_taken[i] = true;
            b_taken[j] = true;
            pairs.push((Some(i), Some(j)));
        }
    }
    pairs.extend((0..v.len()).filter(|&i| !v_taken[i]).map(|i| (Some(i), None)));
    pairs.extend((0..b.len()).filter(|&j| !b_taken[j]).map(|j| (None, Some(j))));
    pairs
}

/// Make one glyph's two drawings point-compatible, changing neither.
///
/// The result always satisfies [`MasterPair::is_compatible`], and each side
/// draws exactly what it drew before: every added point is a repeat of one
/// already there, and every added contour has zero area.
#[cfg_attr(not(test), expect(dead_code))]
pub(super) fn compatible_masters(
    vector: &[Vec<(i16, i16)>],
    bitmap: &[Vec<(i16, i16)>],
) -> MasterPair {
    let mut out = MasterPair {
        vector: Vec::new(),
        bitmap: Vec::new(),
    };
    for (vi, bi) in pair_contours(vector, bitmap) {
        match (vi, bi) {
            (Some(i), Some(j)) => {
                let (p, q) = align_pair(&vector[i], &bitmap[j]);
                out.vector.push(p);
                out.bitmap.push(q);
            }
            // A contour only one master has: the other gets it collapsed.
            (Some(i), None) => {
                let c = &vector[i];
                out.bitmap.push(vec![collapse_point(c); c.len()]);
                out.vector.push(c.clone());
            }
            (None, Some(j)) => {
                let c = &bitmap[j];
                out.vector.push(vec![collapse_point(c); c.len()]);
                out.bitmap.push(c.clone());
            }
            (None, None) => unreachable!("pair_contours never emits an empty pair"),
        }
    }
    out
}
