//! What an expansion returns, the lookups it is given, and the tests of whether a part's size fits a slot.

use super::ink::InkProfile;
use super::variant::padding_across;
use crate::document::{GlyphRef, Margin};
use crate::issues::Severity;

/// What expanding one IDC line comes to: the `ref`s it stands for, and what is
/// wrong with it, each message with a severity and no location — the caller
/// owns the [`crate::resolve::ItemRef`].
pub type ComposeExpansion = (Vec<GlyphRef>, Vec<(Severity, String)>);

/// How an *undecided* component's family is answered: the sizes every variant
/// of its base name draws, which is what separates a line waiting for a
/// decision from one whose decision cannot be made. A callback for the same
/// reason [`InkLookup`] is one — the caller decides what a name means.
pub type FamilyLookup<'a> = dyn Fn(&str) -> Vec<FamilyVariant> + 'a;

/// One variant of a family, as [`FamilyLookup`] answers it: the box it
/// declares and the margin it asks for around it.
pub type FamilyVariant = ((u16, u16), Margin);

/// How a component name is answered with the ink it draws. See
/// [`ClearanceRule::ink`] for why this is a callback.
pub type InkLookup<'a> = dyn Fn(&str) -> Option<&'a InkProfile> + 'a;

/// One `audit ideal-clearance` rule, as [`expand_compose`](super::expand_compose) applies it: the range
/// and how to reach a component's [`InkProfile`].
///
/// The lookup is a callback rather than a map because the caller decides what a
/// component name means — the expansion pass has the expanded glyph items, and
/// no consumer of this module should have to build a second index to be asked.
pub struct ClearanceRule<'a> {
    /// The prefix the rule was written with, for the message.
    pub written: &'a str,
    /// The two bands the rule states; which one applies is the operator's to
    /// say ([`crate::audit::ClearanceBand::range`]).
    pub band: &'a crate::audit::ClearanceBand,
    pub ink: &'a InkLookup<'a>,
    /// The longest contact run `audit max-contact-run` tolerates, or `None`
    /// when the source states no such rule and contact is not measured.
    pub max_contact_run: Option<u16>,
    /// The prefix *that* rule was written with, for its own message.
    pub contact_written: &'a str,
}

/// Whether a part `along` long may share an axis `axis_extent` long with the
/// rest of an IDC line.
///
/// A part as long as the glyph it is a part of fills the glyph on its own, so
/// whatever else the line names has nowhere to stand: the layout does not
/// exist, however well a score happens to take it. (And a score does take it:
/// a total that has gone negative is as far outside the ideal range as one that
/// is too large, so an oversized variant reads as an improvement over parts
/// that are merely too thin.) The bound is the glyph's *declared* axis, the
/// same rectangle the clearances are measured over.
///
/// Two stages ask it and they have to agree: [`crate::fix::clearance`] over
/// what it may *propose* for a slot, and [`expand_compose`](super::expand_compose) over what an
/// undecided component's family could ever put there.
pub fn fits_axis(along: i32, axis_extent: i32) -> bool {
    along < axis_extent
}

/// Whether a glyph this size could fill one slot of a line split along
/// `axis_extent`, in a glyph `cross_extent` across: [`fits_axis`] along the
/// split, and the exact box the line demands across it — or the box its
/// margin makes up to it ([`padding_across`]).
pub fn fits_slot(
    size: (u16, u16),
    margin: Margin,
    axis_extent: u16,
    cross_extent: u16,
    horizontal: bool,
) -> bool {
    let along = if horizontal { size.0 } else { size.1 };
    padding_across(size, margin, cross_extent, horizontal).is_some()
        && fits_axis(along as i32, axis_extent as i32)
}

/// Whether a glyph this size could fill one slot of an *enclosure*.
///
/// The two slots are held to opposite rules, and neither is [`fits_axis`]:
///
/// - the **outer** part must be the glyph exactly. It is the thing whose walls
///   are the glyph's walls, so a drawing even one cell short offers its cavity
///   against a box that is not the one the line lays out in — which is the same
///   objection [`fits_axis`] makes to an oversized part on a split, arrived at
///   from the other side;
/// - the **inner** part must fit inside the glyph. Nothing pins either of its
///   dimensions the way a split's cross axis pins one: what room there really
///   is, is the cavity's to say, and that is a measurement rather than a
///   number a box carries ([`cavity_fits`](super::enclosure::cavity_fits)).
pub fn fits_enclosure_slot(
    size: (u16, u16),
    margin: Margin,
    parent: (u16, u16),
    outer: bool,
) -> bool {
    match outer {
        true => outer_padding(size, margin, parent).is_some(),
        false => size.0 <= parent.0 && size.1 <= parent.1,
    }
}

/// Where an enclosure's outer part of `size` sits in the glyph: `(0, 0)` for
/// one the glyph's size, and on each axis its box falls short of, the near
/// side of the [margin](crate::compose#a-parts-margin) that makes it up —
/// `None` where nothing does. The inner part is placed by the line's own
/// offsets and takes none.
pub fn outer_padding(size: (u16, u16), margin: Margin, parent: (u16, u16)) -> Option<(u16, u16)> {
    let axis = |size: u16, parent: u16, margin: Option<(u16, u16)>| {
        if size == parent {
            return Some(0);
        }
        let (lo, hi) = margin?;
        (size as u32 + lo as u32 + hi as u32 == parent as u32).then_some(lo)
    };
    Some((
        axis(size.0, parent.0, margin.x)?,
        axis(size.1, parent.1, margin.y)?,
    ))
}

/// What an undecided component's family draws when *none* of it fits the slot,
/// as a list for the message — or `None` when the caller offered no family, the
/// family is empty, or something in it fits.
///
/// An empty family stays a TODO on purpose: a component nothing has been drawn
/// for yet is the ordinary state of a source populated from IDS, and the work
/// it names is "draw it", which is what a TODO already says. The warning is for
/// the case where the drawings exist and none of them can go there.
pub(super) fn misfit_variants(
    name: &str,
    family: Option<&FamilyLookup>,
    fits: &dyn Fn((u16, u16), Margin) -> bool,
) -> Option<String> {
    let variants = family?(name);
    if variants.is_empty() || variants.iter().any(|&(size, margin)| fits(size, margin)) {
        return None;
    }
    // By the numbers rather than by the text, so `5x16` comes before `15x16`.
    let mut sizes: Vec<String> = variants
        .iter()
        .map(|(size, _)| size)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|(w, h)| format!("{w}x{h}"))
        .collect();
    // Enough to see what the family is; a long tail of them says no more than
    // the first few do.
    let rest = sizes.len().saturating_sub(MAX_LISTED_VARIANTS);
    sizes.truncate(MAX_LISTED_VARIANTS);
    let listed = sizes.join(", ");
    Some(match rest {
        0 => listed,
        n => format!("{listed} and {n} more"),
    })
}

/// How many of an undecided component's sizes a message lists before counting
/// the rest.
pub(super) const MAX_LISTED_VARIANTS: usize = 4;
