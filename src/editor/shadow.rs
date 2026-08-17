//! What a *shadow* is: a dim layer drawn under the glyph being edited, made of
//! other glyphs the edited one has a relationship with, placed exactly where
//! that relationship puts them.
//!
//! Two kinds exist and they share everything below — the union rule, the bound
//! on how far a candidate may sit, the grid the result is carried in — because
//! the only thing that differs is which glyphs are collected and by what
//! placement:
//!
//! - [`crate::editor::anchor_shadow`]: the glyphs that could *attach* at the
//!   selected anchor, placed so the two anchors coincide.
//! - [`crate::editor::backref_shadow`]: the glyphs that *refer to* this one,
//!   placed so their copy of this glyph coincides with it.
//!
//! What is drawn is the *union* of all of them — a cell is inked when any
//! candidate inks it, and its geometry is the exact union of every candidate's
//! geometry there, which is what [`PixelGrid::blit`] already computes (a cell
//! whose union is no catalog shape becomes a `PX_CUSTOM` detail).
//!
//! A shadow is read off *ink*, never off a cell merely being written: a
//! hardblank (`$$`) and the ink-less subcell `BitmapFill` writes both draw
//! nothing, so they contribute nothing here either
//! ([`PixelShape::is_blank`](crate::pixel::PixelShape::is_blank)). Counting
//! them would dim a cell no candidate actually inks, and — because the grid
//! painter skips its own background wherever the shadow has ink — leave that
//! cell without one.
//!
//! Only one shadow is ever live: an anchor shadow needs [`crate::editor::EditMode::LayerMove`]
//! and a backreference shadow [`crate::editor::EditMode::PixelSelect`], so the
//! view carries a single `Option<Shadow>` and [`ShadowKind`] says which it is.

use crate::document::PixelGrid;
use crate::pixel::{PX_ALMOSTFULL, PX_CUSTOM};

/// Rows/columns beyond which a candidate is taken to be misplaced rather than
/// merely far away. Without a bound one stray anchor pair — or one `ref` with
/// an absurd offset — could ask for a grid of millions of cells, all of it off
/// screen.
pub(crate) const MAX_EXTENT: i32 = 1024;

/// Which relationship a shadow is showing, for the one thing that has to tell
/// them apart: the colour it is drawn in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShadowKind {
    Anchor,
    Backref,
}

/// The union of every candidate glyph, in the edited glyph's own grid
/// coordinates.
#[derive(Clone)]
pub(crate) struct Shadow {
    pub(crate) grid: PixelGrid,
    /// Grid coordinate of the shadow's raster cell `(0, 0)`. Negative when the
    /// shadow reaches above/left of the edited glyph's own grid, exactly as a
    /// composite's `own_offset_*` allows.
    pub(crate) row: i16,
    pub(crate) col: i16,
    /// How many placements went into the union. Zero never reaches here — no
    /// candidate means no shadow at all.
    pub(crate) count: usize,
    pub(crate) kind: ShadowKind,
}

/// Collects placed candidate grids and unions them into one [`Shadow`].
///
/// The two-pass shape is what the bounding box forces: the union's own grid
/// cannot be allocated until every placement is known, so each is held with the
/// coordinate it lands at and blitted in [`Self::finish`].
pub(crate) struct ShadowBuilder {
    kind: ShadowKind,
    placements: Vec<(PixelGrid, i32, i32)>,
    min_r: i32,
    min_c: i32,
    max_r: i32,
    max_c: i32,
}

impl ShadowBuilder {
    pub(crate) fn new(kind: ShadowKind) -> Self {
        Self {
            kind,
            placements: Vec::new(),
            min_r: i32::MAX,
            min_c: i32::MAX,
            max_r: i32::MIN,
            max_c: i32::MIN,
        }
    }

    /// Add one candidate with its raster top-left at `(row, col)` in the edited
    /// glyph's coordinates. A placement past [`MAX_EXTENT`] is dropped rather
    /// than clamped: it is a misplacement, and clamping would draw it somewhere
    /// it does not belong. A candidate that inks nothing is dropped too — see
    /// [`has_ink`].
    pub(crate) fn push(&mut self, grid: PixelGrid, row: i32, col: i32) {
        if grid.width == 0 || grid.height == 0 || !has_ink(&grid) {
            return;
        }
        let (h, w) = (grid.height as i32, grid.width as i32);
        if row < -MAX_EXTENT || col < -MAX_EXTENT || row + h > MAX_EXTENT || col + w > MAX_EXTENT {
            return;
        }
        self.min_r = self.min_r.min(row);
        self.min_c = self.min_c.min(col);
        self.max_r = self.max_r.max(row + h);
        self.max_c = self.max_c.max(col + w);
        self.placements.push((grid, row, col));
    }

    pub(crate) fn finish(self) -> Option<Shadow> {
        if self.placements.is_empty() {
            return None;
        }
        let mut grid = PixelGrid::new(
            (self.max_c - self.min_c) as u16,
            (self.max_r - self.min_r) as u16,
        );
        for (src, row, col) in &self.placements {
            union_into(&mut grid, src, row - self.min_r, col - self.min_c);
        }
        Some(Shadow {
            grid,
            row: self.min_r as i16,
            col: self.min_c as i16,
            count: self.placements.len(),
            kind: self.kind,
        })
    }
}

/// Whether any cell of `grid` draws something. A candidate that draws nothing
/// is not a shadow: it would widen the drawn area to cover a glyph the reader
/// cannot see.
fn has_ink(grid: &PixelGrid) -> bool {
    (0..grid.height).any(|r| (0..grid.width).any(|c| !grid.get(r, c).is_contour_empty()))
}

/// Union `src` into `dst` at `(off_r, off_c)` — the same rule
/// [`PixelGrid::blit`] applies, with the two cases a shadow is mostly made of
/// taken first. A shadow is every candidate glyph at once, so its cells
/// overlap far more than a composite's do, and going through the exact sweep
/// for each of them costs more than the whole rest of the view rebuild.
fn union_into(dst: &mut PixelGrid, src: &PixelGrid, off_r: i32, off_c: i32) {
    for r in 0..src.height as i32 {
        for c in 0..src.width as i32 {
            let shape = src.get(r as u16, c as u16);
            if shape.is_contour_empty() {
                continue;
            }
            let (dr, dc) = (off_r + r, off_c + c);
            if dr < 0 || dc < 0 || dr >= dst.height as i32 || dc >= dst.width as i32 {
                continue;
            }
            let (dr, dc) = (dr as u16, dc as u16);
            let current = dst.get(dr, dc);
            // Already a whole inked pixel: nothing unions into more than that.
            if current.shape_id() == PX_ALMOSTFULL && current.is_bitmap_filled() {
                continue;
            }
            let filled = current.is_bitmap_filled() || shape.is_bitmap_filled();
            if current.is_clear() {
                if shape.shape_id() == PX_CUSTOM {
                    dst.set_detail(dr, dc, &src.region_at(r as u16, c as u16), filled);
                } else {
                    dst.set(dr, dc, shape);
                }
                continue;
            }
            // The same geometry twice — only the ink flag can still change.
            if current.shape_id() == shape.shape_id() && shape.shape_id() != PX_CUSTOM {
                dst.set_filled(dr, dc, filled);
                continue;
            }
            let union = crate::detail::bool_op(
                &dst.region_at(dr, dc),
                &src.region_at(r as u16, c as u16),
                crate::detail::BoolOp::Union,
            );
            dst.set_detail(dr, dc, &union, filled);
        }
    }
}
