//! Planning one enclosure line: the placements are searched where a split's gaps
//! are solved.

use super::PlannedLine;
use super::inventory::{EnclosurePart, EnclosureSlot, Inventory};
#[cfg(doc)]
use super::layout::arrange;
use super::layout::distance;
use super::names::is_plain_name;
#[cfg(doc)]
use super::split::Key;
use crate::document::{ComposeItem, GlyphCompose};

/// How many placements one variant pair may be tried at, and how much work a
/// whole enclosure line may be. A 16x16 glyph holding a 4x4 part has 169
/// placements and a handful of variant pairs, so no real line comes near
/// either; the caps are there so that an unexpected inventory cannot turn one
/// line into an afternoon.
pub(super) const MAX_ENCLOSURE_PLACEMENTS: usize = 4_096;
pub(super) const MAX_ENCLOSURE_WORK: usize = 1_048_576;

/// How the optimizer orders two answers for an enclosure line. Derived `Ord`
/// is the whole rule, and the fields are the split's own
/// ([`Key`]) with the two that mean something different here changed:
///
/// - **`edge_sum`** is over the clearances that touch the glyph's *own*
///   boundary, which for an enclosure is one per open side and none at all on
///   a `⿴`. Minimizing it is the same statement it is on a split — push the
///   parts out against the box — read on the sides the operator leaves open;
/// - **`inner_spread`** is over the axes with no open side, where there is
///   nothing to push against. Evening the two out is what centres the inner
///   part of a `⿴`, and without it the lexicographic rule below would wedge it
///   into a corner of the ring and call that an answer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EnclosureKey {
    score: i32,
    mismatched: usize,
    directed: std::cmp::Reverse<usize>,
    /// The clearances that run to a side the operator leaves open, added.
    edge_sum: i32,
    /// How far apart an axis's two clearances are, over the axes walled on
    /// both sides.
    inner_spread: i32,
    clearances: Vec<i32>,
    /// `false` — the line as written — sorts first.
    changed: bool,
    names: Vec<String>,
    /// Last, so that two answers alike in every way still order.
    placement: (i32, i32),
}

/// What the check would warn about on an enclosure line, as one number: every
/// clearance's distance from the range, plus **each axis's** total's.
///
/// Per axis and not over all four, for the reason
/// [`crate::compose::measure_enclosure_clearances`] gives: the two sums say two
/// different things, and one number made of both would be neither.
pub(super) fn enclosure_score(clearances: &[crate::compose::Clearance], lo: i32, hi: i32) -> i32 {
    let per_gap: i32 = clearances
        .iter()
        .map(|c| distance(c.value, lo, hi))
        .sum::<i32>();
    let per_axis: i32 = [true, false]
        .into_iter()
        .map(|h| {
            let total: i32 = clearances
                .iter()
                .filter(|c| c.horizontal == h)
                .map(|c| c.value)
                .sum();
            distance(total, lo, hi)
        })
        .sum();
    per_gap + per_axis
}

/// Score one placement of one variant pair.
#[allow(clippy::too_many_arguments)]
fn evaluate_enclosure(
    outer: &EnclosurePart,
    inner: &EnclosurePart,
    written: &[&str],
    walls: crate::compose::Walls,
    parent: (u16, u16),
    at: (i32, i32),
    lo: i32,
    hi: i32,
    contact: Option<u16>,
) -> Option<EnclosureKey> {
    let clearances = crate::compose::measure_enclosure_clearances(
        walls,
        parent,
        (&outer.name, &outer.profile, outer.at),
        (&inner.name, &inner.profile),
        at,
        contact,
    )?;
    let values: Vec<i32> = clearances.iter().map(|c| c.value).collect();
    let edge_sum: i32 = clearances
        .iter()
        .filter(|c| c.at_edge)
        .map(|c| c.value)
        .sum();
    let inner_spread: i32 = [true, false]
        .into_iter()
        .filter(|&h| walls.open_count(h) == 0)
        .map(|h| {
            let pair: Vec<i32> = clearances
                .iter()
                .filter(|c| c.horizontal == h)
                .map(|c| c.value)
                .collect();
            match pair.as_slice() {
                [a, b] => (a - b).abs(),
                _ => 0,
            }
        })
        .sum();
    let names = [&outer.name, &inner.name];
    Some(EnclosureKey {
        score: enclosure_score(&clearances, lo, hi),
        mismatched: [outer.rank, inner.rank].iter().filter(|&&r| r == 2).count(),
        directed: std::cmp::Reverse([outer.rank, inner.rank].iter().filter(|&&r| r == 0).count()),
        edge_sum,
        inner_spread,
        clearances: values,
        changed: names.iter().zip(written).any(|(a, b)| a.as_str() != *b),
        names: names.iter().map(|n| (*n).clone()).collect(),
        placement: at,
    })
}

/// Plan one enclosure line, or `None` when it does not warn, cannot be
/// measured, or cannot be improved.
///
/// # Why the placements are searched where a split's gaps are not
///
/// A split's clearances sum to a number that mentions no position, which is
/// what lets [`arrange`] solve the gaps instead of trying them. An enclosure's
/// do too — *per axis* — but the two axes are not independent: how much room
/// the left wall leaves depends on which **rows** the inner part covers, and
/// that is the other axis's answer. A `⿴` 囗 does not care and a `⿺` 辶 cares a
/// great deal, and there is no arithmetic that is right for both.
///
/// So the offsets are tried rather than solved. That is affordable exactly
/// because they are *offsets*: a split's gap is any integer, while an inner
/// part has to sit inside the glyph, so the search is over a box a few cells on
/// a side rather than over the integers. A part that has to overhang the box is
/// the author's to write, and the line as written is always a candidate.
pub(super) fn optimize_enclosure_line(
    inv: &Inventory,
    walls: crate::compose::Walls,
    parent: (u16, u16),
    compose: &GlyphCompose,
    lo: i32,
    hi: i32,
    contact: Option<u16>,
) -> Option<PlannedLine> {
    let written: Vec<&str> = compose.part_names().collect();
    if written.len() != 2 || written.iter().any(|n| !is_plain_name(n)) {
        return None;
    }
    // The line has to be one this file could write back: two components and
    // then their offsets. Anything else is an error the source answers first.
    let mut offsets: Vec<i32> = Vec::new();
    let mut seen_parts = 0usize;
    for item in &compose.items {
        match item {
            ComposeItem::Gap(n) => {
                if seen_parts < 2 {
                    return None;
                }
                offsets.push(*n as i32);
            }
            ComposeItem::Part { .. } => seen_parts += 1,
            ComposeItem::Nested(_) => return None,
        }
    }
    if !matches!(offsets.len(), 0 | 2) {
        return None;
    }
    let placed = (offsets.len() == 2).then(|| (offsets[0], offsets[1]));

    // As written — when there is a "as written" at all. An undecided component
    // or an unwritten placement leaves the line with no layout to have measured
    // badly, exactly as on a split, and it is planned whatever it scores.
    let mut faulty = false;
    let current: Option<[EnclosurePart; 2]> = match placed {
        None => None,
        Some(_) if written.iter().any(|n| crate::compose::is_undecided(n)) => None,
        Some(_) => {
            let mut parts: Vec<EnclosurePart> = Vec::with_capacity(2);
            for (slot, name) in written.iter().enumerate() {
                match inv.enclosure_slot(name, parent, slot == 0) {
                    EnclosureSlot::Ok(part) => parts.push(*part),
                    EnclosureSlot::Faulty => faulty = true,
                    EnclosureSlot::Unmeasurable => return None,
                }
            }
            match (faulty, <[EnclosurePart; 2]>::try_from(parts)) {
                (false, Ok(pair)) => Some(pair),
                _ => None,
            }
        }
    };
    let before = match (&current, placed) {
        (Some([outer, inner]), Some(at)) => {
            let key =
                evaluate_enclosure(outer, inner, &written, walls, parent, at, lo, hi, contact)?;
            if (key.score, key.mismatched) == (0, 0) {
                return None; // nothing warns, so nothing to fix
            }
            Some((key.score, key.mismatched))
        }
        _ => None,
    };

    let outers = inv.enclosure_candidates(written[0], parent, true);
    let inners = inv.enclosure_candidates(written[1], parent, false);
    if outers.is_empty() || inners.is_empty() {
        return None;
    }
    // The box the inner part may sit in, per pair — plus the placement the line
    // already writes, which is the source's own choice and stands whatever it
    // measures.
    let work: usize = outers
        .len()
        .saturating_mul(inners.len())
        .saturating_mul(inners.iter().map(|i| placements(parent, i.size)).max()?);
    if work > MAX_ENCLOSURE_WORK {
        return None;
    }

    let mut best: Option<EnclosureKey> = None;
    for outer in &outers {
        for inner in &inners {
            let (span_x, span_y) = (
                parent.0.saturating_sub(inner.size.0) as i32,
                parent.1.saturating_sub(inner.size.1) as i32,
            );
            if placements(parent, inner.size) > MAX_ENCLOSURE_PLACEMENTS {
                continue;
            }
            let written_here = placed
                .filter(|_| outer.name == written[0] && inner.name == written[1])
                .filter(|&(p, q)| p < 0 || q < 0 || p > span_x || q > span_y);
            let grid = (0..=span_y).flat_map(move |q| (0..=span_x).map(move |p| (p, q)));
            for at in grid.chain(written_here) {
                let Some(key) =
                    evaluate_enclosure(outer, inner, &written, walls, parent, at, lo, hi, contact)
                else {
                    continue;
                };
                if best.as_ref().is_none_or(|b| key < *b) {
                    best = Some(key);
                }
            }
        }
    }
    let key = best?;
    if before.is_some_and(|before| (key.score, key.mismatched) >= before) {
        return None; // a line nobody can improve keeps its warning
    }
    let line = write_enclosure_line(compose, &key.names, key.placement)?;
    Some(PlannedLine {
        line,
        before: before.map(|(score, _)| score),
        after: key.score,
        mismatched: before.map(|(_, mismatched)| (mismatched, key.mismatched)),
        glyphs_warning: None,
        faulty,
    })
}

/// How many places a part that size could sit inside the parent's box.
fn placements(parent: (u16, u16), size: (u16, u16)) -> usize {
    let span = |p: u16, s: u16| p.saturating_sub(s) as usize + 1;
    span(parent.0, size.0).saturating_mul(span(parent.1, size.1))
}

/// The line that puts `names` at `at`: the same operator and comment, the
/// chosen names, and the two offsets.
///
/// Both offsets are always written, `0 0` included — an enclosure line with
/// none is one that has *not decided*, and a plan whose whole point is the
/// decision must not write it back as though nothing had happened.
pub(super) fn write_enclosure_line(
    compose: &GlyphCompose,
    names: &[String],
    at: (i32, i32),
) -> Option<String> {
    let mut items: Vec<ComposeItem> = Vec::new();
    for name in names {
        // A component that did not change keeps how it was written, `@` form
        // and all; a new one is written out as the glyph it names.
        let raw_name = compose.items.iter().find_map(|item| match item {
            ComposeItem::Part {
                name: n, raw_name, ..
            } if n == name => raw_name.clone(),
            _ => None,
        });
        items.push(ComposeItem::Part {
            name: name.clone(),
            raw_name,
        });
    }
    items.push(ComposeItem::Gap(i16::try_from(at.0).ok()?));
    items.push(ComposeItem::Gap(i16::try_from(at.1).ok()?));
    Some(
        GlyphCompose {
            op: compose.op,
            items,
            assumed: compose.assumed,
            comment: compose.comment.clone(),
        }
        .format_line(),
    )
}
