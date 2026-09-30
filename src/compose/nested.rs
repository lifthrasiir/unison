//! Nested splits: the operator, key and size a nested split is inferred with, and the line and body it stands for.

use super::op::IdcOp;
use super::split::{Frame, expand_line};
use super::variant::{PartDims, is_undecided};
use crate::document::{ComposeItem, GlyphCompose};
use crate::issues::Severity;

/// The operator a [nested split](crate::compose#nested-splits) inside a line of `outer`
/// splits its box with: the one across `outer`'s axis, taking as many parts as
/// the nested split writes. One part is the two-part operator's, since a single
/// member only pads its slot across the axis, and so is a count of four or
/// more, which [`expand_compose`](super::expand_compose) then reports.
pub fn nested_op(outer: IdcOp, parts: usize) -> IdcOp {
    match (outer.horizontal(), parts == 3) {
        (true, false) => IdcOp::AboveBelow,
        (true, true) => IdcOp::AboveMiddleBelow,
        (false, false) => IdcOp::LeftRight,
        (false, true) => IdcOp::LeftMiddleRight,
    }
}

/// The name a nested split's ink is kept under: its operator and its members'
/// resolved names. No glyph can be called this — a `|` outside parentheses in
/// a block's name makes it a list of names — and the operator keeps the same
/// token apart in a ⿰ line and a ⿱ one, where it stands for two different
/// shapes.
pub fn nested_key(outer: IdcOp, members: &[ComposeItem]) -> String {
    let parts = members
        .iter()
        .filter(|it| matches!(it, ComposeItem::Part { .. }))
        .count();
    format!(
        "{}{}",
        nested_op(outer, parts).as_char(),
        ComposeItem::nested_token(members, true)
    )
}

/// The box a nested split's glyph would declare, inferred from its members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedSize {
    /// `(width, height)`: along the nested split's own axis, the sum of its gaps and
    /// its members' extents; across it, the first sized member's extent. A
    /// member with no size contributes nothing.
    pub box_: (u16, u16),
    /// Whether every member has a size, so that `box_` is the box and not a
    /// lower bound on it.
    pub complete: bool,
}

/// The IDC line a nested split of a line of `outer` stands for, and the box its glyph
/// would declare. See the module docs (`# Nested splits`).
pub fn nested_line(
    outer: IdcOp,
    members: &[ComposeItem],
    dims: &dyn Fn(&str) -> PartDims,
) -> (GlyphCompose, NestedSize) {
    let parts = members
        .iter()
        .filter(|it| matches!(it, ComposeItem::Part { .. }))
        .count();
    let op = nested_op(outer, parts);
    let mut along: i32 = 0;
    let mut across: Option<u16> = None;
    let mut complete = true;
    for item in members {
        match item {
            ComposeItem::Gap(gap) => along += *gap as i32,
            ComposeItem::Part { name, .. } => match dims(name) {
                PartDims::Size(w, h, _) => {
                    let (a, c) = if op.horizontal() { (w, h) } else { (h, w) };
                    along += a as i32;
                    across.get_or_insert(c);
                }
                PartDims::Unknown | PartDims::Undeclared => complete = false,
            },
            // A nested split does not nest again; the parser writes none.
            ComposeItem::Nested(_) => complete = false,
        }
    }
    let along = along.clamp(0, u16::MAX as i32) as u16;
    let across = across.unwrap_or_else(|| {
        complete = false;
        0
    });
    let box_ = if op.horizontal() {
        (along, across)
    } else {
        (across, along)
    };
    let line = GlyphCompose {
        op,
        items: members.to_vec(),
        assumed: false,
        comment: None,
    };
    (line, NestedSize { box_, complete })
}

/// The glyph a nested split stands for, as a body the resolution can flatten:
/// the refs its line derives, in a box of the size [`nested_line`] infers. It
/// is the refs and not the line because the line of a one-part nested split
/// is not one a glyph could write ([`nested_op`]).
///
/// `None` where the flattening would not be the shape the source means — a
/// member undecided, a box that cannot be inferred, a line that errors — for
/// the reason `ref_composite::derive_compose_body` gives: half a part's ink
/// measured is worse than none.
pub fn nested_body(
    outer: IdcOp,
    members: &[ComposeItem],
    dims: &dyn Fn(&str) -> PartDims,
) -> Option<crate::document::GlyphBody> {
    let (line, size) = nested_line(outer, members, dims);
    if !size.complete || line.part_names().any(is_undecided) {
        return None;
    }
    let frame = Frame {
        context: String::new(),
        nested: true,
    };
    let (refs, issues) = expand_line(&frame, Some(size.box_), 1, &line, dims, None, None);
    if issues.iter().any(|(s, _)| *s == Severity::Error) {
        return None;
    }
    Some(crate::document::GlyphBody {
        refs,
        extent: Some(size.box_),
        ..crate::document::GlyphBody::new()
    })
}
