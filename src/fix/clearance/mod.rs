//! `uniform fix --optimize-clearance`: put an IDC line's clearances back inside
//! the range `audit ideal-clearance` states, by choosing among the variants the
//! source already draws and the gaps the line may write. What it does as a user
//! sees it is in `doc/reference.md` (*Rewriting the Source*); this is how and
//! why.
//!
//! # What it touches
//!
//! **Only a line the source already reports**, and a rewrite is emitted only
//! when it *lowers* what the search minimizes (the cost below) — a line that
//! cannot be improved keeps the warning rather than being shuffled about. Four kinds of report, and they are
//! not the same act:
//!
//! - a **clearance finding**: the line has a layout and it is outside the
//!   range. The search moves it inside. This one is a `Severity::Chore`
//!   rather than a warning — a build counts it instead of printing it, because
//!   the check speaks for every composed glyph in the font — but nothing here
//!   turns on which of the two it is, and the vocabulary below still calls the
//!   glyphs it is about *warning* glyphs;
//! - a **wrong-slot warning**: a component drawn for one side sits on another.
//!   Nothing about the clearances is wrong, so the count of such components is
//!   an objective of its own — the second one, behind the score, so no name is
//!   ever put right by making the layout worse;
//! - a **TODO**: a component has not picked its variant
//!   ([`crate::compose::is_undecided`]). There is no layout and so no score to
//!   lower, but choosing from the family is exactly what the TODO asks for, so
//!   the line is planned whatever it scores, with [`ClearanceFix::before`]
//!   `None` to say so;
//! - an **error**: a component is wrong about the glyph it names
//!   ([`SlotState::Faulty`] lists the cases against `compose`'s messages). The
//!   TODO's case again — a name that draws nothing has picked no variant — so
//!   the family is searched *without* the erroring name, and
//!   [`ClearanceFix::faulty`] tells the two apart in the report.
//!
//! What is skipped is what cannot be measured *after* a choice either: a part
//! with no ink of its own, a composite this pass cannot flatten, a component
//! whose family draws nothing that could fill the slot.
//!
//! # A line that stands for a family
//!
//! A pattern block writes one line for every glyph it declares, each composed
//! of its own parts. What a rewrite may move there is what the family
//! *shares*: the gaps, and a component's variant **label** whenever the block's
//! own pattern does not reach it — `han-4ee4-($han-regions):9x16` says the same
//! `9x16` for every glyph, so that label is the family's answer
//! ([`slot_choices`]); `han-4ee4-g:(7|9)x16` says something different per glyph
//! and is left alone. The *base* is never searched. A component with no label
//! at all keeps what the line writes before the `:` verbatim — a `($-1)` and
//! all — and the label found for the family is appended
//! ([`write_pattern_line`]). Which family a label is looked for in is each
//! member's own, under the name the *member* carries, since a name reached only
//! through an `exists`-scoped alias is a family like any other.
//!
//! One set of gaps and labels then has to serve every glyph, so the objective
//! is **the fewest glyphs warning at all**, then the fewest with no layout,
//! then the summed cost and the tie-breaks below. The warnings are a work
//! queue and its length is what the command is there to shorten, so a family in
//! which one more glyph is finished beats one in which every glyph is a little
//! less wrong. The middle number keeps a TODO from outliving every answer to
//! it: a glyph with no layout measures nothing and would otherwise beat every
//! decision that leaves a warning ([`PatternKey::unresolved`]).
//! [`optimize_pattern_line`] is the whole of it, and [`Member`] is why it stays
//! cheap over thousands: one glyph costs a handful of additions per set of
//! gaps, whatever the labels chose.
//!
//! # The search
//!
//! Per slot, the candidates are the variants of the component's base name,
//! filtered to those that could go there: the box must fit across the axis, a
//! `:WxH` in the name must be true, a drawing as long as the glyph's own axis is
//! out ([`fits_beside`]), and a name drawn for another direction is out
//! (`compose::direction_rank` = 2). The component as written is always a
//! candidate — it is the source's choice, not a proposal — unless it names no
//! drawing that could fill anything.
//!
//! The score of a layout is how far its clearances fall outside the range,
//! summed over the n+1 clearances plus their total — exactly the numbers the
//! check reports on. Zero is "no clearance finding", not "no warning": a
//! wrong-slot component warns at any score and is counted beside it.
//!
//! What the search minimizes is not quite the score but its **cost**
//! ([`cost`]): the score, plus one and a half per cell the parts' boxes leave
//! of the glyph's at its two edges — a leading gap, or what the last part
//! leaves at the far end. Room at an edge reads as the glyph shoved to one
//! side; the same room between the parts reads as their spacing, even past
//! `max`. So `a 2 b` is written in preference to `a 1 b` with a cell left at
//! the far edge, although the check counts one more cell against it — and a
//! rewrite may report a larger score than the line had, having lowered the
//! cost. It is the boxes that are held to the edges and not the ink, so a part
//! drawn with a bearing of its own keeps it.
//!
//! # Why the gaps need no search
//!
//! Because the clearances *are* the free variables, and their sum is not one.
//! Placing the parts is the same as choosing `c₀ … c_{k-1}` freely, since
//! moving a part moves exactly the two clearances beside it in opposite
//! directions, and their sum telescopes to
//!
//! ```text
//! T = near(first) + Σ facing(a, b) + (extent - 1 - far(last))
//! ```
//!
//! which mentions no position — a property of the *variants*. So the question
//! is "which integers summing to a fixed T cost least", which is arithmetic:
//! [`cost`] is one convex function per clearance, and a fixed sum of those is
//! minimized a cell at a time. [`arrange`] is that walk, and only the variants
//! are searched.
//!
//! `audit max-contact-run` does not disturb any of that: what a junction owes it
//! is a property of the pair, not of where the line puts them, so it lands
//! inside the facing measurement ([`crate::compose::effective_facing`]) exactly
//! where a hardblank would have, and every sum below reads it unknowingly.
//!
//! # Which of the equally good answers
//!
//! 1. fewer components in a slot they are not drawn for (the warning above);
//!    then more variants stating their slot's own direction. Two numbers and not
//!    one sum of ranks, because a sum cannot tell `[-l, -r]` reversed from two
//!    unmarked names, and only the first warns;
//! 2. the smallest sum of the two edge clearances — parts pushed out against
//!    the box, room grown between them. [`cost`] says the same of the boxes;
//!    this says it of the ink, for the layouts the boxes do not decide;
//! 3. the most even inner clearances, when there are two (`⿲`/`⿳`);
//! 4. the least room the line writes around the parts' boxes, gaps and the
//!    leftover at the far end alike: of two variants that draw the same ink,
//!    the one whose box is a cell longer, rather than a gap standing in for
//!    that cell beside the shorter one;
//! 5. lexicographically smallest, left first;
//! 6. the line as written, then the names in order — so a run over an unchanged
//!    source is a no-op.
//!
//! Steps 2, 3 and 5 are the order [`arrange`] builds one layout in; they
//! appear again because two *different* variant choices also have to be
//! ordered.
//!
//! # An enclosure
//!
//! Planned by [`optimize_enclosure_line`], differing in four ways that follow
//! from what the layout is:
//!
//! - **the placements are searched, not solved.** The two axes are not
//!   independent: how much room the left wall leaves depends on which *rows*
//!   the inner part covers, which is the other axis's answer — a `⿴` 囗 does
//!   not care and a `⿺` 辶 cares a great deal. Trying them is affordable because
//!   they are *offsets* inside the glyph, a box a few cells on a side;
//! - **`edge_sum` is over the sides the operator opens on** — one clearance per
//!   open side, none on a `⿴`;
//! - **`inner_spread` is over the axes walled on both sides**, which centres
//!   the inner part of a `⿴` where the lexicographic rule would wedge it into
//!   a corner;
//! - **the objective is the plain score**, not [`cost`]: the clearances of an
//!   enclosure are the inner part against its walls, and `edge_sum` already
//!   pushes it out on the open sides.
//!
//! [`optimize_pattern_enclosure_line`] reads those rules over a family the way
//! [`optimize_pattern_line`] reads a split's: the two offsets and the labels
//! are chosen for the family as a whole, scored against every member at once,
//! over the smallest of the members' boxes. That the walls differ from member
//! to member is exactly why the choice is collective.

mod enclosure;
mod inventory;
mod layout;
mod names;
mod pattern;
mod pattern_enclosure;
mod split;

use self::enclosure::optimize_enclosure_line;
use self::inventory::Inventory;
#[cfg(doc)]
use self::inventory::SlotState;
#[cfg(doc)]
use self::layout::{arrange, cost, fits_beside};
use self::names::is_plain_name;
use self::pattern::optimize_pattern_line;
#[cfg(doc)]
use self::pattern::{Member, PatternKey, slot_choices, write_pattern_line};
use self::pattern_enclosure::optimize_pattern_enclosure_line;
use self::split::optimize_line;
use crate::document::{Document, DocumentItem};
use std::path::PathBuf;

/// One IDC line the optimizer would rewrite.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearanceFix {
    /// The glyph whose block the line is in.
    pub glyph: String,
    /// Where that glyph was in the document the plan was made from — a hint,
    /// re-checked against the name when the fix is applied.
    pub item_idx: usize,
    /// Which IDC line of the block, in written order. Always 0 in a source
    /// that builds, a second line being an error.
    pub compose_idx: usize,
    /// The line as it stands, canonically formatted. For the report; what is
    /// on the line is whatever the author wrote.
    pub old_line: String,
    /// The line to put there instead.
    pub new_line: String,
    /// The score before and after: how far the clearances fall outside the
    /// range, summed — over the whole family for a pattern line.
    ///
    /// What a rewrite lowers is the [`cost`], not this, so `after` may be the
    /// larger of the two: a cell moved off an edge into the middle may put the
    /// middle past the range. For a pattern line the objective is
    /// [`glyphs_warning`](Self::glyphs_warning) first, so `after` may also be
    /// larger when the rewrite finishes a glyph at the others' expense.
    ///
    /// `None` before means the line had no layout to score — a component had
    /// not picked its variant, so there was nothing measured rather than
    /// something measured badly.
    pub before: Option<i32>,
    pub after: i32,
    /// How many components sit in a slot their name is not drawn for, before
    /// and after — the other thing this command answers, and the one a score of
    /// zero says nothing about. For a pattern line it counts *slots*, the
    /// label a slot carries being one thing the whole family shares. `None`
    /// when the line was not scored as written at all (an undecided
    /// component), there being no before to compare against.
    pub mismatched: Option<(usize, usize)>,
    /// How many of the glyphs the line stands for warn at all, before and
    /// after. `None` for a line that stands for one glyph, which either warns
    /// or is not planned.
    pub glyphs_warning: Option<(usize, usize)>,
    /// Whether the line as written names something the check *errors* on — a
    /// component nothing defines, one whose box does not fill the slot across
    /// the axis, one whose name is wrong about its own size. Such a line has no
    /// layout to have measured badly, so a plain one's [`before`](Self::before)
    /// is `None` for the same reason a TODO's is; the flag is how the report
    /// tells the two apart.
    pub faulty: bool,
}

/// What one document's fixes are, in the order the lines appear.
#[derive(Clone, Debug)]
pub struct DocumentFixes {
    /// Index into the `docs` slice the plan was made from.
    pub doc_idx: usize,
    #[cfg_attr(not(feature = "editor"), expect(dead_code))]
    pub path: PathBuf,
    pub fixes: Vec<ClearanceFix>,
}

/// Candidates are capped per slot, and their product per line, so that a source
/// with an unexpectedly large variant family cannot turn one line into an hour.
/// A real one has a handful: the widest measured Han radical family is 5.
const MAX_CANDIDATES: usize = 32;
const MAX_COMBINATIONS: usize = 32_768;
/// The same, for a pattern line, where the unit of work is one glyph of the
/// family scored at one set of gaps: a few million of those is a few
/// milliseconds, and no real line comes close.
const MAX_PATTERN_WORK: usize = 4_194_304;

/// Plan every IDC line rewrite the source's `audit ideal-clearance` rules ask
/// for. Reads the documents and nothing else; see [`crate::fix`] for who
/// applies the result.
pub fn optimize_clearance(docs: &[&Document]) -> Vec<DocumentFixes> {
    let audit = crate::audit::AuditRules::collect(docs);
    let rules = &audit.ideal_clearance;
    if rules.is_empty() {
        return Vec::new();
    }
    // The base bindings, not a slice's: a `$` in an IDC line stands for a name
    // whichever face is being built, and a slice-scoped binding would make one
    // line several, which is not something one rewrite could be right for.
    let name_parts = crate::document::collect_name_parts(docs);
    // The searches too, so that the inventory reads the same set of names the
    // build does. A `glyph A = B` under an `exists` is how a source states a
    // whole family of second names at once ([`crate::exists`]), and the fixer
    // that cannot see one cannot find the drawing behind a component that uses
    // it; the diagnostics are the resolution pass's to report, not this one's.
    let (exists, _) = crate::exists::resolve_scopes(docs, &name_parts);
    let inventory = Inventory::collect(docs, &name_parts, &exists);

    let mut out: Vec<DocumentFixes> = Vec::new();
    for (doc_idx, doc) in docs.iter().enumerate() {
        let mut fixes: Vec<ClearanceFix> = Vec::new();
        for (item_idx, item) in doc.items.iter().enumerate() {
            let DocumentItem::Glyph { name, body } = item else {
                continue;
            };
            // A glyph split twice is an error the source has to answer first;
            // both lines claim to be the whole shape, so neither is a layout
            // to improve.
            if body.compose.len() != 1 {
                continue;
            }
            let glyph = name.display();
            let Some(parent) = body.declared_extent() else {
                continue;
            };
            for (compose_idx, compose) in body.compose.iter().enumerate() {
                // An `assume`d layout is the author's call, and the one thing
                // a rewrite would do to it is undo that. See `crate::compose`.
                if compose.assumed {
                    continue;
                }
                // A pattern block is one line over a family, and what its
                // glyphs share is the gaps; a plain one is a layout of its own.
                let planned: Option<PlannedLine> = match (is_plain_name(&glyph), compose.op.walls())
                {
                    (true, walls) => rules.for_glyph(&glyph).and_then(|(_, band)| {
                        let (min, max) = band.range(walls.is_some());
                        let contact = audit.max_contact_run.for_glyph(&glyph).map(|(_, m)| m);
                        match walls {
                            Some(walls) => optimize_enclosure_line(
                                &inventory, walls, parent, compose, min as i32, max as i32, contact,
                            ),
                            None => optimize_line(
                                &inventory, parent, compose, min as i32, max as i32, contact,
                            ),
                        }
                    }),
                    // A pattern block that *encloses* shares its two offsets
                    // and its labels the way a split shares its gaps, and is
                    // planned over the whole family for the same reason.
                    (false, Some(walls)) => optimize_pattern_enclosure_line(
                        &inventory,
                        &audit,
                        &name_parts,
                        &glyph,
                        body.scale,
                        walls,
                        parent,
                        compose,
                    ),
                    (false, None) => optimize_pattern_line(
                        &inventory,
                        &audit,
                        &name_parts,
                        &glyph,
                        body.scale,
                        parent,
                        compose,
                    ),
                };
                let Some(planned) = planned else { continue };
                fixes.push(ClearanceFix {
                    glyph: glyph.clone(),
                    item_idx,
                    compose_idx,
                    old_line: compose.format_line(),
                    new_line: planned.line,
                    before: planned.before,
                    after: planned.after,
                    mismatched: planned.mismatched,
                    glyphs_warning: planned.glyphs_warning,
                    faulty: planned.faulty,
                });
            }
        }
        if !fixes.is_empty() {
            out.push(DocumentFixes {
                doc_idx,
                path: doc.path.clone(),
                fixes,
            });
        }
    }
    out
}

/// What planning one IDC line comes to: the line to write in its place, the
/// score before it and after, and — for a line that stands for a family — how
/// many of its glyphs warn on each side.
/// What planning one line comes to, whichever of the two planners did it.
/// The fields are [`ClearanceFix`]'s, minus the ones naming where the line is.
struct PlannedLine {
    line: String,
    before: Option<i32>,
    after: i32,
    mismatched: Option<(usize, usize)>,
    glyphs_warning: Option<(usize, usize)>,
    faulty: bool,
}

#[cfg(test)]
#[path = "../clearance_tests.rs"]
mod tests;
