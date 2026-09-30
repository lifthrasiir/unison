//! The `:` suffix of a component name (`VariantSpec`, the size tokens), how well a name suits a slot, and a part's box (`PartDims`).

use super::op::Direction;
use crate::document::Margin;

/// What a glyph name's `:` suffix claims about the glyph. See the module docs
/// for the rule; a name with no suffix, or a suffix saying neither thing,
/// simply claims nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VariantSpec {
    pub size: Option<(u16, u16)>,
    /// The **cavity** a `WxH.NxM` size token claims: an `NxM` rectangle the
    /// drawing leaves clear, flush against whichever sides the enclosure it is
    /// written for leaves open. Only an enclosure's outer part states one, and
    /// stating one is what marks a drawing as an outer part at all
    /// ([`enclosure_rank`]).
    ///
    /// It is a *lower bound* and not an equality, where [`Self::size`] is an
    /// equality: the size is the box the glyph declares and there is one right
    /// answer, while a cavity is room the drawing happens to leave and what
    /// matters is that there is at least as much of it as the name promised.
    /// See [`cavity_fits`](super::enclosure::cavity_fits).
    pub inner: Option<(u16, u16)>,
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
                && let Some((size, inner)) = parse_size_token(word)
            {
                spec.size = Some(size);
                spec.inner = inner;
                continue;
            }
            if spec.direction.is_none() {
                spec.direction = Direction::from_token(word);
            }
        }
        spec
    }
}

/// A size token's two halves: the box, and the cavity it promises.
type SizeToken = ((u16, u16), Option<(u16, u16)>);

/// A size token: `4x16` → the box alone, `15x16.9x10` → the box and the cavity
/// it promises. A `.` with nothing usable on either side is not a size token at
/// all, so a name carrying one claims nothing rather than half a thing.
fn parse_size_token(word: &str) -> Option<SizeToken> {
    match word.split_once('.') {
        Some((outer, inner)) => Some((parse_size(outer)?, Some(parse_size(inner)?))),
        None => Some((parse_size(word)?, None)),
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
// The editor's completion popup orders a variant listing by this
// (`editor/autocomplete.rs`), and `fix::clearance` refuses a candidate it ranks
// last — a drawing made for the other side of the glyph.
pub fn direction_rank(name: &str, slot: Option<Direction>) -> u8 {
    match (VariantSpec::parse(name).direction, slot) {
        (None, _) | (_, None) => 1,
        (Some(d), Some(s)) if d == s => 0,
        _ => 2,
    }
}

/// The same ranking for an enclosure's two slots, where the claim a name makes
/// is the cavity in it rather than a direction letter.
///
/// A drawing that promises a cavity was made to hold something; one that does
/// not was made to be held. So the outer slot ranks a cavity-bearing name 0 and
/// everything else 2, and the inner slot the other way round — the same three
/// values [`direction_rank`] uses, and read by the same consumers, so that a
/// last-ranked name is refused for a slot on both kinds of line alike.
///
/// A name that has picked no variant at all ranks 1 on either slot: it claims
/// nothing, which is not the same as claiming the wrong thing.
pub fn enclosure_rank(name: &str, outer_slot: bool) -> u8 {
    if is_undecided(name) {
        return 1;
    }
    match (VariantSpec::parse(name).inner.is_some(), outer_slot) {
        (true, true) | (false, false) => 0,
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
    /// `(width, height)`, in the parent's own units, and the margin the part
    /// asks for around that box when its slot is larger across the split.
    Size(u16, u16, Margin),
}

impl PartDims {
    /// What a glyph body says about itself as a part: its declared box and its
    /// margin.
    pub fn of(body: &crate::document::GlyphBody) -> Self {
        Self::declared(body.declared_extent(), body.margin)
    }

    /// The same from the two numbers alone, for a caller that keeps them
    /// rather than the body.
    pub fn declared(size: Option<(u16, u16)>, margin: Margin) -> Self {
        match size {
            Some((w, h)) => Self::Size(w, h, margin),
            None => Self::Undeclared,
        }
    }
}

/// Where a part of `size` starts across a slot `cross_extent` across, in a line
/// split along `horizontal`: 0 for a part as wide as the slot, the near side of
/// its [margin](crate::compose#a-parts-margin) for one that its margin makes up
/// to the slot, and `None` for one that does not fill the slot either way.
pub fn padding_across(
    size: (u16, u16),
    margin: Margin,
    cross_extent: u16,
    horizontal: bool,
) -> Option<u16> {
    let across = if horizontal { size.1 } else { size.0 };
    if across == cross_extent {
        return Some(0);
    }
    let (lo, hi) = margin.across(horizontal)?;
    (across as u32 + lo as u32 + hi as u32 == cross_extent as u32).then_some(lo)
}

/// Whether an IDC component has yet to pick its variant — the `:` is the whole
/// of the test, since that is what introduces a variant suffix at all.
///
/// This is what separates "not done" from "wrong" for an IDC line, and more
/// than one stage has to agree on it: [`expand_compose`](super::expand_compose) to report a
/// [`Severity::Todo`](crate::issues::Severity::Todo) and stand the clearance check down, and the expansion
/// pass to leave the ref it derives unresolved *without* also calling it an
/// unresolved ref. Two copies of `!name.contains(':')` would be two chances to drift.
pub fn is_undecided(component_name: &str) -> bool {
    !component_name.contains(':')
}
