//! What hangs off a `glyph` block: its `ref`s, its anchors, its IDC line and
//! the [`GlyphBody`] that holds them together with the flags — `advance`,
//! `origin`, `extent` — that state the box it claims.

use super::PixelGrid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerVisibility {
    Both,
    ColorOnly,
    MonoOnly,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefFill {
    pub color: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRef {
    /// The subglyph name, with a leading `@` already expanded — this is what
    /// resolution looks up. [`written_name`](GlyphRef::written_name) is what
    /// serializing puts back.
    pub name: String,
    /// The name as written when that differs from `name`: an `@…` form, whose
    /// `@` stands for the enclosing base glyph. See [`crate::document::names::expand_at_name`].
    pub raw_name: Option<String>,
    /// `(col, row)` offset. `None` = auto-resolve from points (adjoin), defaulting to (0, 0).
    pub offset: Option<(i16, i16)>,
    pub negated: bool,
    /// Whether the composite exposes this target's surviving anchors (the ones
    /// not consumed inside the composite) as its own. Off by default: anchor
    /// inheritance is opt-in, so a digraph or a circled letter does not leak
    /// its components' attachment points. Attachment *inside* the composite
    /// (a sibling ref consuming this target's `+` anchors) works regardless.
    pub inherit: bool,
    pub fill: Option<RefFill>,
    pub visibility: Option<LayerVisibility>,
    /// Written `goto`: "go to definition" on the *enclosing* glyph lands on
    /// this target instead. Navigation only — nothing the font is built from
    /// reads it. A wrapper glyph declared as a pattern (`glyph han-($1)` over
    /// `ref ($0)`) is one line for a whole block, so a jump to any of the
    /// thousands of names it declares would otherwise always arrive at that
    /// same line; this says which ref to carry the jump on to. See
    /// [`crate::app`]'s `goto_redirect`.
    pub goto: bool,
    /// Trailing `// …` comment of the `ref` line, without its marker.
    pub comment: Option<String>,
}

impl GlyphRef {
    /// The name as written — the `@…` form when there is one, the resolved
    /// name otherwise. Serializing writes this, so a source that names its
    /// subglyph with `@` keeps saying `@`.
    #[cfg(any(feature = "editor", test))]
    pub fn written_name(&self) -> &str {
        self.raw_name.as_deref().unwrap_or(&self.name)
    }

    pub fn row(&self) -> i16 {
        self.offset.map_or(0, |(_, r)| r)
    }

    pub fn col(&self) -> i16 {
        self.offset.map_or(0, |(c, _)| c)
    }

    /// Format as a `ref …` line. When `offset_override` is `Some`, that
    /// offset is written instead of `self.offset` (and is always explicit,
    /// even for `0 0`).
    #[cfg(any(feature = "editor", test))]
    pub fn format_line(&self, offset_override: Option<(i16, i16)>) -> String {
        use crate::document_io::quote_token;
        let rname = quote_token(self.written_name());
        let mut parts = vec![format!("ref {rname}")];
        match offset_override {
            Some((c, r)) => parts.push(format!("{c} {r}")),
            None => {
                if let Some((c, r)) = self.offset {
                    parts.push(format!("{c} {r}"));
                }
            }
        }
        if self.negated {
            parts.push("negated".into());
        }
        if self.inherit {
            parts.push("inherit".into());
        }
        if self.goto {
            parts.push("goto".into());
        }
        if let Some(ref fill) = self.fill {
            parts.push(format!("fill {}", quote_token(&fill.color)));
        }
        match self.visibility {
            Some(LayerVisibility::ColorOnly) => parts.push("coloronly".into()),
            Some(LayerVisibility::MonoOnly) => parts.push("monoonly".into()),
            Some(LayerVisibility::Both) | None => {}
        }
        format!(
            "{}{}",
            parts.join(" "),
            crate::document_io::comment_suffix(&self.comment),
        )
    }
}

/// One axis of an [`AnchorAlign`]: which end of an anchor's range stands for
/// the whole of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align1 {
    /// The low end — the lowest column, or the topmost row (grid rows grow
    /// downward). Written `l` and `u`, and the default.
    #[default]
    Low,
    /// The middle. Written `c` on either axis.
    Center,
    /// The high end. Written `r` and `d`.
    High,
}

/// Where a mark sits in a slot wider (or taller) than itself.
///
/// An anchor states a *range*, and the range does two jobs: its size says which
/// drawing of a mark a base wants ([`GlyphPoint::size_matches`]), and it has to
/// become the one point GPOS attaches by. This is the reduction — the same one
/// applied to both sides of a pairing, so that the difference the shaper
/// computes means something. Aligning the low ends puts a 3-wide mark flush
/// against a 7-wide slot's left edge; centring both puts it in the middle,
/// whatever the two widths are.
///
/// It belongs to the anchor *class* and to nothing smaller. A mark carries one
/// anchor point in the `MarkArray`, shared with every base of its class, so a
/// mark reduced by one rule against bases reduced by another produces a
/// difference of no meaning. A mark that wants a different rule wants a
/// different anchor name.
///
/// The letters are [`crate::compose::Direction`]'s (`l r u d c`); unlike a 1-D
/// split, an anchor needs both axes, so a token is `[u|c|d][l|c|r]` — or a lone
/// `c` for both. The default, `ul`, is the low end of each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnchorAlign {
    pub vertical: Align1,
    pub horizontal: Align1,
}

impl AnchorAlign {
    /// Parses an `align` token. `None` for anything that is not one.
    pub fn from_token(token: &str) -> Option<Self> {
        let mut chars = token.chars();
        let (first, second) = (chars.next()?, chars.next());
        if chars.next().is_some() {
            return None;
        }
        let vertical = match first {
            'u' => Align1::Low,
            'c' => Align1::Center,
            'd' => Align1::High,
            _ => return None,
        };
        let horizontal = match second {
            None if first == 'c' => Align1::Center,
            None => return None,
            Some('l') => Align1::Low,
            Some('c') => Align1::Center,
            Some('r') => Align1::High,
            Some(_) => return None,
        };
        Some(Self {
            vertical,
            horizontal,
        })
    }

    /// The written form, or `None` for the default (which is written by
    /// leaving the `align` off).
    pub fn to_token(self) -> Option<String> {
        if self == Self::default() {
            return None;
        }
        if self.vertical == Align1::Center && self.horizontal == Align1::Center {
            return Some("c".to_string());
        }
        let v = match self.vertical {
            Align1::Low => 'u',
            Align1::Center => 'c',
            Align1::High => 'd',
        };
        let h = match self.horizontal {
            Align1::Low => 'l',
            Align1::Center => 'c',
            Align1::High => 'r',
        };
        Some(format!("{v}{h}"))
    }
}

/// The `align` every anchor class states, by class name (with no `+`/`-`
/// sign). A class no `feature` line names is absent, and reduces by the
/// default — which is what every class did before `align` existed.
///
/// The map is collected once per document set and handed to whoever pairs two
/// anchors up: the GPOS builder ([`crate::render::ttf_builder`]) and the
/// composite derivation ([`crate::ref_composite`]), which have to reduce the
/// same pair to the same two points or a precomposed glyph would sit
/// somewhere the shaped one does not.
pub type AnchorAligns = crate::hash::HashMap<String, AnchorAlign>;

/// The [`AnchorAligns`] the given items declare. Takes an item iterator rather
/// than the documents so an already-expanded item list — which carries the
/// same `feature` lines — can be asked the same question.
pub fn collect_anchor_aligns<'a>(
    items: impl Iterator<Item = &'a super::DocumentItem>,
) -> AnchorAligns {
    let mut map = AnchorAligns::default();
    for item in items {
        if let super::DocumentItem::FeatureAnchor { anchor, align, .. } = item {
            // Last naming wins, as in the GPOS builder's own map: a class
            // named twice is one class, and the two must not disagree.
            map.insert(anchor.clone(), *align);
        }
    }
    map
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphPoint {
    pub position: String,
    pub col: i16,
    pub row: i16,
    /// Inclusive end of the column range. Equal to `col` for single-cell anchors.
    pub col_end: i16,
    /// Inclusive end of the row range. Equal to `row` for single-cell anchors.
    pub row_end: i16,
    /// How many of the units above make one declared cell: the `scale` of the
    /// grid the point is written on. See [`GlyphPoint::size`] for why the
    /// point carries it rather than leaving it to the glyph.
    pub scale: u16,
    /// Trailing `// …` comment of the `anchor` line, without its marker.
    pub comment: Option<String>,
}

impl GlyphPoint {
    /// The range's width in the point's own units — fine cells of a `scale`d
    /// grid. What the editor draws; [`size`](Self::size) is what is matched.
    pub fn width(&self) -> u16 {
        (self.col_end - self.col + 1) as u16
    }

    pub fn height(&self) -> u16 {
        (self.row_end - self.row + 1) as u16
    }

    fn scale_i64(&self) -> i64 {
        i64::from(self.scale.max(1))
    }

    /// The range's `(width, height)` in **declared** cells, which is what two
    /// anchors are matched on.
    ///
    /// An anchor is written on its glyph's grid, so on a `scale N` glyph its
    /// coordinates are fine cells — but a mark and its base are routinely
    /// drawn at different scales, and a `scale 2` mark's 2×2 `-dot` is the
    /// same size as a `scale 1` base's 1×1 `+dot`. Hence the scale travels
    /// with the point instead of staying on the glyph: a composite's exposed
    /// anchors come from components of several scales, and every consumer
    /// that pairs two of them would otherwise have to know where each came
    /// from.
    ///
    /// Only the *size* is held to whole declared cells — a range that is not
    /// is an error (`issues::anchors`), rounded up here so it still means
    /// something. The position is free: placing a mark at half a cell is what
    /// a `scale 2` glyph is for.
    pub fn size(&self) -> (u16, u16) {
        let s = self.scale.max(1);
        (self.width().div_ceil(s), self.height().div_ceil(s))
    }

    /// Whether the range covers whole declared cells on both axes — the rule
    /// [`size`](Self::size) relies on.
    pub fn is_whole_size(&self) -> bool {
        let s = self.scale.max(1);
        self.width().is_multiple_of(s) && self.height().is_multiple_of(s)
    }

    #[cfg(any(feature = "editor", test))]
    pub fn is_single_cell(&self) -> bool {
        self.col == self.col_end && self.row == self.row_end
    }

    pub fn size_matches(&self, other: &GlyphPoint) -> bool {
        self.size() == other.size()
    }

    /// Whether this `+` anchor's range is big enough to hold a `-` of
    /// `(width, height)`. The one rule both attachment paths ask — GPOS's
    /// slot-to-mark fit and the composite derivation's — so that a mark a
    /// shaped run puts in a slot is a mark a precomposed glyph puts there
    /// too. An exact size is the case both prefer; a larger slot still holds
    /// the mark, which then reduces by the class's [`AnchorAlign`].
    /// Both are in declared cells, as [`size`](Self::size) gives them.
    pub fn holds_size(&self, (width, height): (u16, u16)) -> bool {
        let (w, h) = self.size();
        w >= width && h >= height
    }

    /// [`Self::holds_size`] against another anchor's range.
    pub fn holds(&self, mark: &GlyphPoint) -> bool {
        self.holds_size(mark.size())
    }

    /// The `(col, row)` this anchor's range stands for under `align`, in
    /// declared cells. Half-integral where a range of even size is centred,
    /// which is exact in font units and cancels against the other side of the
    /// pairing whenever the two ranges are the same size — see
    /// [`AnchorAlign`], and `issues::anchors` for the parity a centred class
    /// is held to. Any fraction at all on a `scale`d glyph.
    pub fn aligned_point(&self, align: AnchorAlign) -> (f32, f32) {
        let (col, row) = self.aligned_halves(align);
        let den = (2 * self.scale_i64()) as f32;
        (col as f32 / den, row as f32 / den)
    }

    /// [`aligned_point`](Self::aligned_point) exactly: numerators over
    /// `2 * scale`.
    ///
    /// The reduction is of the *declared* cell the range stands for, not of
    /// its fine cells: the high end of `14..15` on a `scale 2` glyph is
    /// declared cell 7, as `7..7` is on a `scale 1` one, where reading the last
    /// fine cell would give 7½. So the far edge is taken, and one declared
    /// cell is stepped back from it.
    pub fn aligned_halves(&self, align: AnchorAlign) -> (i64, i64) {
        let s = self.scale_i64();
        let reduce = |low: i16, high: i16, axis: Align1| {
            let (low, far) = (i64::from(low), i64::from(high) + 1);
            match axis {
                Align1::Low => 2 * low,
                Align1::Center => low + far - s,
                Align1::High => 2 * far - 2 * s,
            }
        };
        (
            reduce(self.col, self.col_end, align.horizontal),
            reduce(self.row, self.row_end, align.vertical),
        )
    }

    /// This point placed at `(col, row)` cells of a grid at `scale`, in the
    /// coarsest units that still hold it exactly.
    ///
    /// The units are the least common multiple of the two scales, then
    /// reduced: a `scale 2` component's anchor placed in a `scale 1` composite
    /// stays in half cells only if it actually falls on one.
    ///
    /// Saturates where the coordinates would overflow, as the `i16`s do.
    pub fn translated(&self, col: i16, row: i16, scale: u8) -> GlyphPoint {
        let (own, at) = (self.scale_i64(), i64::from(scale.max(1)));
        let unit = own / crate::math::gcd_u64(own as u64, at as u64) as i64 * at;
        let (k, m) = (unit / own, unit / at);
        // Edges, not cells: the far edge is what scales.
        let mut edges = [
            i64::from(self.col) * k + i64::from(col) * m,
            (i64::from(self.col_end) + 1) * k + i64::from(col) * m,
            i64::from(self.row) * k + i64::from(row) * m,
            (i64::from(self.row_end) + 1) * k + i64::from(row) * m,
        ];
        let g = edges.iter().fold(unit as u64, |g, &e| {
            crate::math::gcd_u64(g, e.unsigned_abs())
        }) as i64;
        for e in &mut edges {
            *e /= g;
        }
        let narrow = |v: i64| v.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        GlyphPoint {
            position: self.position.clone(),
            col: narrow(edges[0]),
            col_end: narrow(edges[1] - 1),
            row: narrow(edges[2]),
            row_end: narrow(edges[3] - 1),
            scale: u16::try_from(unit / g).unwrap_or(u16::MAX),
            comment: None,
        }
    }

    /// The `anchor` line for this point, comment included. Single implementation
    /// shared by the serializer and by layer dragging in the editor.
    #[cfg(any(feature = "editor", test))]
    pub fn format_line(&self) -> String {
        let range = |start: i16, end: i16| {
            if start == end {
                format!("{start}")
            } else {
                format!("{start}..{end}")
            }
        };
        format!(
            "anchor {} {} {}{}",
            crate::document_io::quote_token(&self.position),
            range(self.col, self.col_end),
            range(self.row, self.row_end),
            crate::document_io::comment_suffix(&self.comment),
        )
    }

    /// This point in cells of a grid at `scale`, widened outward to whole
    /// cells where it does not fall on them. For drawing a point on a grid it
    /// was not written on — an anchor a composite inherits from a component
    /// of another scale — and for nothing that matches or places anchors,
    /// which [`size`](Self::size) and [`aligned_halves`](Self::aligned_halves)
    /// do exactly.
    #[cfg(feature = "editor")]
    pub fn on_grid(&self, scale: u8) -> GlyphPoint {
        let (own, at) = (self.scale_i64(), i64::from(scale.max(1)));
        if own == at {
            return self.clone();
        }
        let narrow = |v: i64| v.clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
        let low = |v: i16| narrow((i64::from(v) * at).div_euclid(own));
        let high = |v: i16| narrow(((i64::from(v) + 1) * at + own - 1).div_euclid(own) - 1);
        GlyphPoint {
            col: low(self.col),
            col_end: high(self.col_end),
            row: low(self.row),
            row_end: high(self.row_end),
            scale: at as u16,
            ..self.clone()
        }
    }

    /// A copy of this point moved by `(dcol, drow)` whole cells.
    #[cfg(feature = "editor")]
    pub fn shifted(&self, dcol: i16, drow: i16) -> GlyphPoint {
        GlyphPoint {
            col: self.col + dcol,
            col_end: self.col_end + dcol,
            row: self.row + drow,
            row_end: self.row_end + drow,
            ..self.clone()
        }
    }
}

/// One item of an IDC line: a component, or a number beside it.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposeItem {
    /// A number written between (or around) the components. What it says is the
    /// operator's to answer and [`crate::compose`] is where that is written
    /// down: on a **split** it is a gap — how much room is left at that
    /// position along the axis, negative being an overlap — and on an
    /// **enclosure** the two trailing numbers are the inner component's
    /// top-left offsets inside the box. The parser keeps the token where it was
    /// written and asks no further, so one item type covers both.
    Gap(i16),
    /// A component, in written order. `raw_name` holds the name as *written*
    /// whenever that differs from `name` — the `@…` form, as
    /// [`GlyphRef::raw_name`] does, and the pre-alias name once the build has
    /// canonicalized this one. A component's name is a claim about which slot
    /// the author picked ([`crate::compose`]'s variant name rule), so the
    /// written form has to survive the canonicalization that points `name` at
    /// the drawing.
    Part {
        name: String,
        raw_name: Option<String>,
    },
    /// `1|foo|1|bar|1`: a slot of a split filled by a *nested* split across it,
    /// its gaps and components written in one token with `|` between them.
    /// Only [`Gap`](Self::Gap)s and [`Part`](Self::Part)s are inside — a
    /// nested split does not nest again — and only a split writes one; what it
    /// stands for is [`crate::compose`]'s (`# Nested splits`).
    Nested(Vec<ComposeItem>),
}

impl ComposeItem {
    /// The components this item names, in written order: itself, or a
    /// nested split's members.
    pub fn parts(&self) -> impl Iterator<Item = (&String, Option<&String>)> {
        let items: &[ComposeItem] = match self {
            Self::Nested(items) => items,
            one => std::slice::from_ref(one),
        };
        items.iter().filter_map(|it| match it {
            Self::Part { name, raw_name } => Some((name, raw_name.as_ref())),
            _ => None,
        })
    }

    /// [`Self::parts`], mutably.
    pub fn parts_mut(&mut self) -> impl Iterator<Item = (&mut String, &mut Option<String>)> {
        let items: &mut [ComposeItem] = match self {
            Self::Nested(items) => items,
            one => std::slice::from_mut(one),
        };
        items.iter_mut().filter_map(|it| match it {
            Self::Part { name, raw_name } => Some((name, raw_name)),
            _ => None,
        })
    }

    /// The token a nested split is written as, with each component under its
    /// resolved `name` when `resolved` and as written otherwise. The resolved
    /// form doubles as the name the nested split's ink is kept under
    /// ([`crate::compose::nested_key`]).
    pub fn nested_token(items: &[ComposeItem], resolved: bool) -> String {
        items
            .iter()
            .map(|it| match it {
                Self::Gap(g) => g.to_string(),
                Self::Part { name, raw_name } => match resolved {
                    true => name.clone(),
                    false => raw_name.clone().unwrap_or_else(|| name.clone()),
                },
                Self::Nested(inner) => Self::nested_token(inner, resolved),
            })
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// An IDC line: the glyph's box split along one axis (`⿰⿱⿲⿳`) or filled by one
/// component with the other seated in the cavity it leaves (`⿴⿵⿶⿷⿸⿹⿺⿼⿽`).
///
/// A sibling of `ref` inside a glyph block, not sugar for one — see
/// [`crate::compose`] for what each operator means, which sizes it reads, and
/// the `ref`s it derives.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphCompose {
    pub op: crate::compose::IdcOp,
    pub items: Vec<ComposeItem>,
    /// Written `assume ⿰ …`: the layout is the author's, and what the audit
    /// would say about it is not asked. See [`crate::compose`].
    pub assumed: bool,
    /// Trailing `// …` comment of the line, without its marker.
    pub comment: Option<String>,
}

impl GlyphCompose {
    /// The component names, in written order, a nested split's members included.
    pub fn part_names(&self) -> impl Iterator<Item = &str> {
        self.items
            .iter()
            .flat_map(|it| it.parts().map(|(name, _)| name.as_str()))
    }

    /// Every component, a nested split's members included, for a pass that rewrites
    /// names: `(name, raw_name)`.
    pub fn parts_mut(&mut self) -> impl Iterator<Item = (&mut String, &mut Option<String>)> {
        self.items.iter_mut().flat_map(ComposeItem::parts_mut)
    }

    /// How many slots the line fills: its components, a nested split counting once.
    /// What an operator's arity is checked against.
    pub fn slot_count(&self) -> usize {
        self.items
            .iter()
            .filter(|it| !matches!(it, ComposeItem::Gap(_)))
            .count()
    }

    /// Format as an IDC line, the way [`GlyphRef::format_line`] formats a `ref`.
    // Not editor-gated like its `ref` counterpart: `uniform fix` rewrites IDC
    // lines and is a headless command.
    pub fn format_line(&self) -> String {
        use crate::document_io::quote_token;
        let mut parts = Vec::with_capacity(self.items.len() + 2);
        if self.assumed {
            parts.push(crate::compose::ASSUME.to_string());
        }
        parts.push(self.op.as_char().to_string());
        for item in &self.items {
            parts.push(match item {
                ComposeItem::Gap(g) => g.to_string(),
                ComposeItem::Part { name, raw_name } => {
                    quote_token(raw_name.as_deref().unwrap_or(name))
                }
                ComposeItem::Nested(items) => quote_token(&ComposeItem::nested_token(items, false)),
            });
        }
        format!(
            "{}{}",
            parts.join(" "),
            crate::document_io::comment_suffix(&self.comment),
        )
    }
}

/// `margin-x L|R` and `margin-y T|B`: the room a part asks for around its box
/// when an IDC line gives it a slot larger than the box across the split.
///
/// A part is drawn tight, its ink running to the edges of its box, because a
/// part is placed in slots of many sizes and a margin drawn into it is right
/// for one of them. What most of those slots want is still one answer — 口 in
/// the top of a full-width `⿱` sits two cells in from either side — and this
/// is that answer, stated once on the part instead of as a
/// [nested split](crate::compose#nested-splits) at every use. See
/// [`crate::compose`] (`# A part's margin`) for when the line reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Margin {
    /// Left and right.
    pub x: Option<(u16, u16)>,
    /// Top and bottom.
    pub y: Option<(u16, u16)>,
}

impl Margin {
    /// The margin across a slot of a split along `horizontal`: the one on the
    /// axis the parent's box pins, which is the height of a `⿰` part and the
    /// width of a `⿱` one.
    pub fn across(self, horizontal: bool) -> Option<(u16, u16)> {
        match horizontal {
            true => self.y,
            false => self.x,
        }
    }

    /// The header flag one axis's margin is written as, `margin-x 2` or
    /// `margin-y 1|3`: one value when both sides agree.
    pub fn flag(x_axis: bool, (lo, hi): (u16, u16)) -> String {
        let axis = if x_axis { 'x' } else { 'y' };
        match lo == hi {
            true => format!("margin-{axis} {lo}"),
            false => format!("margin-{axis} {lo}|{hi}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBody {
    pub pixels: Option<PixelGrid>,
    pub refs: Vec<GlyphRef>,
    /// IDC lines, in written order. More than one is an error
    /// ([`crate::compose`]); the parser keeps them all so serializing puts the
    /// file back as it was.
    pub compose: Vec<GlyphCompose>,
    pub points: Vec<GlyphPoint>,
    pub keep: bool,
    pub inline: bool,
    pub mark: bool,
    /// `desync`: the pixel grid is bitmap ink only. The vector build of the
    /// font ignores its geometry and resolves the glyph from its `ref`s alone,
    /// so the two faces can describe different shapes; see
    /// [`crate::render::ttf_builder`].
    pub desync: bool,
    /// `vectoronly`: this drawing is not meant to be rendered as pixels, so
    /// the bitmap build is to draw it exactly as the vector build does rather
    /// than squaring it off. The mirror of [`desync`](Self::desync), which
    /// keeps a grid *out* of the vector build: this keeps the vector geometry
    /// *in* the bitmap one. Flag artwork and the like, where the squared-off
    /// form is unreadable and paying for it is pointless.
    ///
    /// It is the one glyph flag whose effect reaches past the glyph it is
    /// written on: see [`crate::render::ttf_builder`] for why the exemption is
    /// a closure over the `ref` graph, and [`crate::issues`] for what that
    /// costs a component shared with an unflagged glyph.
    pub vectoronly: bool,
    /// Which of a glyph's layers the [`vectoronly`](Self::vectoronly) above
    /// covers. `None` — everything a source writes — is the whole drawing.
    ///
    /// A synthesized color/mono glyph is the one exception: it carries *two*
    /// drawings, one per half, and the flag was written on one of them. The
    /// closure over the `ref` graph must not cross from the half that asked
    /// into the half that did not, or a flagged colour drawing would exempt
    /// every component the mono fallback is built from. Each half is still an
    /// item of its own, and so is its own root of that closure, so nothing is
    /// lost by stopping the merged glyph at its own drawing.
    pub vectoronly_layers: Option<LayerVisibility>,
    /// `advance W`: the declared box's width, with its height left to the
    /// grid. The common half of [`extent`](Self::extent), and the one a source
    /// states on its own — see [`GlyphBody::declared_extent`].
    pub advance: Option<u16>,
    /// `origin C R`: where the declared box's top-left corner sits, in the
    /// glyph's own logical cells. See [`GlyphBody::declared_origin`], which is
    /// what everything downstream reads, and what the exported side bearings
    /// are the negation of.
    pub origin: Option<(i16, i16)>,
    /// `extent W H`: the size of the declared box, measured from
    /// [`origin`](Self::origin), for the glyph whose height is not its grid's
    /// either. Either component may be zero. A glyph that only means to take no
    /// width writes [`advance 0`](Self::advance) instead and keeps the grid's
    /// height; writing both is an error. See [`GlyphBody::declared_extent`].
    pub extent: Option<(u16, u16)>,
    pub scale: u8,
    /// `margin-x` / `margin-y`: see [`Margin`].
    pub margin: Margin,
    /// The header's name as written when that differs from the
    /// [`crate::document::names::GlyphName`] the item carries: an `@…` form. Like `comment`, this is
    /// header data the body holds so serializing the block puts the line back
    /// as it was. See [`crate::document::names::expand_at_name`].
    pub raw_name: Option<String>,
    /// Trailing `// …` comment of the `glyph` header line, without its marker.
    pub comment: Option<String>,
}

impl GlyphBody {
    /// Where the declared box's top-left corner sits, in this glyph's own
    /// logical cells: `(col, row)`, the same order a `ref` offset is written
    /// in. `(0, 0)` — the grid's own corner — when nothing says otherwise.
    ///
    /// A `ref` offset is measured to the child's corner, so declaring an origin
    /// moves everything the glyph places and everything that places it; see
    /// [`crate::ref_composite::rebase_offsets_to_box`].
    pub fn declared_origin(&self) -> (i16, i16) {
        self.origin.unwrap_or((0, 0))
    }

    /// The box's width as the source *stated* it, in logical cells, or `None`
    /// when it stated none.
    ///
    /// The narrower question [`declared_extent`](Self::declared_extent) answers
    /// with the grid: this one keeps the difference, because an unstated width
    /// is not the grid's but the *resolved* one — a glyph whose refs reach past
    /// its own grid has always advanced by what it draws. Everything that
    /// exports or draws an advance goes through here so the editor's overlay
    /// and the font's `hmtx` cannot disagree.
    pub fn stated_advance(&self) -> Option<u16> {
        match (self.advance, self.extent) {
            (Some(advance), _) => Some(advance),
            (None, Some((w, _))) => Some(w),
            (None, None) => None,
        }
    }

    /// The declared box's size in logical cells, or `None` for a glyph that
    /// declares neither an extent nor a grid to take one from (a composite,
    /// whose box is the union of what it places — see
    /// [`crate::ref_composite`]).
    ///
    /// Three ways in, narrowing: `extent` states both numbers, `advance` states
    /// the width and takes the height from the grid, and a bare grid states
    /// both by being the box. That middle one is not a lesser spelling of the
    /// first — it is what a source writes when only the width is unusual, which
    /// is nearly always — so the two are separate flags and stating both is an
    /// error.
    ///
    /// Zero is a real answer, not a missing one: `advance 0` is how a combining
    /// mark says it takes no width.
    ///
    /// A glyph with `advance` and no grid still answers `None`: the width alone
    /// is not a box, and the height it would need is exactly what a gridless
    /// composite has to be measured for.
    ///
    /// What the grid states is the box's **far edge**, not its size: an
    /// [`origin`](Self::origin) has already moved the near one, and the two
    /// meet at the raster's own corner. So `glyph foo 6 16 origin 1 0` claims
    /// five cells and gives its first column away as a bearing, and a negative
    /// origin — a bearing the other way — makes the box wider than the grid.
    /// Sizing the box by the grid instead would ignore the origin outright and
    /// so be wrong for nearly every glyph that states one.
    pub fn declared_extent(&self) -> Option<(u16, u16)> {
        if let Some(extent) = self.extent {
            return Some(extent);
        }
        let grid = self.pixels.as_ref();
        let s = self.scale.max(1) as u16;
        let (origin_c, origin_r) = self.declared_origin();
        // Floor: a grid that is not a whole number of declared cells has no
        // last cell to speak of. An origin past the far edge leaves nothing to
        // claim, rather than wrapping.
        let from_origin = |extent: u16, origin: i16| {
            (extent as i32 - origin as i32).clamp(0, u16::MAX as i32) as u16
        };
        let height = grid.map(|g| from_origin(g.height / s, origin_r));
        match (
            self.advance,
            grid.map(|g| from_origin(g.width / s, origin_c)),
            height,
        ) {
            (Some(advance), _, Some(h)) => Some((advance, h)),
            (None, Some(w), Some(h)) => Some((w, h)),
            _ => None,
        }
    }

    /// Whether a `vectoronly` exemption scoped to `layers` (see
    /// [`vectoronly_layers`](Self::vectoronly_layers)) reaches through one
    /// `ref`. A scope of `None` — every glyph a source writes — reaches
    /// through all of them.
    pub fn vectoronly_covers(layers: Option<LayerVisibility>, r: &GlyphRef) -> bool {
        match (layers, r.visibility) {
            (None, _) | (_, None) | (_, Some(LayerVisibility::Both)) => true,
            (Some(scope), Some(vis)) => scope == vis,
        }
    }

    pub fn new() -> Self {
        Self {
            pixels: None,
            refs: Vec::new(),
            compose: Vec::new(),
            points: Vec::new(),
            keep: false,
            inline: false,
            mark: false,
            desync: false,
            vectoronly: false,
            vectoronly_layers: None,
            advance: None,
            origin: None,
            extent: None,
            scale: 1,
            margin: Margin::default(),
            raw_name: None,
            comment: None,
        }
    }
}
