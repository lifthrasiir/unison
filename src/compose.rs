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
//! `ifexists` — the line's last token, and only ever the last — says the
//! components may or may not be there. See [`stands_for_nothing`] for what a
//! line missing one stands for, which is nothing.
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
//! PREFIX* MIN MAX` — holds each of them, *and* their total, to one range; a
//! violation is a warning ([`check_clearances`]). Both halves are needed, and
//! the reason is arithmetic: the total telescopes down to the parent's extent
//! less the parts' ink extents, so it does not depend on the gaps at all. A
//! source that only had to satisfy the total could never fix a failing line by
//! moving anything — the per-part bound is what makes the check something an
//! author can act on, and the total is what catches parts that are simply too
//! fat for the box together, however they are shuffled.
//!
//! A part is measured over its declared box, but not *bounded* by it along the
//! split axis: what it draws outside is read where it is drawn. That is how a
//! part writes a side bearing — the box is the cells it fills and a hardblank
//! beyond it is the space it wants its neighbour to leave — and two parts that
//! each claim a column and are placed box to box overlap by exactly what they
//! claim ([`InkProfile::of`]).
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
    pub fn of(grid: &PixelGrid, scale: u8, origin: (i16, i16), extent: (u16, u16)) -> Self {
        let s = scale.max(1) as u16;
        let (w, h) = extent;
        // Per declared cell: nothing / hardblank only / ink, whichever is
        // greatest over the sub-cells, so any ink makes the cell ink.
        const CLEAR: u8 = 0;
        const HARDBLANK: u8 = 1;
        const INK: u8 = 2;
        // The box coordinates the grid reaches on each axis, box and grid
        // together, so a cell outside the box still has a place to be counted.
        let span = |extent: u16, len: u16, origin: i16| -> (i32, i32) {
            if extent == 0 || len == 0 {
                return (0, extent as i32);
            }
            let (lo, hi) = (-(origin as i32), ((len - 1) / s) as i32 - origin as i32);
            (lo.min(0), hi.max(extent as i32 - 1) + 1)
        };
        let (col_lo, col_hi) = span(w, grid.width, origin.0);
        let (row_lo, row_hi) = span(h, grid.height, origin.1);
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
                    let box_r = (row / s) as i32 - origin.1 as i32;
                    let box_c = (col / s) as i32 - origin.0 as i32;
                    let folded_r = box_r.clamp(0, h as i32 - 1) as usize;
                    let folded_c = box_c.clamp(0, w as i32 - 1) as usize;
                    let at = &mut by_row[folded_r * cols_wide + (box_c - col_lo) as usize];
                    *at = (*at).max(level);
                    let at = &mut by_col[folded_c * rows_tall + (box_r - row_lo) as usize];
                    *at = (*at).max(level);
                }
            }
        }
        let scan = |lo: i32, len: usize, at: &dyn Fn(usize) -> u8| -> Option<InkLine> {
            let near = (0..len).find(|&i| at(i) != CLEAR)?;
            let far = (near..len).rev().find(|&i| at(i) != CLEAR)?;
            Some(InkLine {
                near: lo + near as i32,
                far: lo + far as i32,
                near_hardblanks: (near..=far).take_while(|&i| at(i) == HARDBLANK).count() as u16,
                far_hardblanks: (near..=far)
                    .rev()
                    .take_while(|&i| at(i) == HARDBLANK)
                    .count() as u16,
            })
        };
        Self {
            rows: (0..h as usize)
                .map(|r| scan(col_lo, cols_wide, &|c| by_row[r * cols_wide + c]))
                .collect(),
            cols: (0..w as usize)
                .map(|c| scan(row_lo, rows_tall, &|r| by_col[c * rows_tall + r]))
                .collect(),
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
pub fn facing_offset(a: &InkProfile, b: &InkProfile, horizontal: bool) -> Option<i32> {
    a.along(horizontal)
        .iter()
        .zip(b.along(horizontal).iter())
        .filter_map(|(x, y)| match (x, y) {
            (Some(a), Some(b)) => {
                let shared = a.far_hardblanks.min(b.near_hardblanks) as i32;
                Some(b.near - a.far - 1 + shared)
            }
            _ => None,
        })
        .min()
}

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
    pub min: i16,
    pub max: i16,
    pub ink: &'a InkLookup<'a>,
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

/// Whether an `ifexists` line stands for nothing, because one of the components
/// it names is not a glyph the sources declare.
///
/// `ifexists` on an IDC line holds for the line and not for a component, and
/// the reason is that a split is one shape: a ⿰ whose right half is missing is
/// not a left half, it is a glyph nobody drew. So the line derives no refs at
/// all — and reports nothing, which is what the flag is for.
///
/// "Exists" is asked of [`PartDims`] rather than of the wider existence test a
/// `ref` uses ([`glyph_name_exists`](crate::render::ttf_builder::expand)),
/// because a component needs more than a name: it fills a slot with a box, and
/// only a `glyph` header states one. A name the font would generate on demand
/// is no more usable as a component with the flag than without it.
pub fn stands_for_nothing(compose: &GlyphCompose, dims: &dyn Fn(&str) -> PartDims) -> bool {
    compose.if_exists
        && compose
            .part_names()
            .any(|name| matches!(dims(name), PartDims::Unknown))
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
    // Before anything is said about the line: an `ifexists` line whose parts
    // are not all there is not a line with a problem, it is not a line.
    if stands_for_nothing(compose, dims) {
        return (Vec::new(), Vec::new());
    }
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
            // The derived ref is not conditional: the line already answered
            // that question for the whole of itself above.
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
) -> Option<Vec<(String, i32)>> {
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
    let mut clearances: Vec<(String, i32)> = Vec::new();
    let near = first.profile.frontier(horizontal)?.near + first.offset;
    clearances.push((format!("{near_edge} and '{}'", first.name), near));
    for pair in parts.windows(2) {
        let [a, b] = pair else { continue };
        let facing = facing_offset(a.profile, b.profile, horizontal)?;
        clearances.push((
            format!("'{}' and '{}'", a.name, b.name),
            (b.offset - a.offset) + facing,
        ));
    }
    let far = axis_extent as i32 - 1 - (last.offset + last.profile.frontier(horizontal)?.far);
    clearances.push((format!("'{}' and {far_edge}", last.name), far));
    Some(clearances)
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
    let Some(clearances) = measure_clearances(op, axis_extent, placed, rule.ink) else {
        return Vec::new();
    };

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
