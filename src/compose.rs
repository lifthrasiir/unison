//! IDC composition: the `⿰⿱⿲⿳` splits and the `⿴⿵⿶⿷⿸⿹⿺⿼⿽` enclosures inside a
//! glyph block, and the variant name rule they read.
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
//! # Two kinds of line, one measurement
//!
//! The four **splits** hand each part a share of one axis. The nine
//! **enclosures** ([`Walls`]) hand one part the whole box and seat the other in
//! the cavity it leaves. They are laid out by different code — a split walks a
//! cursor along its axis, an enclosure places one part at written offsets — but
//! everything after that is shared: the same [`InkProfile`], the same
//! [`facing_offset`], the same `audit ideal-clearance` band, the same
//! [`Clearance`] list.
//!
//! What makes that possible is [`GapSide`]. Every gap either layout measures is
//! between two *boundaries*, and the only things that vary are which of a
//! line's four boundaries faces the gap and how far along the cross axis the
//! two parts sit. A split reads the two ends everyone can see and both parts
//! span the parent, so it passes [`GapSide::linear`] and reads exactly the
//! numbers it always did. An enclosure reads the inner face of a wall, against
//! an inner part sitting somewhere inside the box.
//!
//! `⿻` (overlaid), `⿾` (mirrored) and `⿿` (rotated) are deliberately absent.
//! The first says two drawings share a box and nothing about where; the other
//! two transform one drawing rather than composing two, which is a different
//! mechanism. This is not a general IDS layout engine.
//!
//! ```text
//! glyph han-6cb3 15 16
//! ⿰ han-6c35:4x16 han-53ef:11x16
//! ⿰ han-6c35:4x16 -1 han-53ef:12x16    // a negative gap: the boxes overlap
//! ```
//!
//! # The line
//!
//! `IDC TOKEN…`, where each token is a **number** if it parses as one and a
//! **component name** otherwise. What a number means is the operator's to say,
//! and it is the one thing the two kinds of line do not share.
//!
//! On a **split** a number is a **gap**. ⿰/⿱ take two components, ⿲/⿳ three;
//! gaps may appear anywhere among them, including before the first and after
//! the last (which is how a bearing inside the box is written), and default to
//! none. Placement walks the axis in written order: a gap advances the cursor,
//! a component is placed at the cursor and advances it by its own extent. Each
//! component's extent *across* the axis must equal the parent's — a ⿰ part is
//! as tall as the glyph — and that is an error.
//!
//! On an **enclosure** the line is `IDC OUTER INNER P Q`, and `P Q` are the
//! inner part's **top-left offsets** inside the box — not gaps. A gap would be
//! the natural spelling and it does not work: an enclosure has two gaps on each
//! axis, and fixing all four still leaves the layout ambiguous wherever a
//! wall's inner face is ragged, since "one cell from the left wall" is a
//! different column on every row. An offset is one answer to where the part is,
//! and the clearances are then measured rather than declared. Both offsets are
//! written or neither: a line with neither has decided nothing and is a
//! [`Severity::Todo`], exactly as an unpicked variant is, and is *not* read as
//! `0 0`. [`expand_enclosure`] is the whole of it.
//!
//! # A pattern line, and a line that may name nothing
//!
//! A component is a glyph name like a `ref` target, and takes the same two
//! things a `ref` target takes. A [name pattern](crate::pattern) expands in
//! lock-step with the enclosing block's name, so one line writes the split of
//! every glyph the block declares; unlike a `ref`, though, what the line
//! *derives* is not shared between them, because each expansion's parts declare
//! their own boxes and so land at their own offsets. That is why the expansion
//! happens before this module runs at all
//! (`ttf_builder::expand::expand_compose_lines`): what reaches here is always
//! one concrete glyph's line.
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
//! what makes the layout a lookup rather than a search.
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
//! them: 0 means they touch, negative means they overlap. Two hardblanks that
//! face each other are one space and not two, though, so as far as both sides'
//! hardblank runs reach the clearance counts the shared depth as well — a part
//! keeping two cells clear beside a neighbour keeping one shares one of them,
//! and the pair may sit that much closer for the same clearance. The parent's own
//! edges take part too, as the distance from the edge inward (negative when the
//! ink crosses it), so an n-part line has n+1 clearances. An edge is the limit
//! of that same rule: it is hardblank as far out as anyone could ask, since
//! there is nothing outside the box to keep clear of, so the whole of a line's
//! facing hardblank run collapses into it and the edge measures to the ink
//! behind it.
//!
//! [`IdealClearances`](crate::audit::IdealClearances) — `audit ideal-clearance
//! PREFIX* MIN MAX [MIN MAX]` — holds each of them, *and* their total, to one
//! range; a violation is a warning ([`check_clearances`]). Both halves are
//! needed, and the reason is arithmetic: the total telescopes down to the
//! parent's extent less the parts' ink extents, so it does not depend on the
//! gaps at all. A source that only had to satisfy the total could never fix a
//! failing line by moving anything — the per-part bound is what makes the check
//! something an author can act on, and the total is what catches parts that are
//! simply too fat for the box together, however they are shuffled.
//!
//! The total is held **per axis** ([`Clearance::horizontal`]). A split has one
//! axis and this says exactly what it always did; an enclosure has two, and
//! they telescope separately — adding them together would be a number that is
//! neither. The optional second `MIN MAX` is the band an *enclosure* is held
//! to, since seating a whole drawing inside another spends room for a different
//! reason than standing two side by side; a source that writes one pair holds
//! both kinds of line to it.
//!
//! A part is measured over its declared box, but not *bounded* by it along the
//! split axis: what it draws outside is read where it is drawn. That is how a
//! part writes a side bearing — the box is the cells it fills and a hardblank
//! beyond it is the space it wants its neighbour to leave — and two parts that
//! each claim a column and are placed box to box overlap by exactly what they
//! claim ([`InkProfile::of`]).
//!
//! Everything here reads what a part *draws*. A part drawn with its own pixels
//! is read off them; one that is a composite is flattened first and read off
//! that, since a radical written as a `ref` to a shared drawing draws exactly
//! as much as one written out. A part that is itself **split by an IDC line**
//! is the same case one step further out — `⿱艹林` names 林, which is
//! `⿰木木` — so its line is derived and then flattened. What has no frontier
//! at all is a part that draws nothing yet, and one whose own line has an
//! undecided component in it: a line with one of those in it is not measured
//! rather than measured wrong. `ttf_builder::expand::ink_profiles` is where the
//! cases are told apart.
//!
//! # The variant name rule (D1)
//!
//! Everything after a name's first `:` is split on `-`; the first `WxH` token
//! is the variant's **size** and the first `l`/`r`/`u`/`d`/`c` token is its
//! **position** (left, right, up, down, centre — `c` is the centre of either
//! axis). Neither is required, and a name carrying neither is not an error.
//!
//! A size token may be written `WxH.NxM`, which additionally promises a
//! **cavity**: an `NxM` rectangle the drawing leaves clear, flush against
//! whichever sides the enclosure it is written for opens on ([`cavity_fits`]).
//! Only an enclosure's outer part states one, and stating one is what marks a
//! drawing as an outer part at all — it is the enclosure's answer to the
//! `l`/`r` a split's name carries, and [`enclosure_rank`] reads it the way
//! [`direction_rank`] reads those. Two things ride on it:
//!
//! - the promise makes variant selection a **lookup** rather than a search. An
//!   ink profile exists only where an `audit ideal-clearance` rule is in force,
//!   so a name that did not say what it could hold would leave the editor's
//!   completion and `uniform fix` with nothing to go on — which is exactly what
//!   `WxH` does for a split's cross axis;
//! - unlike `WxH`, it is a **lower bound**. A size is the box a glyph declares
//!   and there is one right answer; a cavity is room a drawing happens to
//!   leave, and a drawing more generous than its name is not a fault.
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

use crate::detail::DetailRegion;
use crate::document::{ComposeItem, GlyphCompose, GlyphRef, PixelGrid};
use crate::issues::Severity;

/// Which sides of the parent's box an enclosing operator's outer part fills.
///
/// This is the whole of what tells the nine enclosing operators apart, and it
/// is read by everything: which boundary a clearance is measured against, which
/// side the cavity is flush with, and — through both of those — where the fixer
/// is allowed to put the inner part. Keeping it one table is why there is no
/// second place for `⿷` to mean something slightly different.
///
/// A side that is *not* a wall is **open**: the inner part is measured against
/// the parent's own edge there rather than against anything the outer part
/// draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walls {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Walls {
    /// The walls that face along one axis, low side first: left/right for the
    /// x axis, top/bottom for the y axis.
    pub fn along(self, horizontal: bool) -> (bool, bool) {
        if horizontal {
            (self.left, self.right)
        } else {
            (self.top, self.bottom)
        }
    }

    /// How many of an axis's two clearances touch the parent's own edge — 0
    /// when the axis is walled on both sides, 1 otherwise. Never 2: every
    /// enclosing operator walls at least one side of each axis.
    pub fn open_count(self, horizontal: bool) -> usize {
        let (lo, hi) = self.along(horizontal);
        usize::from(!lo) + usize::from(!hi)
    }
}

/// The IDCs a source may write: the four one-dimensional splits and the nine
/// enclosures.
///
/// `⿻` (overlaid), `⿾` (mirrored) and `⿿` (rotated) are deliberately absent.
/// The first is not a layout — it says two drawings occupy the same box and
/// nothing about where — and the other two are transformations of one drawing
/// rather than a composition of two, which is a different mechanism entirely.
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
    /// ⿴ U+2FF4 surround — walled on all four sides (囗).
    Surround,
    /// ⿵ U+2FF5 surround from above — open below (冂, 門).
    SurroundAbove,
    /// ⿶ U+2FF6 surround from below — open above (凵).
    SurroundBelow,
    /// ⿷ U+2FF7 surround from the left — open right (匚).
    SurroundLeft,
    /// ⿸ U+2FF8 surround from the upper left — open right and below (广, 尸).
    SurroundUpperLeft,
    /// ⿹ U+2FF9 surround from the upper right — open left and below (勹, 气).
    SurroundUpperRight,
    /// ⿺ U+2FFA surround from the lower left — open right and above (辶, 廴).
    SurroundLowerLeft,
    /// ⿼ U+2FFC surround from the right — open left.
    SurroundRight,
    /// ⿽ U+2FFD surround from the lower right — open left and above.
    SurroundLowerRight,
}

impl IdcOp {
    pub fn from_char(c: char) -> Option<Self> {
        match c {
            '\u{2FF0}' => Some(Self::LeftRight),
            '\u{2FF1}' => Some(Self::AboveBelow),
            '\u{2FF2}' => Some(Self::LeftMiddleRight),
            '\u{2FF3}' => Some(Self::AboveMiddleBelow),
            '\u{2FF4}' => Some(Self::Surround),
            '\u{2FF5}' => Some(Self::SurroundAbove),
            '\u{2FF6}' => Some(Self::SurroundBelow),
            '\u{2FF7}' => Some(Self::SurroundLeft),
            '\u{2FF8}' => Some(Self::SurroundUpperLeft),
            '\u{2FF9}' => Some(Self::SurroundUpperRight),
            '\u{2FFA}' => Some(Self::SurroundLowerLeft),
            '\u{2FFC}' => Some(Self::SurroundRight),
            '\u{2FFD}' => Some(Self::SurroundLowerRight),
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
            Self::Surround => '\u{2FF4}',
            Self::SurroundAbove => '\u{2FF5}',
            Self::SurroundBelow => '\u{2FF6}',
            Self::SurroundLeft => '\u{2FF7}',
            Self::SurroundUpperLeft => '\u{2FF8}',
            Self::SurroundUpperRight => '\u{2FF9}',
            Self::SurroundLowerLeft => '\u{2FFA}',
            Self::SurroundRight => '\u{2FFC}',
            Self::SurroundLowerRight => '\u{2FFD}',
        }
    }

    /// Which sides the outer part fills, or `None` for a one-dimensional
    /// operator. This is the test for "is this an enclosure" everywhere:
    /// asking for the walls and asking whether there are any are the same
    /// question, and two spellings of it would be two chances to drift.
    pub fn walls(self) -> Option<Walls> {
        let w = |left, right, top, bottom| {
            Some(Walls {
                left,
                right,
                top,
                bottom,
            })
        };
        match self {
            Self::LeftRight | Self::AboveBelow | Self::LeftMiddleRight | Self::AboveMiddleBelow => {
                None
            }
            Self::Surround => w(true, true, true, true),
            Self::SurroundAbove => w(true, true, true, false),
            Self::SurroundBelow => w(true, true, false, true),
            Self::SurroundLeft => w(true, false, true, true),
            Self::SurroundRight => w(false, true, true, true),
            Self::SurroundUpperLeft => w(true, false, true, false),
            Self::SurroundUpperRight => w(false, true, true, false),
            Self::SurroundLowerLeft => w(true, false, false, true),
            Self::SurroundLowerRight => w(false, true, false, true),
        }
    }

    /// Whether the operator encloses rather than splits along an axis.
    pub fn enclosing(self) -> bool {
        self.walls().is_some()
    }

    /// Whether the split runs along the x axis.
    ///
    /// Only a one-dimensional operator has one axis; an enclosure lays out on
    /// both, and every consumer of this has to ask [`Self::walls`] first.
    /// `false` here is the answer for an operator that has no axis at all, not
    /// a claim that it is vertical.
    pub fn horizontal(self) -> bool {
        matches!(self, Self::LeftRight | Self::LeftMiddleRight)
    }

    /// How many components the operator takes.
    pub fn arity(self) -> usize {
        match self {
            Self::LeftMiddleRight | Self::AboveMiddleBelow => 3,
            _ => 2,
        }
    }

    /// Which position a component sits in, for the name check and the
    /// tie-break. Slots past the arity have no direction, and so does every
    /// slot of an enclosure: `l`/`r`/`u`/`d`/`c` describe a share of an axis,
    /// which is not what an outer and an inner part are to each other. What
    /// says which slot an enclosure's name was drawn for is the cavity in it
    /// — see [`VariantSpec::inner`] and [`enclosure_rank`].
    pub fn slot_direction(self, slot: usize) -> Option<Direction> {
        if self.enclosing() {
            return None;
        }
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
    /// See [`cavity_fits`].
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
    /// Per row, read left to right, or `None` for a row that draws nothing.
    pub rows: Vec<Option<InkLine>>,
    /// Per column, read top to bottom.
    pub cols: Vec<Option<InkLine>>,
    /// Per row, what its two ink frontiers actually cover of the boundary they
    /// face. Parallel to [`Self::rows`].
    pub row_edges: Vec<EdgeCover>,
    /// Per column, the same. Parallel to [`Self::cols`].
    pub col_edges: Vec<EdgeCover>,
    /// The lattice the covers are counted over: one declared cell across a line
    /// is `edge_den`. Catalog geometry lies on the half lattice, so this is
    /// twice the grid's `scale` and every endpoint is exact on it.
    pub edge_den: u16,
}

/// How much of the boundary a line's frontier faces its ink actually covers, as
/// sorted disjoint intervals over [`InkProfile::edge_den`], measured across the
/// line (down a row, along a column).
///
/// A frontier is a *cell*, and a cell is inked long before its ink reaches the
/// side of it that faces the neighbour: a diagonal ending in a corner inks the
/// cell and covers none of the edge, or a sliver of it. That difference is the
/// whole reason this is kept — [`contact_run`] asks whether two parts really
/// run together, and the cells alone answer a coarser question, always the
/// stricter way round.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgeCover {
    /// What the near frontier covers of its near side.
    pub near: Vec<(u16, u16)>,
    /// What the far frontier covers of its far side.
    pub far: Vec<(u16, u16)>,
    /// What the *first run's* far frontier covers of its far side — a wall's
    /// inner face, the one a cavity sees. Identical to [`Self::far`] on a line
    /// that is one run, which is every line of a part that does not enclose.
    pub first_far: Vec<(u16, u16)>,
    /// The same for the last run's near frontier. Identical to [`Self::near`]
    /// on a one-run line.
    pub last_near: Vec<(u16, u16)>,
}

/// Whether two covers, each over its own lattice, share any length at all.
fn covers_meet(a: &[(u16, u16)], a_den: u16, b: &[(u16, u16)], b_den: u16) -> bool {
    let (a_den, b_den) = (a_den.max(1) as i64, b_den.max(1) as i64);
    // Cross-multiplied onto the shared lattice, which needs no gcd: the
    // comparison is all anyone wants of it.
    a.iter().any(|&(p, q)| {
        let (p, q) = (p as i64 * b_den, q as i64 * b_den);
        b.iter().any(|&(r, t)| {
            let (r, t) = (r as i64 * a_den, t as i64 * a_den);
            p.max(r) < q.min(t)
        })
    })
}

/// Sorted, with everything that meets or overlaps run together.
fn merge_cover(list: &mut Vec<(u16, u16)>) {
    list.sort_unstable();
    let mut out: Vec<(u16, u16)> = Vec::with_capacity(list.len());
    for &(s, e) in list.iter() {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    *list = out;
}

/// What one line of a grid occupies along an axis: the two frontiers, plus how
/// far a hardblank run reaches in from each of them.
///
/// The runs are what lets two facing parts share space. A hardblank draws
/// nothing yet occupies its cell (it is space the source *means*), so it holds
/// the frontier out where an empty cell would not; but where the two parts'
/// runs face each other, the same nothing is written twice, and the shared
/// depth is clearance rather than a part's own extent — see [`facing_offset`].
///
/// The two frontiers are in the box's coordinates and are *not* bounded by it:
/// a part that draws outside what it declared is measured where it draws, so
/// `near` may be negative and `far` may reach past the extent. See
/// [`InkProfile::of`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InkLine {
    /// The lowest occupied coordinate on this line.
    pub near: i32,
    /// The highest occupied coordinate on this line.
    pub far: i32,
    /// Hardblank cells running inward from `near`, `near` included.
    pub near_hardblanks: u16,
    /// Hardblank cells running inward from `far`, `far` included.
    pub far_hardblanks: u16,
    /// The run this line presents to a cavity from the **low** side, looking
    /// toward higher coordinates. `None` when the line draws nothing on that
    /// side and so has no wall there to be measured against.
    pub low_wall: Option<WallFace>,
    /// The same from the **high** side, looking toward lower coordinates.
    pub high_wall: Option<WallFace>,
}

/// A wall's cavity-facing end on one line: which run it is, and what the run's
/// claim beyond it comes to.
///
/// **Which run** is the whole question, and the answer is *the run nearest the
/// box's middle*. Taking the line's first and last runs would be the obvious
/// rule and it is wrong on real drawings: every han part writes its side
/// bearing as a detached hardblank column at the box's edge, so the first run
/// of nearly every line is a bearing rather than a wall, and a cavity measured
/// against it would swallow the wall itself. The middle is where a cavity is,
/// so the runs on either side of it are what bound one; a run that *contains*
/// the middle fills the line and presents its two far ends, which reads as the
/// overlap it is.
///
/// What this cannot see is a mark the drawing leaves inside its own cavity —
/// the run adjacent to the middle is the only one either side offers. That is
/// deliberate: choosing by where the inner part actually sits would make the
/// measurement depend on the placement, and with it the axis totals that
/// [`measure_enclosure_clearances`] telescopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WallFace {
    /// The run's cavity-facing cell.
    pub at: i32,
    /// Hardblank cells running back from `at` into the run.
    pub hardblanks: u16,
    /// How long the run is, so that a claim which has eaten it whole reads as
    /// no ink at all.
    pub run: u16,
}

/// One boundary of one line, as everything that measures a gap sees it: where
/// the material stops, how deep the source's claim on the space beyond runs,
/// and which way the boundary looks.
///
/// The four boundaries a line has are the two it presents to the world
/// ([`InkLine::near`] and [`InkLine::far`]) and the two it presents to a cavity
/// between its first and last run. A part that does not enclose draws one run
/// per line and its four boundaries collapse onto two, which is why every
/// one-dimensional measurement reads exactly the numbers it always did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    /// The occupied cell the boundary is at.
    pub at: i32,
    /// Hardblank cells running back from `at` into the run behind it.
    pub hardblanks: u16,
    /// How long that run is, in cells — what says whether the claim has eaten
    /// it whole.
    pub run: u16,
    /// `+1` when the boundary looks toward higher coordinates, `-1` toward
    /// lower ones. Every "pull the claim back" and "step past the frontier"
    /// below is written once, with this as its sign.
    pub toward: i32,
}

impl Face {
    /// Where the run's **ink** stops on this side: `at` pulled back by the
    /// hardblank claim, or `None` when the claim is the whole run and there is
    /// no ink here to touch anything.
    ///
    /// This is the reason [`contact_run`] needs no hardblank term of its own: a
    /// claim parts two parts by holding their *ink* apart, and ink that is
    /// already apart touches over no lines.
    pub fn ink(self) -> Option<i32> {
        (self.hardblanks < self.run).then(|| self.at - self.toward * self.hardblanks as i32)
    }
}

impl InkLine {
    /// One line of a grid, from the runs of occupied cells it holds and the
    /// coordinate a cavity on it would be around ([`WallFace`]).
    ///
    /// `runs` is in ascending order and non-empty; each is `(near, far,
    /// hardblanks running in from near, hardblanks running in from far)`.
    pub fn from_runs(runs: &[(i32, i32, u16, u16)], pivot: i32) -> Option<Self> {
        let first = runs.first()?;
        let last = runs.last()?;
        let len = |r: &(i32, i32, u16, u16)| (r.1 - r.0 + 1).clamp(0, u16::MAX as i32) as u16;
        // A run straddling the middle fills the line: it is the wall on both
        // sides at once, and presents each of its own far ends.
        let straddling = runs.iter().find(|r| r.0 <= pivot && pivot <= r.1);
        let low = straddling.or_else(|| runs.iter().rev().find(|r| r.1 < pivot));
        let high = straddling.or_else(|| runs.iter().find(|r| r.0 > pivot));
        Some(Self {
            near: first.0,
            far: last.1,
            near_hardblanks: first.2,
            far_hardblanks: last.3,
            low_wall: low.map(|r| WallFace {
                at: r.1,
                hardblanks: r.3,
                run: len(r),
            }),
            high_wall: high.map(|r| WallFace {
                at: r.0,
                hardblanks: r.2,
                run: len(r),
            }),
        })
    }

    /// The boundary looking toward **higher** coordinates: the line's own far
    /// end, or — with `inner` — the low-side wall's cavity face.
    pub fn upper(self, inner: bool) -> Option<Face> {
        let (at, hardblanks, run) = match inner {
            false => (
                self.far,
                self.far_hardblanks,
                (self.far - self.near + 1).clamp(0, u16::MAX as i32) as u16,
            ),
            true => {
                let w = self.low_wall?;
                (w.at, w.hardblanks, w.run)
            }
        };
        Some(Face {
            at,
            hardblanks,
            run,
            toward: 1,
        })
    }

    /// The boundary looking toward **lower** coordinates: the line's own near
    /// end, or — with `inner` — the high-side wall's cavity face.
    pub fn lower(self, inner: bool) -> Option<Face> {
        let (at, hardblanks, run) = match inner {
            false => (
                self.near,
                self.near_hardblanks,
                (self.far - self.near + 1).clamp(0, u16::MAX as i32) as u16,
            ),
            true => {
                let w = self.high_wall?;
                (w.at, w.hardblanks, w.run)
            }
        };
        Some(Face {
            at,
            hardblanks,
            run,
            toward: -1,
        })
    }
}

impl InkProfile {
    /// Read a part's frontiers over its **declared box**: `origin` is where the
    /// box's corner sits in the grid and `extent` is its size, both in declared
    /// cells, so the profile is indexed the way the IDC layout is.
    ///
    /// A cell counts as occupied when the source put *something* there, which
    /// includes a hardblank — see
    /// [`PixelShape::is_clear`](crate::pixel::PixelShape::is_clear). This is
    /// where the `CLEAR` / `HARDBLANK` / `INK` ladder those predicates are
    /// named for is actually read: a declared cell is hardblank when the source
    /// wrote hardblanks there and no ink, so a `scale 2` part is read on the
    /// same units its layout is in.
    ///
    /// **Along** the line being read, a cell outside the box keeps the
    /// coordinate it is drawn at, negative or past the extent as the case may
    /// be. That is the whole point on this axis: a hardblank drawn beyond the
    /// box is a claim on the neighbour's space (it is how a part writes a side
    /// bearing), and one folded onto the box's edge would be lost the moment
    /// that edge held ink. Ink out there is read the same way — a part drawing
    /// where it said it would not can only cost its neighbour room, never gain
    /// any.
    ///
    /// **Across** it the cell folds into the nearest line instead: the lines of
    /// two parts of one IDC line are matched by index, so a profile that is not
    /// exactly the box's size across cannot be measured against anything. A box
    /// reaching past the grid is simply clear out there — there is nothing
    /// drawn to report.
    /// `raster` is the raster coordinate `(col, row)` of grid cell `(0, 0)`:
    /// `(0, 0)` for a glyph's own pixels, and the flattening's own origin
    /// ([`ResolvedGlyph::origin_col`](crate::ref_composite::ResolvedGlyph) and
    /// `origin_row`) for a part that is a composite. A ref reaching left of or
    /// above the origin makes it negative, and the ink out there is exactly the
    /// ink this profile may not lose (see "along the line" below). It is in
    /// *raster* cells, not declared ones, because a `ref` offset is: it need
    /// not be a multiple of `scale`, so it cannot be folded into `origin`.
    pub fn of(
        grid: &PixelGrid,
        scale: u8,
        raster: (i32, i32),
        origin: (i16, i16),
        extent: (u16, u16),
    ) -> Self {
        let s = scale.max(1) as u16;
        let (w, h) = extent;
        // Raster → box coordinate, on the one axis at a time the callers below
        // read it: floor division, since a grid starting left of the origin has
        // negative raster coordinates and `-1 / 2` is not the cell `-1` is in.
        let box_of = |raster: i32, origin: i16| raster.div_euclid(s as i32) - origin as i32;
        // Per declared cell: nothing / hardblank only / ink, whichever is
        // greatest over the sub-cells, so any ink makes the cell ink.
        const CLEAR: u8 = 0;
        const HARDBLANK: u8 = 1;
        const INK: u8 = 2;
        // The box coordinates the grid reaches on each axis, box and grid
        // together, so a cell outside the box still has a place to be counted.
        let span = |extent: u16, len: u16, origin: i16, base: i32| -> (i32, i32) {
            if extent == 0 || len == 0 {
                return (0, extent as i32);
            }
            let (lo, hi) = (box_of(base, origin), box_of(base + len as i32 - 1, origin));
            (lo.min(0), hi.max(extent as i32 - 1) + 1)
        };
        let (col_lo, col_hi) = span(w, grid.width, origin.0, raster.0);
        let (row_lo, row_hi) = span(h, grid.height, origin.1, raster.1);
        let (cols_wide, rows_tall) = ((col_hi - col_lo) as usize, (row_hi - row_lo) as usize);
        // One map per axis, since which coordinate folds depends on which way
        // the line is read: `by_row` keeps the column exact and `by_col` the row.
        let mut by_row = vec![CLEAR; cols_wide * h as usize];
        let mut by_col = vec![CLEAR; rows_tall * w as usize];
        if w > 0 && h > 0 {
            for row in 0..grid.height {
                for col in 0..grid.width {
                    let px = grid.get(row, col);
                    if px.is_clear() {
                        continue;
                    }
                    let level = if px.is_hardblank() { HARDBLANK } else { INK };
                    let box_r = box_of(raster.1 + row as i32, origin.1);
                    let box_c = box_of(raster.0 + col as i32, origin.0);
                    let folded_r = box_r.clamp(0, h as i32 - 1) as usize;
                    let folded_c = box_c.clamp(0, w as i32 - 1) as usize;
                    let at = &mut by_row[folded_r * cols_wide + (box_c - col_lo) as usize];
                    *at = (*at).max(level);
                    let at = &mut by_col[folded_c * rows_tall + (box_r - row_lo) as usize];
                    *at = (*at).max(level);
                }
            }
        }
        // Every run of occupied cells on one line, which is what a cavity
        // between two of them is bounded by. A part that does not enclose draws
        // one run per line and its walls coincide with `near`/`far`, so every
        // one-dimensional measurement reads what it always did.
        let scan = |lo: i32, len: usize, pivot: i32, at: &dyn Fn(usize) -> u8| -> Option<InkLine> {
            let blanks_from = |from: usize, step: isize| -> u16 {
                let mut n = 0u16;
                let mut i = from as isize;
                while i >= 0 && (i as usize) < len && at(i as usize) == HARDBLANK {
                    n += 1;
                    i += step;
                }
                n
            };
            let mut runs: Vec<(i32, i32, u16, u16)> = Vec::new();
            let mut i = 0usize;
            while i < len {
                if at(i) == CLEAR {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < len && at(i) != CLEAR {
                    i += 1;
                }
                let end = i - 1;
                runs.push((
                    lo + start as i32,
                    lo + end as i32,
                    blanks_from(start, 1),
                    blanks_from(end, -1),
                ));
            }
            InkLine::from_runs(&runs, pivot)
        };
        // The coordinate a cavity on the line would be around: the middle of
        // the *declared box*, which is the rectangle an enclosure lays out in.
        // See [`WallFace`] for why the walls are chosen by it.
        let (col_pivot, row_pivot) = (w as i32 / 2, h as i32 / 2);
        let rows: Vec<Option<InkLine>> = (0..h as usize)
            .map(|r| scan(col_lo, cols_wide, col_pivot, &|c| by_row[r * cols_wide + c]))
            .collect();
        let cols: Vec<Option<InkLine>> = (0..w as usize)
            .map(|c| scan(row_lo, rows_tall, row_pivot, &|r| by_col[c * rows_tall + r]))
            .collect();

        // A second pass, now that the frontiers are known: what each of them
        // covers of the boundary it faces. Only the outermost sub-cell of a
        // declared cell touches that boundary, which is why `scale` shows up
        // here as a position and not only as a divisor.
        let mut row_edges = vec![EdgeCover::default(); h as usize];
        let mut col_edges = vec![EdgeCover::default(); w as usize];
        let push = |out: &mut Vec<(u16, u16)>, list: &[(u8, u8)], sub: u16, den: u8| {
            let mul = 2 / den.max(1) as u16;
            out.extend(
                list.iter()
                    .map(|&(a, b)| (sub * 2 + a as u16 * mul, sub * 2 + b as u16 * mul)),
            );
        };
        for row in 0..grid.height {
            for col in 0..grid.width {
                let px = grid.get(row, col);
                // A hardblank draws nothing, so it covers nothing — the same
                // statement [`InkLine::ink`] makes about the frontiers.
                if px.is_clear() || px.is_hardblank() {
                    continue;
                }
                let id = px.catalog_shape_id();
                let region = match id {
                    crate::pixel::PX_CUSTOM => grid.details.get(&(row, col)).cloned(),
                    _ => Some(DetailRegion::from_shape(id)),
                };
                let Some(region) = region else { continue };
                let cov = region.edge_coverage();
                let (abs_r, abs_c) = (raster.1 + row as i32, raster.0 + col as i32);
                let (box_r, box_c) = (box_of(abs_r, origin.1), box_of(abs_c, origin.0));
                let (sub_j, sub_i) = (
                    abs_r.rem_euclid(s as i32) as u16,
                    abs_c.rem_euclid(s as i32) as u16,
                );
                let fr = box_r.clamp(0, h as i32 - 1) as usize;
                let fc = box_c.clamp(0, w as i32 - 1) as usize;
                // Four boundaries per line, not two: the outward pair and the
                // pair a cavity between the line's first and last run sees. On
                // a one-run line the two pairs are the same cells and the same
                // covers are collected twice, which is what makes an enclosure
                // measurement read a non-enclosing part correctly.
                if let Some(line) = rows.get(fr).copied().flatten() {
                    let mut at = |inner: bool| {
                        if line.upper(inner).and_then(Face::ink) == Some(box_c) && sub_i == s - 1 {
                            let out = match inner {
                                false => &mut row_edges[fr].far,
                                true => &mut row_edges[fr].first_far,
                            };
                            push(out, &cov.right, sub_j, cov.den);
                        }
                        if line.lower(inner).and_then(Face::ink) == Some(box_c) && sub_i == 0 {
                            let out = match inner {
                                false => &mut row_edges[fr].near,
                                true => &mut row_edges[fr].last_near,
                            };
                            push(out, &cov.left, sub_j, cov.den);
                        }
                    };
                    at(false);
                    at(true);
                }
                if let Some(line) = cols.get(fc).copied().flatten() {
                    let mut at = |inner: bool| {
                        if line.upper(inner).and_then(Face::ink) == Some(box_r) && sub_j == s - 1 {
                            let out = match inner {
                                false => &mut col_edges[fc].far,
                                true => &mut col_edges[fc].first_far,
                            };
                            push(out, &cov.bottom, sub_i, cov.den);
                        }
                        if line.lower(inner).and_then(Face::ink) == Some(box_r) && sub_j == 0 {
                            let out = match inner {
                                false => &mut col_edges[fc].near,
                                true => &mut col_edges[fc].last_near,
                            };
                            push(out, &cov.top, sub_i, cov.den);
                        }
                    };
                    at(false);
                    at(true);
                }
            }
        }
        for e in row_edges.iter_mut().chain(col_edges.iter_mut()) {
            merge_cover(&mut e.near);
            merge_cover(&mut e.far);
            merge_cover(&mut e.first_far);
            merge_cover(&mut e.last_near);
        }
        Self {
            rows,
            cols,
            row_edges,
            col_edges,
            edge_den: 2 * s,
        }
    }

    /// The covers of the lines that face along the split axis, indexed like
    /// [`Self::along`].
    fn along_edges(&self, horizontal: bool) -> &[EdgeCover] {
        if horizontal {
            &self.row_edges
        } else {
            &self.col_edges
        }
    }

    /// The lines that face along the split axis: rows for a horizontal split
    /// (each gives the leftmost and rightmost column), columns for a vertical
    /// one. Indexed by position across the axis, so two parts of one line index
    /// alike.
    fn along(&self, horizontal: bool) -> &[Option<InkLine>] {
        if horizontal { &self.rows } else { &self.cols }
    }

    /// How far the ink reaches towards each end of the axis, in the part's own
    /// coordinates, or `None` for a part that draws nothing at all.
    ///
    /// These are the two numbers a clearance against a *parent edge* is
    /// arithmetic on: the near edge measures against the smallest near
    /// frontier and the far edge against the largest far one, since a
    /// clearance is the smallest distance over all the lines.
    ///
    /// A parent edge is taken to be hardblank all the way out — there is
    /// nothing beyond it for a part to keep clear of — so a line's facing
    /// hardblank run collapses into the edge entirely, and the frontier the
    /// edge sees is the first cell past that run. A line that is nothing but
    /// hardblanks therefore constrains neither edge, which is the same
    /// statement: it draws nothing to be clear of.
    pub fn frontier(&self, horizontal: bool) -> Option<AxisFrontier> {
        let lines = self.along(horizontal);
        Some(AxisFrontier {
            near: lines
                .iter()
                .filter_map(|l| l.map(|l| l.near + l.near_hardblanks as i32))
                .min()?,
            far: lines
                .iter()
                .filter_map(|l| l.map(|l| l.far - l.far_hardblanks as i32))
                .max()?,
        })
    }
}

/// How far a part's ink reaches towards each end of the split axis, as an edge
/// sees it — the hardblanks facing the edge already collapsed into it (see
/// [`InkProfile::frontier`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisFrontier {
    /// The lowest coordinate any line's ink starts at.
    pub near: i32,
    /// The highest coordinate any line's ink ends at.
    pub far: i32,
}

/// The clearance two adjacent parts would leave each other if the second sat
/// exactly at the first's own origin — so the clearance at any placement is
/// this plus the distance between the two origins.
///
/// `None` when the two share no line on which both draw: there is then no pair
/// of frontiers to measure between, and the line is not measured rather than
/// measured wrong.
///
/// A line's clearance is the gap between the two frontiers *plus* the depth the
/// parts' facing hardblank runs share ([`InkLine`]): where both sides wrote a
/// hardblank the space is written twice over, and space written twice is one
/// space. Only the shared depth counts — the smaller of the two runs — so
/// nothing is ever measured through a cell one side draws ink in. The result is
/// therefore this side of the frontier-only measurement: a pair that meets
/// hardblank to hardblank reads as *more* clear than its frontiers say, and so
/// sits that much closer once the ideal clearance is solved for.
pub fn facing_offset(lo: GapSide, hi: GapSide, horizontal: bool) -> Option<i32> {
    lo.shared_span(hi, horizontal)
        .filter_map(|line| {
            let f = lo.line_at(horizontal, line)?.upper(lo.inner)?;
            let g = hi.line_at(horizontal, line)?.lower(hi.inner)?;
            let shared = f.hardblanks.min(g.hardblanks) as i32;
            Some(g.at - f.at - 1 + shared)
        })
        .min()
}

/// One side of one gap: whose drawing bounds it, which of that drawing's
/// boundaries faces the gap, and where the drawing sits across the axis.
///
/// A one-dimensional line needs none of the last two — both parts fill the
/// parent's whole cross extent and present the ends everyone can see — which is
/// what [`GapSide::linear`] says. An enclosure needs both: its outer part
/// bounds the gap with the *inner* face of a wall, and its inner part sits
/// somewhere along the cross axis rather than spanning it.
#[derive(Clone, Copy, Debug)]
pub struct GapSide<'a> {
    pub profile: &'a InkProfile,
    /// Read the inner face — a wall's cavity side — rather than the outward
    /// one.
    pub inner: bool,
    /// Where this part's box starts along the *cross* axis, in the parent's
    /// declared cells, so that two parts' lines can be matched by the parent's
    /// coordinate rather than by index.
    pub cross: i32,
}

impl<'a> GapSide<'a> {
    /// A part of a one-dimensional line: its outward face, spanning the
    /// parent across the axis. Every measurement a `⿰`/`⿱`/`⿲`/`⿳` line makes
    /// is between two of these, and reads exactly the numbers it always did.
    pub fn linear(profile: &'a InkProfile) -> Self {
        Self {
            profile,
            inner: false,
            cross: 0,
        }
    }

    /// The stretch of the parent's cross axis both sides have a line on, as a
    /// half-open range of parent coordinates. Outside it one of the two parts
    /// simply is not there, and a gap between a drawing and nothing is not a
    /// gap.
    fn shared_span(self, other: GapSide<'a>, horizontal: bool) -> std::ops::Range<i32> {
        let (a, b) = (
            self.profile.along(horizontal).len() as i32,
            other.profile.along(horizontal).len() as i32,
        );
        let start = self.cross.max(other.cross);
        let end = (self.cross + a).min(other.cross + b);
        start..start.max(end)
    }

    /// This side's line at one parent cross coordinate, or `None` where it
    /// draws nothing there.
    fn line_at(self, horizontal: bool, line: i32) -> Option<InkLine> {
        let index = usize::try_from(line - self.cross).ok()?;
        *self.profile.along(horizontal).get(index)?
    }

    /// The covers of the boundary this side presents to the gap, per line,
    /// indexed the way [`Self::paired_lines`] walks them.
    fn facing_cover(self, horizontal: bool, upward: bool, index: usize) -> Option<&'a [(u16, u16)]> {
        let e = self.profile.along_edges(horizontal).get(index)?;
        Some(match (upward, self.inner) {
            (true, false) => &e.far,
            (true, true) => &e.first_far,
            (false, false) => &e.near,
            (false, true) => &e.last_near,
        })
    }
}

/// How many consecutive lines the two parts' ink touches along, with `b` sitting
/// `delta` declared cells past `a`'s own origin — the longest such run, since a
/// split is spoiled by one long seam rather than by scattered nicks.
///
/// A line counts when both parts draw ink on it and their *contours* meet:
/// the two frontier cells abut and the ink in them covers some of the boundary
/// they share ([`EdgeCover`]). Cells alone would be the stricter question and
/// the less intuitive one — a diagonal that ends in a corner inks its cell
/// without reaching the side of it, and two such cells pass each other with no
/// seam to speak of.
///
/// Either way it is a question about the *pattern* of two facing edges and not
/// about the distance between them, which is the whole point: two flat faces
/// meeting over sixteen lines and two tips grazing over one are the same
/// clearance and want opposite answers.
///
/// Hardblanks need no term here. A hardblank holds a part's *ink* frontier back
/// (see [`InkLine::ink`]), so a claim that has already parted two parts leaves
/// nothing touching for this to count, and the two mechanisms never both fire
/// on one line. A line either part draws nothing on breaks the run: there is no
/// contact where one side is not there.
pub fn contact_run(lo: GapSide, hi: GapSide, horizontal: bool, delta: i32) -> u16 {
    let (mut longest, mut run) = (0u16, 0u16);
    for line in lo.shared_span(hi, horizontal) {
        let faces = lo
            .line_at(horizontal, line)
            .zip(hi.line_at(horizontal, line));
        let touching = match faces
            .map(|(x, y)| {
                (
                    x.upper(lo.inner).and_then(Face::ink),
                    y.lower(hi.inner).and_then(Face::ink),
                )
            })
            .unwrap_or((None, None))
        {
            (Some(a_far), Some(b_near)) => match delta + b_near - a_far - 1 {
                // Their cells overlap: whatever the contours do inside them,
                // the parts are into each other and the line is not the place
                // to be subtle about it.
                gap if gap < 0 => true,
                // The two frontier cells abut, so the boundary they share is
                // one line of geometry — and only the part of it both actually
                // cover is a seam. A tip that inks its cell without reaching
                // the side of it touches nothing.
                0 => {
                    let ae = lo.facing_cover(horizontal, true, (line - lo.cross) as usize);
                    let be = hi.facing_cover(horizontal, false, (line - hi.cross) as usize);
                    match (ae, be) {
                        (Some(ae), Some(be)) => {
                            covers_meet(ae, lo.profile.edge_den, be, hi.profile.edge_den)
                        }
                        // A profile from before the covers were kept: the cells
                        // are all there is to go on.
                        _ => true,
                    }
                }
                _ => false,
            },
            _ => false,
        };
        run = if touching { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    longest
}

/// What `audit max-contact-run` asks of one junction: the run its two parts
/// would share **if they were drawn together until their ink met**, and the
/// cell that costs the layout.
///
/// The run is measured at that meeting and not where the line happens to put
/// the parts, because the demand is a property of the *pair*: a junction that
/// was given its cell has to keep reading as though it had none to spare, or
/// the space the rule asked for would be counted a second time as room the
/// glyph could spend elsewhere. This is exactly how a hardblank behaves — it
/// occupies its cell wherever the parts sit — and the two are the same
/// statement made twice, which is why `owed` nets one against the other: a
/// hardblank already holding the parts apart is the rule's answer, not a second
/// claim on top of it.
///
/// `None` when the two share no line on which both draw ink.
pub fn contact_demand(
    lo: GapSide,
    hi: GapSide,
    horizontal: bool,
    max: u16,
) -> Option<ContactDemand> {
    let facing = facing_offset(lo, hi, horizontal)?;
    // The same measurement with every hardblank stripped off: where the *ink*
    // would meet, which is where the run is counted.
    let ink = lo
        .shared_span(hi, horizontal)
        .filter_map(|line| {
            let a_far = lo.line_at(horizontal, line)?.upper(lo.inner)?.ink()?;
            let b_near = hi.line_at(horizontal, line)?.lower(hi.inner)?.ink()?;
            Some(b_near - a_far - 1)
        })
        .min()?;
    let run = contact_run(lo, hi, horizontal, -ink);
    Some(ContactDemand {
        run,
        // What the hardblanks already hold open, netted off what the rule asks.
        owed: (i32::from(run > max) - (ink - facing)).max(0),
    })
}

/// What [`contact_demand`] found: how far two parts would run together, and
/// what the layout owes them for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactDemand {
    /// The lines their ink would share if they met.
    pub run: u16,
    /// The cells the junction owes on top of what its hardblanks already claim
    /// — 0 or 1, since the rule asks for one cell and asks once.
    pub owed: i32,
}

/// The facing measurement a layout is *scored* on: [`facing_offset`], less what
/// [`contact_demand`] asks of the pair.
///
/// Everything that lays an IDC line out reads this rather than the raw
/// measurement, which is what keeps the rule from being a second kind of
/// number: it is a hardblank the source did not have to write, and it lands in
/// the one place a hardblank would have landed.
pub fn effective_facing(
    lo: GapSide,
    hi: GapSide,
    horizontal: bool,
    max_contact_run: Option<u16>,
) -> Option<i32> {
    let facing = facing_offset(lo, hi, horizontal)?;
    let owed = max_contact_run
        .and_then(|max| contact_demand(lo, hi, horizontal, max))
        .map_or(0, |d| d.owed);
    Some(facing - owed)
}

/// What expanding one IDC line comes to: the `ref`s it stands for, and what is
/// wrong with it, each message with a severity and no location — the caller
/// owns the [`crate::resolve::ItemRef`].
pub type ComposeExpansion = (Vec<GlyphRef>, Vec<(Severity, String)>);

/// How an *undecided* component's family is answered: the sizes every variant
/// of its base name draws, which is what separates a line waiting for a
/// decision from one whose decision cannot be made. A callback for the same
/// reason [`InkLookup`] is one — the caller decides what a name means.
pub type FamilyLookup<'a> = dyn Fn(&str) -> Vec<(u16, u16)> + 'a;

/// How a component name is answered with the ink it draws. See
/// [`ClearanceRule::ink`] for why this is a callback.
pub type InkLookup<'a> = dyn Fn(&str) -> Option<&'a InkProfile> + 'a;

/// One `audit ideal-clearance` rule, as [`expand_compose`] applies it: the range
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
/// what it may *propose* for a slot, and [`expand_compose`] over what an
/// undecided component's family could ever put there.
pub fn fits_axis(along: i32, axis_extent: i32) -> bool {
    along < axis_extent
}

/// Whether a glyph this size could fill one slot of a line split along
/// `axis_extent`, in a glyph `cross_extent` across: [`fits_axis`] along the
/// split, and the exact box the line demands across it.
pub fn fits_slot(size: (u16, u16), axis_extent: u16, cross_extent: u16, horizontal: bool) -> bool {
    let (along, across) = if horizontal { size } else { (size.1, size.0) };
    across == cross_extent && fits_axis(along as i32, axis_extent as i32)
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
///   number a box carries ([`cavity_fits`]).
pub fn fits_enclosure_slot(size: (u16, u16), parent: (u16, u16), outer: bool) -> bool {
    match outer {
        true => size == parent,
        false => size.0 <= parent.0 && size.1 <= parent.1,
    }
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

/// What an undecided component's family draws when *none* of it fits the slot,
/// as a list for the message — or `None` when the caller offered no family, the
/// family is empty, or something in it fits.
///
/// An empty family stays a TODO on purpose: a component nothing has been drawn
/// for yet is the ordinary state of a source populated from IDS, and the work
/// it names is "draw it", which is what a TODO already says. The warning is for
/// the case where the drawings exist and none of them can go there.
fn misfit_variants(
    name: &str,
    family: Option<&FamilyLookup>,
    fits: &dyn Fn((u16, u16)) -> bool,
) -> Option<String> {
    let sizes = family?(name);
    if sizes.is_empty() || sizes.iter().copied().any(fits) {
        return None;
    }
    // By the numbers rather than by the text, so `5x16` comes before `15x16`.
    let mut sizes: Vec<String> = sizes
        .iter()
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
const MAX_LISTED_VARIANTS: usize = 4;

/// Turn one IDC line into the `ref`s it stands for, plus what is wrong with it.
///
/// Best effort: a component whose size is unknown is placed where the walk has
/// got to and advances it by nothing, so the parts that *are* known still land
/// where they belong and the editor can draw the glyph while it is being filled
/// in. The diagnostics are what stop a wrong glyph from passing
/// for a right one.
///
/// `parent` is the enclosing `glyph` header's box, `dims` answers for a
/// component name, and `family` — where the caller has one — answers what the
/// *family* of an undecided component draws, which is what separates a line
/// waiting for a decision from one whose decision cannot be made. Messages come
/// back with a severity and no location — the caller owns the
/// [`crate::resolve::ItemRef`].
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
    family: Option<&FamilyLookup>,
    clearance: Option<&ClearanceRule>,
) -> ComposeExpansion {
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

    // An enclosure lays out on both axes at once and has no cursor to walk, so
    // it is its own pass from here. Everything above it — the arity, the
    // parent's box — is what every IDC line answers for alike.
    if let Some(walls) = op.walls() {
        let (encl_refs, encl_issues) = expand_enclosure(
            walls,
            (parent_w, parent_h),
            scale,
            compose,
            dims,
            family,
            clearance,
        );
        refs.extend(encl_refs);
        issues.extend(encl_issues.into_iter().map(|(s, m)| (s, at(m))));
        return (refs, issues);
    }

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
        // The size and the position a name states are claims the *author*
        // made, so they are read off the name as written: a component named
        // through an alias has had `name` pointed at the drawing, and the
        // drawing's own name says which slot it was made for, not which slot
        // this line picked it for. Everything else here — what the component
        // resolves to, and whether it resolves at all — is the resolved name's
        // to answer, including whether a variant has been picked, since an
        // alias may name one without saying so.
        let spec = VariantSpec::parse(raw_name.as_deref().unwrap_or(name));
        let unpicked = is_undecided(name);
        if unpicked {
            let slot_size = if op.horizontal() {
                format!("{axis_extent}x{cross_extent}")
            } else {
                format!("{cross_extent}x{axis_extent}")
            };
            // A family that draws nothing this slot could hold is not a
            // decision waiting to be made: whatever is picked, the line cannot
            // be laid out, and the drawing that would let it be does not exist
            // yet. Saying TODO there loses it — a TODO flags no glyph and is
            // hidden by default — so it is a warning, and it names what the
            // family does draw, since that is the thing to be looked at.
            let fits = |size| fits_slot(size, axis_extent, cross_extent, op.horizontal());
            match misfit_variants(name, family, &fits) {
                Some(sizes) => issues.push((
                    Severity::Warning,
                    at(format!(
                        "component '{name}' has no variant that fits a {axis_extent}-{} slot; \
                         its family draws {sizes}",
                        if op.horizontal() { "wide" } else { "tall" },
                    )),
                )),
                None => issues.push((
                    Severity::Todo,
                    at(format!(
                        "component '{name}' has no variant picked yet; a component names the \
                         sized variant it wants, as in `{name}:{slot_size}`"
                    )),
                )),
            }
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
            // The derived ref is not conditional: the line already answered
            // that question for the whole of itself above.
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

/// Turn one *enclosure* line into the `ref`s it stands for, plus what is wrong
/// with it. The enclosing half of [`expand_compose`]; the caller has already
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
fn expand_enclosure(
    walls: Walls,
    parent: (u16, u16),
    scale: u8,
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
                "has no placement picked yet; an enclosure writes the inner part's top-left                  offsets, as in `X Y 3 2`"
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
    for (slot, &(name, raw_name)) in names.iter().enumerate() {
        let outer = slot == 0;
        let spec = VariantSpec::parse(raw_name.unwrap_or(name));
        let role = if outer { "outer" } else { "inner" };
        if is_undecided(name) {
            unresolved = true;
            let fits = |size| fits_enclosure_slot(size, parent, outer);
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
        // cavity may still be the thing the author wanted inside another.
        if enclosure_rank(name, outer) == 2 {
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
            PartDims::Size(w, h) => {
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
                } else if !fits_enclosure_slot((w, h), parent, outer) {
                    issues.push((
                        Severity::Error,
                        match outer {
                            true => format!(
                                "component '{name}' is {w}x{h}, not the glyph's {}x{}: the outer \
                                 part's walls are the glyph's, so it fills the box exactly",
                                parent.0, parent.1,
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
                }
            }
        }
    }

    let (p, q) = placement.unwrap_or((0, 0));
    for (slot, &(name, raw_name)) in names.iter().enumerate() {
        let (col, row) = match slot {
            0 => (0, 0),
            _ => (p * scale.max(1) as i32, q * scale.max(1) as i32),
        };
        refs.push(GlyphRef {
            name: name.clone(),
            raw_name: raw_name.cloned(),
            offset: Some((clamp_offset(col), clamp_offset(row))),
            negated: false,
            inherit: false,
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
                && !cavity_fits(outer, walls, parent, cavity)
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
                (outer_name, outer),
                (inner_name, inner),
                (p, q),
                rule.max_contact_run,
            ) {
                issues.extend(report_clearances(
                    compose.op,
                    &clearances,
                    rule,
                ));
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
/// is nothing there for a layout to have got wrong.
///
/// As on a split, each axis's sum is a property of the parts alone — the
/// placement cancels between the axis's two clearances — which is what lets
/// [`crate::fix::clearance`] solve the offsets arithmetically instead of
/// searching them. It is **per axis**, though: the two sums are two different
/// statements and adding them together would make neither.
pub fn measure_enclosure_clearances(
    walls: Walls,
    parent: (u16, u16),
    outer: (&str, &InkProfile),
    inner: (&str, &InkProfile),
    at: (i32, i32),
    max_contact_run: Option<u16>,
) -> Option<Vec<Clearance>> {
    let (outer_name, outer) = outer;
    let (inner_name, inner) = inner;
    let mut out: Vec<Clearance> = Vec::new();
    for horizontal in [true, false] {
        let (axis_extent, pos, cross) = match horizontal {
            true => (parent.0 as i32, at.0, at.1),
            false => (parent.1 as i32, at.1, at.0),
        };
        let (wall_lo, wall_hi) = walls.along(horizontal);
        // The outer part reads its cavity-facing side; the inner part is a
        // plain drawing and reads the ends everyone can see.
        let wall = GapSide {
            profile: outer,
            inner: true,
            cross: 0,
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
                pos + inner.frontier(horizontal)?.near,
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
                axis_extent - 1 - (pos + inner.frontier(horizontal)?.far),
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
pub fn cavity_fits(
    profile: &InkProfile,
    walls: Walls,
    parent: (u16, u16),
    cavity: (u16, u16),
) -> bool {
    let (w, h) = (parent.0 as i32, parent.1 as i32);
    let (n, m) = (cavity.0 as i32, cavity.1 as i32);
    if n <= 0 || m <= 0 || n > w || m > h {
        return false;
    }
    // Per row, the columns the walls leave free. A row the drawing puts nothing
    // on is free all the way across.
    let free: Vec<(i32, i32)> = (0..h)
        .map(|row| {
            let line = profile.rows.get(row as usize).copied().flatten();
            // The wall's cavity face, chosen the way every other measurement
            // chooses it ([`WallFace`]), so that the room a name promises is
            // the room the clearances will be measured in. A side the operator
            // leaves open runs to the box's edge and reads nothing the drawing
            // does out there — a bearing at the box's rim is a claim on the
            // glyph's *neighbour*, not on what goes inside it.
            let lo = match (walls.left, line.and_then(|l| l.low_wall)) {
                (true, Some(w)) => (w.at + 1).max(0),
                (true, None) | (false, _) => 0,
            };
            let hi = match (walls.right, line.and_then(|l| l.high_wall)) {
                (true, Some(f)) => (f.at - 1).min(w - 1),
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

/// Every clearance of a placed IDC line, near edge to far edge, each with what
/// it is between; `None` when the line cannot be measured at all.
///
/// `placed` is each component and where it starts along the axis, in declared
/// units. A component with no ink to measure — see the module docs — makes the
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
    placed: &[(&str, i32)],
    ink: &InkLookup<'a>,
    max_contact_run: Option<u16>,
) -> Option<Vec<Clearance>> {
    /// A component of the line, ready to measure: where it sits along the axis
    /// and what its ink does.
    struct Placed<'a> {
        name: &'a str,
        offset: i32,
        profile: &'a InkProfile,
    }

    let horizontal = op.horizontal();
    let mut parts: Vec<Placed> = Vec::new();
    for &(name, offset) in placed {
        parts.push(Placed {
            name,
            offset,
            profile: ink(name)?,
        });
    }
    let (first, last) = (parts.first()?, parts.last()?);

    // `(what it is between, how much)`, near edge to far edge.
    let (near_edge, far_edge) = match horizontal {
        true => ("the left edge", "the right edge"),
        false => ("the top edge", "the bottom edge"),
    };
    let mut clearances: Vec<Clearance> = Vec::new();
    let near = first.profile.frontier(horizontal)?.near + first.offset;
    clearances.push(Clearance {
        between: format!("{near_edge} and '{}'", first.name),
        value: near,
        contact: None,
        horizontal,
        at_edge: true,
    });
    for pair in parts.windows(2) {
        let [a, b] = pair else { continue };
        let facing = facing_offset(
            GapSide::linear(a.profile),
            GapSide::linear(b.profile),
            horizontal,
        )?;
        // Measured only where a rule asks for it: a source stating none pays
        // nothing, exactly as it pays nothing for the profiles themselves.
        let contact =
            max_contact_run.and_then(|max| {
            contact_demand(
                GapSide::linear(a.profile),
                GapSide::linear(b.profile),
                horizontal,
                max,
            )
        });
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
    let far = axis_extent as i32 - 1 - (last.offset + last.profile.frontier(horizontal)?.far);
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
fn check_clearances(
    op: IdcOp,
    axis_extent: u16,
    placed: &[(&str, i32)],
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
/// differently ([`measure_enclosure_clearances`]) and is held to them exactly
/// the same way: one range for every gap, and one for what the axis leaves in
/// total.
fn report_clearances(
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
    let mut out: Vec<(Severity, String)> = clearances
        .iter()
        .filter(|c| !(min..=max).contains(&c.value))
        .map(|c| {
            (
                Severity::Warning,
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
            Severity::Warning,
            format!("leaves {total}{axis} in total, outside {range} — {breakdown}"),
        ));
    }
    out
}

/// The axes an operator's clearances are grouped by, each with the word a
/// message names it by. A one-dimensional line has one and names it nothing —
/// there is no other axis for it to be told apart from.
fn axes_of(op: IdcOp) -> Vec<(bool, &'static str)> {
    match op.enclosing() {
        false => vec![(op.horizontal(), "")],
        true => vec![(true, " across"), (false, " down")],
    }
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
