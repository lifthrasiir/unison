//! Planning the IDC line of a pattern block that encloses.

#[cfg(doc)]
use super::enclosure::{EnclosureKey, optimize_enclosure_line, write_enclosure_line};
use super::enclosure::{MAX_ENCLOSURE_PLACEMENTS, MAX_ENCLOSURE_WORK, enclosure_score};
#[cfg(doc)]
use super::inventory::Candidate;
use super::inventory::{EnclosurePart, EnclosureSlot, Inventory};
use super::names::{Combinations, ExpandedMember, MemberNames, expand_members};
use super::pattern::slot_family;
#[cfg(doc)]
use super::pattern::{
    LabelChoice, PatternKey, SlotFamily, optimize_pattern_line, slot_choices, write_pattern_line,
};
#[cfg(doc)]
use super::split::Key;
use super::{MAX_CANDIDATES, MAX_COMBINATIONS, PlannedLine};
use crate::document::{ComposeItem, GlyphCompose};

/// One answer for a slot of a pattern line that encloses: the counterpart of
/// [`LabelChoice`], and different from it in exactly the way the two layouts
/// are — what a member draws at the label is an [`EnclosurePart`], placed on
/// both axes, rather than a [`Candidate`] with one axis's frontier.
pub(super) struct EnclosureLabelChoice {
    /// The label this choice puts on the slot, and the name the line would
    /// write with it. `None` is the line's own component, left as written.
    relabel: Option<(String, String)>,
    /// [`crate::compose::enclosure_rank`] for the slot. One number for the
    /// family: the label is what carries a cavity, and the label is what every
    /// glyph here shares.
    rank: u8,
    /// The glyph each member puts in the slot, in `members` order. `None` for
    /// a member the line as written errors on at this slot.
    parts: Vec<Option<EnclosurePart>>,
}

/// [`slot_choices`] for an enclosure slot. The rule is the same one, read
/// through [`SlotFamily`]: a label is a candidate only where the block's own
/// pattern does not reach it and every glyph of the family draws at it. What
/// differs is only what is measured at each label — an enclosure slot is
/// filled by [`Inventory::enclosure_slot`], which is where the cavity a name
/// promises and the slot's own side are checked.
pub(super) fn enclosure_slot_choices(
    inv: &Inventory,
    members: &[MemberNames],
    as_written: &[Vec<Option<EnclosurePart>>],
    written: &str,
    slot: usize,
    parent: (u16, u16),
) -> Vec<EnclosureLabelChoice> {
    let outer = slot == 0;
    let mut out = vec![EnclosureLabelChoice {
        relabel: None,
        // Ranked on the name as *written*, which is the name the check reads
        // when it decides whether to warn.
        rank: crate::compose::enclosure_rank(written, outer),
        parts: as_written.iter().map(|p| p[slot].clone()).collect(),
    }];
    let Some(family) = slot_family(inv, members, written, slot) else {
        return out; // the family shares no label this slot could carry
    };
    for candidate_label in family.labels.iter().map(String::as_str) {
        if out.len() >= MAX_CANDIDATES {
            break;
        }
        if Some(candidate_label) == family.label.as_deref() {
            continue; // the line's own, already first
        }
        let name = format!("{}:{candidate_label}", family.base);
        // A drawing made for the other slot is not an alternative for this
        // one; see `compose::enclosure_rank`.
        let rank = crate::compose::enclosure_rank(&name, outer);
        if rank > 1 {
            continue;
        }
        // Measured as the line would write it, and only where that is the same
        // glyph the family offered — the canonicalization [`slot_choices`]
        // does, for the same reason.
        let Some(parts) = family
            .bases
            .iter()
            .map(|(written_base, base)| {
                let name = format!("{written_base}:{candidate_label}");
                let EnclosureSlot::Ok(part) = inv.enclosure_slot(&name, parent, outer) else {
                    return None;
                };
                (inv.canonical(&name) == inv.canonical(&format!("{base}:{candidate_label}")))
                    .then_some(*part)
            })
            .collect::<Option<Vec<EnclosurePart>>>()
        else {
            continue; // some glyph of the family draws nothing at this label
        };
        out.push(EnclosureLabelChoice {
            relabel: Some((candidate_label.to_string(), name)),
            rank,
            parts: parts.into_iter().map(Some).collect(),
        });
    }
    out
}

/// How the optimizer orders two answers for a pattern line that encloses.
/// [`EnclosureKey`]'s fields with the family's own objective — how many glyphs
/// warn — in front, and each of the rest summed over the family, exactly as
/// [`PatternKey`] is [`Key`]'s.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct PatternEnclosureKey {
    /// How many of the glyphs the line stands for warn at all. The objective.
    warnings: usize,
    /// Of those, how many have no layout at all — see [`PatternKey`], where
    /// the reason this number sits between the other two is written out.
    unresolved: usize,
    score: i32,
    mismatched: usize,
    directed: std::cmp::Reverse<usize>,
    edge_sum: i32,
    inner_spread: i32,
    /// `false` — the line as written — sorts first.
    changed: bool,
    placement: (i32, i32),
    names: Vec<String>,
}

/// Score one choice of labels at one placement over the whole family, or
/// `None` when a glyph the choice *does* lay out cannot be measured there — a
/// placement at which two parts share no line is no answer, exactly as it is
/// on a plain enclosure line.
#[allow(clippy::too_many_arguments)]
fn evaluate_enclosure_family(
    members: &[MemberNames],
    slots: &[Vec<EnclosureLabelChoice>],
    pick: &[usize],
    walls: crate::compose::Walls,
    parent: (u16, u16),
    at: (i32, i32),
    written_at: Option<(i32, i32)>,
) -> Option<PatternEnclosureKey> {
    let chosen: Vec<&EnclosureLabelChoice> = slots.iter().zip(pick).map(|(c, &i)| &c[i]).collect();
    let mut key = PatternEnclosureKey {
        warnings: 0,
        unresolved: 0,
        score: 0,
        mismatched: chosen.iter().filter(|c| c.rank == 2).count(),
        directed: std::cmp::Reverse(chosen.iter().filter(|c| c.rank == 0).count()),
        edge_sum: 0,
        inner_spread: 0,
        changed: written_at != Some(at) || chosen.iter().any(|c| c.relabel.is_some()),
        placement: at,
        names: chosen
            .iter()
            .map(|c| match &c.relabel {
                Some((_, name)) => name.clone(),
                None => String::new(),
            })
            .collect(),
    };
    for (m, member) in members.iter().enumerate() {
        // A glyph this choice leaves with no part in some slot is one the
        // check errors on and nothing here has answered: it warns, at every
        // placement alike.
        let (Some(outer), Some(inner)) = (&chosen[0].parts[m], &chosen[1].parts[m]) else {
            key.warnings += 1;
            key.unresolved += 1;
            continue;
        };
        let clearances = crate::compose::measure_enclosure_clearances(
            walls,
            parent,
            (&outer.name, &outer.profile, outer.at),
            (&inner.name, &inner.profile),
            at,
            member.contact,
        )?;
        let s = enclosure_score(&clearances, member.lo, member.hi);
        key.warnings += usize::from(s > 0);
        key.score += s;
        key.edge_sum += clearances
            .iter()
            .filter(|c| c.at_edge)
            .map(|c| c.value)
            .sum::<i32>();
        key.inner_spread += [true, false]
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
            .sum::<i32>();
    }
    Some(key)
}

/// Plan the IDC line of a pattern block that encloses.
///
/// The two planners this one sits between say the whole of it: what a family
/// shares is chosen for the family ([`optimize_pattern_line`]) and what an
/// enclosure lays out is searched rather than solved
/// ([`optimize_enclosure_line`]). A `⿺ 辶-($-1) X p q` line writes one pair of
/// offsets and one label per slot for every glyph the block declares, so those
/// are what a rewrite may move; the component *names* are the family's own and
/// each member's walls are its own, which is why the placement is scored
/// against every member rather than against one.
#[allow(clippy::too_many_arguments)]
pub(super) fn optimize_pattern_enclosure_line(
    inv: &Inventory,
    audit: &crate::audit::AuditRules,
    name_parts: &crate::document::NamePartsMap,
    glyph: &str,
    scale: u8,
    walls: crate::compose::Walls,
    parent: (u16, u16),
    compose: &GlyphCompose,
) -> Option<PlannedLine> {
    let written: Vec<&str> = compose.part_names().collect();
    if written.len() != 2 {
        return None;
    }
    // The line has to be one this file could write back: two components and
    // then their offsets, exactly as [`optimize_enclosure_line`] asks.
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

    let expanded = expand_members(audit, name_parts, glyph, scale, compose, 2)?;
    let mut members: Vec<MemberNames> = Vec::new();
    // The parts of each member as the line writes them: the first choice of
    // every slot below.
    let mut as_written: Vec<Vec<Option<EnclosurePart>>> = Vec::new();
    for member in expanded {
        let ExpandedMember {
            lo,
            hi,
            contact,
            names,
        } = member;
        let mut parts: Vec<Option<EnclosurePart>> = Vec::with_capacity(2);
        let mut faulty = false;
        let mut errored = false;
        let mut unmeasurable = false;
        for (slot, n) in names.iter().enumerate() {
            // An undecided component names no drawing however the source
            // declares it — the same state a name the check errors on leaves
            // the slot in, and answered the same way, by the label the family
            // shares.
            if crate::compose::is_undecided(n) {
                faulty = true;
                parts.push(None);
                continue;
            }
            match inv.enclosure_slot(n, parent, slot == 0) {
                EnclosureSlot::Ok(part) => parts.push(Some(*part)),
                EnclosureSlot::Faulty => {
                    faulty = true;
                    errored = true;
                    parts.push(None);
                }
                EnclosureSlot::Unmeasurable => unmeasurable = true,
            }
        }
        if unmeasurable {
            continue; // nothing this pass could measure after a choice either
        }
        members.push(MemberNames {
            lo,
            hi,
            names,
            contact,
            faulty,
            errored,
        });
        as_written.push(parts);
    }
    if members.is_empty() {
        return None;
    }

    let slots: Vec<Vec<EnclosureLabelChoice>> = (0..2)
        .map(|slot| enclosure_slot_choices(inv, &members, &as_written, written[slot], slot, parent))
        .collect();
    let lengths: Vec<usize> = slots.iter().map(Vec::len).collect();
    let label_combinations: usize = lengths.iter().product();
    if label_combinations == 0 || label_combinations > MAX_COMBINATIONS {
        return None;
    }

    // As written — when there is an "as written" at all. An unwritten
    // placement leaves the line with no layout to have measured badly, exactly
    // as an undecided component does, and it is planned whatever it scores.
    let unchanged = vec![0usize; slots.len()];
    let before = match placed {
        None => None,
        Some(at) => {
            let key =
                evaluate_enclosure_family(&members, &slots, &unchanged, walls, parent, at, placed)?;
            if (key.warnings, key.mismatched) == (0, 0) {
                return None; // nothing warns, so nothing to fix
            }
            Some(key)
        }
    };
    // Whether any glyph of the family had a layout at all, which is what
    // [`ClearanceFix::before`] reports a score for.
    let measured_before = before
        .as_ref()
        .map(|key| members.len() - key.unresolved)
        .unwrap_or_default();

    let mut best: Option<(PatternEnclosureKey, Vec<usize>)> = None;
    let mut work = 0usize;
    for pick in Combinations::new(&lengths) {
        // The box every glyph of the family could hold its inner part in: the
        // smallest of theirs, since one placement has to serve them all. A
        // member with no inner part at all constrains nothing — there is
        // nothing of it to keep inside the box.
        let inner: Vec<&EnclosurePart> = slots[1][pick[1]].parts.iter().flatten().collect();
        let span = |axis: fn((u16, u16)) -> u16| {
            inner
                .iter()
                .map(|part| axis(parent).saturating_sub(axis(part.size)) as i32)
                .min()
                .unwrap_or(0)
        };
        let (span_x, span_y) = (span(|s| s.0), span(|s| s.1));
        let count = ((span_x + 1) * (span_y + 1)) as usize;
        if count > MAX_ENCLOSURE_PLACEMENTS {
            continue;
        }
        work = work.saturating_add(count.saturating_mul(members.len()));
        if work > MAX_ENCLOSURE_WORK {
            return None;
        }
        // The placement the line already writes is the source's own choice and
        // stands whatever it measures, even where it puts a part outside the
        // box the search covers.
        let written_here = placed.filter(|&(p, q)| p < 0 || q < 0 || p > span_x || q > span_y);
        let grid = (0..=span_y).flat_map(move |q| (0..=span_x).map(move |p| (p, q)));
        for at in grid.chain(written_here) {
            let Some(key) =
                evaluate_enclosure_family(&members, &slots, &pick, walls, parent, at, placed)
            else {
                continue; // a placement some glyph of the family is not laid out at
            };
            if best.as_ref().is_none_or(|(b, _)| key < *b) {
                best = Some((key, pick.clone()));
            }
        }
    }
    let (key, pick) = best?;
    if before.as_ref().is_some_and(|before| {
        (key.warnings, key.unresolved, key.score, key.mismatched)
            >= (
                before.warnings,
                before.unresolved,
                before.score,
                before.mismatched,
            )
    }) {
        return None; // a line nobody can improve keeps its warnings
    }
    let line = write_pattern_enclosure_line(compose, key.placement, &slots, &pick)?;
    Some(PlannedLine {
        line,
        before: (measured_before > 0).then(|| before.as_ref().map_or(0, |b| b.score)),
        after: key.score,
        mismatched: before.as_ref().map(|b| (b.mismatched, key.mismatched)),
        glyphs_warning: Some((
            before.as_ref().map_or(members.len(), |b| b.warnings),
            key.warnings,
        )),
        faulty: members.iter().any(|m| m.errored),
    })
}

/// [`write_pattern_line`] for an enclosure: the chosen labels on the block's
/// own component names, and then the two offsets.
///
/// Both offsets are always written, `0 0` included, for the reason
/// [`write_enclosure_line`] gives.
fn write_pattern_enclosure_line(
    compose: &GlyphCompose,
    at: (i32, i32),
    slots: &[Vec<EnclosureLabelChoice>],
    pick: &[usize],
) -> Option<String> {
    let relabel: Vec<Option<(String, String)>> = slots
        .iter()
        .zip(pick)
        .map(|(choices, &i)| choices.get(i).and_then(|c| c.relabel.clone()))
        .collect();
    let mut items = relabelled_parts(compose, &relabel)?;
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

/// The line's components with the labels a plan chose in place of the ones
/// they carry, and everything else — the base names as the block spells them,
/// the `@` form — left exactly as it is. A pattern line's component *names*
/// are the family's and not this pass's to choose; only the label they share
/// is.
pub(super) fn relabelled_parts(
    compose: &GlyphCompose,
    relabel: &[Option<(String, String)>],
) -> Option<Vec<ComposeItem>> {
    let mut parts = compose
        .items
        .iter()
        .filter(|i| !matches!(i, ComposeItem::Gap(_)));
    let mut out = Vec::with_capacity(relabel.len());
    for chosen in relabel {
        let part = parts.next()?;
        let raw_name = match part {
            ComposeItem::Part { raw_name, .. } => raw_name,
            // Never relabelled; see [`slot_names`].
            ComposeItem::Nested(_) if chosen.is_none() => {
                out.push(part.clone());
                continue;
            }
            _ => return None,
        };
        out.push(match chosen {
            // The block's own component, untouched: the `@` form and all.
            None => part.clone(),
            // Only the label moved, so the name is written the way the line
            // already writes it, with the new label in place of the old.
            Some((label, name)) => ComposeItem::Part {
                name: name.clone(),
                raw_name: raw_name.as_deref().map(|raw| match raw.split_once(':') {
                    Some((raw_base, _)) => format!("{raw_base}:{label}"),
                    // An undecided component has no label to replace; the one
                    // chosen for it is simply appended.
                    None => format!("{raw}:{label}"),
                }),
            },
        });
    }
    Some(out)
}
