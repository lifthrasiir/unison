//! Planning a pattern block's IDC line: one line standing for a family of glyphs.

use super::inventory::{Candidate, Inventory, SlotState};
use super::layout::{affine_layout, clearances_at, cost, fits_beside, score};
use super::names::{
    Combinations, ExpandedMember, MemberNames, expand_members, is_nested_slot, is_plain_name,
    is_undecided_slot, slot_names, slot_rank,
};
#[cfg(doc)]
use super::pattern_enclosure::enclosure_slot_choices;
use super::pattern_enclosure::relabelled_parts;
#[cfg(doc)]
use super::split::{Key, optimize_line};
use super::{MAX_CANDIDATES, MAX_COMBINATIONS, MAX_PATTERN_WORK, PlannedLine};
use crate::compose::Direction;
use crate::document::{ComposeItem, GlyphCompose};

/// One glyph of the family at one choice of labels: the range its own rule
/// holds it to, and its layout as a function of the gaps.
///
/// The clearances are affine in the gaps and are kept that way: `base[i]` is
/// what clearance `i` is before the gaps are added (`c_i = gap_i + base[i]`),
/// and `total` is the sum every layout of these parts has, whatever the gaps
/// do — the same telescoping the module docs derive — so the last clearance is
/// `total` less the others. That is what makes one combination cost a handful
/// of additions per glyph, which matters when one line stands for thousands.
pub(super) struct Member {
    lo: i32,
    hi: i32,
    base: Vec<i32>,
    total: i32,
    /// What the parts' boxes leave of the glyph along the axis before any gap:
    /// the room the line's gaps and its trailing leftover share.
    room: i32,
    /// The `audit max-contact-run` rule this glyph is held to, for the
    /// verification pass; the layout itself already has it folded into `base`
    /// and `total`.
    contact: Option<u16>,
}

impl Member {
    /// The clearances the gaps `gaps` leave, into a buffer the caller reuses:
    /// one line stands for thousands of glyphs and is scored once per set of
    /// gaps, so this is the innermost loop there is here.
    fn clearances_into(&self, gaps: &[i32], out: &mut Vec<i32>) {
        out.clear();
        out.extend(gaps.iter().zip(&self.base).map(|(gap, base)| gap + base));
        out.push(self.total - out.iter().sum::<i32>());
    }

    fn clearances(&self, gaps: &[i32]) -> Vec<i32> {
        let mut out = Vec::with_capacity(self.base.len() + 1);
        self.clearances_into(gaps, &mut out);
        out
    }
}

/// One answer for a slot of a pattern line: a variant label the whole family
/// can carry, and what each of its glyphs draws when it does.
pub(super) struct LabelChoice {
    /// The label this choice puts on the slot, and the name the line would
    /// write with it. `None` is the line's own component, left exactly as
    /// written — the only choice a slot whose label the block's own pattern
    /// reaches ever has.
    relabel: Option<(String, String)>,
    /// How the name suits the slot, as [`crate::compose::direction_rank`]
    /// scores it. One number for the family: the label is what carries a
    /// direction, and the label is what every glyph here shares.
    rank: u8,
    /// The glyph each member puts in the slot, in `members` order. `None` for
    /// a member the line as written errors on at this slot; only the line's own
    /// component ([`relabel`](Self::relabel) `None`) ever carries one, since a
    /// label is offered only where every glyph of the family draws at it.
    parts: Vec<Option<Candidate>>,
}

/// Plan a pattern block's IDC line.
///
/// One line here stands for a family, and what a rewrite may move is what the
/// family *shares*: the gaps, and — this is [`slot_choices`] — a component's
/// variant label whenever the block's own pattern does not reach it. A
/// component written `(rx|ry):5x4` says the same `5x4` for every glyph the
/// block declares, so that label is the family's answer and not one glyph's,
/// and each glyph's own family is asked for the same label in turn. A
/// component written `rx:(4|5)x4` says something different per glyph and is
/// left alone. The *base* is never searched: a name is one glyph's answer.
///
/// The objective is that shared choice, in this order:
///
/// 1. **the fewest glyphs warning at all**, a glyph the check errors on
///    counting among them. A family in which one more glyph is finished is
///    worth more than one in which every glyph is slightly less wrong: the
///    warnings are a work queue, and its length is what the command is there to
///    shorten;
/// 2. then the summed [`cost`], and the same tie-breaks a single line is
///    ordered by ([`Key`]) summed over the family — a gap moves every glyph's
///    first clearance by the same amount, so ordering the gaps
///    lexicographically orders the clearances the way [`Key`] does.
///
/// A glyph whose parts this pass cannot measure — a part that is itself a
/// composite, a component with no variant picked, a name no
/// `audit ideal-clearance` rule reaches — is left out of the answer rather than
/// making the whole family unfixable. Which glyphs those are is decided once,
/// on the line as written, so that every choice below is scored over the same
/// family; a *choice* that one of them cannot be measured at is dropped
/// instead. The line still has to warn about something, and the answer still
/// has to improve on what is written.
///
/// A glyph the check *errors* on ([`SlotState::Faulty`]) is the one thing that
/// is neither: it has no layout to be scored at either, but a name nothing
/// draws is precisely what a shared label could answer, so it is kept — with no
/// layout, counted among the glyphs that warn, and asked for the label along
/// with the rest of the family. Fixing one is then the same act as bringing a
/// glyph inside its range, and the first objective values it the same way.
pub(super) fn optimize_pattern_line(
    inv: &Inventory,
    audit: &crate::audit::AuditRules,
    name_parts: &crate::document::NamePartsMap,
    glyph: &str,
    scale: u8,
    parent: (u16, u16),
    compose: &GlyphCompose,
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

    let expanded = expand_members(audit, name_parts, glyph, scale, compose, written.len())?;

    let mut members: Vec<MemberNames> = Vec::new();
    // The parts of each member as the line writes them, kept beside it: they
    // are the first choice of every slot below.
    let mut as_written: Vec<Vec<Option<Candidate>>> = Vec::new();
    for member in expanded {
        let ExpandedMember {
            lo,
            hi,
            contact,
            names,
        } = member;
        // Per slot, so that a glyph one of whose components errors keeps the
        // parts of the slots that are sound: a label found for the slot that
        // errors has to be laid out beside them.
        let mut parts: Vec<Option<Candidate>> = Vec::with_capacity(names.len());
        let mut faulty = false;
        let mut errored = false;
        let mut unmeasurable = false;
        for (slot, n) in names.iter().enumerate() {
            // An undecided component names no drawing however the source
            // declares it, so this glyph has no part here — the same state a
            // name the check errors on leaves the slot in, and planned the same
            // way, since picking from the family is what the TODO asks for.
            if crate::compose::is_undecided(n) && !is_nested_slot(n) {
                faulty = true;
                parts.push(None);
                continue;
            }
            // A nested split with an undecided member is the inside of it
            // waiting on a decision, which no label of this line answers.
            if is_undecided_slot(n) {
                unmeasurable = true;
                continue;
            }
            match inv.candidate_state(n, op.slot_direction(slot), cross_extent, horizontal) {
                SlotState::Ok(candidate) => parts.push(Some(*candidate)),
                SlotState::Faulty => {
                    faulty = true;
                    errored = true;
                    parts.push(None);
                }
                SlotState::Unmeasurable => unmeasurable = true,
            }
        }
        if unmeasurable {
            continue;
        }
        if !faulty {
            // Two neighbours with no line on which both draw: nothing to
            // measure, here or at any label.
            let refs: Vec<&Candidate> = parts.iter().flatten().collect();
            if affine_layout(&refs, axis_extent, horizontal, contact).is_none() {
                continue;
            }
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

    let slots: Vec<Vec<LabelChoice>> = (0..written.len())
        .map(|slot| {
            slot_choices(
                inv,
                &members,
                &as_written,
                written[slot],
                slot,
                op.slot_direction(slot),
                cross_extent,
                axis_extent,
                horizontal,
            )
        })
        .collect();
    let lengths: Vec<usize> = slots.iter().map(Vec::len).collect();
    let label_combinations: usize = lengths.iter().product();
    if label_combinations == 0 || label_combinations > MAX_COMBINATIONS {
        return None;
    }

    // The gaps as written: everything before each component, summed, since a
    // line may write two numbers in a row. A trailing one moves nothing.
    let mut written_gaps: Vec<i32> = Vec::new();
    let mut pending = 0i32;
    for item in &compose.items {
        match item {
            ComposeItem::Gap(gap) => pending += *gap as i32,
            ComposeItem::Part { .. } | ComposeItem::Nested(_) => {
                written_gaps.push(std::mem::take(&mut pending));
            }
        }
    }

    let unchanged = vec![0usize; slots.len()];
    let written_members = member_layouts(&members, &slots, &unchanged, axis_extent, horizontal)?;
    // Whether the line as written has a layout at all. A family every glyph of
    // which waits on a decision is the pattern line's TODO, and its `before` is
    // `None` for the same reason [`optimize_line`]'s is: there is no
    // measurement that came out wrong, so there is none to report against.
    let measured_before = written_members.iter().flatten().count();
    let before = evaluate_gaps(
        &written_members,
        &slots,
        &unchanged,
        &written_gaps,
        &written_gaps,
    );
    if before.warnings == 0 && before.mismatched == 0 {
        return None; // nothing warns, so nothing to fix
    }

    let mut best: Option<(PatternKey, Vec<usize>, Vec<i32>)> = None;
    // A family is scored whole, once per set of gaps and per choice of labels,
    // so what a line costs is the product of the three. A line that would cost
    // more than the budget is left alone rather than allowed to take a minute,
    // exactly as an over-large variant family is above.
    let mut work = 0usize;
    for pick in Combinations::new(&lengths) {
        let Some(family) = member_layouts(&members, &slots, &pick, axis_extent, horizontal) else {
            continue; // a choice some glyph of the family cannot be measured at
        };
        // What each gap could usefully be: enough to put that clearance inside
        // some glyph's range, and one cell either side of that. Outside the
        // hull every glyph's clearance `i` is on the same side of its range, so
        // stepping back towards it gains each glyph a cell there and costs it
        // at most the one it takes from the last clearance — never a worse
        // answer, and often a better one. [`cost`] adds two places a gap may
        // usefully be outside that: a leading gap of 0, which puts the first
        // box flush with the glyph's, and an inner gap as large as all the room
        // the boxes leave, which puts the last one flush too. The gaps as
        // written are in the set whatever it says, so that "as written" is
        // always one of the answers compared.
        let mut choices: Vec<Vec<i32>> = Vec::with_capacity(written_gaps.len());
        for (i, gap) in written_gaps.iter().enumerate() {
            let (mut lo, mut hi) = (*gap, *gap);
            for member in family.iter().flatten() {
                let flush = if i == 0 { 0 } else { member.room };
                lo = lo.min(member.lo - member.base[i] - 1).min(flush - 1);
                hi = hi.max(member.hi - member.base[i] + 1).max(flush + 1);
            }
            choices.push((lo..=hi).collect());
        }
        let combinations: usize = choices.iter().map(Vec::len).product();
        if combinations == 0 {
            continue;
        }
        work = work.saturating_add(combinations.saturating_mul(family.len()));
        if work > MAX_PATTERN_WORK {
            return None;
        }

        let mut gaps = vec![0i32; choices.len()];
        for mut counter in 0..combinations {
            for (slot, values) in choices.iter().enumerate() {
                gaps[slot] = values[counter % values.len()];
                counter /= values.len();
            }
            let key = evaluate_gaps(&family, &slots, &pick, &gaps, &written_gaps);
            if best.as_ref().is_none_or(|(b, _, _)| key < *b) {
                best = Some((key, pick.clone(), gaps.clone()));
            }
        }
    }
    let (key, pick, gaps) = best?;
    if (key.warnings, key.unresolved, key.cost, key.mismatched)
        >= (
            before.warnings,
            before.unresolved,
            before.cost,
            before.mismatched,
        )
    {
        return None; // a line nobody can improve keeps its warnings
    }

    let line = write_pattern_line(compose, &gaps, &slots, &pick)?;
    // What the arithmetic says the layout measures, measured. Same rule as
    // [`optimize_line`]: a command that quietly writes a layout it was wrong
    // about is worse than one that writes nothing.
    let family = member_layouts(&members, &slots, &pick, axis_extent, horizontal)?;
    let (mut score_after, mut warnings_after, mut unresolved_after) = (0, 0, 0);
    for (m, member) in family.iter().enumerate() {
        let Some(member) = member else {
            warnings_after += 1;
            unresolved_after += 1;
            continue;
        };
        let parts = parts_at(&slots, &pick, m)?;
        let mut positions = vec![gaps[0]];
        for (i, part) in parts.iter().enumerate().take(parts.len() - 1) {
            positions.push(positions[i] + part.extent + gaps[i + 1]);
        }
        let measured = clearances_at(&parts, &positions, axis_extent, horizontal, member.contact)?;
        if measured != member.clearances(&gaps) {
            return None;
        }
        let s = score(&measured, member.lo, member.hi);
        score_after += s;
        warnings_after += usize::from(s > 0);
    }
    if (warnings_after, unresolved_after, score_after) != (key.warnings, key.unresolved, key.score)
    {
        return None;
    }
    Some(PlannedLine {
        line,
        before: (measured_before > 0).then_some(before.score),
        after: key.score,
        mismatched: Some((before.mismatched, key.mismatched)),
        glyphs_warning: Some((before.warnings, key.warnings)),
        faulty: members.iter().any(|m| m.errored),
    })
}

/// What one slot of a pattern line offers the whole family: the text a rewrite
/// puts back before the label, and the labels every glyph of the family draws
/// at.
///
/// This is the half of a slot's search that says nothing about how the line
/// lays out, so both planners share it — [`slot_choices`] for a split and
/// [`enclosure_slot_choices`] for an enclosure — and differ only in what they
/// then measure at each label.
pub(super) struct SlotFamily {
    /// Everything the line writes before the `:`, kept verbatim and never
    /// searched: it may itself be a pattern (`A-($-1)`), and a name is one
    /// glyph's answer where a label is the family's.
    pub(super) base: String,
    /// The label the line writes, or `None` for an undecided component, which
    /// has none to replace.
    pub(super) label: Option<String>,
    /// Per member, in `members` order: the text that member writes before the
    /// label — what a rewrite puts back for it — and the canonical base its
    /// own variant family is looked up under.
    pub(super) bases: Vec<(String, String)>,
    /// The labels *every* member's family offers, in name order. A family
    /// whose members do not all draw at one label offers no choice at all,
    /// which is the answer for a slot whose glyphs are drawn one by one.
    pub(super) labels: Vec<String>,
}

/// [`SlotFamily`] for one slot, or `None` when there is nothing here to
/// search: a label the block's own pattern reaches, a member whose name does
/// not carry the label the line writes, or a family that shares no label.
pub(super) fn slot_family(
    inv: &Inventory,
    members: &[MemberNames],
    written: &str,
    slot: usize,
) -> Option<SlotFamily> {
    // The line's component, taken apart the only way a pattern can be: what it
    // writes before the label and what it writes as one.
    let (base, label) = match written.split_once(':') {
        Some((base, label)) => (base, Some(label)),
        // An undecided component: no label to move, and the whole name is the
        // text a chosen one is appended to.
        None => (written, None),
    };
    if label.is_some_and(|label| !is_plain_name(label)) {
        return None; // a label the block's own pattern reaches
    }
    let suffix = label.map_or_else(String::new, |label| format!(":{label}"));
    // Each glyph's own base, twice over: as the line's own pattern expands it,
    // which is what a rewrite writes, and canonically — through the aliases,
    // exactly as [`Inventory::candidates`] does — which is the family a variant
    // is looked for in.
    let mut bases: Vec<(String, String)> = Vec::with_capacity(members.len());
    for member in members {
        // The line's label is not this glyph's after all.
        let written_base = member.names[slot].strip_suffix(&suffix)?;
        let canonical = inv.canonical(&member.names[slot]);
        let canonical_base = canonical
            .split_once(':')
            .map_or(canonical.as_str(), |(base, _)| base);
        bases.push((written_base.to_string(), canonical_base.to_string()));
    }
    // The labels every one of them offers, and no more: one label has to serve
    // the whole family.
    let mut shared: Option<std::collections::BTreeSet<&str>> = None;
    for (_, base) in &bases {
        let mine: std::collections::BTreeSet<&str> = inv
            .variants
            .get(base)
            .into_iter()
            .flatten()
            .filter_map(|name| name.split_once(':').map(|(_, l)| l))
            .collect();
        shared = Some(match shared {
            None => mine,
            Some(prev) => prev.intersection(&mine).copied().collect(),
        });
        if shared.as_ref().is_some_and(|s| s.is_empty()) {
            return None;
        }
    }
    Some(SlotFamily {
        base: base.to_string(),
        label: label.map(str::to_string),
        bases,
        labels: shared.into_iter().flatten().map(str::to_string).collect(),
    })
}

/// Everything one slot of a pattern line could carry, the line's own component
/// first.
///
/// A label is a candidate only when the block's pattern does not reach it —
/// the line writes it as one string, and it means the same thing in every
/// glyph the block declares — and only when *every* glyph of the family draws
/// something at it. A family whose members' variants do not all offer the same
/// label offers no choice at all, which is the answer for a slot whose glyphs
/// are drawn one by one.
#[allow(clippy::too_many_arguments)]
pub(super) fn slot_choices(
    inv: &Inventory,
    members: &[MemberNames],
    as_written: &[Vec<Option<Candidate>>],
    written: &str,
    slot: usize,
    dir: Option<Direction>,
    cross: u16,
    along: i32,
    horizontal: bool,
) -> Vec<LabelChoice> {
    let mut out = vec![LabelChoice {
        relabel: None,
        // Ranked on the name as *written*, which is the name the check reads
        // when it decides whether to warn.
        rank: slot_rank(written, dir),
        // A glyph the line as written errors on *at this slot* has no part
        // here to keep; one that errors at another slot keeps this one.
        parts: as_written.iter().map(|p| p[slot].clone()).collect(),
    }];
    // A nested split is one part with one candidate; see [`slot_names`].
    if is_nested_slot(written) {
        return out;
    }
    let Some(family) = slot_family(inv, members, written, slot) else {
        return out; // the family shares no label this slot could carry
    };
    let (base, label) = (family.base.as_str(), family.label.as_deref());
    let bases = &family.bases;
    for candidate_label in family.labels.iter().map(String::as_str) {
        if out.len() >= MAX_CANDIDATES {
            break;
        }
        if Some(candidate_label) == label {
            continue; // the line's own, already first
        }
        let name = format!("{base}:{candidate_label}");
        // A drawing made for the other side of the glyph is not an alternative
        // for this slot; see `compose::direction_rank`.
        let rank = crate::compose::direction_rank(&name, dir);
        if rank > 1 {
            continue;
        }
        // Measured as the line would write it — a base whose alias exists at
        // one label only would otherwise be relabelled into a name nothing
        // defines — and only where that is the same glyph the family offered.
        let Some(parts) = bases
            .iter()
            .map(|(written_base, base)| {
                let name = format!("{written_base}:{candidate_label}");
                let part = inv.candidate(&name, dir, cross, horizontal)?;
                // The same glyph the family offered, and not merely a name
                // that reads like it: both sides are canonicalized, because
                // the base a family is looked up under is not itself stable
                // under the aliases — a component the line leaves undecided is
                // looked up under the name as written, whose labels are the
                // very things the aliases move.
                (inv.canonical(&name) == inv.canonical(&format!("{base}:{candidate_label}")))
                    .then_some(part)
            })
            .collect::<Option<Vec<Candidate>>>()
        else {
            continue; // some glyph of the family draws nothing at this label
        };
        // One glyph the label would not fit in is enough: the label is the
        // family's answer and every glyph of it has to be able to hold it.
        if parts.iter().any(|part| !fits_beside(part.extent, along)) {
            continue;
        }
        out.push(LabelChoice {
            relabel: Some((candidate_label.to_string(), name)),
            rank,
            parts: parts.into_iter().map(Some).collect(),
        });
    }
    out
}

/// The parts one member of the family puts in its slots, at one choice of
/// labels.
fn parts_at<'a>(
    slots: &'a [Vec<LabelChoice>],
    pick: &[usize],
    member: usize,
) -> Option<Vec<&'a Candidate>> {
    slots
        .iter()
        .zip(pick)
        .map(|(choices, &i)| choices[i].parts[member].as_ref())
        .collect()
}

/// The whole family's layouts at one choice of labels, one per member — or
/// `None` for the whole choice when a glyph the line *does* lay out cannot be
/// measured at it, which is no answer, since every choice is scored over the
/// same family.
///
/// A member's own entry is `None` when the choice leaves it with no layout at
/// all: that is a glyph the line as written errors on, which no label has
/// answered. It stays in the family and is counted among the ones that warn
/// ([`evaluate_gaps`]) rather than dropping the choice, since the choice is not
/// what made it unmeasurable.
fn member_layouts(
    members: &[MemberNames],
    slots: &[Vec<LabelChoice>],
    pick: &[usize],
    axis_extent: i32,
    horizontal: bool,
) -> Option<Vec<Option<Member>>> {
    members
        .iter()
        .enumerate()
        .map(|(m, member)| {
            let layout = parts_at(slots, pick, m).and_then(|parts| {
                let room = axis_extent - parts.iter().map(|p| p.extent).sum::<i32>();
                affine_layout(&parts, axis_extent, horizontal, member.contact)
                    .map(|(base, total)| (base, total, room))
            });
            match layout {
                Some((base, total, room)) => Some(Some(Member {
                    lo: member.lo,
                    hi: member.hi,
                    base,
                    total,
                    room,
                    contact: member.contact,
                })),
                // A glyph that was measured as written and is not measurable
                // here: the choice is dropped, exactly as before.
                None => member.faulty.then_some(None),
            }
        })
        .collect()
}

/// How the optimizer orders two answers for a pattern line. Derived `Ord`
/// again, and the fields are [`Key`]'s with the family's own objective — how
/// many glyphs warn — in front, and each of the rest summed over the family.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PatternKey {
    /// How many of the glyphs the line stands for warn at all. The objective.
    warnings: usize,
    /// Of those, how many have no layout at all — a slot still waiting on a
    /// decision, or one the check errors on, that this choice has not answered.
    /// It sits between the two numbers because such a glyph *scores* nothing:
    /// there is nothing to measure, so a family every glyph of which is a TODO
    /// would otherwise beat every decision that leaves a warning behind, and
    /// the line would keep its TODO forever. A decided layout that warns is
    /// still more than none — the same rule [`optimize_line`] states by
    /// leaving a TODO's `before` `None` — while a glyph that *did* lay out is
    /// protected by `warnings` ahead of this, so no decision is bought by
    /// breaking one that was already right.
    unresolved: usize,
    /// [`cost`], summed over the family.
    cost: i32,
    /// How many slots hold a label drawn for another slot — one number for the
    /// family, the label being what every glyph here shares.
    mismatched: usize,
    /// More labels drawn *for* their slot first.
    directed: std::cmp::Reverse<usize>,
    edge_sum: i32,
    inner_spread: i32,
    blank: i32,
    /// `false` — the line as written — sorts first.
    changed: bool,
    gaps: Vec<i32>,
    names: Vec<String>,
    /// How far outside their ranges the family is, summed: what the check
    /// reports. The clearances behind it are what everything above is about,
    /// so it has nothing left to order.
    score: i32,
}

/// Score one choice of labels and gaps over the whole family.
fn evaluate_gaps(
    family: &[Option<Member>],
    slots: &[Vec<LabelChoice>],
    pick: &[usize],
    gaps: &[i32],
    written: &[i32],
) -> PatternKey {
    let chosen: Vec<&LabelChoice> = slots.iter().zip(pick).map(|(c, &i)| &c[i]).collect();
    let mut key = PatternKey {
        warnings: 0,
        unresolved: 0,
        cost: 0,
        score: 0,
        mismatched: chosen.iter().filter(|c| c.rank == 2).count(),
        directed: std::cmp::Reverse(chosen.iter().filter(|c| c.rank == 0).count()),
        edge_sum: 0,
        inner_spread: 0,
        blank: 0,
        changed: gaps != written || chosen.iter().any(|c| c.relabel.is_some()),
        gaps: gaps.to_vec(),
        names: chosen
            .iter()
            .map(|c| match &c.relabel {
                Some((_, name)) => name.clone(),
                None => String::new(),
            })
            .collect(),
    };
    let mut clearances: Vec<i32> = Vec::with_capacity(4);
    let written_blank: i32 = gaps.iter().map(|g| g.abs()).sum();
    let gap_sum: i32 = gaps.iter().sum();
    for member in family {
        // A glyph this choice leaves with no layout is one the check errors on
        // and nothing here has answered: it warns, at every set of gaps alike.
        let Some(member) = member else {
            key.warnings += 1;
            key.unresolved += 1;
            continue;
        };
        member.clearances_into(gaps, &mut clearances);
        let n = clearances.len();
        let s = score(&clearances, member.lo, member.hi);
        key.warnings += usize::from(s > 0);
        key.score += s;
        key.cost += cost(s, gaps[0], member.room - gap_sum);
        key.blank += written_blank + (member.room - gap_sum).abs();
        key.edge_sum += clearances[0] + clearances[n - 1];
        if n == 4 {
            key.inner_spread += (clearances[1] - clearances[2]).abs();
        }
    }
    key
}

/// The line with `gaps` in place of the ones it writes and the chosen labels in
/// place of the ones its components carry, and everything else — the operator,
/// the components' base names as the block spells them, the comment — left
/// exactly as it is. A pattern line's component *names* are the family's and
/// not this pass's to choose; only the label they share is.
pub(super) fn write_pattern_line(
    compose: &GlyphCompose,
    gaps: &[i32],
    slots: &[Vec<LabelChoice>],
    pick: &[usize],
) -> Option<String> {
    let relabel: Vec<Option<(String, String)>> = slots
        .iter()
        .zip(pick)
        .map(|(choices, &i)| choices.get(i).and_then(|c| c.relabel.clone()))
        .collect();
    let parts = relabelled_parts(compose, &relabel)?;
    let mut items: Vec<ComposeItem> = Vec::new();
    for (slot, gap) in gaps.iter().enumerate() {
        if *gap != 0 {
            items.push(ComposeItem::Gap(i16::try_from(*gap).ok()?));
        }
        items.push(parts.get(slot)?.clone());
    }
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
