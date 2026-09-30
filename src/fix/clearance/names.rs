//! Slot-name helpers, the expansion of a pattern block into its members, and the
//! candidate-product iterator.

#[cfg(doc)]
use super::ClearanceFix;
#[cfg(doc)]
use super::inventory::{Inventory, SlotState};
#[cfg(doc)]
use super::pattern::{LabelChoice, optimize_pattern_line};
use crate::compose::Direction;
use crate::document::{ComposeItem, DocumentItem, GlyphBody, GlyphCompose};

/// A name the optimizer is willing to reason about as one glyph: spelled out,
/// with no pattern and no `$` in it.
///
/// A block whose *name* is one of those is a family, and is planned by
/// [`optimize_pattern_line`] instead. Inside the inventory the test is what it
/// says: a name that is not one glyph names no drawing to measure.
pub(super) fn is_plain_name(name: &str) -> bool {
    !crate::pattern::is_name_pattern(name) && !name.contains('$')
}

/// One name per slot of a split, in written order: a component's own, and for
/// a nested split the name its glyph is kept under
/// ([`crate::compose::nested_key`]).
///
/// # A nested split is one part
///
/// This pass lays out the line it is given and goes no further in: a nested
/// split is a part of that line like any other, drawn by a glyph the
/// inventory makes up for it ([`Inventory::register_nested`]), but a part with
/// exactly one candidate — itself, as written. Its members and its own gaps are
/// the inside of that glyph, so choosing among their variants or moving them is
/// optimizing a different line; a layout that wants that is written as a glyph
/// of its own. What moves is what is around it: the gaps beside it and the
/// other slots' variants.
pub(super) fn slot_names(compose: &GlyphCompose) -> Vec<String> {
    compose
        .items
        .iter()
        .filter_map(|item| match item {
            ComposeItem::Gap(_) => None,
            ComposeItem::Part { name, .. } => Some(name.clone()),
            ComposeItem::Nested(members) => Some(crate::compose::nested_key(compose.op, members)),
        })
        .collect()
}

/// Whether a slot name is a nested split's ([`slot_names`]). No glyph can be
/// named like one: it opens with an operator and has a `|` outside
/// parentheses, which in a block's name makes it a list.
pub(super) fn is_nested_slot(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| crate::compose::IdcOp::from_char(c).is_some())
        && crate::pattern::has_top_level_pipe(name)
}

/// The members of a nested slot's name, gaps left out.
fn nested_members(name: &str) -> impl Iterator<Item = &str> {
    let mut chars = name.chars();
    chars.next();
    crate::pattern::split_top_level_pipes(chars.as_str())
        .into_iter()
        .filter(|piece| piece.parse::<i16>().is_err())
}

/// [`is_plain_name`] for a slot: a nested split is plain when its members are.
pub(super) fn is_plain_slot(name: &str) -> bool {
    match is_nested_slot(name) {
        true => nested_members(name).all(is_plain_name),
        false => is_plain_name(name),
    }
}

/// [`crate::compose::is_undecided`] for a slot: a nested split is undecided
/// when a member is, and then it has no layout this pass could supply — the
/// member is the inside of it.
pub(super) fn is_undecided_slot(name: &str) -> bool {
    match is_nested_slot(name) {
        true => nested_members(name).any(crate::compose::is_undecided),
        false => crate::compose::is_undecided(name),
    }
}

/// [`crate::compose::direction_rank`] for a slot. A nested split claims no
/// direction: it has no name, and its members' names are about the slots of
/// its own line.
pub(super) fn slot_rank(name: &str, slot: Option<Direction>) -> u8 {
    match is_nested_slot(name) {
        true => 1,
        false => crate::compose::direction_rank(name, slot),
    }
}

/// Every glyph a `glyph` block declares, as a part this pass could measure.
///
/// A block whose name is a pattern draws all of them with the one grid it
/// holds ([`crate::document::expand_glyph_block`]), so each of its names names
/// the same drawing — and a component naming one of them is the common case in
/// a Han source, where a radical's variants are written as one block per size.
/// A name still standing on a `$` after the base bindings have been applied is
/// no name at all and declares nothing here.
pub(super) fn block_names(
    display: &str,
    name_parts: &crate::document::NamePartsMap,
) -> Vec<String> {
    if is_plain_name(display) {
        return vec![display.to_string()];
    }
    let substituted = crate::document::substitute_name_parts(display, name_parts);
    if substituted.contains('$') {
        return Vec::new();
    }
    let Ok(pattern) = crate::pattern::NamePattern::parse(&substituted) else {
        return Vec::new();
    };
    (0..pattern.len())
        .map(|i| crate::document::parse_glyph_name(&pattern.get(i)).display())
        .collect()
}

/// One glyph of the family a pattern line stands for: what it is held to, and
/// the components it writes once the block's pattern has been expanded.
///
/// The names are the glyph's own, and every slot the family shares a label on
/// ([`LabelChoice`]) rewrites all of them at once.
pub(super) struct MemberNames {
    pub(super) lo: i32,
    pub(super) hi: i32,
    /// One per slot, in the line's order.
    pub(super) names: Vec<String>,
    pub(super) contact: Option<u16>,
    /// Whether any of this glyph's slots holds no part as the line writes it —
    /// a name the check errors on ([`SlotState::Faulty`]), or one it leaves
    /// undecided. Such a glyph has no layout as written — it is counted among
    /// the ones that warn and left out of every sum — and a label that draws
    /// for the whole family, this glyph included, is what puts it right.
    pub(super) faulty: bool,
    /// Of those, the half the check calls an *error* rather than a TODO. It
    /// changes nothing about the search and only says which of the two a
    /// report is about ([`ClearanceFix::faulty`]).
    pub(super) errored: bool,
}

/// One glyph of the family a pattern block declares, as [`expand_members`]
/// reads it off the block's own expansion: the range its name is held to, and
/// the components its line writes.
pub(super) struct ExpandedMember {
    pub(super) lo: i32,
    pub(super) hi: i32,
    pub(super) contact: Option<u16>,
    /// One per slot, in the line's order.
    pub(super) names: Vec<String>,
}

/// Every glyph a pattern block's IDC line stands for, expanded exactly as the
/// build expands it — the block's name drives the count and each component
/// pattern is consumed in lock-step with it — and filtered to the ones an
/// `audit ideal-clearance` rule reaches and whose line writes the slots the
/// block's own line does.
///
/// `None` only when the block does not expand at all; a family with no glyph
/// left in it comes back empty, which each caller answers for itself.
pub(super) fn expand_members(
    audit: &crate::audit::AuditRules,
    name_parts: &crate::document::NamePartsMap,
    glyph: &str,
    scale: u8,
    compose: &GlyphCompose,
    arity: usize,
) -> Option<Vec<ExpandedMember>> {
    let expanded = expand_block_lines(name_parts, glyph, scale, compose)?;

    let mut out: Vec<ExpandedMember> = Vec::new();
    for (member_name, line) in &expanded {
        let Some((_, band)) = audit.ideal_clearance.for_glyph(member_name) else {
            continue;
        };
        let (lo, hi) = band.range(line.op.enclosing());
        let names = slot_names(line);
        if names.len() != arity {
            continue;
        }
        out.push(ExpandedMember {
            lo: lo as i32,
            hi: hi as i32,
            contact: audit.max_contact_run.for_glyph(member_name).map(|(_, m)| m),
            names,
        });
    }
    Some(out)
}

/// Every glyph a pattern block's IDC line stands for and the line it writes,
/// expanded exactly as the build expands it; `None` when the block does not
/// expand at all.
pub(super) fn expand_block_lines(
    name_parts: &crate::document::NamePartsMap,
    glyph: &str,
    scale: u8,
    compose: &GlyphCompose,
) -> Option<Vec<(String, GlyphCompose)>> {
    let body = GlyphBody {
        compose: vec![compose.clone()],
        scale,
        ..GlyphBody::new()
    };
    Some(
        expand_block(name_parts, glyph, &body)?
            .into_iter()
            .filter_map(|(name, mut body)| {
                (!body.compose.is_empty()).then(|| (name, body.compose.swap_remove(0)))
            })
            .collect(),
    )
}

/// Every glyph a pattern block declares and the body it has, with the names
/// in its refs and IDC lines substituted and expanded exactly as the build
/// does (`expand.rs::expand_glyph_item`); `None` when the block does not
/// expand at all.
pub(super) fn expand_block(
    name_parts: &crate::document::NamePartsMap,
    glyph: &str,
    body: &GlyphBody,
) -> Option<Vec<(String, GlyphBody)>> {
    let mut substituted = body.clone();
    for gref in &mut substituted.refs {
        gref.name = crate::document::substitute_name_parts(&gref.name, name_parts);
    }
    for (name, _) in substituted.compose.iter_mut().flat_map(|c| c.parts_mut()) {
        *name = crate::document::substitute_name_parts(name, name_parts);
    }
    let expanded = crate::document::expand_glyph_block(
        &crate::document::GlyphName(crate::document::substitute_name_parts(glyph, name_parts)),
        &substituted,
    )
    .ok()?;
    Some(
        expanded
            .into_iter()
            .filter_map(|item| match item {
                DocumentItem::Glyph { name, body } => Some((name.display(), body)),
                _ => None,
            })
            .collect(),
    )
}

/// The cartesian product of the slots' candidate lists, as index vectors over
/// the lists' lengths. The last slot varies fastest, so the order is the one
/// the lists are written in.
pub(super) struct Combinations {
    lengths: Vec<usize>,
    next: Option<Vec<usize>>,
}

impl Combinations {
    pub(super) fn new(lengths: &[usize]) -> Self {
        let lengths = lengths.to_vec();
        let next = lengths
            .iter()
            .all(|n| *n > 0)
            .then(|| vec![0usize; lengths.len()]);
        Self { lengths, next }
    }
}

impl Iterator for Combinations {
    type Item = Vec<usize>;

    fn next(&mut self) -> Option<Vec<usize>> {
        let current = self.next.take()?;
        let mut advanced = current.clone();
        for slot in (0..advanced.len()).rev() {
            advanced[slot] += 1;
            if advanced[slot] < self.lengths[slot] {
                self.next = Some(advanced);
                return Some(current);
            }
            advanced[slot] = 0;
        }
        Some(current)
    }
}
