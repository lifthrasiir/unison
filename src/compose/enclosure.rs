//! Expanding an *enclosure* line, and measuring the clearances and cavity of a placed one.

use super::clearance::{Clearance, report_clearances};
use super::fit::{
    ClearanceRule, ComposeExpansion, FamilyLookup, fits_enclosure_slot, misfit_variants,
    outer_padding,
};
use super::gap::{GapSide, contact_demand, facing_offset};
use super::ink::InkProfile;
use super::op::Walls;
use super::split::clamp_offset;
use super::variant::{PartDims, VariantSpec, enclosure_rank, is_undecided};
use crate::document::{ComposeItem, GlyphCompose, GlyphRef, Margin};
use crate::issues::Severity;

/// What an outer part's margin would have made its box, for the message that
/// says it is not the glyph's: `" (7x4 with its `margin-x 1` `margin-y 0`)"`,
/// or nothing for a part that states no margin.
pub(super) fn padded_box(size: (u16, u16), margin: Margin) -> String {
    if margin == Margin::default() {
        return String::new();
    }
    let add = |n: u16, m: Option<(u16, u16)>| {
        m.map_or(n as u32, |(lo, hi)| n as u32 + lo as u32 + hi as u32)
    };
    let flags: Vec<String> = [(true, margin.x), (false, margin.y)]
        .into_iter()
        .filter_map(|(x_axis, m)| Some(format!("`{}`", Margin::flag(x_axis, m?))))
        .collect();
    format!(
        " ({}x{} with its {})",
        add(size.0, margin.x),
        add(size.1, margin.y),
        flags.join(" "),
    )
}

/// Turn one *enclosure* line into the `ref`s it stands for, plus what is wrong
/// with it. The enclosing half of [`expand_compose`](super::expand_compose); the caller has already
/// checked the arity and the parent's box, and prefixes the messages.
///
/// # The line
///
/// `IDC OUTER INNER P Q`. The two numbers are the inner part's **top-left
/// offsets** inside the parent's box, and not gaps — which is the one place an
/// enclosure line reads differently from a split. A gap would be the natural
/// spelling and it does not work: an enclosure has two gaps on each axis and
/// fixing all four still leaves the layout ambiguous wherever a wall's inner
/// face is ragged, since "one cell from the left wall" is a different column on
/// every row. An offset is one answer to where the part is, and the clearances
/// are then measured rather than declared.
///
/// # An unplaced line is not a wrong one
///
/// A line that writes no offsets has not decided where its inner part goes.
/// That is a [`Severity::Todo`] for the same reason an unpicked variant is: it
/// is the state every enclosure populated from IDS starts in, and the work it
/// names is "decide", which `uniform fix --optimize-clearance` is there to do.
/// It is deliberately *not* read as `0 0` — that would wedge the inner part
/// into the corner of the walls and report it as a decision someone made.
pub(super) fn expand_enclosure(
    walls: Walls,
    parent: (u16, u16),
    scale: i32,
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
    family: Option<&FamilyLookup>,
    clearance: Option<&ClearanceRule>,
) -> ComposeExpansion {
    let mut issues: Vec<(Severity, String)> = Vec::new();
    let mut refs: Vec<GlyphRef> = Vec::new();

    // The written order matters here in a way it does not on a split: the
    // numbers are the second component's, so they follow it.
    let mut names: Vec<(&String, Option<&String>)> = Vec::new();
    let mut offsets: Vec<i16> = Vec::new();
    let mut number_before_part = false;
    for item in &compose.items {
        match item {
            ComposeItem::Gap(n) => offsets.push(*n),
            ComposeItem::Part { name, raw_name } => {
                number_before_part |= !offsets.is_empty();
                names.push((name, raw_name.as_ref()));
            }
            // Only a split's slot is one axis's share, which is what a nested split
            // divides further; see the module docs.
            ComposeItem::Nested(members) => issues.push((
                Severity::Error,
                format!(
                    "writes the nested split '{}', but only a split takes one: an enclosure's parts \
                     are one glyph each",
                    ComposeItem::nested_token(members, false),
                ),
            )),
        }
    }
    if number_before_part {
        issues.push((
            Severity::Error,
            "writes the inner part's offsets after both components, as `X Y P Q`".to_string(),
        ));
    }
    let placement = match offsets.len() {
        0 => {
            // Not a placement of `0 0`: nothing has been decided about where
            // the inner part goes, and saying it sits in the corner of the
            // walls would report a decision nobody made. See the module docs.
            issues.push((
                Severity::Todo,
                "has no placement picked yet; an enclosure writes the inner part's top-left \
                 offsets, as in `X Y 3 2`"
                    .to_string(),
            ));
            None
        }
        2 => Some((offsets[0] as i32, offsets[1] as i32)),
        n => {
            issues.push((
                Severity::Error,
                format!(
                    "takes the inner part's two offsets or none at all, not {n}: they are where \
                     its top-left corner sits, not the room around it"
                ),
            ));
            None
        }
    };
    if names.len() != 2 {
        // The arity message is the caller's; there is nothing here to lay out.
        return (refs, issues);
    }

    // Per slot: what the name claims, and whether the drawing bears it out.
    // `unresolved` is the line's own "nothing has been decided yet" — an
    // unpicked variant or an unwritten placement, either of which makes every
    // measurement below a measurement of a layout nobody meant.
    let mut unresolved = placement.is_none();
    let mut sizes: [Option<(u16, u16)>; 2] = [None, None];
    // Where the outer part sits: off the glyph's corner only where its margin
    // pads it.
    let mut outer_at = (0i32, 0i32);
    for (slot, &(name, raw_name)) in names.iter().enumerate() {
        let outer = slot == 0;
        // What the name claims is what the *author* wrote, so both the size and
        // the cavity are read off the written name; see the split's own note.
        let written = raw_name.unwrap_or(name);
        let spec = VariantSpec::parse(written);
        let role = if outer { "outer" } else { "inner" };
        if is_undecided(name) {
            unresolved = true;
            let fits = |size, margin| fits_enclosure_slot(size, margin, parent, outer);
            match misfit_variants(name, family, &fits) {
                Some(sizes) => issues.push((
                    Severity::Warning,
                    format!(
                        "component '{name}' has no variant that could be the {role} part of a \
                         {}x{} glyph; its family draws {sizes}",
                        parent.0, parent.1,
                    ),
                )),
                None => issues.push((
                    Severity::Todo,
                    format!(
                        "component '{name}' has no variant picked yet; the {role} part of an \
                         enclosure names the sized variant it wants, as in `{name}:{}`",
                        match outer {
                            true => format!("{}x{}.NxM", parent.0, parent.1),
                            false => "NxM".to_string(),
                        },
                    ),
                )),
            }
            continue;
        }
        // A cavity is what marks a drawing as one made to enclose, which is
        // the enclosure's version of the `-l`/`-r` claim a split's name makes
        // — and, like it, a mismatch is a warning: a drawing that promises a
        // cavity may still be the thing the author wanted inside another. Like
        // that claim, it is read off the name *as written*: a name reached
        // through an alias promises whatever the alias says and nothing more,
        // and resolving it would report a claim the author never made.
        if enclosure_rank(written, outer) == 2 {
            issues.push((
                Severity::Warning,
                match outer {
                    true => format!(
                        "component '{name}' promises no cavity, so nothing says it was drawn to \
                         enclose; an outer part names the room it offers, as `:{}x{}.NxM`",
                        parent.0, parent.1,
                    ),
                    false => format!(
                        "component '{name}' promises a cavity, so it was drawn to enclose \
                         something rather than to sit inside one"
                    ),
                },
            ));
        }
        match dims(name) {
            PartDims::Unknown => {
                issues.push((
                    Severity::Error,
                    format!("component '{name}' is not defined"),
                ));
            }
            PartDims::Undeclared => {
                issues.push((
                    Severity::Error,
                    format!(
                        "component '{name}' declares no `W H` on its `glyph` header, so it has \
                         no box to be placed by"
                    ),
                ));
            }
            PartDims::Size(w, h, margin) => {
                if let Some(size) = spec.size
                    && size != (w, h)
                {
                    issues.push((
                        Severity::Error,
                        format!(
                            "component '{name}' names {}x{} but the glyph is {w}x{h}",
                            size.0, size.1
                        ),
                    ));
                } else if !fits_enclosure_slot((w, h), margin, parent, outer) {
                    issues.push((
                        Severity::Error,
                        match outer {
                            true => format!(
                                "component '{name}' is {w}x{h}, not the glyph's {}x{}{}: the outer \
                                 part's walls are the glyph's, so it fills the box exactly",
                                parent.0,
                                parent.1,
                                padded_box((w, h), margin),
                            ),
                            false => format!(
                                "component '{name}' is {w}x{h}, which does not fit the glyph's \
                                 {}x{}",
                                parent.0, parent.1,
                            ),
                        },
                    ));
                } else {
                    sizes[slot] = Some((w, h));
                    if outer && let Some((x, y)) = outer_padding((w, h), margin, parent) {
                        outer_at = (x as i32, y as i32);
                    }
                }
            }
        }
    }

    let (p, q) = placement.unwrap_or((0, 0));
    for (slot, &(name, raw_name)) in names.iter().enumerate() {
        let (col, row) = match slot {
            0 => (outer_at.0 * scale, outer_at.1 * scale),
            _ => (p * scale, q * scale),
        };
        refs.push(GlyphRef {
            name: name.clone(),
            raw_name: raw_name.cloned(),
            offset: Some((clamp_offset(col), clamp_offset(row))),
            negated: false,
            inherit: false,
            goto: false,
            fill: None,
            visibility: None,
            comment: None,
        });
    }

    // Only over a line that is otherwise sound, exactly as on a split.
    let sound = !unresolved
        && sizes.iter().all(Option::is_some)
        && !issues.iter().any(|(s, _)| *s == Severity::Error);
    if let Some(rule) = clearance
        && sound
    {
        let outer_name = names[0].0.as_str();
        let inner_name = names[1].0.as_str();
        if let (Some(outer), Some(inner)) = ((rule.ink)(outer_name), (rule.ink)(inner_name)) {
            // The cavity the outer part's *name* promises, against the room its
            // drawing actually leaves. A lower bound and not an equality: what
            // matters is that the promise is kept, and a drawing more generous
            // than its name is not a fault.
            if let Some(cavity) = VariantSpec::parse(names[0].1.unwrap_or(names[0].0)).inner
                && !cavity_fits(outer, walls, parent, cavity, outer_at)
            {
                issues.push((
                    Severity::Warning,
                    format!(
                        "component '{outer_name}' promises a {}x{} cavity, but its drawing \
                         leaves no room that size where this operator opens",
                        cavity.0, cavity.1,
                    ),
                ));
            }
            if let Some(clearances) = measure_enclosure_clearances(
                walls,
                parent,
                (outer_name, outer, outer_at),
                (inner_name, inner),
                (p, q),
                rule.max_contact_run,
            ) {
                issues.extend(report_clearances(compose.op, &clearances, rule));
            }
        }
    }
    (refs, issues)
}

/// The four clearances of a placed enclosure line, across then down, each with
/// what it is between.
///
/// On each axis the inner part is measured against the **inner face** of a wall
/// where the operator has one and against the parent's own edge where it does
/// not ([`Walls`]). The outer part's relationship to the parent's edges is not
/// measured at all, and does not need to be: it fills the box exactly, so there
/// is nothing there for a layout to have got wrong. It sits at the third member
/// of `outer`, which is `(0, 0)` but for a part its margin pads
/// ([`outer_padding`]).
///
/// As on a split, each axis's sum is a property of the parts alone — the
/// placement cancels between the axis's two clearances — which is what lets
/// [`crate::fix::clearance`] solve the offsets arithmetically instead of
/// searching them. It is **per axis**, though: the two sums are two different
/// statements and adding them together would make neither.
pub fn measure_enclosure_clearances(
    walls: Walls,
    parent: (u16, u16),
    outer: (&str, &InkProfile, (i32, i32)),
    inner: (&str, &InkProfile),
    at: (i32, i32),
    max_contact_run: Option<u16>,
) -> Option<Vec<Clearance>> {
    let (outer_name, outer, outer_at) = outer;
    let (inner_name, inner) = inner;
    let mut out: Vec<Clearance> = Vec::new();
    for horizontal in [true, false] {
        let (axis_extent, at_edge_pos, cross) = match horizontal {
            true => (parent.0 as i32, at.0, at.1),
            false => (parent.1 as i32, at.1, at.0),
        };
        let (outer_along, outer_across) = match horizontal {
            true => outer_at,
            false => (outer_at.1, outer_at.0),
        };
        // Against a wall the inner part is measured from the outer part's own
        // origin, which is where the facing offsets are counted from.
        let pos = at_edge_pos - outer_along;
        let (wall_lo, wall_hi) = walls.along(horizontal);
        // The outer part reads its cavity-facing side; the inner part is a
        // plain drawing and reads the ends everyone can see.
        let wall = GapSide {
            profile: outer,
            inner: true,
            cross: outer_across,
        };
        let held = GapSide {
            profile: inner,
            inner: false,
            cross,
        };
        let (edge_lo, edge_hi) = match horizontal {
            true => ("the left edge", "the right edge"),
            false => ("the top edge", "the bottom edge"),
        };
        // The low side: from the wall's inner face (or the parent's edge) to
        // the inner part.
        let (value, contact, between, at_edge) = match wall_lo {
            true => {
                let facing = facing_offset(wall, held, horizontal)?;
                let demand =
                    max_contact_run.and_then(|max| contact_demand(wall, held, horizontal, max));
                (
                    pos + facing - demand.map_or(0, |d| d.owed),
                    demand,
                    format!("'{outer_name}' and '{inner_name}'"),
                    false,
                )
            }
            false => (
                at_edge_pos + inner.frontier(horizontal)?.near,
                None,
                format!("{edge_lo} and '{inner_name}'"),
                true,
            ),
        };
        out.push(Clearance {
            between,
            value,
            contact,
            horizontal,
            at_edge,
        });
        // The high side, the two roles swapped: the inner part's far end
        // faces the wall's other inner face.
        let (value, contact, between, at_edge) = match wall_hi {
            true => {
                let facing = facing_offset(held, wall, horizontal)?;
                let demand =
                    max_contact_run.and_then(|max| contact_demand(held, wall, horizontal, max));
                (
                    facing - pos - demand.map_or(0, |d| d.owed),
                    demand,
                    format!("'{inner_name}' and '{outer_name}'"),
                    false,
                )
            }
            false => (
                axis_extent - 1 - (at_edge_pos + inner.frontier(horizontal)?.far),
                None,
                format!("'{inner_name}' and {edge_hi}"),
                true,
            ),
        };
        out.push(Clearance {
            between,
            value,
            contact,
            horizontal,
            at_edge,
        });
    }
    Some(out)
}

/// Whether a drawing leaves the `NxM` rectangle its name promises, in the place
/// the operator says the cavity is.
///
/// The rectangle has to be **flush** against every side the operator leaves
/// open and may sit anywhere along an axis that is walled on both sides: a `⿸`
/// hands its inner part the bottom-right corner and nothing else, while a `⿴`
/// hands it a hole that may be anywhere in the ring. That is the whole of what
/// makes the promise a claim about *this* operator's cavity rather than about
/// empty space in general.
///
/// The walls are read as [`InkProfile`]'s first and last runs — the same faces
/// a clearance is measured against — so a hardblank counts as wall, which is
/// right: it is space the source deliberately keeps clear of whatever goes
/// inside. Anything the drawing puts *between* those two runs is invisible
/// here, which is the price of the sum staying a property of the parts alone;
/// see [`measure_enclosure_clearances`].
///
/// The drawing is read where it sits in the glyph, at `at` — off the corner only
/// for a part its margin pads — so the room it offers is the room the glyph
/// has: a margin on a side the operator opens on is part of the cavity, and one
/// behind a wall is outside it.
pub fn cavity_fits(
    profile: &InkProfile,
    walls: Walls,
    parent: (u16, u16),
    cavity: (u16, u16),
    at: (i32, i32),
) -> bool {
    let (w, h) = (parent.0 as i32, parent.1 as i32);
    let (n, m) = (cavity.0 as i32, cavity.1 as i32);
    if n <= 0 || m <= 0 || n > w || m > h {
        return false;
    }
    // Per row, the columns the walls leave free. A row the drawing puts nothing
    // on is free all the way across.
    let (ax, ay) = at;
    let drawn = profile.rows.len() as i32;
    let free: Vec<(i32, i32)> = (0..h)
        .map(|row| {
            // A row of the margin above or below the drawing is open all the
            // way across when the operator opens on that side, and behind a
            // wall otherwise.
            let walled = match row - ay {
                r if r < 0 => Some(walls.top),
                r if r >= drawn => Some(walls.bottom),
                _ => None,
            };
            match walled {
                Some(true) => return (w, -1),
                Some(false) => return (0, w - 1),
                None => {}
            }
            let line = profile.rows.get((row - ay) as usize).copied().flatten();
            // The wall's cavity face, chosen the way every other measurement
            // chooses it ([`WallFace`]), so that the room a name promises is
            // the room the clearances will be measured in. A side the operator
            // leaves open runs to the box's edge and reads nothing the drawing
            // does out there — a bearing at the box's rim is a claim on the
            // glyph's *neighbour*, not on what goes inside it.
            let lo = match (walls.left, line.and_then(|l| l.low_wall)) {
                (true, Some(w)) => (w.at + ax + 1).max(0),
                (true, None) | (false, _) => 0,
            };
            let hi = match (walls.right, line.and_then(|l| l.high_wall)) {
                (true, Some(f)) => (f.at + ax - 1).min(w - 1),
                (true, None) | (false, _) => w - 1,
            };
            (lo, hi)
        })
        .collect();
    // The rows the rectangle may start on: pinned to an open side, free where
    // the axis is walled on both.
    let starts: Vec<i32> = match (walls.top, walls.bottom) {
        (true, true) => (0..=h - m).collect(),
        (false, _) => vec![0],
        (_, false) => vec![h - m],
    };
    starts.into_iter().any(|start| {
        if start < 0 || start + m > h {
            return false;
        }
        let (lo, hi) = free[start as usize..(start + m) as usize]
            .iter()
            .fold((i32::MIN, i32::MAX), |(a, b), &(lo, hi)| {
                (a.max(lo), b.min(hi))
            });
        match (walls.left, walls.right) {
            // Walled both ways: the rectangle may sit anywhere in the run.
            (true, true) => hi - lo + 1 >= n,
            // Open on the left: flush against column 0.
            (false, _) => lo <= 0 && hi >= n - 1,
            // Open on the right: flush against the last column.
            (_, false) => hi >= w - 1 && lo <= w - n,
        }
    })
}
