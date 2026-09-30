//! The arithmetic a split's planners share: walking a line, scoring clearances
//! and arranging the gaps.

use super::inventory::Candidate;
#[cfg(doc)]
use super::inventory::Inventory;
#[cfg(doc)]
use super::pattern::Member;
use crate::compose::effective_facing;
use crate::document::{ComposeItem, GlyphCompose};

/// One glyph's clearances as a function of the gaps: `(base, total)`, the
/// affine form [`Member`] documents. `None` when two neighbours share no line
/// on which both draw.
pub(super) fn affine_layout(
    parts: &[&Candidate],
    axis_extent: i32,
    horizontal: bool,
    contact: Option<u16>,
) -> Option<(Vec<i32>, i32)> {
    let last = parts.len() - 1;
    let mut base = vec![parts[0].frontier.near];
    let mut total = parts[0].frontier.near + (axis_extent - 1 - parts[last].frontier.far);
    for pair in parts.windows(2) {
        let facing = effective_facing(pair[0].side(), pair[1].side(), horizontal, contact)?;
        base.push(pair[0].extent + facing);
        total += facing;
    }
    Some((base, total))
}

/// Where the line's own gaps put each part, in declared units.
pub(super) fn walk(compose: &GlyphCompose, parts: &[Candidate]) -> Vec<i32> {
    let mut at = 0i32;
    let mut out = Vec::with_capacity(parts.len());
    for item in &compose.items {
        match item {
            ComposeItem::Gap(gap) => at += *gap as i32,
            ComposeItem::Part { .. } | ComposeItem::Nested(_) => {
                out.push(at);
                at += parts[out.len() - 1].extent;
            }
        }
    }
    out
}

/// The n+1 clearances of parts placed at `positions`. `None` when two
/// neighbours share no line on which both draw.
pub(super) fn clearances_at(
    parts: &[&Candidate],
    positions: &[i32],
    axis_extent: i32,
    horizontal: bool,
    contact: Option<u16>,
) -> Option<Vec<i32>> {
    let last = parts.len() - 1;
    let mut out = vec![positions[0] + parts[0].frontier.near];
    for i in 0..last {
        let facing = effective_facing(parts[i].side(), parts[i + 1].side(), horizontal, contact)?;
        out.push(positions[i + 1] - positions[i] + facing);
    }
    out.push(axis_extent - 1 - (positions[last] + parts[last].frontier.far));
    Some(out)
}

/// Whether a part this long may be *proposed* for a glyph whose own axis is
/// `along` long: [`crate::compose::fits_axis`], which is where the rule and the
/// reason for it live.
///
/// It is a bound on what may be *offered*, not on what may be written: the
/// component as the line already writes it stays a candidate whatever its size,
/// since it is the source's own choice rather than a proposal — the same rule
/// [`Inventory::candidates`] states for a drawing made for the wrong slot.
pub(super) fn fits_beside(extent: i32, along: i32) -> bool {
    crate::compose::fits_axis(extent, along)
}

/// How far `v` is from the inclusive range; 0 inside it.
pub(super) fn distance(v: i32, lo: i32, hi: i32) -> i32 {
    (lo - v).max(v - hi).max(0)
}

/// What the check would warn about, as one number: every clearance's distance
/// from the range, plus their total's — the total being held to the range too
/// (see [`crate::compose`] for why).
pub(super) fn score(clearances: &[i32], lo: i32, hi: i32) -> i32 {
    let total: i32 = clearances.iter().sum();
    clearances.iter().map(|c| distance(*c, lo, hi)).sum::<i32>() + distance(total, lo, hi)
}

/// What the optimizer minimizes, in half cells: the check's [`score`], plus one
/// and a half for every cell the parts' *boxes* leave of the glyph at its two
/// edges — `lead`, the gap written before the first part, and `trail`, what the
/// last one leaves at the far end. A box that overhangs the glyph's costs
/// nothing here: sources do that on purpose, to pull a part drawn with a wide
/// bearing in against the edge, and the score already says whether it went too
/// far.
///
/// Room written at an edge reads as the whole glyph shoved to one side, which
/// is worse than the same room between the parts even where the range would
/// allow it at the edge and not in the middle: `a 2 b` beats `a 1 b` with a
/// cell left over, although the check counts one more cell against it. It is
/// the boxes and not the ink that are held to the edges, since a part drawn
/// with a bearing of its own was drawn that way on purpose.
pub(super) fn cost(score: i32, lead: i32, trail: i32) -> i32 {
    2 * score + 3 * (lead.max(0) + trail.max(0))
}

/// The `n` clearances summing to `total` that the module's rules pick: the
/// least [`cost`], then in at the edges, then even in the middle, then
/// lexicographically least. `bearings` is what the first part's ink leaves
/// before its box starts and the last part's after its box ends: the edge
/// clearances at which the boxes sit flush with the glyph's.
///
/// The total is fixed, so [`cost`]'s own term for it is, and the rest is a sum
/// of one convex function per clearance: `2·distance` for an inner one, and
/// that plus `3·max(0, c - bearing)` for an edge. Such a sum under a fixed total is
/// minimized greedily, a cell at a time from every clearance at its own
/// cheapest: each cell goes where it costs least — or comes from where taking
/// it costs least — and on a tie to wherever the later rules want it: into
/// the middle rather than an edge, the smaller inner clearance first (the later
/// of two equal ones), the far edge before the near one; and taken out of the
/// edges first, near before far, then the larger inner clearance. Where a
/// clearance's cheapest is a range rather than one value, it starts at the end
/// those rules favour: an inner one at `hi`, an edge at the low end.
pub(super) fn arrange(n: usize, total: i32, lo: i32, hi: i32, bearings: (i32, i32)) -> Vec<i32> {
    debug_assert!(n >= 3, "an IDC line has at least two parts");
    let each = |i: usize, c: i32| {
        let edge = match i {
            0 => 3 * (c - bearings.0).max(0),
            _ if i == n - 1 => 3 * (c - bearings.1).max(0),
            _ => 0,
        };
        2 * distance(c, lo, hi) + edge
    };
    // An edge is cheapest from `lo` up to the bearing, or at the bearing alone
    // when that is short of the range.
    let low_edge = |bearing: i32| bearing.min(lo);
    let mut out: Vec<i32> = (0..n)
        .map(|i| match i {
            0 => low_edge(bearings.0),
            _ if i == n - 1 => low_edge(bearings.1),
            _ => hi,
        })
        .collect();
    let mut sum: i32 = out.iter().sum();
    let inner: Vec<usize> = (1..n - 1).collect();
    while sum != total {
        let step = (total - sum).signum();
        let mut order = inner.clone();
        if step > 0 {
            order.sort_by_key(|&i| (out[i], std::cmp::Reverse(i)));
            order.extend([n - 1, 0]);
        } else {
            order.sort_by_key(|&i| (std::cmp::Reverse(out[i]), i));
            order.splice(0..0, [0, n - 1]);
        }
        let pick = order
            .into_iter()
            .min_by_key(|&i| each(i, out[i] + step) - each(i, out[i]))
            .expect("an IDC line has clearances");
        out[pick] += step;
        sum += step;
    }
    out
}
