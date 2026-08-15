//! IDC composition: `⿰`/`⿱`/`⿲`/`⿳` inside a glyph block, and the variant
//! name rule it reads.
//!
//! # Why the line is a first-class item
//!
//! A CJK glyph built from parts is a *box split along one axis*: the parts fill
//! the parent's box exactly, and the constraint that they do is the whole
//! design. Written as ordinary `ref`s with hand-written offsets there is no
//! place for that constraint to live — two parts that do not add up look
//! exactly like two parts that do, and at 20k glyphs "quietly off by one" is
//! undetectable. So the offsets are *derived* from the parts' declared sizes
//! and the sum is checked: a violation is unrepresentable rather than silent.
//!
//! Only the four one-dimensional operators exist here. ⿰ and ⿱ cover 91% of
//! the URO and the four cover 99% of what decomposes one-dimensionally; ⿴⿵⿸⿺
//! and friends do not close as a sum along one axis, so they stay ordinary
//! `ref` + offset. This is not a general IDS layout engine and is not meant to
//! become one.
//!
//! ```text
//! glyph han-6cb3 15 16
//! ⿰ han-6c35:4x16 han-53ef:11x16
//! ⿰ han-6c35:4x16 -1 han-53ef:12x16    // an overlap: 4 + (-1) + 12 == 15
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
//! component is placed at the cursor and advances it by its own extent. So the
//! sum of every component extent and every gap must equal the parent's extent
//! along that axis, and each component's extent *across* the axis must equal
//! the parent's — a ⿰ part is as tall as the glyph. Both are errors.
//!
//! # An undecided line is not a wrong one
//!
//! A component written without a `:` suffix has not picked its variant yet.
//! That is the initial state of every Han glyph populated from IDS, not a
//! mistake, so it is a [`Severity::Todo`] and not an error: one per unpicked
//! component, and the sum rule — which is about widths that have not been
//! chosen — stands down for the whole line, as do the unpicked component's own
//! size and cross-axis checks. What is left is the line's *decided* half, still
//! fully checked. The glyph is no more built than an erroring one is; the
//! difference is that a build, a `uniform test` run and CI do not fail over it.
//!
//! Sizes are read from the components' `glyph` headers, never from the
//! composed result: a part's width is a property the part *declares*, which is
//! what makes the layout a lookup rather than a search (see `PLANS.han.md`).
//! A component that names no glyph, or one whose header declares no `W H`, is
//! an error for the same reason.
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

use crate::document::{ComposeItem, GlyphCompose, GlyphRef};
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

/// Turn one IDC line into the `ref`s it stands for, plus what is wrong with it.
///
/// Best effort: a component whose size is unknown is placed where the walk has
/// got to and contributes nothing to the sum, so the parts that *are* known
/// still land where they belong and the editor can draw the glyph while it is
/// being filled in. The diagnostics are what stop a wrong glyph from passing
/// for a right one.
///
/// `parent` is the enclosing `glyph` header's box, `dims` answers for a
/// component name. Messages come back with a severity and no location — the
/// caller owns the [`crate::resolve::ItemRef`].
///
/// Every length here is in *declared* units, the ones the `glyph` header
/// writes: the sum rule is the same at any `scale`, and a component drawn at a
/// different scale than its parent still fills the same box. Only the derived
/// offsets leave in the parent's raster units, multiplied by `scale` on the way
/// out, because that is what a `ref` offset means.
pub fn expand_compose(
    glyph_name: &str,
    parent: Option<(u16, u16)>,
    scale: u8,
    compose: &GlyphCompose,
    dims: &dyn Fn(&str) -> PartDims,
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
    // filled with is simply not chosen. Everything the sum rule would say
    // about such a line is a consequence of that one gap, so the sum check
    // and the unpicked component's own checks stand down and the line reports
    // one TODO per unpicked component instead. See [`Severity::Todo`].
    let unresolved = compose.part_names().any(|name| !name.contains(':'));

    let mut cursor: i32 = 0;
    let mut slot = 0usize;
    for item in &compose.items {
        let (name, raw_name) = match item {
            ComposeItem::Gap(gap) => {
                cursor += *gap as i32;
                continue;
            }
            ComposeItem::Part { name, raw_name } => (name, raw_name),
        };
        let spec = VariantSpec::parse(name);
        let unpicked = !name.contains(':');
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

    if !unresolved && cursor != axis_extent as i32 {
        issues.push((
            Severity::Error,
            at(format!(
                "components and gaps add up to {cursor} across the glyph's {axis_extent}",
            )),
        ));
    }
    (refs, issues)
}

/// A derived offset is an `i16` like any other; a source absurd enough to
/// overflow one gets a saturated offset and the sum error that comes with it.
fn clamp_offset(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

#[cfg(test)]
#[path = "compose_tests.rs"]
mod tests;
