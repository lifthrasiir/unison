//! Planning one IDC split line: the ordering key, the search and the rewrite.

use super::inventory::{Candidate, Inventory, SlotState};
use super::layout::{arrange, clearances_at, cost, score, walk};
use super::names::{Combinations, is_nested_slot, is_plain_slot, is_undecided_slot, slot_names};
use super::{MAX_COMBINATIONS, PlannedLine};
use crate::compose::{VariantSpec, effective_facing};
use crate::document::{ComposeItem, GlyphCompose};

/// How the optimizer orders two answers. Derived `Ord` is the whole rule: the
/// fields are in the order the module docs list them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Key {
    /// [`cost`]: how far outside the range the layout is, summed, with the
    /// room the boxes leave at the edges added. The objective.
    cost: i32,
    /// How many components are drawn for a slot other than the one they sit
    /// in — the second objective, and the only one that moves a line whose
    /// clearances are already perfect.
    mismatched: usize,
    /// More names drawn *for* their slot first.
    directed: std::cmp::Reverse<usize>,
    /// More slots whose name is as long along the axis as the one the line
    /// asked for, first — counted only over the slots that have to be filled
    /// from the family because what the line writes errors, where the size the
    /// erroring name states is the one thing about it that still says what its
    /// author wanted. Zero everywhere else, so this orders nothing on a line
    /// whose components are sound.
    asked: std::cmp::Reverse<usize>,
    /// The two edge clearances, added.
    edge_sum: i32,
    /// How far apart the two inner clearances are (`⿲`/`⿳` only; 0 otherwise).
    inner_spread: i32,
    /// The room the line writes around the parts' boxes — every gap and what
    /// the last part leaves of the glyph, as magnitudes. Two variant choices
    /// that measure alike differ here when one box is a cell longer than the
    /// other with the ink where it was: the longer one is the drawing made for
    /// that room, and the gap beside the shorter one is standing in for it.
    blank: i32,
    clearances: Vec<i32>,
    /// `false` — the variants as written — sorts first.
    changed: bool,
    names: Vec<String>,
    /// [`score`], which is what the check reports. `clearances` fixes it, so it
    /// orders nothing.
    score: i32,
}

/// Plan one IDC line, or `None` when the line does not warn, cannot be
/// measured, or cannot be improved.
pub(super) fn optimize_line(
    inv: &Inventory,
    parent: (u16, u16),
    compose: &GlyphCompose,
    lo: i32,
    hi: i32,
    contact: Option<u16>,
) -> Option<PlannedLine> {
    let op = compose.op;
    let horizontal = op.horizontal();
    let (axis_extent, cross_extent) = match horizontal {
        true => (parent.0 as i32, parent.1),
        false => (parent.1 as i32, parent.0),
    };
    let written = slot_names(compose);
    let written: Vec<&str> = written.iter().map(String::as_str).collect();
    if written.len() != op.arity() {
        return None;
    }
    if written.iter().any(|n| !is_plain_slot(n)) {
        return None;
    }
    // An undecided component has not chosen a width, so the line has no layout
    // to measure — the check stands down and reports a TODO instead. There is
    // still something to do here, though: picking from the family it names is
    // exactly what the TODO asks for, so the line is optimized *towards* a
    // decision rather than away from a warning. Its `before` is `None`, and the
    // "must lower the cost" rule below has nothing to compare against.
    let undecided = written.iter().any(|n| is_undecided_slot(n));

    // A component the check *errors* on — a name nothing defines, a box that
    // does not fill the slot — is the other way a line comes to have no layout
    // this pass can score. It is not a measurement that came out wrong, so
    // there is nothing to lower and nothing to compare against, exactly as for
    // an undecided component; and the family the component names is searched
    // without it below, since it names no drawing that could go there.
    let mut faulty = false;
    // What each erroring slot asked for, along the axis: the size its name
    // states is wrong about the glyph it names, but it is still the extent its
    // author wanted, and among two answers that measure alike it decides. Only
    // the erroring slots are in it — see [`Key::asked`].
    let mut asked: Vec<Option<i32>> = vec![None; written.len()];
    let current: Option<Vec<Candidate>> = match undecided {
        true => None,
        false => {
            let mut parts = Vec::with_capacity(written.len());
            for (slot, name) in written.iter().enumerate() {
                match inv.candidate_state(name, op.slot_direction(slot), cross_extent, horizontal) {
                    SlotState::Ok(candidate) => parts.push(*candidate),
                    SlotState::Faulty => {
                        faulty = true;
                        asked[slot] = (!is_nested_slot(name))
                            .then(|| VariantSpec::parse(name).size)
                            .flatten()
                            .map(|(w, h)| i32::from(if horizontal { w } else { h }));
                    }
                    // Nothing this pass could measure after a choice either.
                    SlotState::Unmeasurable => return None,
                }
            }
            (!faulty).then_some(parts)
        }
    };

    // As written: the parts where the line's own gaps put them, and how many
    // of them are drawn for a slot they do not sit in. Both are things the
    // check warns about, and a line is left alone only when neither does.
    let before = match current {
        None => None,
        Some(current) => {
            let placed = walk(compose, &current);
            let as_written: Vec<&Candidate> = current.iter().collect();
            let clearances = clearances_at(&as_written, &placed, axis_extent, horizontal, contact)?;
            let score = score(&clearances, lo, hi);
            let last = current.len() - 1;
            let trail = axis_extent - (placed[last] + current[last].extent);
            let before = (
                score,
                cost(score, placed[0], trail),
                current.iter().filter(|c| c.rank == 2).count(),
            );
            if (before.0, before.2) == (0, 0) {
                return None; // nothing warns, so nothing to fix
            }
            Some(before)
        }
    };

    let slots: Vec<Vec<Candidate>> = written
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            inv.candidates(
                name,
                op.slot_direction(slot),
                cross_extent,
                axis_extent,
                horizontal,
            )
        })
        .collect();
    let lengths: Vec<usize> = slots.iter().map(Vec::len).collect();
    let combinations: usize = lengths.iter().product();
    if combinations == 0 || combinations > MAX_COMBINATIONS {
        return None;
    }

    let mut best: Option<(Key, Vec<usize>)> = None;
    for pick in Combinations::new(&lengths) {
        let chosen: Vec<&Candidate> = pick
            .iter()
            .enumerate()
            .map(|(s, &i)| &slots[s][i])
            .collect();
        let Some(key) = evaluate(
            &chosen,
            &written,
            &asked,
            axis_extent,
            horizontal,
            lo,
            hi,
            contact,
        ) else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| key < *b) {
            best = Some((key, pick));
        }
    }
    let (key, pick) = best?;
    if before.is_some_and(|(_, cost, mismatched)| (key.cost, key.mismatched) >= (cost, mismatched))
    {
        return None; // a line nobody can improve keeps its warning
    }
    let chosen: Vec<&Candidate> = pick
        .iter()
        .enumerate()
        .map(|(s, &i)| &slots[s][i])
        .collect();

    // Where the chosen clearances put each part. `c₀` is the first part's ink
    // against the near edge, and every later one is the distance from the
    // previous part's origin, which is what `facing_offset` is measured from.
    let mut positions = Vec::with_capacity(chosen.len());
    let mut at = key.clearances[0] - chosen[0].frontier.near;
    positions.push(at);
    for (i, pair) in chosen.windows(2).enumerate() {
        let facing = effective_facing(pair[0].side(), pair[1].side(), horizontal, contact)?;
        at += key.clearances[i + 1] - facing;
        positions.push(at);
    }

    let line = write_line(compose, &chosen, &positions)?;
    // The arithmetic says what this layout measures; the measurement says so
    // too, or the line is left alone. Cheap, and the alternative is a command
    // that quietly writes a layout it was wrong about.
    let verified = clearances_at(&chosen, &positions, axis_extent, horizontal, contact)?;
    if score(&verified, lo, hi) != key.score {
        return None;
    }
    Some(PlannedLine {
        line,
        before: before.map(|(score, _, _)| score),
        after: key.score,
        mismatched: before.map(|(_, _, mismatched)| (mismatched, key.mismatched)),
        glyphs_warning: None,
        faulty,
    })
}

/// Score one candidate combination, and the layout it is scored at.
#[allow(clippy::too_many_arguments)]
fn evaluate(
    chosen: &[&Candidate],
    written: &[&str],
    asked: &[Option<i32>],
    axis_extent: i32,
    horizontal: bool,
    lo: i32,
    hi: i32,
    contact: Option<u16>,
) -> Option<Key> {
    let n = chosen.len() + 1;
    // The sum every layout of these variants has, whatever the gaps do.
    let mut total = chosen[0].frontier.near + (axis_extent - 1 - chosen[n - 2].frontier.far);
    let mut facings = Vec::with_capacity(n - 2);
    for pair in chosen.windows(2) {
        let facing = effective_facing(pair[0].side(), pair[1].side(), horizontal, contact)?;
        total += facing;
        facings.push(facing);
    }
    let last = chosen[n - 2];
    let bearings = (chosen[0].frontier.near, last.extent - 1 - last.frontier.far);
    let clearances = arrange(n, total, lo, hi, bearings);
    // The gaps those clearances write, walked the way [`optimize_line`] places
    // the parts: each is the clearance less what the ink already leaves.
    let lead = clearances[0] - bearings.0;
    let trail = clearances[n - 1] - bearings.1;
    let mut blank = lead.abs() + trail.abs();
    for (i, facing) in facings.iter().enumerate() {
        blank += (clearances[i + 1] - facing - chosen[i].extent).abs();
    }
    let score = score(&clearances, lo, hi);
    Some(Key {
        cost: cost(score, lead, trail),
        mismatched: chosen.iter().filter(|c| c.rank == 2).count(),
        directed: std::cmp::Reverse(chosen.iter().filter(|c| c.rank == 0).count()),
        asked: std::cmp::Reverse(
            chosen
                .iter()
                .zip(asked)
                .filter(|(c, asked)| **asked == Some(c.extent))
                .count(),
        ),
        edge_sum: clearances[0] + clearances[n - 1],
        inner_spread: match n {
            4 => (clearances[1] - clearances[2]).abs(),
            _ => 0,
        },
        blank,
        score,
        clearances,
        changed: chosen.iter().zip(written).any(|(c, w)| c.name != *w),
        names: chosen.iter().map(|c| c.name.clone()).collect(),
    })
}

/// The line that places `chosen` at `positions`: the same operator and comment,
/// the chosen names, and the gaps the positions imply.
///
/// A gap of zero is not written, and neither is a trailing one — it would move
/// nothing, the cursor having nothing left to place.
fn write_line(compose: &GlyphCompose, chosen: &[&Candidate], positions: &[i32]) -> Option<String> {
    let mut items: Vec<ComposeItem> = Vec::new();
    let mut cursor = 0i32;
    for (i, part) in chosen.iter().enumerate() {
        let gap = positions[i] - cursor;
        if gap != 0 {
            items.push(ComposeItem::Gap(i16::try_from(gap).ok()?));
        }
        // A nested split is written back exactly as it was: it is one part
        // here, and its inside is not this line's to change.
        if is_nested_slot(&part.name) {
            items.push(compose.items.iter().find_map(|item| match item {
                ComposeItem::Nested(members)
                    if crate::compose::nested_key(compose.op, members) == part.name =>
                {
                    Some(item.clone())
                }
                _ => None,
            })?);
            cursor = positions[i] + part.extent;
            continue;
        }
        // A component that did not change keeps how it was written, `@` form
        // and all; a new one is written out as the glyph it names.
        let raw_name = compose.items.iter().find_map(|item| match item {
            ComposeItem::Part { name, raw_name } if *name == part.name => raw_name.clone(),
            _ => None,
        });
        items.push(ComposeItem::Part {
            name: part.name.clone(),
            raw_name,
        });
        cursor = positions[i] + part.extent;
    }
    Some(
        GlyphCompose {
            op: compose.op,
            items,
            assumed: compose.assumed,
            // A rewrite picks variants and moves gaps; whether the line is
            // conditional is not its business.
            comment: compose.comment.clone(),
        }
        .format_line(),
    )
}
