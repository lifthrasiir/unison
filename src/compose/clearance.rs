//! The clearances of a placed IDC line, and their report against an `audit ideal-clearance` rule.

use super::fit::{ClearanceRule, InkLookup};
use super::gap::{ContactDemand, GapSide, contact_demand, facing_offset};
use super::op::IdcOp;
use crate::issues::Severity;

/// Every clearance of a placed IDC line, near edge to far edge, each with what
/// it is between; `None` when the line cannot be measured at all.
///
/// `placed` is each component, where it starts along the axis, and where it
/// starts across it — 0 but for a part its margin pads — in declared units. A
/// component with no ink to measure — see the module docs — makes the
/// whole line unmeasurable, and so does a neighbouring pair that shares no line
/// on which both draw something.
///
/// An n-component line yields n+1 clearances, and their **sum is a property of
/// the parts alone**: it telescopes down to the parent's extent less what the
/// parts' ink spans, so nothing about where the parts were placed survives in
/// it. That is why the check below holds the sum to the range as well as each
/// clearance, and it is what [`crate::fix::clearance`] optimizes against.
pub fn measure_clearances<'a>(
    op: IdcOp,
    axis_extent: u16,
    placed: &[(&str, i32, i32)],
    ink: &InkLookup<'a>,
    max_contact_run: Option<u16>,
) -> Option<Vec<Clearance>> {
    /// A component of the line, ready to measure: where it sits along the axis
    /// and what its ink does.
    struct Placed<'a> {
        name: &'a str,
        offset: i32,
        side: GapSide<'a>,
    }

    let horizontal = op.horizontal();
    let mut parts: Vec<Placed> = Vec::new();
    for &(name, offset, cross) in placed {
        parts.push(Placed {
            name,
            offset,
            side: GapSide {
                cross,
                ..GapSide::linear(ink(name)?)
            },
        });
    }
    let (first, last) = (parts.first()?, parts.last()?);

    // `(what it is between, how much)`, near edge to far edge.
    let (near_edge, far_edge) = match horizontal {
        true => ("the left edge", "the right edge"),
        false => ("the top edge", "the bottom edge"),
    };
    let mut clearances: Vec<Clearance> = Vec::new();
    let near = first.side.profile.frontier(horizontal)?.near + first.offset;
    clearances.push(Clearance {
        between: format!("{near_edge} and '{}'", first.name),
        value: near,
        contact: None,
        horizontal,
        at_edge: true,
    });
    for pair in parts.windows(2) {
        let [a, b] = pair else { continue };
        let facing = facing_offset(a.side, b.side, horizontal)?;
        // Measured only where a rule asks for it: a source stating none pays
        // nothing, exactly as it pays nothing for the profiles themselves.
        let contact =
            max_contact_run.and_then(|max| contact_demand(a.side, b.side, horizontal, max));
        // The rule says its piece *as* a clearance: the cell it asks for is not
        // room the glyph still has, and whether what is left is worth a warning
        // is `ideal-clearance`'s answer and not a second one.
        clearances.push(Clearance {
            between: format!("'{}' and '{}'", a.name, b.name),
            value: (b.offset - a.offset) + facing - contact.map_or(0, |d| d.owed),
            contact,
            horizontal,
            at_edge: false,
        });
    }
    let far = axis_extent as i32 - 1 - (last.offset + last.side.profile.frontier(horizontal)?.far);
    clearances.push(Clearance {
        between: format!("'{}' and {far_edge}", last.name),
        value: far,
        contact: None,
        horizontal,
        at_edge: true,
    });
    Some(clearances)
}

/// One measured clearance of an IDC line: what it is between, how much room is
/// there, and — for a clearance between two parts — how far their ink runs
/// together.
///
/// `value` is what the check holds to `audit ideal-clearance`, with the cell
/// `audit max-contact-run` takes back already gone from it. The two rules meet
/// in this one number on purpose: a source states one range for how a split
/// should look, and a contact too long is a way of not looking like it rather
/// than a separate complaint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clearance {
    /// What it is between, ready to drop into a message.
    pub between: String,
    /// The room left, the contact rule's cell already taken back.
    pub value: i32,
    /// What the contact rule made of the two parts; `None` at a glyph edge, and
    /// for every clearance when no `audit max-contact-run` rule is in force.
    pub contact: Option<ContactDemand>,
    /// Which of the parent's axes the gap is measured along. Every clearance of
    /// a one-dimensional line is on the split's own axis; an enclosure has two
    /// on each, and it is **per axis** that the sum telescopes to a property of
    /// the parts alone — which is what makes the check and the fixer both read
    /// this rather than assume one axis.
    pub horizontal: bool,
    /// Whether the gap runs to the parent's own edge rather than to another
    /// part. An enclosure's tie-break is "push the inner part out against the
    /// sides the operator leaves open", and this is the set it is over.
    pub at_edge: bool,
}

/// Report the clearances outside `rule`'s range, plus their sum. See
/// [`measure_clearances`] for the measurement and the module docs for what a
/// clearance is; a line that cannot be measured says nothing.
pub(super) fn check_clearances(
    op: IdcOp,
    axis_extent: u16,
    placed: &[(&str, i32, i32)],
    rule: &ClearanceRule,
) -> Vec<(Severity, String)> {
    let Some(clearances) =
        measure_clearances(op, axis_extent, placed, rule.ink, rule.max_contact_run)
    else {
        return Vec::new();
    };
    report_clearances(op, &clearances, rule)
}

/// Report the clearances outside `rule`'s range, plus each axis's total.
///
/// Split from the measurement because an enclosure measures its four
/// differently ([`measure_enclosure_clearances`](super::enclosure::measure_enclosure_clearances)) and is held to them exactly
/// the same way: one range for every gap, and one for what the axis leaves in
/// total.
pub(super) fn report_clearances(
    op: IdcOp,
    clearances: &[Clearance],
    rule: &ClearanceRule,
) -> Vec<(Severity, String)> {
    let (min, max) = rule.band.range(op.enclosing());
    let (min, max) = (min as i32, max as i32);
    // The band's own numbers are already in the message, so the directive is
    // quoted by the prefix that selected it, exactly as it was when there was
    // only one band to select.
    let range = format!(
        "the ideal {min}..{max} (`audit ideal-clearance {}`)",
        rule.written,
    );
    // Why a clearance came out a cell short, said where the shortfall is read.
    let blamed_on_contact = |c: &Clearance| match (c.contact, rule.max_contact_run) {
        (Some(d), Some(limit)) if d.owed > 0 => format!(
            " — they would run together over {} lines if they met, more than the ideal \
             {limit} (`audit max-contact-run {}`), so a cell between them is spoken for",
            d.run, rule.contact_written,
        ),
        _ => String::new(),
    };
    // A clearance finding is a [`Severity::Chore`]: the same defect a warning
    // reports, held to the same band and flagging the same glyph, but one every
    // IDC line in the font is measured against — so it is counted rather than
    // printed by a build, and the twenty findings that are somebody's next task
    // stay visible. See [`Severity`].
    let mut out: Vec<(Severity, String)> = clearances
        .iter()
        .filter(|c| !(min..=max).contains(&c.value))
        .map(|c| {
            (
                Severity::Chore,
                format!(
                    "leaves {} between {}, outside {range}{}",
                    c.value,
                    c.between,
                    blamed_on_contact(c),
                ),
            )
        })
        .collect();
    // Per axis, because that is the unit the sum is a property of the parts
    // over: a one-dimensional line has one axis and this says exactly what it
    // always did, while an enclosure has two sums that mean two different
    // things and adding them together would mean neither.
    for (horizontal, axis) in axes_of(op) {
        let on_axis = || clearances.iter().filter(|c| c.horizontal == horizontal);
        let total: i32 = on_axis().map(|c| c.value).sum();
        if on_axis().next().is_none() || (min..=max).contains(&total) {
            continue;
        }
        let breakdown = on_axis()
            .map(|c| format!("{} between {}", c.value, c.between))
            .collect::<Vec<_>>()
            .join(", ");
        out.push((
            Severity::Chore,
            format!("leaves {total}{axis} in total, outside {range} — {breakdown}"),
        ));
    }
    out
}

/// The axes an operator's clearances are nested by, each with the word a
/// message names it by. A one-dimensional line has one and names it nothing —
/// there is no other axis for it to be told apart from.
fn axes_of(op: IdcOp) -> Vec<(bool, &'static str)> {
    match op.enclosing() {
        false => vec![(op.horizontal(), "")],
        true => vec![(true, " across"), (false, " down")],
    }
}
