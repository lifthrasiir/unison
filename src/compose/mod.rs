//! IDC composition: the `⿰⿱⿲⿳` splits and the `⿴⿵⿶⿷⿸⿹⿺⿼⿽` enclosures inside a
//! glyph block, and the variant name rule they read. The syntax and what an
//! author is held to are in `doc/reference.md` (*IDC composition*); this is
//! why it is built so.
//!
//! # Why the line is a first-class item
//!
//! A CJK glyph built from parts is a *box split along one axis*: each part
//! takes a share of the parent's box, and how much room the shares leave one
//! another is the whole design. Written as ordinary `ref`s with hand-written
//! offsets there is no place for that to live — two parts that crowd each other
//! look exactly like two that do not, and at 20k glyphs "quietly off by one" is
//! undetectable. So the offsets are *derived* from the parts' declared sizes,
//! and what the parts leave each other is measured against a declared range.
//!
//! Sizes are read from the components' `glyph` headers, never from the composed
//! result: a part's width is a property the part *declares*, which is what makes
//! the layout a lookup rather than a search. The expansion of a pattern line
//! happens before this module runs (`ttf_builder::expand::expand_compose_lines`),
//! because each expansion's parts declare their own boxes and so land at their
//! own offsets; what reaches here is always one concrete glyph's line.
//!
//! # Two kinds of line, one measurement
//!
//! The four **splits** hand each part a share of one axis. The nine
//! **enclosures** ([`Walls`]) hand one part the whole box and seat the other in
//! the cavity it leaves. They are laid out by different code — a split walks a
//! cursor along its axis, an enclosure places one part at written offsets — but
//! everything after that is shared: the same [`InkProfile`], the same
//! [`facing_offset`](gap::facing_offset), the same `audit ideal-clearance` band, the same
//! [`Clearance`] list.
//!
//! What makes that possible is [`GapSide`]. Every gap either layout measures is
//! between two *boundaries*, and the only things that vary are which of a
//! line's four boundaries faces the gap and how far along the cross axis the
//! two parts sit. A split passes [`GapSide::linear`] and reads exactly the
//! numbers it always did; an enclosure reads the inner face of a wall, against
//! an inner part sitting somewhere inside the box.
//!
//! An enclosure's numbers are **offsets**, not gaps, because an enclosure has
//! two gaps on each axis and fixing all four still leaves the layout ambiguous
//! wherever a wall's inner face is ragged. Both are written or neither: a line
//! with neither has decided nothing and is a [`Severity::Todo`](crate::issues::Severity::Todo), exactly as an
//! unpicked variant is, and is *not* read as `0 0` ([`expand_enclosure`](enclosure::expand_enclosure)).
//!
//! `⿻`, `⿾` and `⿿` are deliberately absent: the first says two drawings
//! share a box and nothing about where; the other two transform one drawing
//! rather than composing two. This is not a general IDS layout engine.
//!
//! # Nested splits
//!
//! A slot of a split may be written `1|foo|1|bar|1`: a *nested split*, which is the
//! glyph `⿱ 1 foo 1 bar 1` would be, standing in the slot without a name of
//! its own. It is the perpendicular split — ⿱ or ⿳ inside ⿰ and ⿲, ⿰ or ⿲
//! inside ⿱ and ⿳ — by how many parts it writes ([`nested_op`](nested::nested_op)), and it is
//! laid out, checked and measured as that glyph would be ([`nested_line`](nested::nested_line),
//! [`nested_body`]),
//! with one difference: a glyph *declares* its box, and a nested split's is
//! **inferred** from its members — along its axis the sum of its gaps and
//! parts, across it the parts' extent ([`NestedSize`](nested::NestedSize)). So `⿱ 1 foo 1 bar`
//! written as a glyph passes when its header says what the sum leaves out,
//! and the same nested split does not: the sum has to fill the slot by itself.
//!
//! It may also be **one** part, which no glyph's line could be: `1|foo|1` is
//! `foo` padded across the axis, the one layout a part shorter than its slot
//! needs and that would otherwise take a glyph of its own to write. A lone
//! member is at both ends of its axis at once, so it claims no direction.
//!
//! Only a split takes one, since a nested split divides one axis's share further and
//! an enclosure hands out no share of an axis; and it does not nest again,
//! since a nested split inside a nested split is the same axis as the line around both. A
//! part that has to be both is a glyph of its own.
//!
//! Its ink is kept under [`nested_key`] for the line around it to measure
//! against, but its **own** clearances are not measured. `uniform fix` lays a
//! line holding one out with the nested split as one fixed part and does not
//! look inside it (`fix::clearance::slot_names`) — the inside is the nested
//! glyph's own line, and one the optimizer is meant to work on is written as a
//! glyph — so a chore about the inside would be a finding nothing answers. A
//! one-part nested split makes that plain: its total is the padding the source
//! wrote, and it could never be moved into the band.
//!
//! # A part's margin
//!
//! A part is drawn tight, its ink running to the edges of its box, because it
//! is placed in slots of many sizes and a margin drawn into it would be right
//! for one of them. Most of those slots still want one answer — 口 on top of a
//! full-width `⿱` sits two cells in from either side — and a nested split at
//! every use of it would write that answer out thousands of times, and fix it
//! there: redrawing the default would move none of them. So the part states it
//! once, as a `margin-x L|R` or `margin-y T|B` on its header
//! ([`Margin`](crate::document::Margin)), and a line that finds the part short
//! of the slot **across** the axis by exactly that much pads it there
//! ([`padding_across`]).
//!
//! Only across, because along the axis the room between the parts is the line's
//! gaps, and those are already stated, measured and solved by `uniform fix`; a
//! margin there would be the same room counted twice. Only when the box falls
//! short, because a slot the box fills needs no padding, and the part a slot
//! of another size needs is a variant or a nested split. And only in a glyph's
//! own line: a nested split is the written-out form of exactly this padding,
//! and its members are taken at their boxes, so `1|口|1` means what it says
//! whatever 口 would have asked for.
//!
//! An enclosure's **outer** part is the other slot a box pins, on both axes at
//! once: its walls are the glyph's, so it has to be the glyph's size, and a
//! tight 凵 is short of that by its margin on each side it has one
//! ([`outer_padding`]). It is placed there and read there — its cavity
//! ([`cavity_fits`](enclosure::cavity_fits)) and its walls ([`measure_enclosure_clearances`]) — so a
//! margin on a side the operator opens on is room the cavity has, and a name's
//! `.NxM` promises the room of the part as placed, which is the only room
//! anything is ever put in. The **inner** part is placed by the offsets the
//! line writes and takes none.
//!
//! The padded part is the one-part nested split it abbreviates, and everything
//! after the layout reads it as that: its ink is measured where it sits across
//! the slot ([`GapSide::cross`]), the family of an undecided component fits a
//! slot its margin makes up, and the fixer and the editor's completion offer
//! it for one.
//!
//! # An undecided line is not a wrong one
//!
//! A component written without a `:` suffix has not picked its variant yet.
//! That is the initial state of every Han glyph populated from IDS, not a
//! mistake, so it is a [`Severity::Todo`](crate::issues::Severity::Todo) and not an error: one per unpicked
//! component, and the clearance check — which is about a layout that has not
//! been chosen — stands down for the whole line, as do the unpicked component's
//! own size and cross-axis checks. The glyph is no more built than an erroring
//! one is; the difference is that a build, a `uniform test` run and CI do not
//! fail over it.
//!
//! # An assumed line
//!
//! `assume ⿰ …` is the same line with its clearances taken on trust: the
//! [`Severity::Chore`](crate::issues::Severity::Chore)s the band reports are dropped, and `uniform fix` leaves
//! the line alone. It exists for the layout that is right although the band
//! says otherwise — two parts meant to overlap by a cell, a part with a canyon
//! beside it on purpose — which would otherwise be written as the `ref`s it
//! derives, and then no longer move when a part is redrawn or check that the
//! parts still fit.
//!
//! Exactly the chores and nothing else, because a clearance is the one finding
//! that is a matter of taste line by line: the band is a font-wide default.
//! Everything else stays: an error is a line that cannot be laid out (a part
//! that does not span the box, a size its name lies about), a todo is a
//! decision nobody made, and a warning — a part drawn for the other slot, a
//! cavity not promised or not kept — is a claim a *name* makes, answered by
//! picking the right name rather than by overruling the check.
//!
//! # Clearance
//!
//! A box says nothing about where the ink inside it stops, so the check reads
//! the drawing rather than the boxes: a part's **frontier** per line across the
//! split axis (a hardblank counts), and the **clearance** between two facing
//! frontiers in cells, with the parent's edges taking part as an n+1-th
//! clearance. Two facing hardblank runs share their depth, and an edge is the
//! limit of that rule — hardblank as far out as anyone could ask — so a facing
//! run collapses into it.
//!
//! [`IdealClearances`](crate::audit::IdealClearances) holds each clearance
//! *and* their total to one range; a violation is a warning
//! ([`check_clearances`](clearance::check_clearances)). Both halves are needed, and the reason is
//! arithmetic: the total telescopes down to the parent's extent less the parts'
//! ink extents, so it does not depend on the gaps at all. A source that only had
//! to satisfy the total could never fix a failing line by moving anything — the
//! per-part bound is what an author can act on, and the total is what catches
//! parts that are simply too fat for the box together.
//!
//! The total is held **per axis** ([`Clearance::horizontal`]): a split has one
//! axis, an enclosure two that telescope separately, and adding them would be a
//! number that is neither. The optional second `MIN MAX` of the audit rule is
//! the band an enclosure is held to.
//!
//! A part is measured over its declared box but not *bounded* by it along the
//! split axis: what it draws outside is read where it is drawn, which is how a
//! part writes a side bearing ([`InkProfile::of`]). A part that is a composite
//! is flattened first, and one that is itself split by an IDC line has its line
//! derived and then flattened — `⿱艹林` names 林, which is `⿰木木`. What has no
//! frontier is a part that draws nothing yet, or one whose own line has an
//! undecided component: a line with one of those is not measured rather than
//! measured wrong. `ttf_builder::expand::ink_profiles` tells the cases apart.
//!
//! # The variant name rule (D1)
//!
//! Everything after a name's first `:` is split on `-`; the first `WxH` token
//! is the variant's **size** and the first `l`/`r`/`u`/`d` token its
//! **position**. Neither is required. A size may be `WxH.NxM`, which promises
//! a **cavity** flush against the sides the enclosure it is written for opens
//! on ([`cavity_fits`](enclosure::cavity_fits)); only an outer part states one, stating one is what
//! marks a drawing as an outer part, and [`enclosure_rank`] reads it the way
//! [`direction_rank`] reads a position. The cavity is a *lower bound* where the
//! size is an equality: a size is the box a glyph declares and there is one
//! right answer, a cavity is room a drawing happens to leave.
//!
//! Why a name says these things at all: an ink profile exists only where an
//! `audit ideal-clearance` rule is in force, so a name that did not say what it
//! could hold would leave the editor's completion and `uniform fix` with
//! nothing to go on. What they buy: a declared size must equal the glyph's
//! actual size, checked where the name is *used* (a claim is nothing until
//! someone believes it); a declared position is matched against the slot, and
//! a mismatch is a warning, since a part drawn for one side that fits the other
//! is a design decision. The position is also the tie-break among same-sized
//! variants ([`direction_rank`]): the slot's own direction first, unmarked
//! second, the wrong direction last — the order the editor's listing and the
//! fixer both use, so it lives here with the parse.
//!
//! A three-part split's middle slot claims no direction at all, and the reason
//! is about Han characters rather than about this code. The rules that give a
//! part a positional form all read the *other* side of it — a part with
//! something to its right pulls its last stroke in, one with something below it
//! flattens — and none of them read what is to a part's left. So the middle of
//! a ⿲ fires exactly the rules the left slot fires, and no character has a
//! shape that only appears there: the middle borrows a form that already exists
//! for one side or the other, and which one it borrows is a fact about the
//! character (阝 in the middle is whichever of 阜 and 邑 it descends from) and
//! not about the slot. A middle slot therefore accepts `-l` and `-r` alike, and
//! the component names the side whose drawing it actually wants.

mod clearance;
mod enclosure;
mod fit;
pub(crate) mod gap;
mod ink;
mod nested;
mod op;
mod split;
mod variant;

pub use clearance::Clearance;
pub use enclosure::measure_enclosure_clearances;
pub use fit::{ClearanceRule, FamilyVariant, fits_axis, fits_enclosure_slot, outer_padding};
pub use gap::{GapSide, effective_facing};
pub use ink::{AxisFrontier, InkProfile};
pub use nested::{nested_body, nested_key};
pub use op::{ASSUME, Direction, IdcOp, Raster, Walls};
pub use split::expand_compose;
pub use variant::{
    PartDims, VariantSpec, direction_rank, enclosure_rank, is_undecided, padding_across,
};

#[cfg(test)]
#[path = "../compose_tests/mod.rs"]
mod tests;

#[cfg(test)]
#[path = "../compose_nested_tests.rs"]
mod nested_tests;

#[cfg(test)]
#[path = "../compose_margin_tests.rs"]
mod margin_tests;
