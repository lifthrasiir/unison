//! Geometry of the document view: pixel-grid extents and strips, and the
//! visual-line model the frame loop lays text out with.

use super::*;

#[derive(Clone, Copy)]
pub(crate) struct GridExtent {
    pub(crate) top: i16,
    pub(crate) left: i16,
    pub(crate) bottom: i16,
    pub(crate) right: i16,
}

impl GridExtent {
    pub(crate) fn own_area(width: u16, height: u16) -> Self {
        Self {
            top: 0,
            left: 0,
            bottom: height as i16,
            right: width as i16,
        }
    }

    pub(crate) fn display_width(&self, grid_cell: f32) -> f32 {
        (self.right - self.left) as f32 * grid_cell
    }

    /// Widen the drawn area so the whole metric box fits in it. A mark glyph
    /// is where this bites: `dia-below` is two rows of ink, but its em box
    /// reaches fourteen rows above them, and the box is the only thing that
    /// says where on the line those two rows land.
    pub(crate) fn include_metrics(&mut self, m: &GlyphMetrics) {
        self.top = self.top.min(m.top);
        self.left = self.left.min(m.left);
        self.bottom = self.bottom.max(m.bottom);
        self.right = self.right.max(m.right);
        // The baseline is normally well inside the box, but a `top` that pushes
        // the ink up can put it below `bottom`; drawn outside the extent it
        // would simply be clipped away.
        if let Some(baseline) = m.baseline {
            self.top = self.top.min(baseline);
            self.bottom = self.bottom.max(baseline);
        }
    }

    /// Widen the drawn area so the whole anchor shadow fits in it. Same reason
    /// as [`GridExtent::include_metrics`], and the same glyph is where it
    /// bites: a two-row mark shows the bases it attaches to only if the rows
    /// they occupy are drawn at all.
    pub(crate) fn include_shadow(&mut self, s: &AnchorShadow) {
        self.top = self.top.min(s.row);
        self.left = self.left.min(s.col);
        self.bottom = self.bottom.max(s.row + s.grid.height as i16);
        self.right = self.right.max(s.col + s.grid.width as i16);
    }
}

/// A glyph's metric box in grid coordinates — the em box as `left`, `top` and
/// `advance` place it relative to the drawn pixels.
///
/// `left`/`top` move the *ink*, not the box: `left -3` shifts the outline three
/// columns left of the origin (`collect.rs::scale_glyph_contours`), so in the
/// grid the origin sits three columns right of column 0. The box is therefore
/// at `-left` / `-top`, and `bottom` follows from `meta height` — which is
/// why it is computed and never written in a glyph header.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct GlyphMetrics {
    pub(crate) left: i16,
    pub(crate) right: i16,
    pub(crate) top: i16,
    pub(crate) bottom: i16,
    /// Baseline row, present only for a glyph tall enough to reach it (see
    /// [`glyph_metrics`]).
    pub(crate) baseline: Option<i16>,
}

/// The metric box of one glyph body. `own_w`/`own_h` are what
/// [`compute_grid_display_extent`] reports for it.
///
/// **Units.** The grid is in subcells for a `scale N` glyph — `document_io`
/// multiplies the declared dimensions by the scale — but `left`, `top`,
/// `advance` and everything out of `meta` are logical pixels, exactly as
/// `ttf_builder::collect` reads them. Everything from the latter group is
/// scaled here; `own_w`/`own_h` and the composite's extent already are.
pub(crate) fn glyph_metrics(
    body: &GlyphBody,
    composite: Option<&GlyphComposite>,
    own_w: u16,
    own_h: u16,
    meta: crate::meta::FontMetrics,
) -> GlyphMetrics {
    let s = body.scale.max(1) as i16;
    // The advance falls back to the resolved extent *right of the origin*;
    // area a negative ref offset reaches is a bearing and does not count.
    let (resolved_w, resolved_h) = match composite {
        Some(comp) => (
            (comp.width as i16 - comp.own_offset_col).max(own_w as i16),
            (comp.height as i16 - comp.own_offset_row).max(own_h as i16),
        ),
        None => (own_w as i16, own_h as i16),
    };
    let left = -body.left.unwrap_or(0) * s;
    let top = -body.top.unwrap_or(0) * s;
    let ascent = meta.ascent() as i16 * s;
    let em = ascent + meta.descent() as i16 * s;
    GlyphMetrics {
        left,
        right: left + body.advance.map_or(resolved_w, |a| a as i16 * s),
        top,
        // Clamped both ways. The em box is the *upper* bound, but a glyph
        // shorter than it has no cell below its own last row either, and
        // padding one out to the full em height showed a one-row glyph as
        // sixteen rows of grid.
        bottom: resolved_h.min(top + em).max(top),
        // Drawn wherever there is room for both lines, whatever the font maps.
        // A glyph is normally drawn before it is mapped — and a `flags` glyph
        // is reached through its own `:mono`/`:color` variants and never
        // mapped at all — so metrics that wait for a `map` are metrics you
        // cannot design against. Room means the glyph clears the ascent; the
        // rows below it are the descent.
        baseline: (resolved_h > ascent).then_some(top + ascent),
    }
}

/// The horizontal band the glyph grids are drawn in. It spans the editor's
/// full width, except while a glyph is being edited — then it stops short of
/// the right edge to leave the inline tool panel its room. A grid wider than
/// the band is clipped to it and scrolled with `scroll`.
#[derive(Clone, Debug)]
pub(crate) struct GridStrip {
    /// Left edge in screen coordinates.
    pub(crate) x: f32,
    /// Width of the visible band.
    pub(crate) w: f32,
    /// Shared scroll offset, `>= 0`. Applied to a grid only as far as that
    /// grid actually overflows, so narrower grids are unaffected.
    pub(crate) scroll: f32,
    /// Horizontal scrollbars drawn over the band. A pointer inside one of
    /// them drives the scrollbar, not the grid underneath.
    pub(crate) bars: Vec<egui::Rect>,
    /// Set while a scrollbar drag is in flight. The drag keeps following the
    /// pointer after it leaves the bar, so the grid must ignore it for the
    /// whole gesture, not just while it is over the bar.
    pub(crate) captured: bool,
}

impl GridStrip {
    pub(crate) fn right(&self) -> f32 {
        self.x + self.w
    }

    /// How far a grid of `content_w` extends past the band.
    pub(crate) fn overflow(&self, content_w: f32) -> f32 {
        (content_w - self.w).max(0.0)
    }

    /// Screen x of column `extent.left` for a grid of `content_w`.
    pub(crate) fn grid_x(&self, content_w: f32) -> f32 {
        self.x - self.scroll.min(self.overflow(content_w))
    }

    pub(crate) fn contains_x(&self, x: f32) -> bool {
        x >= self.x && x < self.right()
    }

    /// Whether a pointer position should be routed to the grid: inside the
    /// band and not on top of a scrollbar.
    pub(crate) fn accepts_pointer(&self, p: egui::Pos2) -> bool {
        !self.captured && self.contains_x(p.x) && !self.bars.iter().any(|r| r.contains(p))
    }

    /// Clip a grid-relative span to the visible band.
    pub(super) fn clip_span(&self, x0: f32, x1: f32) -> Option<(f32, f32)> {
        let lo = x0.max(self.x);
        let hi = x1.min(self.right());
        (lo < hi).then_some((lo, hi))
    }
}

/// The part of a grid block that is actually visible inside the band, or
/// `None` when the band has scrolled past it entirely.
pub(super) fn visible_grid_rect(
    strip: &GridStrip,
    grid_x: f32,
    grid_y: f32,
    content_w: f32,
    content_h: f32,
) -> Option<egui::Rect> {
    let (x0, x1) = strip.clip_span(grid_x, grid_x + content_w)?;
    Some(egui::Rect::from_min_max(
        egui::pos2(x0, grid_y),
        egui::pos2(x1, grid_y + content_h),
    ))
}

/// Room kept to the right of the grid band for the inline tool panel. Only
/// subtracted while a glyph is being edited, i.e. while the panel is shown.
pub(crate) fn inline_panel_reserved_width(zoom: f32) -> f32 {
    INLINE_PANEL_GAP * zoom
        + crate::editor::glyph_widget::palette_cols() as f32 * INLINE_PALETTE_CELL * zoom
}

/// A run of consecutive grid rows belonging to one glyph, in scroll-area
/// coordinates. Used to place the horizontal scrollbar and to bound
/// drag-driven auto-scrolling.
pub(super) struct GridBlock {
    pub(super) item_idx: usize,
    pub(super) y0: f32,
    pub(super) y1: f32,
    pub(super) content_w: f32,
}

pub(super) fn collect_grid_blocks(
    vlines: &[VisualLine],
    row_height: f32,
    grid_cell: f32,
) -> Vec<GridBlock> {
    let mut blocks: Vec<GridBlock> = Vec::new();
    let mut y = 0.0f32;
    for vl in vlines {
        let h = vl.height(row_height, grid_cell);
        if let VLineKind::GridRow { item_idx, extent, .. } = &vl.kind {
            let content_w = extent.display_width(grid_cell);
            match blocks.last_mut() {
                Some(b) if b.item_idx == *item_idx && (b.y1 - y).abs() < 0.5 => {
                    b.y1 = y + h;
                    b.content_w = b.content_w.max(content_w);
                }
                _ => blocks.push(GridBlock {
                    item_idx: *item_idx,
                    y0: y,
                    y1: y + h,
                    content_w,
                }),
            }
        }
        y += h;
    }
    blocks
}

pub(crate) fn compute_grid_display_extent(
    pixels: Option<&PixelGrid>,
    composite: Option<&GlyphComposite>,
    points: &[GlyphPoint],
) -> (u16, u16, GridExtent) {
    let (own_w, own_h, mut extent) = if let Some(grid) = pixels {
        let own_w = grid.width;
        let own_h = grid.height;
        if let Some(comp) = composite {
            let extent = GridExtent {
                top: (-comp.own_offset_row).min(0),
                left: (-comp.own_offset_col).min(0),
                bottom: (comp.height as i16 - comp.own_offset_row).max(own_h as i16),
                right: (comp.width as i16 - comp.own_offset_col).max(own_w as i16),
            };
            (own_w, own_h, extent)
        } else {
            (own_w, own_h, GridExtent::own_area(own_w, own_h))
        }
    } else if let Some(comp) = composite {
        let own_w = (comp.width as i16 - comp.own_offset_col) as u16;
        let own_h = (comp.height as i16 - comp.own_offset_row) as u16;
        let extent = GridExtent {
            top: (-comp.own_offset_row).min(0),
            left: (-comp.own_offset_col).min(0),
            bottom: own_h as i16,
            right: own_w as i16,
        };
        (own_w, own_h, extent)
    } else {
        (
            0,
            0,
            GridExtent {
                top: 0,
                left: 0,
                bottom: 0,
                right: 0,
            },
        )
    };

    // Anchors widen the drawn area whether declared or inherited: an anchor
    // outside the ink (a `+above` two rows over the cap height, say) is
    // otherwise a layer that exists in the palette but is invisible on the
    // grid.
    let inherited = composite
        .map_or(&[][..], |c| c.inherited_anchors.as_slice())
        .iter()
        .map(|(p, _)| p);
    for pt in points.iter().chain(inherited) {
        extent.top = extent.top.min(pt.row);
        extent.left = extent.left.min(pt.col);
        extent.bottom = extent.bottom.max(pt.row_end + 1);
        extent.right = extent.right.max(pt.col_end + 1);
    }

    (own_w, own_h, extent)
}

pub(crate) struct VisualLine {
    pub(crate) doc_line: usize,
    pub(crate) kind: VLineKind,
    pub(crate) color: egui::Color32,
    pub(crate) error_spans: Vec<(usize, usize, String)>,
    pub(crate) col_offset: usize,
    /// Display-only text spliced into this line, at columns relative to the
    /// segment (i.e. already shifted by `col_offset`). Empty for grid rows.
    pub(crate) annotations: Vec<InlineAnnotation>,
    /// Column (relative to the segment) where the line's `// …` comment
    /// starts, if any of it falls in this segment. Painted in the comment
    /// color whatever the rest of the line is.
    pub(crate) comment_col: Option<usize>,
}

impl VisualLine {
    /// The line's text paired with its annotations, for measuring and painting.
    /// Returns `None` for grid rows.
    #[cfg(test)]
    pub(crate) fn annotated_text(&self) -> Option<AnnotatedText<'_>> {
        match &self.kind {
            VLineKind::Text(text) => Some(AnnotatedText::new(text, &self.annotations)),
            VLineKind::GridRow { .. } => None,
        }
    }
}

#[derive(Clone)]
pub(crate) enum VLineKind {
    Text(String),
    GridRow {
        item_idx: usize,
        row: i16,
        own_width: u16,
        own_height: u16,
        grid_doc_line: usize,
        extent: GridExtent,
        /// `None` when the metrics overlay is switched off.
        metrics: Option<GlyphMetrics>,
    },
}

/// Everything derivable from the document/line buffers that `show_document`
/// needs each frame. Rebuilding it is O(document); the cache below keeps the
/// last result so idle frames (no edits, no layout change) skip the rebuild.
pub(crate) struct ViewData {
    pub(crate) composites: HashMap<usize, GlyphComposite>,
    pub(crate) vlines: Vec<VisualLine>,
    pub(crate) source_offsets: Vec<usize>,
    /// The shadow of the anchor layer currently selected, with the item it
    /// belongs to. `None` whenever the selected layer is not an anchor (or no
    /// glyph attaches there).
    pub(crate) shadow: Option<(usize, AnchorShadow)>,
}

/// Inputs `ViewData` was computed from. `edit_gen` stands in for the document
/// contents, so anything that mutates `lines` without an immediate rederive
/// must drop the cache instead (see the `needs_rederive` handling below).
#[derive(PartialEq)]
pub(super) struct ViewCacheKey {
    pub(super) edit_gen: u64,
    pub(super) derived_gen: u64,
    pub(super) font_gen: u64,
    pub(super) zoom_level: u32,
    pub(super) editing_item_idx: Option<usize>,
    /// The selected *anchor* layer as `(item, point index)`, which the shadow
    /// and the extents that make room for it are derived from. Only anchors
    /// are keyed on: cycling through ref layers changes nothing the view is
    /// built from, and rebuilding it is O(document).
    pub(super) active_point: Option<(usize, usize)>,
    pub(super) show_metrics: bool,
    pub(super) wrap_width_bits: Option<u32>,
    pub(super) font_id: egui::FontId,
    pub(super) dark_mode: bool,
    pub(super) ppp_bits: u32,
}

pub(crate) struct ViewCache {
    pub(super) key: ViewCacheKey,
    pub(super) data: std::sync::Arc<ViewData>,
}

#[cfg(test)]
impl ViewCache {
    pub(crate) fn data_ptr(&self) -> *const ViewData {
        std::sync::Arc::as_ptr(&self.data)
    }
}

impl VisualLine {
    pub(crate) fn height(&self, row_h: f32, grid_cell: f32) -> f32 {
        match &self.kind {
            VLineKind::Text(_) => row_h,
            VLineKind::GridRow { .. } => grid_cell,
        }
    }

    pub(super) fn kind_row(&self) -> Option<i16> {
        match &self.kind {
            VLineKind::GridRow { row, .. } => Some(*row),
            _ => None,
        }
    }
}

/// Vertical offset (in pixels) of the first visual line belonging to
/// `target_doc_line`, i.e. the sum of heights of all visual lines before it.
pub(super) fn doc_line_to_y(
    vlines: &[VisualLine],
    row_height: f32,
    grid_cell: f32,
    target_doc_line: usize,
) -> f32 {
    let mut y = 0.0f32;
    for vl in vlines {
        if vl.doc_line >= target_doc_line {
            break;
        }
        y += vl.height(row_height, grid_cell);
    }
    y
}

/// The source-file line number drawn in the gutter for a visual line, if any.
/// Wrapped text continuations and grid rows outside the glyph's own area
/// carry no number.
pub(crate) fn gutter_line_number(
    vl: &VisualLine,
    lines: &[DocLine],
    source_offsets: &[usize],
) -> Option<usize> {
    match &vl.kind {
        VLineKind::Text(_) if vl.col_offset == 0 => {
            source_offsets.get(vl.doc_line).map(|&off| off + 1)
        }
        VLineKind::Text(_) => None,
        VLineKind::GridRow {
            row,
            own_height,
            grid_doc_line,
            ..
        } => {
            if *row >= 0
                && *row < *own_height as i16
                && matches!(lines.get(*grid_doc_line), Some(DocLine::Grid(g)) if !g.is_all_empty())
            {
                source_offsets
                    .get(*grid_doc_line)
                    .map(|&off| off + *row as usize + 1)
            } else {
                None
            }
        }
    }
}
