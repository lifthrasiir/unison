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
    /// `@` stands for the enclosing base glyph. See [`expand_at_name`].
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

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphPoint {
    pub position: String,
    pub col: i16,
    pub row: i16,
    /// Inclusive end of the column range. Equal to `col` for single-cell anchors.
    pub col_end: i16,
    /// Inclusive end of the row range. Equal to `row` for single-cell anchors.
    pub row_end: i16,
    /// Trailing `// …` comment of the `anchor` line, without its marker.
    pub comment: Option<String>,
}

impl GlyphPoint {
    pub fn width(&self) -> u16 {
        (self.col_end - self.col + 1) as u16
    }

    pub fn height(&self) -> u16 {
        (self.row_end - self.row + 1) as u16
    }

    #[cfg(any(feature = "editor", test))]
    pub fn is_single_cell(&self) -> bool {
        self.col == self.col_end && self.row == self.row_end
    }

    pub fn size_matches(&self, other: &GlyphPoint) -> bool {
        self.width() == other.width() && self.height() == other.height()
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

/// One item of an IDC line: a component, or the gap in front of it.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposeItem {
    /// A number written between (or around) the components: how much room is
    /// left at that position along the split axis. Negative is an overlap.
    Gap(i16),
    /// A component, in written order. `raw_name` holds the `@…` form when the
    /// name was written with one, exactly as [`GlyphRef::raw_name`] does.
    Part {
        name: String,
        raw_name: Option<String>,
    },
}

/// A `⿰`/`⿱`/`⿲`/`⿳` line: the glyph's box split along one axis.
///
/// A sibling of `ref` inside a glyph block, not sugar for one — see
/// [`crate::compose`] for what the split means, which sizes it reads, and the
/// `ref`s it derives.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphCompose {
    pub op: crate::compose::IdcOp,
    pub items: Vec<ComposeItem>,
    /// Trailing `// …` comment of the line, without its marker.
    pub comment: Option<String>,
}

impl GlyphCompose {
    /// The component names, in written order.
    pub fn part_names(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(|it| match it {
            ComposeItem::Part { name, .. } => Some(name.as_str()),
            ComposeItem::Gap(_) => None,
        })
    }

    /// Format as an IDC line, the way [`GlyphRef::format_line`] formats a `ref`.
    // Not editor-gated like its `ref` counterpart: `uniform fix` rewrites IDC
    // lines and is a headless command.
    pub fn format_line(&self) -> String {
        use crate::document_io::quote_token;
        let mut parts = vec![self.op.as_char().to_string()];
        for item in &self.items {
            parts.push(match item {
                ComposeItem::Gap(g) => g.to_string(),
                ComposeItem::Part { name, raw_name } => {
                    quote_token(raw_name.as_deref().unwrap_or(name))
                }
            });
        }
        format!(
            "{}{}",
            parts.join(" "),
            crate::document_io::comment_suffix(&self.comment),
        )
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
    /// The header's name as written when that differs from the
    /// [`GlyphName`] the item carries: an `@…` form. Like `comment`, this is
    /// header data the body holds so serializing the block puts the line back
    /// as it was. See [`expand_at_name`].
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
            advance: None,
            origin: None,
            extent: None,
            scale: 1,
            raw_name: None,
            comment: None,
        }
    }
}
