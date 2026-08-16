//! IDC composition: `⿰`/`⿱`/`⿲`/`⿳` inside a glyph block, and the variant
//! name rule it reads.
//!
//! # Why the line is a first-class item
//!
//! A CJK glyph built from parts is a *box split along one axis*: each part
//! takes a share of the parent's box, and how much room the shares leave one
//! another is the whole design. Written as ordinary `ref`s with hand-written
//! offsets there is no place for that to live — two parts that crowd each other
//! look exactly like two that do not, and at 20k glyphs "quietly off by one" is
//! undetectable. So the offsets are *derived* from the parts' declared sizes,
//! and what the parts leave each other is measured against a declared range
//! (see [Clearance](#clearance) below).
//!
//! Only the four one-dimensional operators exist here. ⿰ and ⿱ cover 91% of
//! the URO and the four cover 99% of what decomposes one-dimensionally; ⿴⿵⿸⿺
//! and friends do not lay out along one axis, so they stay ordinary `ref` +
//! offset. This is not a general IDS layout engine and is not meant to become
//! one.
//!
//! ```text
//! glyph han-6cb3 15 16
//! ⿰ han-6c35:4x16 han-53ef:11x16
//! ⿰ han-6c35:4x16 -1 han-53ef:12x16    // a negative gap: the boxes overlap
//! ```
//!
//! # The line
//!
//! `IDC TOKEN…`, where each token is a **gap** if it parses as a number and a
//! **component name** otherwise. ⿰/⿱ take two components, ⿲/⿳ three; gaps may
//! appear anywhere among them, including before the first and after the last
//! (which is how a bearing inside the box is written), and default to none.
//!
//! Placement walks the axis in written order: a gap advances the cursor, a
//! component is placed at the cursor and advances it by its own extent. Each
//! component's extent *across* the axis must equal the parent's — a ⿰ part is
//! as tall as the glyph — and that is an error.
//!
//! # An undecided line is not a wrong one
//!
//! A component written without a `:` suffix has not picked its variant yet.
//! That is the initial state of every Han glyph populated from IDS, not a
//! mistake, so it is a [`Severity::Todo`] and not an error: one per unpicked
//! component, and the clearance check — which is about a layout that has not
//! been chosen — stands down for the whole line, as do the unpicked component's
//! own size and cross-axis checks. What is left is the line's *decided* half,
//! still fully checked. The glyph is no more built than an erroring one is; the
//! difference is that a build, a `uniform test` run and CI do not fail over it.
//!
//! Sizes are read from the components' `glyph` headers, never from the
//! composed result: a part's width is a property the part *declares*, which is
//! what makes the layout a lookup rather than a search (see `PLANS.han.md`).
//! A component that names no glyph, or one whose header declares no `W H`, is
//! an error for the same reason.
//!
//! # Clearance
//!
//! A box says nothing about where the ink inside it stops: two parts whose
//! boxes tile the parent perfectly can still collide, or leave a canyon down
//! the middle. So the check reads the drawing itself rather than the boxes.
//!
//! A glyph's **frontier** is, for each line across the split axis, the first
//! and last cell of that line holding anything — a hardblank counts, since it
//! is a cell the source deliberately keeps clear of a neighbour. The
//! **clearance** between two adjacent parts is the smallest per-line distance
//! between the two frontiers that face each other, counted in cells between
//! them: 0 means they touch, negative means they overlap. The parent's own
//! edges take part too, as the distance from the edge inward (negative when the
//! ink crosses it), so an n-part line has n+1 clearances.
//!
//! [`IdealClearances`](crate::audit::IdealClearances) — `audit ideal-clearance
//! PREFIX* MIN MAX` — holds each of them, *and* their total, to one range; a
//! violation is a warning ([`check_clearances`]). Both halves are needed, and
//! the reason is arithmetic: the total telescopes down to the parent's extent
//! less the parts' ink extents, so it does not depend on the gaps at all. A
//! source that only had to satisfy the total could never fix a failing line by
//! moving anything — the per-part bound is what makes the check something an
//! author can act on, and the total is what catches parts that are simply too
//! fat for the box together, however they are shuffled.
//!
//! Everything here reads the parts' *own* pixels. A part that draws nothing
//! yet, or that is a composite with no pixels of its own, has no frontier, and
//! a line with one of those in it is not measured rather than measured wrong.
//!
//! # The variant name rule (D1)
//!
//! Everything after a name's first `:` is split on `-`; the first `WxH` token
//! is the variant's **size** and the first `l`/`r`/`u`/`d`/`c` token is its
//! **position** (left, right, up, down, centre — `c` is the centre of either
//! axis). Neither is required, and a name carrying neither is not an error.
//!
//! What they buy, when they are there:
//!
//! - a declared size must equal the glyph's actual size, checked where the name
//!   is *used* as a component — a name is a claim about the glyph, so an
//!   unused `:4x16` that lies is nothing until someone believes it;
//! - a declared position is matched against the slot the component sits in, and
//!   a mismatch is a warning rather than an error: 阝 really is two different
//!   characters left and right, but a part drawn for the right that happens to
//!   fit on the left is a design decision, not a broken source.
//!
//! The position is also the tie-break when several variants of a part have the
//! same size ([`direction_rank`]): the slot's own direction first, an unmarked
//! name second, the wrong direction last. Nothing selects variants
//! automatically yet — an IDC component names the variant it wants outright —
//! but the ranking is the rule that the editor's variant picker and the
//! most-common-choice default will both use, so it lives here with the parse.

use crate::document::{ComposeItem, GlyphCompose, GlyphRef, PixelGrid};
use crate::issues::Severity;

/// The four one-dimensional IDCs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdcOp {
    /// ⿰ U+2FF0 left to right.
    LeftRight,
    /// ⿱ U+2FF1 above to below.
    AboveBelow,
    /// ⿲ U+2FF2 left to middle and right.
    LeftMiddleRight,
    /// ⿳ U+2FF3 above to middle and below.
    AboveMiddleBelow,
}

impl IdcOp {
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '\u{2FF0}' => Some(Self::LeftRight),
            '\u{2FF1}' => Some(Self::AboveBelow),
            '\u{2FF2}' => Some(Self::LeftMiddleRight),
            '\u{2FF3}' => Some(Self::AboveMiddleBelow),
            _ => None,
        }
    }

    /// The operator a whole token spells, or `None` for a token that is not one
    /// IDC character.
    pub fn from_token(token: &str) -> Option<Self> {
        let mut chars = token.chars();
        let op = Self::from_char(chars.next()?)?;
        chars.next().is_none().then_some(op)
    }

    pub fn as_char(self) -> char {
        match self {
            Self::LeftRight => '\u{2FF0}',
            Self::AboveBelow => '\u{2FF1}',
            Self::LeftMiddleRight => '\u{2FF2}',
            Self::AboveMiddleBelow => '\u{2FF3}',
        }
    }

    /// Whether the split runs along the x axis.
    pub fn horizontal(self) -> bool {
        matches!(self, Self::LeftRight | Self::LeftMiddleRight)
    }

    /// How many components the operator takes.
    pub fn arity(self) -> usize {
        match self {
            Self::LeftRight | Self::AboveBelow => 2,
            Self::LeftMiddleRight | Self::AboveMiddleBelow => 3,
        }
    }

    /// Which position a component sits in, for the name check and the
    /// tie-break. Slots past the arity have no direction.
    pub fn slot_direction(self, slot: usize) -> Option<Direction> {
        let arity = self.arity();
        if slot >= arity {
            return None;
        }
        let last = slot + 1 == arity;
        Some(match (self.horizontal(), slot == 0, last) {
            (true, true, _) => Direction::Left,
            (true, _, true) => Direction::Right,
            (false, true, _) => Direction::Up,
            (false, _, true) => Direction::Down,
            _ => Direction::Center,
        })
    }
}

/// The position a variant name claims: `l`, `r`, `u`, `d`, `c`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
    /// The middle, of either axis — one letter covers both, since a part is
    /// never centred horizontally and vertically at once in a 1-D split.
    Center,
}

impl Direction {
    fn from_token(token: &str) -> Option<Self> {
        match token {
            "l" => Some(Self::Left),
            "r" => Some(Self::Right),
            "u" => Some(Self::Up),
            "d" => Some(Self::Down),
            "c" => Some(Self::Center),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "l",
            Self::Right => "r",
            Self::Up => "u",
            Self::Down => "d",
            Self::Center => "c",
        }
    }
}

/// What a glyph name's `:` suffix claims about the glyph. See the module docs
/// for the rule; a name with no suffix, or a suffix saying neither thing,
/// simply claims nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VariantSpec {
    pub size: Option<(u16, u16)>,
    pub direction: Option<Direction>,
}

impl VariantSpec {
    pub fn parse(name: &str) -> Self {
        let mut spec = VariantSpec::default();
        let Some((_, suffix)) = name.split_once(':') else {
            return spec;
        };
        for word in suffix.split('-') {
            if spec.size.is_none()
                && let Some(size) = parse_size(word)
            {
                spec.size = Some(size);
                continue;
            }
            if spec.direction.is_none() {
                spec.direction = Direction::from_token(word);
            }
        }
        spec
    }
}

/// `4x16` → `(4, 16)`.
fn parse_size(word: &str) -> Option<(u16, u16)> {
    let (w, h) = word.split_once('x')?;
    // `04x16` would be a second spelling of one size, and two names for one
    // thing is how an inventory drifts, so a leading zero is not a size.
    let num = |s: &str| -> Option<u16> {
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        if s.len() > 1 && s.starts_with('0') {
            return None;
        }
        s.parse().ok()
    };
    Some((num(w)?, num(h)?))
}

/// How well a candidate name suits a slot: 0 is the slot's own direction, 1 an
/// unmarked name, 2 the wrong direction. Lower wins; `sort_by_key` on this is
/// stable, so equally-ranked candidates keep the caller's order.
// Nothing picks a variant automatically yet — an IDC component names the one it
// wants — so this has only its tests as callers. `expect` and not `allow`: the
// day the editor's picker uses it, the attribute has to come off.
#[cfg_attr(not(test), expect(dead_code))]
pub fn direction_rank(name: &str, slot: Option<Direction>) -> u8 {
    match (VariantSpec::parse(name).direction, slot) {
        (None, _) | (_, None) => 1,
        (Some(d), Some(s)) if d == s => 0,
        _ => 2,
    }
}

/// What a component's `glyph` header says its box is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PartDims {
    /// Nothing defines the name.
    Unknown,
    /// A glyph, but its header declares no `W H` — a pure composite, say. Its
    /// box is whatever it happens to resolve to, which is exactly the thing a
    /// component may not be.
    Undeclared,
    /// `(width, height)`, in the parent's own units.
    Size(u16, u16),
}

/// Where a glyph's ink starts and stops on every line of the grid, in the
/// glyph's *declared* units — the same units the IDC layout is in, so a part
/// drawn at `scale 2` measures the same as one drawn at `scale 1`.
///
/// Both axes are kept because one part can be a component of a ⿰ line and a ⿱
/// line both, and the profile is built once per part per expansion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InkProfile {
    /// Per row: the leftmost and rightmost non-empty column, or `None` for a
    /// row that draws nothing.
    pub rows: Vec<Option<(u16, u16)>>,
    /// Per column: the topmost and bottommost non-empty row.
    pub cols: Vec<Option<(u16, u16)>>,
}

impl InkProfile {
    /// Read a grid's frontiers. A cell counts as occupied when the source put
    /// *something* there, which includes a hardblank — see
    /// [`PixelShape::is_empty`](crate::pixel::PixelShape::is_empty).
    pub fn of(grid: &PixelGrid, scale: u8) -> Self {
        let s = scale.max(1) as u16;
        // Floor, exactly as `declared_box` does: a grid that is not a whole
        // number of declared cells has no last cell to speak of.
        let (w, h) = (grid.width / s, grid.height / s);
        let mut profile = Self {
            rows: vec![None; h as usize],
            cols: vec![None; w as usize],
        };
        let extend = |slot: &mut Option<(u16, u16)>, at: u16| {
            *slot = Some(match *slot {
                None => (at, at),
                Some((lo, hi)) => (lo.min(at), hi.max(at)),
            });
        };
        for row in 0..h * s {
            for col in 0..w * s {
                if grid.get(row, col).is_empty() {
                    continue;
                }
                extend(&mut profile.rows[(row / s) as usize], col / s);
                extend(&mut profile.cols[(col / s) as usize], row / s);
            }
        }
        profile
    }

    /// The lines that face along the split axis: rows for a horizontal split
    /// (each gives the leftmost and rightmost column), columns for a vertical
    /// one. Indexed by position across the axis, so two parts of one line index
    /// alike.
    fn along(&self, horizontal: bool) -> &[Option<(u16, u16)>] {
        if horizontal { &self.rows } else { &self.cols }
    }
}

/// One `audit ideal-clearance` rule, as [`expand_compose`] applies it: the range
/// and how to reach a component's [`InkProfile`].
///
/// The lookup is a callback rather than a map because the caller decides what a
/// component name means — the expansion pass has the expanded glyph items, and
/// no consumer of this module should have to build a second index to be asked.
pub struct ClearanceRule<'a> {
    /// The prefix the rule was written with, for the message.
    pub written: &'a str,
    pub min: i16,
    pub max: i16,
    pub ink: &'a dyn Fn(&str) -> Option<&'a InkProfile>,
}

/// Whether an IDC component has yet to pick its variant — the `:` is the whole
/// of the test, since that is what introduces a variant suffix at all.
///
/// This is what separates "not done" from "wrong" for an IDC line, and more
/// than one stage has to agree on it: [`expand_compose`] to report a
/// [`Severity::Todo`] and stand the clearance check down, and the expansion
/// pass to leave the ref it derives unresolved *without* also calling it an
/// unresolved ref. Two copies of `!name.contains(':')` would be two chances to drift.
pub fn is_undecided(component_name: &str) -> bool {
    !component_name.contains(':')
}

/// Turn one IDC line into the `ref`s it stands for, plus what is wrong with it.
///
/// Best effort: a component whose size is unknown is placed where the walk has
/// got to and advances it by nothing, so the parts that *are* known still land
/// where they belong and the editor can draw the glyph while it is being filled
/// in. The diagnostics are what stop a wrong glyph from passing
/// for a right one.
///
/// `parent` is the enclosing `glyph` header's box, `dims` answers for a
/// component name. Messages come back with a severity and no location — the
/// caller owns the [`crate::resolve::ItemRef`].
///
/// Every length here is in *declared* units, the ones the `glyph` header
/// writes: the layout is the same at any `scale`, and a component drawn at a
/// different scale than its parent still fills the same box. Only the derived
/// offsets leave in the parent's raster units, multiplied by `scale` on the way
/// out, because that is what a `ref` offset means.
pub fn expand_compose(
    glyph_name: &str,
    parent: Option<(u16, u16)>,
    scale: u8,
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
    clearance: Option<&ClearanceRule>,
) -> (Vec<GlyphRef>, Vec<(Severity, String)>) {
    let op = compose.op;
    let mut issues: Vec<(Severity, String)> = Vec::new();
    let mut refs: Vec<GlyphRef> = Vec::new();
    let at = |msg: String| format!("glyph '{glyph_name}': `{}` {msg}", op.as_char());

    let parts = compose.part_names().count();
    if parts != op.arity() {
        issues.push((
            Severity::Error,
            at(format!("takes {} components, not {parts}", op.arity())),
        ));
    }

    let Some((parent_w, parent_h)) = parent else {
        issues.push((
            Severity::Error,
            at(
                "needs the enclosing `glyph` header to declare its `W H`: the parts are \
                placed by filling that box"
                    .to_string(),
            ),
        ));
        return (refs, issues);
    };
    let (axis_extent, cross_extent) = if op.horizontal() {
        (parent_w, parent_h)
    } else {
        (parent_h, parent_w)
    };

    // A component that has not picked its variant yet leaves the line
    // *unresolved* rather than wrong: the width the slot was going to be
    // filled with is simply not chosen. Everything a measurement of such a
    // line would say is a consequence of that one gap, so the clearance check
    // and the unpicked component's own checks stand down and the line reports
    // one TODO per unpicked component instead. See [`Severity::Todo`].
    let unresolved = compose.part_names().any(is_undecided);

    let mut cursor: i32 = 0;
    let mut slot = 0usize;
    // Where each component landed along the axis, for the clearance check
    // below. Collected on the way through because that walk is what knows it.
    let mut placed_parts: Vec<(&str, i32)> = Vec::new();
    for item in &compose.items {
        let (name, raw_name) = match item {
            ComposeItem::Gap(gap) => {
                cursor += *gap as i32;
                continue;
            }
            ComposeItem::Part { name, raw_name } => (name, raw_name),
        };
        let spec = VariantSpec::parse(name);
        let unpicked = is_undecided(name);
        if unpicked {
            issues.push((
                Severity::Todo,
                at(format!(
                    "component '{name}' has no variant picked yet; a component names the sized \
                     variant it wants, as in `{name}:{}`",
                    if op.horizontal() {
                        format!("{axis_extent}x{cross_extent}")
                    } else {
                        format!("{cross_extent}x{axis_extent}")
                    }
                )),
            ));
        }
        if let Some(slot_dir) = op.slot_direction(slot)
            && let Some(dir) = spec.direction
            && dir != slot_dir
        {
            issues.push((
                Severity::Warning,
                at(format!(
                    "component '{name}' is drawn for `-{}` but sits in the `-{}` slot",
                    dir.as_str(),
                    slot_dir.as_str(),
                )),
            ));
        }
        placed_parts.push((name.as_str(), cursor));
        let placed = cursor * scale.max(1) as i32;
        let (col, row) = if op.horizontal() {
            (placed, 0)
        } else {
            (0, placed)
        };
        refs.push(GlyphRef {
            name: name.clone(),
            raw_name: raw_name.clone(),
            offset: Some((clamp_offset(col), clamp_offset(row))),
            negated: false,
            inherit: false,
            if_exists: false,
            fill: None,
            visibility: None,
            comment: None,
        });
        slot += 1;

        // An unpicked component is still *placed* — the cursor walks over
        // whatever box the bare name happens to have, so the parts that are
        // decided still land where they belong and the editor can draw the
        // glyph as it is filled in — but it says nothing yet, so nothing it
        // says can be wrong.
        let part_dims = dims(name);
        if unpicked {
            if let PartDims::Size(w, h) = part_dims {
                cursor += if op.horizontal() { w } else { h } as i32;
            }
            continue;
        }
        match part_dims {
            PartDims::Unknown => issues.push((
                Severity::Error,
                at(format!("component '{name}' is not defined")),
            )),
            PartDims::Undeclared => issues.push((
                Severity::Error,
                at(format!(
                    "component '{name}' declares no `W H` on its `glyph` header, so it has \
                     no box to fill a slot with"
                )),
            )),
            PartDims::Size(w, h) => {
                if let Some(size) = spec.size
                    && size != (w, h)
                {
                    issues.push((
                        Severity::Error,
                        at(format!(
                            "component '{name}' names {}x{} but the glyph is {w}x{h}",
                            size.0, size.1
                        )),
                    ));
                }
                let (along, across) = if op.horizontal() { (w, h) } else { (h, w) };
                if across != cross_extent {
                    issues.push((
                        Severity::Error,
                        at(format!(
                            "component '{name}' is {} {across}, not the glyph's {cross_extent}",
                            if op.horizontal() { "tall" } else { "wide" },
                        )),
                    ));
                }
                cursor += along as i32;
            }
        }
    }

    // Only over a line that is otherwise sound: on a line whose parts are not
    // chosen, or that names something no glyph answers to, every clearance is
    // measured against a layout nobody meant, and the warnings would be noise
    // on top of whatever matters.
    if let Some(rule) = clearance
        && !unresolved
        && !issues.iter().any(|(s, _)| *s == Severity::Error)
    {
        issues.extend(
            check_clearances(op, axis_extent, &placed_parts, rule)
                .into_iter()
                .map(|(severity, message)| (severity, at(message))),
        );
    }
    (refs, issues)
}

/// Measure an IDC line's clearances and report the ones outside `rule`'s range,
/// plus their sum. See the module docs for what a clearance is.
///
/// `placed` is each component and where it starts along the axis, in declared
/// units. Nothing is reported when any component has no ink to measure — see
/// the module docs — and that includes the case where two neighbours share no
/// line on which both draw something.
fn check_clearances(
    op: IdcOp,
    axis_extent: u16,
    placed: &[(&str, i32)],
    rule: &ClearanceRule,
) -> Vec<(Severity, String)> {
    /// A component of the line, ready to measure: where it sits along the axis
    /// and what its frontiers are on each line across it.
    struct Placed<'a> {
        name: &'a str,
        offset: i32,
        lines: &'a [Option<(u16, u16)>],
    }

    let horizontal = op.horizontal();
    let mut parts: Vec<Placed> = Vec::new();
    for &(name, offset) in placed {
        let Some(profile) = (rule.ink)(name) else {
            return Vec::new();
        };
        parts.push(Placed {
            name,
            offset,
            lines: profile.along(horizontal),
        });
    }
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else {
        return Vec::new();
    };

    // `(what it is between, how much)`, near edge to far edge.
    let (near_edge, far_edge) = match horizontal {
        true => ("the left edge", "the right edge"),
        false => ("the top edge", "the bottom edge"),
    };
    let mut clearances: Vec<(String, i32)> = Vec::new();
    let Some(near) = first
        .lines
        .iter()
        .filter_map(|line| line.map(|(near, _)| first.offset + near as i32))
        .min()
    else {
        return Vec::new();
    };
    clearances.push((format!("{near_edge} and '{}'", first.name), near));
    for pair in parts.windows(2) {
        let [a, b] = pair else { continue };
        // Only lines on which both parts draw: a line where one of them is
        // blank has no pair of frontiers to measure between.
        let Some(gap) = a
            .lines
            .iter()
            .zip(b.lines.iter())
            .filter_map(|(x, y)| match (x, y) {
                (Some((_, a_far)), Some((b_near, _))) => {
                    Some((b.offset + *b_near as i32) - (a.offset + *a_far as i32) - 1)
                }
                _ => None,
            })
            .min()
        else {
            return Vec::new();
        };
        clearances.push((format!("'{}' and '{}'", a.name, b.name), gap));
    }
    let Some(far) = last
        .lines
        .iter()
        .filter_map(|line| line.map(|(_, far)| axis_extent as i32 - 1 - (last.offset + far as i32)))
        .min()
    else {
        return Vec::new();
    };
    clearances.push((format!("'{}' and {far_edge}", last.name), far));

    let (min, max) = (rule.min as i32, rule.max as i32);
    let range = format!(
        "the ideal {}..{} (`audit ideal-clearance {}`)",
        rule.min, rule.max, rule.written,
    );
    let mut out: Vec<(Severity, String)> = clearances
        .iter()
        .filter(|(_, c)| !(min..=max).contains(c))
        .map(|(between, c)| {
            (
                Severity::Warning,
                format!("leaves {c} between {between}, outside {range}"),
            )
        })
        .collect();
    let total: i32 = clearances.iter().map(|(_, c)| c).sum();
    if !(min..=max).contains(&total) {
        let breakdown = clearances
            .iter()
            .map(|(between, c)| format!("{c} between {between}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push((
            Severity::Warning,
            format!("leaves {total} in total, outside {range} — {breakdown}"),
        ));
    }
    out
}

/// A derived offset is an `i16` like any other; a source absurd enough to
/// overflow one gets a saturated offset, and a glyph as visibly wrong as the
/// line that asked for it.
fn clamp_offset(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;
